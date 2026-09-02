//! Core meta-decompilation: run engines (parallel), compile, rank, best-of merge,
//! auto-fix, round-trip.

use crate::config::Engine;
use crate::util::{self, PATHSEP};
use crate::{autofix, normalize};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

static ERR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(.*?\.java):\d+: error:").unwrap());
static TOP_TYPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)\b(class|interface|enum)\s+[A-Za-z_$][\w$]*").unwrap());

const BIG: i64 = 1_000_000_000;

/// Per-engine decompile result.
pub struct EngineData {
    pub type_: String,
    pub src: PathBuf,
    pub files: BTreeMap<String, PathBuf>,
    pub errors: i64,
    pub per: BTreeMap<String, i64>,
    pub nclasses: usize,
    /// Position of this engine in the CONFIG, not in the (alphabetical) map.  The
    /// reference implementation keeps its engine results in an insertion-ordered
    /// dict, so every "first wins" tie — the per-class best-of pick, the donor
    /// scan — resolves in config order.  Keying the map by name alone would resolve
    /// those ties alphabetically instead and silently select a different engine, so
    /// the config position travels with the data.
    pub order: usize,
}

/// EVERY engine, in CONFIG order — the reference walks its insertion-ordered dict,
/// so anything that reports per-engine results (the per-class report) must walk the
/// same order and not the map's alphabetical one.
pub fn all_in_config_order(engine_data: &BTreeMap<String, EngineData>) -> Vec<(&String, &EngineData)> {
    let mut v: Vec<(&String, &EngineData)> = engine_data.iter().collect();
    v.sort_by_key(|(_, d)| d.order);
    v
}

/// Engine names of type `names`, in CONFIG order (the reference's dict order).
pub fn names_in_config_order(engine_data: &BTreeMap<String, EngineData>) -> Vec<&String> {
    let mut v: Vec<(usize, &String)> = engine_data
        .iter()
        .filter(|(_, d)| d.type_ == "names")
        .map(|(n, d)| (d.order, n))
        .collect();
    v.sort_by_key(|(o, _)| *o);
    v.into_iter().map(|(_, n)| n).collect()
}

/// True if the FIRST top-level type declaration in `text` is an interface.
fn is_interface(text: &str) -> bool {
    match TOP_TYPE_RE.captures(text) {
        Some(c) => c.get(1).map(|m| m.as_str() == "interface").unwrap_or(false),
        None => false,
    }
}

// ── decompile / compile ──────────────────────────────────────────────────────
pub fn run_decompiler(eng: &Engine, jar: &Path, outdir: &Path, root: &Path) -> PathBuf {
    let _ = std::fs::create_dir_all(outdir);
    match eng.argv(jar, outdir, root) {
        Ok(argv) => {
            let r = util::run_capture(&argv, Some(Duration::from_secs(900)));
            if let Some(e) = r.spawn_err {
                println!("  [{}] launch error: {e}", eng.name);
            }
        }
        Err(e) => println!("  [{}] launch error: {e}", eng.name),
    }
    let src = eng.sources_dir(outdir);
    if eng.normalize != "none" {
        let moved = normalize::apply(&eng.normalize, &src, true);
        if moved > 0 {
            println!("    [{}] normalised {moved} file(s) out of defpackage", eng.name);
        }
    }
    src
}

pub fn collect_java(srcdir: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    for p in util::rglob_ext(srcdir, "java") {
        if p.file_name().map(|n| n == "_sources.txt").unwrap_or(false) {
            continue;
        }
        if let Ok(rel) = p.strip_prefix(srcdir) {
            out.insert(rel.to_string_lossy().into_owned(), p.clone());
        }
    }
    out
}

fn count_errors_per(log: &str, srcdir: &Path) -> BTreeMap<String, i64> {
    let mut per: BTreeMap<String, i64> = BTreeMap::new();
    let src_abs = util::absolutize(srcdir);
    for c in ERR_RE.captures_iter(log) {
        let raw = c.get(1).unwrap().as_str();
        let rel = match util::absolutize(Path::new(raw)).strip_prefix(&src_abs) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => Path::new(raw)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| raw.to_string()),
        };
        *per.entry(rel).or_insert(0) += 1;
    }
    per
}

/// Compile all .java under srcdir. -> (total_errors, {relpath: count}, log).
pub fn compile_dir(srcdir: &Path, classpath: Option<&str>) -> (i64, BTreeMap<String, i64>, String) {
    let files = util::rglob_ext(srcdir, "java");
    if files.is_empty() {
        return (BIG, BTreeMap::new(), "no sources".into());
    }
    let listf = srcdir.join("_sources.txt");
    let list_body = files
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(&listf, list_body);
    let javac = util::tool("ARLECCHINO_JAVAC", "javac");
    let mut cmd = vec![
        javac,
        "-d".into(),
        srcdir.join("_classes").to_string_lossy().into_owned(),
        "--release".into(),
        "8".into(),
        "-encoding".into(),
        "utf-8".into(),
        "-nowarn".into(),
        "-proc:none".into(),
    ];
    if let Some(cp) = classpath {
        cmd.push("-cp".into());
        cmd.push(cp.to_string());
    }
    cmd.push(format!("@{}", listf.to_string_lossy()));
    // javac в srcdir: свой @argfile он положит туда же, а не в чужой каталог.
    let r = util::run_capture_in(&cmd, None, Some(srcdir));
    let log = r.stderr;
    let per = count_errors_per(&log, srcdir);
    (per.values().sum(), per, log)
}

// ── decompile all (parallel) ─────────────────────────────────────────────────
pub fn decompile_all(
    engines: &[Engine],
    jar: &Path,
    out: &Path,
    root: &Path,
    classpath: Option<&str>,
    jobs: i64,
) -> BTreeMap<String, EngineData> {
    let mut usable: Vec<&Engine> = Vec::new();
    for eng in engines {
        if !eng.enabled {
            continue;
        }
        let binp = eng.resolve_path(root);
        if !binp.exists() {
            println!("  [{}] skip (missing {})", eng.name, binp.display());
            continue;
        }
        usable.push(eng);
    }

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(String, EngineData)>> = Mutex::new(Vec::new());
    let nthreads = (jobs.max(1) as usize).min(usable.len().max(1));

    std::thread::scope(|s| {
        for _ in 0..nthreads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= usable.len() {
                    break;
                }
                let eng = usable[i];
                println!("  decompiling: {} ...", eng.name);
                let src = run_decompiler(eng, jar, &out.join(&eng.name), root);
                let files = collect_java(&src);
                let (total, per, _) = compile_dir(&src, classpath);
                println!(
                    "    [{}] classes={}  compile errors={}",
                    eng.name,
                    files.len(),
                    total
                );
                let d = EngineData {
                    type_: eng.type_.clone(),
                    src,
                    nclasses: files.len(),
                    files,
                    errors: total,
                    per,
                    order: i,
                };
                results.lock().unwrap().push((eng.name.clone(), d));
            });
        }
    });

    let mut data = BTreeMap::new();
    for (n, d) in results.into_inner().unwrap() {
        data.insert(n, d);
    }
    data
}

/// Compile a SINGLE .java against `api_classpath`, counting only that file's errors.
pub fn compile_file_isolated(java_file: &Path, api_classpath: &str, tmp_out: &Path) -> i64 {
    let _ = std::fs::create_dir_all(tmp_out);
    let javac = util::tool("ARLECCHINO_JAVAC", "javac");
    let argv = vec![
        javac,
        "-d".into(),
        tmp_out.to_string_lossy().into_owned(),
        "--release".into(),
        "8".into(),
        "-encoding".into(),
        "utf-8".into(),
        "-nowarn".into(),
        "-proc:none".into(),
        "-cp".into(),
        api_classpath.to_string(),
        java_file.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture_in(&argv, None, Some(tmp_out));
    ERR_RE.find_iter(&r.stderr).count() as i64
}

/// Classpath exposing the ORIGINAL jar's bytecode plus any extra (MIDP stub).
pub fn build_api_classpath(orig_jar: &Path, extra: Option<&str>, _workdir: &Path) -> String {
    let mut parts = vec![orig_jar.to_string_lossy().into_owned()];
    if let Some(e) = extra {
        parts.push(e.to_string());
    }
    parts.join(PATHSEP)
}

/// Per-class best-of merge using ISOLATED, unmasked per-file scoring.
/// Returns (merge_dir, iso_total, log, per_class_isolated_errors).
pub fn merge_isolated(
    engine_data: &BTreeMap<String, EngineData>,
    outdir: &Path,
    orig_jar: &Path,
    classpath: Option<&str>,
    do_autofix: bool,
) -> (PathBuf, i64, Vec<String>, BTreeMap<String, i64>) {
    let merge = outdir.join("merged");
    if merge.exists() {
        let _ = std::fs::remove_dir_all(&merge);
    }
    let _ = std::fs::create_dir_all(&merge);

    let api_cp = build_api_classpath(orig_jar, classpath, outdir);
    let tmp = outdir.join("_iso");
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }

    let names: Vec<&String> = names_in_config_order(engine_data);
    let mut all_rels: BTreeSet<String> = BTreeSet::new();
    for n in &names {
        for r in engine_data[*n].files.keys() {
            all_rels.insert(r.clone());
        }
    }

    let mut log: Vec<String> = Vec::new();
    let mut per_iso: BTreeMap<String, i64> = BTreeMap::new();

    for rel in &all_rels {
        let mut best_text: Option<String> = None;
        let mut best_score: Option<i64> = None;
        let mut best_eng: Option<String> = None;
        for n in &names {
            let f = match engine_data[*n].files.get(rel) {
                Some(f) => f,
                None => continue,
            };
            let score = compile_file_isolated(f, &api_cp, &tmp.join("score"));
            if best_score.is_none() || score < best_score.unwrap() {
                best_score = Some(score);
                best_eng = Some((*n).clone());
                best_text = Some(util::read_text_lossy(f).unwrap_or_default());
            }
        }
        if best_text.is_none() {
            continue;
        }
        // autofix attempt (isolated-scored)
        if do_autofix && best_score.map(|s| s > 0).unwrap_or(false) {
            let donor_texts: Vec<String> = names
                .iter()
                .filter_map(|n| engine_data[*n].files.get(rel))
                .map(|f| util::read_text_lossy(f).unwrap_or_default())
                .collect();
            for src_eng in &names {
                let f = match engine_data[*src_eng].files.get(rel) {
                    Some(f) => f,
                    None => continue,
                };
                let cand_src = util::read_text_lossy(f).unwrap_or_default();
                let (mut fixed, mut applied) = autofix::apply_all(&cand_src);
                let resolved = autofix::resolve_unrepresentable_casts(&fixed, &donor_texts);
                if resolved != fixed {
                    fixed = resolved;
                    applied.push("resolve_unrepresentable_casts".into());
                }
                if applied.is_empty() || fixed == cand_src {
                    continue;
                }
                let tf = tmp.join("fix").join(rel);
                if let Some(p) = tf.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                let _ = std::fs::write(&tf, &fixed);
                let sc = compile_file_isolated(&tf, &api_cp, &tmp.join("score"));
                let bscore = best_score.unwrap();
                let better = sc < bscore;
                let tie_pref = sc == bscore
                    && is_interface(&fixed)
                    && !is_interface(best_text.as_ref().unwrap());
                if better || tie_pref {
                    best_score = Some(sc);
                    best_text = Some(fixed);
                    best_eng = Some(format!("{}+autofix({})", src_eng, applied.join(",")));
                }
            }
        }
        let dst = merge.join(rel);
        if let Some(p) = dst.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let text = best_text.unwrap();
        let _ = std::fs::write(&dst, &text);
        let score = best_score.unwrap_or(0);
        per_iso.insert(rel.clone(), if score != 0 { score } else { 0 });
        let eng = best_eng.unwrap();
        if score != 0 {
            log.push(format!("  {rel}: <- {eng} (isolated errors={score})"));
        } else {
            log.push(format!("  {rel}: <- {eng} (clean)"));
        }
    }

    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    let iso_total: i64 = per_iso.values().sum();
    let (tree_total, _, _) = compile_dir(&merge, classpath);
    log.insert(
        0,
        format!("isolated per-class scoring; sum of per-class errors={iso_total}"),
    );
    let still_failing = per_iso.values().filter(|v| **v != 0).count();
    log.push(format!(
        "RESULT merged: isolated-sum={iso_total}, whole-tree errors={tree_total} \
         (classes still failing: {still_failing})"
    ));
    (merge, iso_total, log, per_iso)
}

/// Best-effort compile with iterative seeding of broken classes from the jar.
/// Returns (n_dropped, set_of_dropped_relpaths).
pub fn compile_best_effort(
    srcdir: &Path,
    orig_jar: Option<&Path>,
    classpath: Option<&str>,
    drop: Option<&HashSet<String>>,
) -> (usize, HashSet<String>) {
    let classes_dir = srcdir.join("_classes");
    if classes_dir.exists() {
        let _ = std::fs::remove_dir_all(&classes_dir);
    }
    let _ = std::fs::create_dir_all(&classes_dir);

    let mut all_rels: HashSet<String> = HashSet::new();
    for p in util::rglob_ext(srcdir, "java") {
        if let Ok(rel) = p.strip_prefix(srcdir) {
            let rel = rel.to_string_lossy().into_owned();
            if !rel.starts_with('_') {
                all_rels.insert(rel);
            }
        }
    }

    let mut dropped: HashSet<String> = match drop {
        Some(d) => d.clone(),
        None => {
            let (total, per, _) = compile_dir(srcdir, classpath);
            if total == 0 {
                return (0, HashSet::new());
            }
            per.keys().cloned().collect()
        }
    };

    let jar_names: HashSet<String> = match orig_jar {
        Some(j) if j.exists() => util::zip_namelist(j).unwrap_or_default().into_iter().collect(),
        _ => HashSet::new(),
    };

    let seed = |rels: &HashSet<String>| {
        if let Some(j) = orig_jar {
            if j.exists() {
                for rel in rels {
                    let stem = rel.strip_suffix(".java").unwrap_or(rel);
                    let cn = format!("{stem}.class");
                    if jar_names.contains(&cn) {
                        if let Ok(bytes) = util::zip_read_entry(j, &cn) {
                            let dst = classes_dir.join(&cn);
                            if let Some(p) = dst.parent() {
                                let _ = std::fs::create_dir_all(p);
                            }
                            let _ = std::fs::write(&dst, bytes);
                        }
                    }
                }
            }
        }
    };

    let compile = |keep_rels: &HashSet<String>| -> BTreeMap<String, i64> {
        let listf = srcdir.join("_sources_clean.txt");
        let mut keep: Vec<String> = keep_rels.iter().cloned().collect();
        keep.sort();
        let body = keep
            .iter()
            .map(|r| srcdir.join(r).to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(&listf, body);
        let mut cp = classes_dir.to_string_lossy().into_owned();
        if let Some(c) = classpath {
            cp = format!("{c}{PATHSEP}{cp}");
        }
        let javac = util::tool("ARLECCHINO_JAVAC", "javac");
        let argv = vec![
            javac,
            "-d".into(),
            classes_dir.to_string_lossy().into_owned(),
            "--release".into(),
            "8".into(),
            "-encoding".into(),
            "utf-8".into(),
            "-nowarn".into(),
            "-proc:none".into(),
            "-cp".into(),
            cp,
            format!("@{}", listf.to_string_lossy()),
        ];
        let r = util::run_capture_in(&argv, None, Some(srcdir));
        count_errors_per(&r.stderr, srcdir)
    };

    for _ in 0..(all_rels.len() + 1) {
        seed(&dropped);
        let keep: HashSet<String> = all_rels.difference(&dropped).cloned().collect();
        let per = compile(&keep);
        let new_bad: HashSet<String> = per
            .keys()
            .filter(|k| !dropped.contains(*k))
            .cloned()
            .collect();
        if new_bad.is_empty() {
            break;
        }
        for b in new_bad {
            dropped.insert(b);
        }
    }
    (dropped.len(), dropped)
}

// ── best-of merge (base+swap+autofix; not on the isolated CLI path) ───────────
pub fn merge_best(
    engine_data: &BTreeMap<String, EngineData>,
    base_name: &str,
    outdir: &Path,
    classpath: Option<&str>,
    do_autofix: bool,
) -> (PathBuf, i64, Vec<String>) {
    let base = &engine_data[base_name];
    let merge = outdir.join("merged");
    if merge.exists() {
        let _ = std::fs::remove_dir_all(&merge);
    }
    let _ = util::copy_tree(&base.src, &merge, &["_classes", "_sources.txt"]);

    let (mut total, per, _) = compile_dir(&merge, classpath);
    let mut log = vec![format!("base={base_name} errors={total}")];
    let mut donors: Vec<&String> = names_in_config_order(engine_data)
        .into_iter()
        .filter(|n| n.as_str() != base_name)
        .collect();
    // stable sort: engines with an equal error count keep their config order
    donors.sort_by_key(|n| engine_data[*n].errors);

    // 1) per-class file swap
    let mut per_keys: Vec<String> = per.keys().cloned().collect();
    per_keys.sort();
    for rel in &per_keys {
        let target = merge.join(rel);
        let mut best_total = total;
        let mut best_donor: Option<&String> = None;
        let backup = if target.exists() {
            Some(util::read_text_lossy(&target).unwrap_or_default())
        } else {
            None
        };
        for dn in &donors {
            let cand = match engine_data[*dn].files.get(rel) {
                Some(c) => c,
                None => continue,
            };
            if let Some(p) = target.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::copy(cand, &target);
            let (t2, _, _) = compile_dir(&merge, classpath);
            if t2 < best_total {
                best_total = t2;
                best_donor = Some(dn);
            }
            if let Some(b) = &backup {
                let _ = std::fs::write(&target, b);
            }
        }
        if let Some(bd) = best_donor {
            let _ = std::fs::copy(&engine_data[bd].files[rel], &target);
            total = best_total;
            log.push(format!("  {rel}: {base_name}->{bd} (errors->{total})"));
        }
    }

    // 2) auto-fix pass on whatever still fails
    if do_autofix && total > 0 {
        let all_names: Vec<&String> = names_in_config_order(engine_data);
        let (_, per_now, _) = compile_dir(&merge, classpath);
        let mut pn_keys: Vec<String> = per_now.keys().cloned().collect();
        pn_keys.sort();
        for rel in &pn_keys {
            let target = merge.join(rel);
            if !target.exists() {
                continue;
            }
            let current = util::read_text_lossy(&target).unwrap_or_default();
            let mut best_text = current.clone();
            let mut best_total2 = total;
            let mut best_tag: Option<String> = None;
            let mut cands: Vec<(String, String)> = vec![("current".into(), current.clone())];
            for n in &all_names {
                if let Some(f) = engine_data[*n].files.get(rel) {
                    cands.push(((*n).clone(), util::read_text_lossy(f).unwrap_or_default()));
                }
            }
            for (tag, src_text) in &cands {
                let (fixed, applied) = autofix::apply_all(src_text);
                if applied.is_empty() || fixed == *src_text {
                    continue;
                }
                let _ = std::fs::write(&target, &fixed);
                let (t2, _, _) = compile_dir(&merge, classpath);
                if t2 < best_total2 {
                    best_total2 = t2;
                    best_text = fixed;
                    best_tag = Some(format!("{tag}+{}", applied.join(",")));
                }
            }
            let _ = std::fs::write(&target, &best_text);
            if let (Some(tag), true) = (best_tag, best_total2 < total) {
                total = best_total2;
                log.push(format!("  {rel}: autofix {tag} (errors->{total})"));
            }
        }
    }

    let (final_total, final_per, _) = compile_dir(&merge, classpath);
    log.push(format!(
        "RESULT merged: errors={final_total} (classes with errors: {})",
        final_per.len()
    ));
    (merge, final_total, log)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(order: usize, type_: &str, errors: i64) -> EngineData {
        EngineData {
            type_: type_.to_string(),
            src: PathBuf::new(),
            files: BTreeMap::new(),
            errors,
            per: BTreeMap::new(),
            nclasses: 0,
            order,
        }
    }

    /// The engine map is keyed by name (alphabetical), but every "first wins" tie
    /// must resolve in CONFIG order, exactly as the reference implementation's
    /// insertion-ordered dict does.
    #[test]
    fn names_follow_config_order_not_alphabet() {
        let mut m: BTreeMap<String, EngineData> = BTreeMap::new();
        for (i, n) in ["vineflower", "cfr", "procyon", "jadx"].iter().enumerate() {
            m.insert((*n).to_string(), ed(i, "names", 0));
        }
        let got: Vec<&str> = names_in_config_order(&m)
            .iter()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(got, vec!["vineflower", "cfr", "procyon", "jadx"]);
        // ... which is NOT what plain map iteration gives
        let alpha: Vec<&str> = m.keys().map(|s| s.as_str()).collect();
        assert_eq!(alpha, vec!["cfr", "jadx", "procyon", "vineflower"]);
    }

    #[test]
    fn names_skips_engines_of_other_types() {
        let mut m: BTreeMap<String, EngineData> = BTreeMap::new();
        m.insert("vineflower".into(), ed(0, "names", 0));
        m.insert("renamed-thing".into(), ed(1, "renamed", 0));
        m.insert("cfr".into(), ed(2, "names", 0));
        let got: Vec<&str> = names_in_config_order(&m)
            .iter()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(got, vec!["vineflower", "cfr"]);
    }
}
