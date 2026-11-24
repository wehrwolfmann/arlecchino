//! `arlecchino port-rust`: mechanical translation of a decompiled J2ME game to Rust.
//!
//! The pipeline is
//!
//! 1. `assets/JavacAstDump.java` (a small helper run on the JDK, like the member
//!    renamer) parses and attributes the Java tree with `javac` and writes it as
//!    JSON: the type of every expression, the owner and descriptor of every call,
//!    the operand types of every operator, folded constants, jump targets;
//! 2. [`gen`] turns that into a Cargo crate on top of the `jrt` runtime crate
//!    (`jrt/` in this repository), binding platform calls through the API table
//!    (`assets/port-rust-api.tsv`) and keeping every name of the input (optionally
//!    renamed by a `.map` name map, see [`names`]);
//! 3. whatever it cannot translate becomes a typed `jrt::todo_ai!`, so the crate
//!    still builds, and is listed in `residue.json` and `report.txt`.
//!
//! The crate has a `trace` binary that runs the game headless and writes a call
//! trace in midp-rt's format; [`trace`] compares it with the trace of the original.
//! With `--frames-dir` it also draws every frame (`jrt::raster`), and [`frames`]
//! compares those pixel by pixel with midp-rt's capture.

pub mod frames;
pub mod gen;
pub mod names;
pub mod trace;

use crate::util;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const AST_DUMP_CLASS: &str = "JavacAstDump";
pub const AST_DUMP_SOURCE: &str = include_str!("../../assets/JavacAstDump.java");
/// The built-in API table.
pub const API_TABLE: &str = include_str!("../../assets/port-rust-api.tsv");
/// Where the published `jrt` crate lives when no local checkout is available.
pub const JRT_REPOSITORY: &str = "https://github.com/wehrwolfmann/arlecchino";

/// javac internals the helper reads.
const JAVAC_EXPORTS: &[&str] = &[
    "jdk.compiler/com.sun.tools.javac.code=ALL-UNNAMED",
    "jdk.compiler/com.sun.tools.javac.tree=ALL-UNNAMED",
    "jdk.compiler/com.sun.tools.javac.util=ALL-UNNAMED",
    "jdk.compiler/com.sun.tools.javac.api=ALL-UNNAMED",
];

fn export_args() -> Vec<String> {
    JAVAC_EXPORTS.iter().flat_map(|e| ["--add-exports".to_string(), e.to_string()]).collect()
}

fn helper_dir() -> PathBuf {
    crate::midp_stub::arlecchino_home().join("helpers").join("port-rust")
}

/// Compiles the AST helper into `~/.arlecchino/helpers/port-rust` (again only when
/// the embedded source changed).  Returns the class directory.
///
/// Safe to call concurrently: the helper is built in a private directory that then
/// replaces the shared one in a single rename.
pub fn ensure_ast_helper() -> Result<PathBuf, String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = helper_dir();
    let is_fresh = |d: &Path| {
        d.join(format!("{AST_DUMP_CLASS}.class")).exists()
            && std::fs::read_to_string(d.join(format!("{AST_DUMP_CLASS}.java"))).map(|s| s == AST_DUMP_SOURCE).unwrap_or(false)
    };
    if is_fresh(&dir) {
        return Ok(dir);
    }
    let parent = dir.parent().unwrap_or(&dir).to_path_buf();
    let build = parent.join(format!(".port-rust-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&build);
    std::fs::create_dir_all(&build).map_err(|e| format!("{}: {e}", build.display()))?;
    let src = build.join(format!("{AST_DUMP_CLASS}.java"));
    std::fs::write(&src, AST_DUMP_SOURCE).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut argv = vec![util::tool("ARLECCHINO_JAVAC", "javac"), "-nowarn".into(), "-proc:none".into()];
    argv.extend(export_args());
    argv.extend(["-d".into(), build.to_string_lossy().into_owned(), src.to_string_lossy().into_owned()]);
    let r = util::run_capture_in(&argv, Some(Duration::from_secs(300)), Some(&build));
    if !(r.success() && is_fresh(&build)) {
        let _ = std::fs::remove_dir_all(&build);
        return Err(format!("javac of the AST helper failed: {}{}", r.stderr.trim(), r.spawn_err.unwrap_or_default()));
    }
    if is_fresh(&dir) {
        // another process finished first
        let _ = std::fs::remove_dir_all(&build);
        return Ok(dir);
    }
    let _ = std::fs::remove_dir_all(&dir);
    match std::fs::rename(&build, &dir) {
        Ok(()) => Ok(dir),
        Err(_) if is_fresh(&dir) => {
            let _ = std::fs::remove_dir_all(&build);
            Ok(dir)
        }
        Err(e) => Err(format!("{}: {e}", dir.display())),
    }
}

/// `*.java` under `dir`, sorted, recursively.
pub fn java_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "java") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Runs the helper over `sources` and returns the parsed AST.  javac errors in the
/// tree are reported but not fatal: the helper writes what it attributed.
pub fn dump_ast(sources: &[PathBuf], classpath: &str, json_out: &Path) -> Result<(serde_json::Value, usize), String> {
    let helper = ensure_ast_helper()?;
    let mut argv = vec![util::tool("ARLECCHINO_JAVA", "java")];
    argv.extend(export_args());
    argv.extend(["-cp".into(), helper.to_string_lossy().into_owned(), AST_DUMP_CLASS.into()]);
    argv.push(if classpath.is_empty() { ".".into() } else { classpath.to_string() });
    argv.push(json_out.to_string_lossy().into_owned());
    argv.extend(sources.iter().map(|p| p.to_string_lossy().into_owned()));
    let r = util::run_capture_in(&argv, Some(Duration::from_secs(900)), json_out.parent());
    let javac_errors = r.stderr.lines().filter(|l| l.contains("error:")).count();
    if !json_out.exists() {
        return Err(format!("the AST helper wrote nothing: {}{}", r.stderr.trim(), r.spawn_err.unwrap_or_default()));
    }
    let text = std::fs::read_to_string(json_out).map_err(|e| e.to_string())?;
    let ast = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", json_out.display()))?;
    Ok((ast, javac_errors))
}

/// `.map` files: each path is a file, or a directory whose `*.map` files are read.
pub fn read_maps(paths: &[PathBuf]) -> Result<Vec<names::MapClass>, String> {
    let mut out = Vec::new();
    for p in paths {
        let files: Vec<PathBuf> = if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(p)
                .map_err(|e| format!("{}: {e}", p.display()))?
                .flatten()
                .map(|e| e.path())
                .filter(|f| f.extension().is_some_and(|x| x == "map"))
                .collect();
            v.sort();
            v
        } else {
            vec![p.clone()]
        };
        for f in files {
            let text = util::read_text_lossy(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            out.extend(names::parse_map(&text));
        }
    }
    Ok(out)
}

/// Inline-table body of the `jrt` dependency for a generated `Cargo.toml`.
pub fn jrt_dependency(explicit: Option<&Path>) -> String {
    let local = explicit.map(Path::to_path_buf).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("jrt"));
    if local.join("Cargo.toml").exists() {
        let p = util::absolutize(&local);
        format!("path = \"{}\"", p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("git = \"{JRT_REPOSITORY}\"")
    }
}

/// A crate name from a free-form name (`My Game!` → `my-game`).
pub fn crate_name_from(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("port-{out}").trim_end_matches('-').to_string()
    } else {
        out
    }
}

/// Everything one translation needs.
pub struct Translation {
    pub sources: PathBuf,
    pub crate_dir: PathBuf,
    pub crate_name: String,
    pub classpath: String,
    pub maps: Vec<PathBuf>,
    pub api_table: String,
    pub jrt_dependency: String,
}

/// What [`translate`] did.
pub struct Translated {
    pub outcome: gen::Outcome,
    pub javac_errors: usize,
    pub map_applied: usize,
    pub map_warnings: Vec<String>,
}

pub fn translate(t: &Translation) -> Result<Translated, String> {
    let sources = java_sources(&t.sources);
    if sources.is_empty() {
        return Err(format!("no .java files under {}", t.sources.display()));
    }
    std::fs::create_dir_all(&t.crate_dir).map_err(|e| format!("{}: {e}", t.crate_dir.display()))?;
    let crate_dir = util::absolutize(&t.crate_dir);
    let (ast, javac_errors) = dump_ast(&sources, &t.classpath, &crate_dir.join("ast.json"))?;
    let maps = read_maps(&t.maps)?;
    let map = names::NameMap::resolve(&maps, &gen::input_classes(&ast));
    let api = gen::ApiTable::parse(&t.api_table);
    let outcome = gen::generate(&ast, &api, &map, &crate_dir, &t.crate_name, &t.jrt_dependency)?;
    Ok(Translated { outcome, javac_errors, map_applied: map.applied, map_warnings: map.warnings.clone() })
}

#[derive(clap::Args)]
pub struct PortRustArgs {
    /// output directory of a previous `arlecchino decompile` (its merged/ tree is translated)
    #[arg(default_value = "arlecchino_out")]
    pub out: String,
    /// translate this Java source tree instead of <OUT>/merged
    #[arg(long = "src")]
    pub src: Option<String>,
    /// where to write the crate (default: <OUT>/rust)
    #[arg(long = "crate-dir")]
    pub crate_dir: Option<String>,
    /// crate name (default: from the jar or directory name)
    #[arg(long = "crate-name")]
    pub crate_name: Option<String>,
    /// name map in Sporeflower `.map` format, applied while generating (a directory
    /// loads its *.map files); repeatable
    #[arg(long = "map")]
    pub map: Vec<String>,
    /// API table to bind platform calls with (default: the built-in one)
    #[arg(long = "api")]
    pub api: Option<String>,
    /// classpath javac attributes the tree against (default: FreeJ2ME, else the MIDP stub)
    #[arg(long = "classpath")]
    pub classpath: Option<String>,
    /// path to a FreeJ2ME jar or another MIDP API jar
    #[arg(long = "midp-stub")]
    pub midp_stub: Option<String>,
    /// local checkout of the jrt crate (default: jrt/ of this source tree, else the git repository)
    #[arg(long = "jrt")]
    pub jrt: Option<String>,
    /// exit with status 1 when anything was left as todo_ai!
    #[arg(long = "strict")]
    pub strict: bool,
    /// print the full report (it is always written to <crate>/report.txt)
    #[arg(long = "report")]
    pub report: bool,
}

pub fn cmd_port_rust(a: &PortRustArgs) -> i32 {
    let out = util::absolutize(&util::expanduser(&a.out));
    let sources = match &a.src {
        Some(s) => util::absolutize(&util::expanduser(s)),
        None => out.join("merged"),
    };
    if !sources.is_dir() {
        println!("port-rust: {} is not a directory — run `arlecchino decompile` first or pass --src", sources.display());
        return 2;
    }
    let crate_dir = a.crate_dir.as_deref().map(|d| util::absolutize(&util::expanduser(d))).unwrap_or_else(|| out.join("rust"));
    let crate_name = a.crate_name.clone().map(|n| crate_name_from(&n)).unwrap_or_else(|| {
        let jar = util::read_text_lossy(&out.join("report.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("jar").and_then(|j| j.as_str()).map(str::to_string));
        let stem = jar
            .as_deref()
            .and_then(|j| Path::new(j).file_stem().map(|s| s.to_string_lossy().into_owned()))
            .or_else(|| sources.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "port".into());
        crate_name_from(&stem)
    });
    let classpath = a
        .classpath
        .clone()
        .or_else(|| crate::emulate::find_freej2me(a.midp_stub.as_deref(), None).map(|p| p.to_string_lossy().into_owned()))
        .or_else(|| crate::midp_stub::stub_jar().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_default();
    if classpath.is_empty() {
        println!("port-rust: warning: no MIDP API jar found (run `arlecchino setup` or pass --classpath); platform calls will not resolve");
    }
    let api_table = match &a.api {
        Some(p) => match util::read_text_lossy(&util::expanduser(p)) {
            Ok(t) => t,
            Err(e) => {
                println!("port-rust: {p}: {e}");
                return 2;
            }
        },
        None => API_TABLE.to_string(),
    };
    let t = Translation {
        sources,
        crate_dir: crate_dir.clone(),
        crate_name,
        classpath,
        maps: a.map.iter().map(|m| util::expanduser(m)).collect(),
        api_table,
        jrt_dependency: jrt_dependency(a.jrt.as_deref().map(util::expanduser).as_deref()),
    };
    let r = match translate(&t) {
        Ok(r) => r,
        Err(e) => {
            println!("port-rust: {e}");
            return 2;
        }
    };
    let st = &r.outcome.stats;
    if a.report {
        print!("{}", r.outcome.report);
    }
    println!(
        "port-rust: {} classes, {} methods ({} without todo_ai), {} todo_ai sites on {} of {} source lines; javac errors in the input: {}",
        r.outcome.classes,
        st.methods,
        st.methods_clean,
        st.todo_sites,
        st.todo_source_lines(),
        st.source_lines(),
        r.javac_errors
    );
    if !t.maps.is_empty() {
        println!("port-rust: name map: {} entries applied, {} not matched", r.map_applied, r.map_warnings.len());
    }
    println!("port-rust: crate {} written to {} (build it with `cargo build`; report.txt, residue.json)", t.crate_name, crate_dir.display());
    if a.strict && st.todo_sites > 0 {
        println!("port-rust: --strict: {} construct(s) left untranslated", st.todo_sites);
        return 1;
    }
    0
}

#[derive(clap::Args)]
pub struct TraceDiffArgs {
    /// trace midp-rt wrote for the original jar (`-Dmidprt.trace`, `run --trace`)
    pub reference: String,
    /// trace the translated crate's `trace` binary wrote
    pub port: String,
    /// ids of objects midp-rt created itself (`I <id>`/`P <id>` of the reference), comma-separated
    #[arg(long = "drop-ids", value_delimiter = ',')]
    pub drop_ids: Vec<i64>,
    /// also leave out images and players no line references
    #[arg(long = "drop-unreferenced")]
    pub drop_unreferenced: bool,
    /// write both canonical traces next to the inputs (`*.canon`)
    #[arg(long = "write-canon")]
    pub write_canon: bool,
}

pub fn cmd_trace_diff(a: &TraceDiffArgs) -> i32 {
    let read = |p: &str| util::read_text_lossy(&util::expanduser(p)).map_err(|e| format!("{p}: {e}"));
    let (reference, port) = match (read(&a.reference), read(&a.port)) {
        (Ok(r), Ok(p)) => (r, p),
        (Err(e), _) | (_, Err(e)) => {
            println!("trace-diff: {e}");
            return 2;
        }
    };
    let d = trace::compare(&reference, &port, &a.drop_ids, a.drop_unreferenced);
    if a.write_canon {
        for (src, c) in [(&a.reference, &d.reference), (&a.port, &d.port)] {
            let p = util::expanduser(&format!("{src}.canon"));
            let _ = std::fs::write(&p, c.lines.join("\n") + "\n");
        }
    }
    println!(
        "trace-diff: reference {} distinct frames / {} lines, port {} / {} lines, differing lines: {}",
        d.reference.frames,
        d.reference.lines.len(),
        d.port.frames,
        d.port.lines.len(),
        d.differing
    );
    i32::from(d.differing != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_names() {
        assert_eq!(crate_name_from("Synth Midlet_240x320"), "synth-midlet-240x320");
        assert_eq!(crate_name_from("3d"), "port-3d");
        assert_eq!(crate_name_from("!!"), "port");
    }

    #[test]
    fn builtin_api_table_parses() {
        let t = gen::ApiTable::parse(API_TABLE);
        assert!(t.len() > 150, "{}", t.len());
    }
}
