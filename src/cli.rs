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
}

#[derive(Args)]
struct SetupArgs {
    #[arg(long = "engines-root")]
    engines_root: Option<String>,
    /// comma list: vineflower,cfr,procyon,jadx
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
        rename_map.as_deref(),
        &ranking,
        &best_base,
        merged_total,
        &merge_log,
        &per_class,
        &data,
        rt.as_ref(),
        deobf_info.as_ref(),
    );
    let _ = std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );
    write_md(
        &out, &jar, &ranking, &best_base, merged_total, &merge_log, &per_class, &all_classes,
        rt.as_ref(),
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
    rename_map: Option<&Path>,
    ranking: &[(&String, &core::EngineData)],
    best_base: &Option<String>,
    merged_total: Option<i64>,
    merge_log: &[String],
    per_class: &BTreeMap<String, PerClass>,
    data: &BTreeMap<String, core::EngineData>,
    rt: Option<&RoundTrip>,
    deobf_info: Option<&serde_json::Value>,
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
    if !a.no_freej2me {
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
        if !first.starts_with('-') && !["decompile", "setup", "emulate"].contains(&first.as_str()) {
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
        None => {
            let mut cmd = <Cli as clap::CommandFactory>::command();
            let _ = cmd.print_help();
            1
        }
    }
}
