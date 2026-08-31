//! Arlecchino — CLI with subcommands.
//!
//!   arlecchino setup     [--engines-root DIR] [--only a,b] [--freej2me-prebuilt P]
//!   arlecchino decompile <jar> [-o OUT] [-cp DEPS] [--config C] [-j N]
//!                              [--per-method] [--deobf] [--no-autofix] [--no-roundtrip]
//!   arlecchino emulate   <jar> [--freej2me P] [-W 176] [-H 220] [-s 2]
//!
//! Backward compatibility: `arlecchino <something.jar>` is treated as
//! `decompile <jar>`.

use crate::config::{load_engines, engines_root_default};
use crate::emulate::{emulate, find_freej2me};
use crate::i18n::tf;
use crate::roundtrip::RoundTrip;
use crate::setup_engines::{setup_engines, setup_freej2me};
use crate::util;
use crate::{core, deobf, permethod, rename, VERSION};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "arlecchino", version = VERSION, about = "Meta-decompiler / J2ME port toolkit (Apache-2.0)")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// multi-decompile + best-of merge
    Decompile(DecompileArgs),
    /// download engines + FreeJ2ME
    Setup(SetupArgs),
    /// run a J2ME jar in FreeJ2ME
    Emulate(EmulateArgs),
    /// run a MIDlet windowed via midp-rt (LGPL MicroEmulator fork, SEPARATE process — stage B3')
    Run(RunArgs),
    /// behavioural diff: run orig + rebuilt jars headless, compare screen-state sequences and RMS (stage A5)
    Difftest(DifftestArgs),
}

#[derive(Args)]
struct RunArgs {
    /// game .jar or .jad
    jar: String,
    /// path to midp-rt.jar (default: $ARLECCHINO_MIDP_RT, <engines root>/midp-rt.jar,
    /// next to the binary, then build/midp-rt.jar of any midp-rt/ checkout found)
    #[arg(long = "midp-rt")]
    midp_rt: Option<String>,
    /// directory for persistent RMS saves (isolated <dir>/.microemulator/...)
    #[arg(long = "rms-dir")]
    rms_dir: Option<String>,
    /// resizable device size, e.g. 240x320 (default: device default)
    #[arg(long = "size", value_name = "WxH")]
    size: Option<String>,
    /// disable MIDlet bytecode instrumentation (escape hatch; the raw loader path
    /// caps classes at 16 KB and fails as a bare NoClassDefFoundError on bigger ones)
    #[arg(long = "no-instrument")]
    no_instrument: bool,
    /// no window: capture PNG frames + index.json via the runtime's headless mode (stage B4')
    #[arg(long = "headless")]
    headless: bool,
    /// --headless only: output dir for frames + index.json
    #[arg(short = 'o', long = "out", default_value = "run_headless_out")]
    out: String,
    /// --headless only: frames to capture
    #[arg(long = "frames", default_value_t = 30)]
    frames: i64,
    /// --headless only: capture period
    #[arg(long = "period-ms", default_value_t = 100)]
    period_ms: i64,
    /// --headless only: seconds before the first frame
    #[arg(long = "boot-wait", default_value_t = 2.0)]
    boot_wait: f64,
    /// --headless only: JSON input script [{t_ms,key,action},…]
    #[arg(long = "inputs")]
    inputs: Option<String>,
    /// windowed only: write the MIDP call trace (per-frame draw list + session histogram)
    #[arg(long = "trace", value_name = "FILE")]
    trace: Option<String>,
}

#[derive(Args)]
struct DifftestArgs {
    /// original jar (the reference)
    orig: String,
    /// rebuilt/merged jar (omit with --calibrate)
    rebuilt: Option<String>,
    /// JSON input script [{t_ms,key,action},...], same for every run
    #[arg(long = "input", visible_alias = "inputs")]
    inputs: Option<String>,
    /// run ONLY the original, twice, and print the noise baseline instead of a verdict
    #[arg(long = "calibrate")]
    calibrate: bool,
    /// A↔B link: run the SAME jar in BOTH engines and compare SOFTLY
    #[arg(long = "cross-engine")]
    cross_engine: bool,
    /// --cross-engine only: allowed difference in unique-state / state-change counts
    #[arg(long = "state-tol", default_value_t = 3)]
    state_tol: i64,
    #[arg(short = 'o', long = "out", default_value = "difftest_out")]
    out: String,
    #[arg(long = "frames", default_value_t = 30)]
    frames: i64,
    #[arg(long = "period-ms", default_value_t = 100)]
    period_ms: i64,
    #[arg(long = "boot-wait", default_value_t = 2.0)]
    boot_wait: f64,
    #[arg(short = 'W', long = "width", default_value_t = 240)]
    width: i64,
    #[arg(short = 'H', long = "height", default_value_t = 320)]
    height: i64,
    /// allowed first-appearance timing shift, frames (widened to the baseline +1)
    #[arg(long = "shift-tol", default_value_t = 3)]
    shift_tol: i64,
    /// how far below the baseline state-set similarity the rebuilt jar may fall before FAIL
    #[arg(long = "jaccard-margin", default_value_t = 0.15)]
    jaccard_margin: f64,
    /// skip the orig-vs-orig calibration runs
    #[arg(long = "no-baseline")]
    no_baseline: bool,
    /// A7 determinism: ASM-instrument the jar(s) with a virtual clock + fixed-seeded RNG
    #[arg(long = "fixed-clock")]
    fixed_clock: bool,
    /// headless engine: freej2me (default) or midp-rt
    #[arg(long = "engine", default_value = "freej2me")]
    engine: String,
}

#[derive(Args)]
struct DecompileArgs {
    jar: String,
    #[arg(short = 'o', long = "out", default_value = "arlecchino_out")]
    out: String,
    /// dependency classpath (e.g. a MIDP stub); overrides auto stub
    #[arg(long = "classpath")]
    classpath: Option<String>,
    /// path to a MIDP/CLDC stub jar (default: auto-find FreeJ2ME)
    #[arg(long = "midp-stub")]
    midp_stub: Option<String>,
    /// do not auto-add a MIDP stub classpath
    #[arg(long = "no-midp-stub")]
    no_midp_stub: bool,
    #[arg(long = "config")]
    config: Option<String>,
    #[arg(short = 'j', long = "jobs", default_value_t = 4)]
    jobs: i64,
    /// force the bytecode member-renamer ON
    #[arg(long = "rename-members", conflicts_with = "no_rename_members")]
    rename_members: bool,
    /// disable the bytecode member-renamer
    #[arg(long = "no-rename-members")]
    no_rename_members: bool,
    /// move EVERY in-jar type into the default (unnamed) package during the ASM rename
    /// pass, so a named-package class can reference a default-package type
    #[arg(long = "flatten-packages")]
    flatten_packages: bool,
    /// experimental per-method merge
    #[arg(long = "per-method")]
    per_method: bool,
    /// readability deobfuscation pass
    #[arg(long = "deobf")]
    deobf: bool,
    #[arg(long = "no-autofix")]
    no_autofix: bool,
    #[arg(long = "no-roundtrip")]
    no_roundtrip: bool,
    /// semantic round-trip check: flag methods whose symbolic reference set drifted
    #[arg(long = "semdiff")]
    semdiff: bool,
    /// implies --semdiff; DISCARD an autofixed class whose recompiled bytecode drifts
    /// from the original (reverting to the best non-autofixed variant when that does
    /// not raise its isolated error count) — strengthens the "fewer errors only" invariant
    #[arg(long = "reject-semantic-drift")]
    reject_semantic_drift: bool,
}

#[derive(Args)]
struct SetupArgs {
    #[arg(long = "engines-root")]
    engines_root: Option<String>,
    /// comma list: vineflower,cfr,procyon,jadx,jd-core,jode,fernflower,dava,asm,midp-rt
    #[arg(long = "only")]
    only: Option<String>,
    #[arg(long = "no-freej2me")]
    no_freej2me: bool,
    #[arg(long = "freej2me-prebuilt")]
    freej2me_prebuilt: Option<String>,
    #[arg(long = "freej2me-src")]
    freej2me_src: Option<String>,
}

#[derive(Args)]
struct EmulateArgs {
    jar: String,
    #[arg(long = "freej2me")]
    freej2me: Option<String>,
    #[arg(short = 'W', long = "width", default_value_t = 176)]
    width: i64,
    #[arg(short = 'H', long = "height", default_value_t = 220)]
    height: i64,
    #[arg(short = 's', long = "scale", default_value_t = 2)]
    scale: i64,
}

// ───────────────────────────── decompile ────────────────────────────────────
fn cmd_decompile(a: &DecompileArgs) -> i32 {
    let jar = util::absolutize(Path::new(&a.jar));
    let out = util::absolutize(Path::new(&a.out));
    let _ = std::fs::create_dir_all(&out);
    if !jar.exists() {
        println!("{}", tf("jar_not_found", &[&jar.to_string_lossy()]));
        return 2;
    }

    let (engines, cfg, root) = match load_engines(a.config.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            println!("config error: {e}");
            return 2;
        }
    };
    println!("== Arlecchino v{VERSION} :: {} ==", file_name(&jar));
    println!("   config: {cfg}");
    println!("   engines root: {}", root.display());

    // resolve classpath (auto MIDP stub)
    let mut classpath: Option<String> = a.classpath.clone();
    if classpath.is_none() && !a.no_midp_stub {
        if let Some(fj) = find_freej2me(a.midp_stub.as_deref(), Some(&root)) {
            println!("   MIDP stub (auto): {}", fj.display());
            classpath = Some(fj.to_string_lossy().into_owned());
        } else {
            println!(
                "   MIDP stub: none found (run `arlecchino setup`, or pass -cp / \
                 --no-midp-stub). javax.microedition.* will count as errors."
            );
        }
    }

    // bytecode member renamer
    let rename_mode = if a.no_rename_members {
        "off"
    } else if a.rename_members {
        "on"
    } else {
        "auto"
    };
    let orig_jar = jar.clone();
    let (jar, rename_map) = rename::maybe_rename(
        &jar,
        &out.join("_rename"),
        Some(&root),
        rename_mode,
        classpath.as_deref(),
        a.flatten_packages,
    );
    let renamed = jar != orig_jar;
    if renamed {
        println!(
            "   member-rename: using renamed jar -> {} (mapping: {})",
            jar.display(),
            rename_map
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
    }

    let data = core::decompile_all(&engines, &jar, &out, &root, classpath.as_deref(), a.jobs);
    if data.is_empty() {
        println!("{}", tf("no_working_engines", &[]));
        return 1;
    }

    // ranking: (errors asc, nclasses desc)
    let mut ranking: Vec<(&String, &core::EngineData)> = data.iter().collect();
    ranking.sort_by(|(_, x), (_, y)| {
        x.errors.cmp(&y.errors).then(y.nclasses.cmp(&x.nclasses))
    });
    println!("\n== RANKING (fewer errors = better) ==");
    for (n, d) in &ranking {
        println!("  {:12} classes={:3} errors={}", n, d.nclasses, d.errors);
    }

    // per-class report
    let mut all_classes: BTreeSet<String> = BTreeSet::new();
    for d in data.values() {
        for c in d.files.keys() {
            all_classes.insert(c.clone());
        }
    }
    let mut per_class: BTreeMap<String, PerClass> = BTreeMap::new();
    for c in &all_classes {
        let mut clean = Vec::new();
        let mut errors: Vec<(String, Option<i64>)> = Vec::new();
        for (n, d) in &data {
            if d.files.contains_key(c) {
                let e = d.per.get(c).copied();
                if e.unwrap_or(0) == 0 {
                    clean.push(n.clone());
                }
                errors.push((n.clone(), e));
            }
        }
        per_class.insert(c.clone(), PerClass { clean_engines: clean, errors });
    }

    let names_rank: Vec<(&String, &core::EngineData)> = ranking
        .iter()
        .filter(|(_, d)| d.type_ == "names")
        .cloned()
        .collect();

    let mut merged_dir: Option<PathBuf> = None;
    let mut merged_total: Option<i64> = None;
    let mut merge_log: Vec<String> = Vec::new();
    let mut per_iso: BTreeMap<String, i64> = BTreeMap::new();

    if !names_rank.is_empty() {
        println!("\n== BEST-OF MERGE (isolated per-class scoring) ==");
        let (md, total, log, pi) =
            core::merge_isolated(&data, &out, &jar, classpath.as_deref(), !a.no_autofix);
        for l in &log {
            println!("  {l}");
        }
        merged_dir = Some(md);
        merged_total = Some(total);
        merge_log = log;
        per_iso = pi;

        if a.per_method && merged_total.map(|t| t > 0).unwrap_or(false) {
            println!("\n== PER-METHOD MERGE (experimental) ==");
            let api_cp = core::build_api_classpath(&jar, classpath.as_deref(), &out);
            let pm_log = permethod::merge_per_method(
                &data,
                merged_dir.as_ref().unwrap(),
                &mut per_iso,
                &api_cp,
                &out.join("_pm"),
            );
            for l in &pm_log {
                println!("  {l}");
            }
            merge_log.extend(pm_log);
            merged_total = Some(per_iso.values().sum());
        }
    }

    // ── reject-semantic-drift (optional, off by default) ────────────────────
    // An ADDITIVE post-merge guard.  For every class the merge log marked as
    // autofixed, we re-derive the best NON-autofixed variant; if the autofixed
    // class's recompiled bytecode drifts from the original jar's bytecode while the
    // non-autofixed variant does NOT (and reverting does not RAISE that class's
    // isolated error count), we revert it.  This only ever discards a semantics-
    // changing autofix — it can never accept a worse result, so it strengthens (never
    // relaxes) the existing "fix accepted only if strictly fewer errors" invariant.
    // core.rs's merge decision logic is untouched; this lives entirely in the CLI.
    let mut drift_reverts: Vec<String> = Vec::new();
    if a.reject_semantic_drift
        && merged_dir.is_some()
        && rename_map.is_some()
        && crate::semdiff::javap_available()
    {
        let md = merged_dir.as_ref().unwrap();
        println!("\n== REJECT-SEMANTIC-DRIFT (autofix re-verification) ==");
        let api_cp = core::build_api_classpath(&jar, classpath.as_deref(), &out);
        let names: Vec<&String> = names_rank.iter().map(|(n, _)| *n).collect();
        let known: HashSet<&String> =
            names.iter().flat_map(|n| data[*n].files.keys()).collect();
        // classes the merge autofixed (merge_log line: "REL: <- ENG+autofix(...)")
        let mut autofixed: Vec<String> = merge_log
            .iter()
            .filter(|l| l.contains("autofix") && l.contains(':'))
            .filter_map(|l| l.trim().split(':').next().map(|r| r.trim().to_string()))
            .filter(|r| known.contains(r))
            .collect();
        autofixed.sort();
        autofixed.dedup();
        let drift_work = out.join("_semdiff_drift");
        for rel in &autofixed {
            let tgt = md.join(rel);
            if !tgt.exists() {
                continue;
            }
            let after_text = match util::read_text_lossy(&tgt) {
                Ok(t) => t,
                Err(_) => continue,
            };
            // best non-autofixed variant = the engine variant with the lowest
            // isolated score (no autofix applied)
            let mut best_raw: Option<String> = None;
            let mut best_raw_score: Option<i64> = None;
            for n in &names {
                let f = match data[*n].files.get(rel) {
                    Some(f) => f,
                    None => continue,
                };
                let sc = core::compile_file_isolated(f, &api_cp, &drift_work.join("score"));
                if best_raw_score.map(|b| sc < b).unwrap_or(true) {
                    match util::read_text_lossy(f) {
                        Ok(t) => {
                            best_raw_score = Some(sc);
                            best_raw = Some(t);
                        }
                        Err(_) => continue,
                    }
                }
            }
            let best_raw = match best_raw {
                Some(t) => t,
                None => continue,
            };
            if !crate::semdiff::variant_drifts(
                &jar,
                &best_raw,
                &after_text,
                rel,
                &api_cp,
                &drift_work,
                rename_map.as_deref(),
            ) {
                continue;
            }
            // autofix drifted; only revert if the raw variant is NOT worse in isolation
            let cur_score = per_iso.get(rel).copied().unwrap_or(0);
            match best_raw_score {
                Some(brs) if brs <= cur_score => {
                    let _ = std::fs::write(&tgt, &best_raw);
                    per_iso.insert(rel.clone(), brs);
                    drift_reverts.push(rel.clone());
                    println!(
                        "  {rel}: autofix DISCARDED (semantic drift; reverted to raw \
                         variant, isolated errors {cur_score}->{brs})"
                    );
                }
                _ => {
                    println!(
                        "  {rel}: autofix flagged as drifting but raw variant scores \
                         worse ({} > {cur_score}) — KEPT (invariant: never accept a \
                         worse result)",
                        best_raw_score.map(|v| v.to_string()).unwrap_or_else(|| "None".into())
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(&drift_work);
        if drift_reverts.is_empty() {
            println!("  no autofixed class reverted.");
        }
        merged_total = Some(per_iso.values().sum());
    }

    // round-trip
    let mut rt: Option<RoundTrip> = None;
    if let Some(md) = &merged_dir {
        if !a.no_roundtrip {
            println!("\n== ROUND-TRIP CHECK ==");
            let broken: HashSet<String> =
                per_iso.iter().filter(|(_, e)| **e != 0).map(|(r, _)| r.clone()).collect();
            let (ndrop, dropped) =
                core::compile_best_effort(md, Some(&jar), classpath.as_deref(), Some(&broken));
            if !dropped.is_empty() {
                let mut ds: Vec<String> = dropped.iter().cloned().collect();
                ds.sort();
                println!(
                    "  ({ndrop} class(es) still broken, excluded from coverage: {})",
                    ds.join(", ")
                );
            }
            let mut r = crate::roundtrip::check(&jar, &md.join("_classes"));
            let mut ds: Vec<String> = dropped.iter().cloned().collect();
            ds.sort();
            r.excluded_broken = ds;
            println!(
                "  coverage: {}/{} classes ({}%)",
                r.present, r.total_original, fmt_pct(r.coverage_pct)
            );
            if !r.missing.is_empty() {
                println!("  MISSING compiled classes: {}", r.missing.join(", "));
            }
            if !r.structural_mismatch.is_empty() {
                println!(
                    "  structural mismatches: {} (member/method counts differ from original bytecode)",
                    r.structural_mismatch.len()
                );
                for m in r.structural_mismatch.iter().take(10) {
                    println!(
                        "    {}: orig({}, {}) merged({}, {})",
                        m.class, m.orig.0, m.orig.1, m.merged.0, m.merged.1
                    );
                }
            }
            rt = Some(r);
        }
    }

    // ── семантический round-trip (по желанию, по умолчанию выключен) ────────
    let mut semdiff_report: Option<crate::semdiff::SemdiffReport> = None;
    if let (true, Some(md), false) =
        (a.semdiff || a.reject_semantic_drift, merged_dir.as_ref(), a.no_roundtrip)
    {
        println!("\n== SEMANTIC ROUND-TRIP (semdiff) ==");
        let classes_dir = md.join("_classes");
        // пропускаем классы, которые не пересобрались (их .class — засеянный
        // ОРИГИНАЛ, самосравнение было бы пустым и обманчивым)
        let skip: BTreeSet<String> = rt
            .as_ref()
            .map(|r| {
                r.excluded_broken
                    .iter()
                    .map(|rel| {
                        rel.strip_suffix(".java").unwrap_or(rel).replace(std::path::MAIN_SEPARATOR, "/")
                    })
                    .collect()
            })
            .unwrap_or_default();
        let sem = crate::semdiff::check(&jar, &classes_dir, rename_map.as_deref(), &skip);
        if !sem.available {
            println!("  javap unavailable — semdiff skipped (no-op).");
        } else {
            println!(
                "  checked {} class(es); skipped {} (not recompiled)",
                sem.checked,
                sem.skipped.len()
            );
            if !drift_reverts.is_empty() {
                println!(
                    "  reverted {} drifted autofix(es): {}",
                    drift_reverts.len(),
                    drift_reverts.join(", ")
                );
            }
            if sem.total_flagged_methods == 0 {
                println!("  no semantic drift flagged.");
            } else {
                println!(
                    "  FLAGGED {} method(s) across {} class(es):",
                    sem.total_flagged_methods,
                    sem.flagged_classes.len()
                );
                for fc in &sem.flagged_classes {
                    for m in &fc.flagged {
                        println!("    {}::{} — {}", fc.class, m.method, m.reason);
                        if let Some(crate::semdiff::OnlyOrig::Refs(v)) = &m.only_orig {
                            if !v.is_empty() {
                                println!("        only-orig: [{}]", v.join(", "));
                            }
                        }
                        if let Some(v) = &m.only_recompiled {
                            if !v.is_empty() {
                                println!("        only-recompiled: [{}]", v.join(", "));
                            }
                        }
                    }
                }
            }
        }
        semdiff_report = Some(sem);
    }
    // quality metrics (syntactic correctness / cross-decompiler agreement / behavioral)
    let mut metrics: Option<crate::metrics::Metrics> = None;
    if let Some(md) = &merged_dir {
        println!("\n== QUALITY METRICS ==");
        let m = crate::metrics::compute(
            &data,
            md,
            &per_iso,
            rt.as_ref(),
            &jar,
            classpath.as_deref(),
            &root,
        );
        for line in m.summary_lines() {
            println!("  {line}");
        }
        metrics = Some(m);
    }

    // deobf
    let mut deobf_info: Option<serde_json::Value> = None;
    if a.deobf {
        if let Some(md) = &merged_dir {
            println!("\n== DEOBFUSCATION (readability) ==");
            let n = deobf::annotate_in_place(md);
            let deobf_dir = out.join("deobf");
            let jadx_bin = root.join("jadx").join("bin").join("jadx");
            let emitted = deobf::emit_jadx_deobf(&jar, &jadx_bin, &deobf_dir);
            deobf_info = Some(json!({
                "annotated": n,
                "jadx_deobf_tree": if emitted { serde_json::Value::String(deobf_dir.to_string_lossy().into_owned()) } else { serde_json::Value::Null },
            }));
            println!("  annotated {n} classified file(s) in merged/");
            println!(
                "  jadx --deobf tree: {}",
                if emitted {
                    format!("-> {}", deobf_dir.display())
                } else {
                    "unavailable".into()
                }
            );
        }
    }

    // reports
    let best_base = names_rank.first().map(|(n, _)| (*n).clone());
    let report = build_report_json(
        &jar,
        &orig_jar,
        renamed,
        rename_mode,
        a.flatten_packages,
        rename_map.as_deref(),
        &ranking,
        &best_base,
        merged_total,
        &merge_log,
        &per_class,
        &data,
        rt.as_ref(),
        deobf_info.as_ref(),
        metrics.as_ref(),
        semdiff_report.as_ref(),
        &drift_reverts,
    );
    let _ = std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );
    write_md(
        &out, &jar, &ranking, &best_base, merged_total, &merge_log, &per_class, &all_classes,
        rt.as_ref(), metrics.as_ref(),
    );
    println!(
        "\n{}",
        tf("done_merged", &[&merged_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "None".into())])
    );
    println!(
        "{}",
        tf(
            "reports_at",
            &[
                &out.join("report.md").display().to_string(),
                &out.join("report.json").display().to_string()
            ]
        )
    );
    0
}

struct PerClass {
    clean_engines: Vec<String>,
    errors: Vec<(String, Option<i64>)>,
}

#[allow(clippy::too_many_arguments)]
fn build_report_json(
    jar: &Path,
    orig_jar: &Path,
    renamed: bool,
    rename_mode: &str,
    flatten_packages: bool,
    rename_map: Option<&Path>,
    ranking: &[(&String, &core::EngineData)],
    best_base: &Option<String>,
    merged_total: Option<i64>,
    merge_log: &[String],
    per_class: &BTreeMap<String, PerClass>,
    data: &BTreeMap<String, core::EngineData>,
    rt: Option<&RoundTrip>,
    deobf_info: Option<&serde_json::Value>,
    metrics: Option<&crate::metrics::Metrics>,
    semdiff: Option<&crate::semdiff::SemdiffReport>,
    drift_reverts: &[String],
) -> serde_json::Value {
    let ranking_json: Vec<serde_json::Value> = ranking
        .iter()
        .map(|(n, d)| json!({"engine": n, "classes": d.nclasses, "errors": d.errors}))
        .collect();

    let mut pc = serde_json::Map::new();
    for (c, v) in per_class {
        let mut errs = serde_json::Map::new();
        // preserve `for n in data` order
        for (n, d) in data {
            if d.files.contains_key(c) {
                let val = v
                    .errors
                    .iter()
                    .find(|(en, _)| en == n)
                    .and_then(|(_, e)| *e);
                errs.insert(
                    n.clone(),
                    match val {
                        Some(x) => json!(x),
                        None => serde_json::Value::Null,
                    },
                );
            }
        }
        pc.insert(
            c.clone(),
            json!({"clean_engines": v.clean_engines, "errors": serde_json::Value::Object(errs)}),
        );
    }

    json!({
        "version": VERSION,
        "jar": jar.to_string_lossy(),
        "original_jar": orig_jar.to_string_lossy(),
        "member_rename": {
            "applied": renamed,
            "mode": rename_mode,
            "flatten_packages": flatten_packages,
            "renamed_jar": if renamed { serde_json::Value::String(jar.to_string_lossy().into_owned()) } else { serde_json::Value::Null },
            "mapping": match rename_map { Some(p) => serde_json::Value::String(p.to_string_lossy().into_owned()), None => serde_json::Value::Null },
        },
        "ranking": ranking_json,
        "best_base": match best_base { Some(b) => serde_json::Value::String(b.clone()), None => serde_json::Value::Null },
        "merged_errors": match merged_total { Some(t) => json!(t), None => serde_json::Value::Null },
        "merge_log": merge_log,
        "per_class": serde_json::Value::Object(pc),
        "roundtrip": match rt { Some(r) => serde_json::to_value(r).unwrap_or(serde_json::Value::Null), None => serde_json::Value::Null },
        "deobf": match deobf_info { Some(d) => d.clone(), None => serde_json::Value::Null },
        "metrics": match metrics { Some(m) => serde_json::to_value(m).unwrap_or(serde_json::Value::Null), None => serde_json::Value::Null },
        "semdiff": match semdiff { Some(m) => serde_json::to_value(m).unwrap_or(serde_json::Value::Null), None => serde_json::Value::Null },
        "semantic_drift_reverts": drift_reverts,
    })
}

#[allow(clippy::too_many_arguments)]
fn write_md(
    out: &Path,
    jar: &Path,
    ranking: &[(&String, &core::EngineData)],
    best_base: &Option<String>,
    merged_total: Option<i64>,
    merge_log: &[String],
    per_class: &BTreeMap<String, PerClass>,
    all_classes: &BTreeSet<String>,
    rt: Option<&RoundTrip>,
    metrics: Option<&crate::metrics::Metrics>,
) {
    let mut md: Vec<String> = vec![
        format!("# Arlecchino — report (v{VERSION})"),
        format!("jar: `{}`", file_name(jar)),
        String::new(),
        "## Engine ranking".into(),
        String::new(),
        "| engine | classes | errors |".into(),
        "|---|---|---|".into(),
    ];
    for (n, d) in ranking {
        md.push(format!("| {} | {} | {} |", n, d.nclasses, d.errors));
    }
    md.push(String::new());
    md.push(format!(
        "**best base:** {}",
        best_base.clone().unwrap_or_else(|| "None".into())
    ));
    md.push(format!("**merged errors:** {}", opt_i64(merged_total)));
    md.push(String::new());
    md.push("## Merge log".into());
    md.push("```".into());
    md.extend(merge_log.iter().cloned());
    md.push("```".into());
    md.push(String::new());
    if let Some(r) = rt {
        md.push("## Round-trip".into());
        md.push(String::new());
        md.push(format!(
            "- coverage: {}/{} ({}%)",
            r.present, r.total_original, fmt_pct(r.coverage_pct)
        ));
        md.push(format!(
            "- missing: {}",
            if r.missing.is_empty() {
                "none".into()
            } else {
                r.missing.join(", ")
            }
        ));
        md.push(format!(
            "- structural mismatches: {}",
            r.structural_mismatch.len()
        ));
        md.push(String::new());
    }
    if let Some(m) = metrics {
        md.push("## Quality metrics".into());
        md.push(String::new());
        let s = &m.syntactic_correctness;
        let a = &m.cross_decompiler_agreement;
        let b = &m.behavioral_equivalence;
        md.push("### 1. Syntactic correctness".into());
        md.push(format!(
            "- recompilable: {}/{} classes ({}%)",
            s.recompilable_classes,
            s.total_classes,
            fmt_pct(s.recompilable_ratio * 100.0)
        ));
        md.push(format!(
            "- isolated errors: {} — whole-tree errors: {}",
            s.total_isolated_errors, s.whole_tree_errors
        ));
        md.push(String::new());
        md.push("### 2. Cross-decompiler agreement (distortion proxy)".into());
        md.push(format!(
            "- mean agreement: {:.3} over {} class(es)",
            a.mean_agreement, a.classes_scored
        ));
        md.push(format!(
            "- chosen-fragment distortion (distance from consensus): {:.3}",
            a.chosen_mean_distortion
        ));
        md.push(String::new());
        md.push("### 3. Behavioral / semantic equivalence".into());
        md.push(format!(
            "- structural round-trip: {}/{} classes match original bytecode ({}%)",
            b.structural_roundtrip.structurally_matching,
            b.structural_roundtrip.total_original,
            fmt_pct(b.structural_roundtrip.match_ratio * 100.0)
        ));
        md.push(format!(
            "- cross-engine bytecode consensus: {:.3} over {} class(es)",
            b.cross_engine_bytecode_consensus.mean_agreement,
            b.cross_engine_bytecode_consensus.classes_scored
        ));
        md.push(format!(
            "- runtime executed: {} (emulator available: {}) — {}",
            b.runtime.executed, b.runtime.emulator_available, b.runtime.note
        ));
        md.push(String::new());
    }
    md.push("## Classes nobody compiles cleanly".into());
    for c in all_classes {
        let pc = &per_class[c];
        if pc.clean_engines.is_empty() {
            md.push(format!("- `{}` — errors: {}", c, py_dict_repr(&pc.errors)));
        }
    }
    let _ = std::fs::write(out.join("report.md"), md.join("\n"));
}

fn py_dict_repr(errors: &[(String, Option<i64>)]) -> String {
    let inner: Vec<String> = errors
        .iter()
        .map(|(k, v)| {
            let vs = match v {
                Some(x) => x.to_string(),
                None => "None".into(),
            };
            format!("'{k}': {vs}")
        })
        .collect();
    format!("{{{}}}", inner.join(", "))
}

/// Format a rounded percentage like Python `str(round(x, 1))` (keeps one decimal).
fn fmt_pct(x: f64) -> String {
    format!("{x:.1}")
}

fn opt_i64(v: Option<i64>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "None".into(),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

// ───────────────────────────── setup ────────────────────────────────────────
fn cmd_setup(a: &SetupArgs) -> i32 {
    let root = a
        .engines_root
        .as_deref()
        .map(util::expanduser)
        .unwrap_or_else(engines_root_default);
    let only: Option<Vec<String>> = a
        .only
        .as_ref()
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect());
    println!("== Arlecchino setup -> {} ==", root.display());
    let res = setup_engines(&root, only.as_deref());
    for (k, v) in &res {
        println!("  {k:12} {v}");
    }
    // `--only` называет ровно те движки, что нужны: FreeJ2ME он тоже фильтрует,
    // иначе `setup --only midp-rt` без спроса лезет в сеть за эмулятором.
    let want_freej2me = only
        .as_deref()
        .map(|o| o.iter().any(|x| x == "freej2me"))
        .unwrap_or(true);
    if !a.no_freej2me && want_freej2me {
        println!("  freej2me     ...");
        let fr = setup_freej2me(
            Some(&root),
            a.freej2me_prebuilt.as_deref().map(Path::new),
            a.freej2me_src.as_deref().map(Path::new),
        );
        println!("  freej2me     {fr}");
    }
    println!("\n{}", tf("setup_tip", &[]));
    0
}

// ───────────────────────────── emulate ──────────────────────────────────────
fn cmd_emulate(a: &EmulateArgs) -> i32 {
    let jar = util::absolutize(Path::new(&a.jar));
    if !jar.exists() {
        println!("{}", tf("jar_not_found", &[&jar.to_string_lossy()]));
        return 2;
    }
    let fj = match find_freej2me(a.freej2me.as_deref(), None) {
        Some(f) => f,
        None => {
            println!("{}", tf("freej2me_not_found", &[]));
            return 1;
        }
    };
    emulate(&jar, &fj, a.width, a.height, a.scale, None)
}

// ───────────────────────────── entry ────────────────────────────────────────
/// Program entry.  `argv` excludes the program name (like Python `sys.argv[1:]`).
pub fn main(argv: Option<Vec<String>>) -> i32 {
    let mut argv: Vec<String> = argv.unwrap_or_else(|| std::env::args().skip(1).collect());

    // translate the argparse-style `-cp` long option to clap's `--classpath`
    for a in argv.iter_mut() {
        if a == "-cp" {
            *a = "--classpath".into();
        }
    }

    // backward-compat: `arlecchino foo.jar ...` == `arlecchino decompile foo.jar ...`
    if let Some(first) = argv.first() {
        if !first.starts_with('-')
            && !["decompile", "setup", "emulate", "run", "difftest"].contains(&first.as_str())
        {
            argv.insert(0, "decompile".into());
        }
    }

    let mut full = vec!["arlecchino".to_string()];
    full.extend(argv);
    let cli = match Cli::try_parse_from(&full) {
        Ok(c) => c,
        Err(e) => {
            // clap prints help/version/errors and sets an exit code
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };

    match cli.cmd {
        Some(Cmd::Decompile(a)) => cmd_decompile(&a),
        Some(Cmd::Setup(a)) => cmd_setup(&a),
        Some(Cmd::Emulate(a)) => cmd_emulate(&a),
        Some(Cmd::Run(a)) => cmd_run(&a),
        Some(Cmd::Difftest(a)) => cmd_difftest(&a),
        None => {
            let mut cmd = <Cli as clap::CommandFactory>::command();
            let _ = cmd.print_help();
            1
        }
    }
}

// ───────────────────────────── run (midp-rt) ────────────────────────────────
fn load_inputs(path: Option<&str>) -> Result<Option<Vec<crate::diffrun::InputEvent>>, String> {
    match path {
        None => Ok(None),
        Some(p) => {
            let text = util::read_text_lossy(Path::new(p)).map_err(|e| e.to_string())?;
            let v: Vec<crate::diffrun::InputEvent> =
                serde_json::from_str(&text).map_err(|e| format!("bad input script {p}: {e}"))?;
            Ok(Some(v))
        }
    }
}

fn parse_size(s: &str) -> Option<(i64, i64)> {
    let low = s.to_lowercase();
    let (w, h) = low.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

fn cmd_run(a: &RunArgs) -> i32 {
    use crate::midprun;
    let game = util::absolutize(Path::new(&a.jar));
    if !game.exists() {
        println!("jar/jad not found: {}", game.display());
        return 2;
    }
    let midp_rt = match midprun::find_midp_rt(a.midp_rt.as_deref(), None) {
        Some(p) => p,
        None => {
            let checked = match a.midp_rt.as_deref() {
                Some(p) => vec![util::expanduser(p)],
                None => Vec::new(),
            };
            println!("{}", midprun::missing_hint(&checked, None));
            return 1;
        }
    };

    let (mut width, mut height) = (None, None);
    if let Some(sz) = &a.size {
        match parse_size(sz) {
            Some((w, h)) => {
                width = Some(w);
                height = Some(h);
            }
            None => {
                println!("bad --size {sz:?} (expected WxH, e.g. 240x320)");
                return 2;
            }
        }
    }

    let rms_dir = a.rms_dir.as_deref().map(|d| {
        let p = util::absolutize(&util::expanduser(d));
        let _ = std::fs::create_dir_all(&p);
        p
    });

    if a.headless {
        let inputs = match load_inputs(a.inputs.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                println!("{e}");
                return 2;
            }
        };
        let out = util::absolutize(Path::new(&a.out));
        println!(
            "== arlecchino run --headless :: {} via {} ==",
            file_name(&game),
            midp_rt.display()
        );
        let opts = crate::diffrun::RunOpts {
            max_frames: a.frames,
            frame_period_ms: a.period_ms,
            boot_wait_s: a.boot_wait,
            // only used for the run timeout here; the device size is passed
            // separately below so that "no --size" stays unset (see `size`).
            width: width.unwrap_or(crate::diffrun::DEFAULT_W),
            height: height.unwrap_or(crate::diffrun::DEFAULT_H),
            rms_dir,
            deterministic_capture: false,
        };
        // No --size => no --resizableDevice => midp-rt's own device default
        // (176x220), exactly as in the Python original.  Forcing 240x320 here
        // changed every captured frame and its sha256.
        let size = match (width, height) {
            (Some(w), Some(h)) => Some((w, h)),
            _ => None,
        };
        let res = match midprun::run_headless_midp_rt_sized(
            &game,
            &out,
            inputs.as_deref(),
            &opts,
            Some(&midp_rt.to_string_lossy()),
            !a.no_instrument,
            size,
        ) {
            Ok(r) => r,
            Err(e) => {
                println!("headless run failed: {e}");
                return 1;
            }
        };
        for f in &res.frames {
            println!(
                "  frame {:03} {}x{} {} sha256={}",
                f.index,
                f.width,
                f.height,
                if f.blank { "BLANK" } else { "data " },
                &f.sha256[..16.min(f.sha256.len())]
            );
        }
        let uniq: BTreeSet<&String> = res.frames.iter().map(|f| &f.sha256).collect();
        println!(
            "  captured {} frames, unique states: {}, non-empty: {}",
            res.frames.len(),
            uniq.len(),
            res.any_nonblank()
        );
        println!("  frames + index.json: {}", out.display());
        return if res.any_nonblank() { 0 } else { 1 };
    }

    let trace = a.trace.as_deref().map(|t| {
        let p = util::absolutize(&util::expanduser(t));
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        println!("   MIDP call trace -> {}", p.display());
        p
    });
    println!("== arlecchino run :: {} via {} ==", file_name(&game), midp_rt.display());
    midprun::run_midlet(
        &game,
        &midp_rt,
        &midprun::WindowOpts {
            width,
            height,
            rms_dir,
            instrument: !a.no_instrument,
            trace,
        },
    )
}

// ───────────────────────────── difftest ─────────────────────────────────────
/// Этап A5 — стенд поведенческого сравнения (PLAN_runtime.md, блок A).
///
/// Обычный режим: откалибровать шум на ОРИГИНАЛЕ (прогнать его дважды), прогнать
/// пересобранный jar, сравнить последовательности состояний экрана + RMS, вердикт.
/// `--calibrate`: только оригинал, дважды — печатает базу шума вместо вердикта.
fn cmd_difftest(a: &DifftestArgs) -> i32 {
    use crate::difftest as dt;
    let orig = util::absolutize(Path::new(&a.orig));
    if !orig.exists() {
        println!("jar not found: {}", orig.display());
        return 2;
    }
    let mut rebuilt = PathBuf::new();
    if !a.calibrate && !a.cross_engine {
        match &a.rebuilt {
            None => {
                println!("difftest needs <rebuilt.jar> (or use --calibrate)");
                return 2;
            }
            Some(r) => {
                rebuilt = util::absolutize(Path::new(r));
                if !rebuilt.exists() {
                    println!("jar not found: {}", rebuilt.display());
                    return 2;
                }
            }
        }
    }
    if !dt::ENGINES.contains(&a.engine.as_str()) {
        println!("unknown --engine {:?} (choose from {:?})", a.engine, dt::ENGINES);
        return 2;
    }

    let inputs = match load_inputs(a.inputs.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            println!("{e}");
            return 2;
        }
    };
    let out = util::absolutize(Path::new(&a.out));
    let opts = crate::diffrun::RunOpts {
        max_frames: a.frames,
        frame_period_ms: a.period_ms,
        boot_wait_s: a.boot_wait,
        width: a.width,
        height: a.height,
        rms_dir: None,
        deterministic_capture: false,
    };

    let fixed_clock = a.fixed_clock || crate::fixedclock::env_enabled();
    if fixed_clock {
        println!(
            "   [A7] fixed-clock ON — instrumenting jar(s) with a virtual clock + \
             fixed-seeded RNG (frame-deterministic replay)."
        );
    }

    if a.cross_engine {
        println!(
            "== difftest --cross-engine :: {} (freej2me vs midp-rt, same inputs) ==",
            file_name(&orig)
        );
        let (cmp, run_a, run_b) = match dt::cross_engine(
            &orig,
            &out,
            inputs.as_deref(),
            ("freej2me", "midp-rt"),
            a.state_tol,
            fixed_clock,
            &opts,
        ) {
            Ok(v) => v,
            Err(e) => {
                println!("difftest unavailable: {e}");
                return 2;
            }
        };
        for r in [&run_a, &run_b] {
            let status = match &r.error {
                Some(e) => format!("FAILED: {e}"),
                None => format!(
                    "{} frames, {} unique states, non-blank={}, rms stores={}",
                    r.hashes.len(),
                    r.hashes.iter().collect::<BTreeSet<_>>().len(),
                    r.nonblank,
                    {
                        let n = dt::rms_store_names(r);
                        if n.is_empty() {
                            "-".to_string()
                        } else {
                            format!("['{}']", n.iter().cloned().collect::<Vec<_>>().join("', '"))
                        }
                    }
                ),
            };
            println!("  {}: {status}", r.label);
        }
        println!(
            "\n== CROSS-ENGINE: {} ==",
            if cmp.agree { "AGREE (structure matches softly)" } else { "DIVERGE" }
        );
        for r in &cmp.reasons {
            println!("  ✗ {r}");
        }
        for n in &cmp.notes {
            println!("  ℹ {n}");
        }
        println!("  report: {}", out.join("cross_engine_report.md").display());
        return if cmp.agree { 0 } else { 1 };
    }

    if a.calibrate {
        println!(
            "== difftest --calibrate :: {} (2 runs, engine {}) ==",
            file_name(&orig),
            a.engine
        );
        let (cmp, run_a, run_b) =
            match dt::calibrate(&orig, &out, inputs.as_deref(), &a.engine, fixed_clock, &opts) {
                Ok(v) => v,
                Err(e) => {
                    println!("difftest unavailable: {e}");
                    return 2;
                }
            };
        for r in [&run_a, &run_b] {
            if let Some(e) = &r.error {
                println!("  run {} FAILED: {e}", r.label);
                return 1;
            }
            println!(
                "  {}: {} frames, {} unique states, non-blank={}, rms files={}",
                r.label,
                r.hashes.len(),
                r.hashes.iter().collect::<BTreeSet<_>>().len(),
                r.nonblank,
                r.rms.len()
            );
        }
        println!("\n  noise baseline (same jar, two runs):");
        println!(
            "    state sets equal:      {} (Jaccard {:.2})",
            cmp.sets_equal(),
            cmp.jaccard
        );
        println!(
            "    first-appearance order: {}",
            if cmp.order_ok { "same" } else { "DIFFERS" }
        );
        println!("    max timing shift:       {} frame(s)", cmp.max_shift);
        println!(
            "    frame-by-frame match:   {:.0}% (timing-sensitive, expected <100%)",
            cmp.frame_match_rate * 100.0
        );
        println!("    RMS byte-identical:     {}", run_a.rms == run_b.rms);
        return 0;
    }

    println!(
        "== difftest :: {} vs {} (engine {}) ==",
        file_name(&orig),
        file_name(&rebuilt),
        a.engine
    );
    let verdict = match dt::difftest(
        &orig,
        &rebuilt,
        &out,
        inputs.as_deref(),
        a.shift_tol,
        a.jaccard_margin,
        !a.no_baseline,
        &a.engine,
        fixed_clock,
        &opts,
    ) {
        Ok(v) => v,
        Err(e) => {
            println!("difftest unavailable: {e}");
            return 2;
        }
    };
    println!(
        "\n== VERDICT: {} ==",
        if verdict.passed { "PASS (matches within noise)" } else { "FAIL" }
    );
    for r in &verdict.reasons {
        println!("  ✗ {r}");
    }
    for n in &verdict.notes {
        println!("  ℹ {n}");
    }
    if let Some(b) = &verdict.baseline {
        println!(
            "  baseline (orig vs orig): Jaccard {:.2}, shift {}, order {}",
            b.jaccard,
            b.max_shift,
            if b.order_ok { "ok" } else { "unstable" }
        );
    }
    let c = &verdict.cross;
    println!(
        "  cross (orig vs rebuilt):  Jaccard {:.2}, shift {}, order {}",
        c.jaccard,
        c.max_shift,
        if c.order_ok { "ok" } else { "DIFFERS" }
    );
    println!("  report: {}", out.join("difftest_report.md").display());
    if verdict.passed {
        0
    } else {
        1
    }
}
