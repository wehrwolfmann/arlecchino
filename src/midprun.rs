//! `arlecchino run <game.jar|jad>` (stages B3'/B4' of PLAN_runtime.md, the Apache side):
//! runs a MIDlet IN A WINDOW through **midp-rt** — our LGPL fork of MicroEmulator
//! (artifact `build/midp-rt.jar`).
//!
//! Port of `arlecchino/midprun.py`.
//!
//! The fork's sources live in this same tree — the `midp-rt/` subtree (LGPL 2.1). The built
//! `midp-rt.jar` is NOT kept in git (the fork has its own `.gitignore` for `build/`), so
//! the path to it is looked up in order: `--midp-rt` → `$ARLECCHINO_MIDP_RT` → engines
//! root → next to the binary → `build/` of any fork checkout found.
//!
//! ⚖️ LGPL boundary = PROCESS boundary (decision B0): this module only BUILDS
//! the java command line and spawns a separate process — not a single line of LGPL code
//! is imported, copied or linked into Arlecchino.
//!
//! Two behavioural lessons learned the hard way (live Allods, 2026-06-10):
//!
//! * **Autostart.** When the runtime is given a PATH TO A JAR, it only opens the suite in
//!   the launcher list — the user has to press Start. Invoking by class name
//!   (`--appclasspath game.jar <MIDletClass>`) creates and starts the MIDlet right away,
//!   so we parse `MIDlet-1` from the manifest (or .jad) ourselves and prefer
//!   this mode; path-to-jar remains the fallback when `MIDlet-1` was not found.
//!
//! * **Instrumentation is ON by default.** The fork's ASM was moved from 3.1 to 9.x,
//!   so instrumentation of MIDlet classes works both on modern bytecode
//!   (major 52+ — our rebuilt jars) and on the original phone jars. More importantly:
//!   the UNinstrumented MicroEmulator loader truncates a class at 16 KB,
//!   and its `ClassNotFoundException` is silently swallowed in `MIDletClassLoader.loadClass`
//!   → the JVM shows a bare `NoClassDefFoundError` (Allods: class `i` = 96 KB).
//!   `--no-instrument` (the fork's own flag) is kept only as an escape hatch.

use crate::config::engines_root_default;
use crate::diffrun::{FrameRecord, HeadlessResult, RunOpts, which, InputEvent};
use crate::util;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Directory of the running binary (for the "next to itself" lookup).
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

/// Name of the subdirectory with the fork's sources (the `midp-rt/` subtree, LGPL 2.1).
pub const REPO_DIRNAME: &str = "midp-rt";
/// Name of the built runtime artifact.
pub const JAR_NAME: &str = "midp-rt.jar";
/// How many levels up from the binary's directory to look (enough for
/// `target/release/arlecchino` → tree root).
const EXE_ANCESTORS: usize = 4;

/// All places where a fork CHECKOUT may live, in priority order:
///
/// 1. `$ARLECCHINO_MIDP_RT_SRC` — explicit setting;
/// 2. `<engines root>/midp-rt` — where `arlecchino setup` may put it;
/// 3. next to the binary: `<binary dir>/midp-rt` and the same subdirectory of each
///    ancestor (this is how a binary built in `target/release/` finds the subtree in the tree);
/// 4. `~/Projects/arlecchino-midp-rt` — a separate checkout, the fallback.
///
/// ⚖️ None of them is pulled into the build: these are the SOURCES of a separate
/// artifact, built by its own `build.sh` and run as a separate process.
pub fn midp_rt_repos(root: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Ok(v) = std::env::var("ARLECCHINO_MIDP_RT_SRC") {
        if !v.is_empty() {
            push(util::expanduser(&v));
        }
    }
    let engines = root.map(|r| r.to_path_buf()).unwrap_or_else(engines_root_default);
    push(engines.join(REPO_DIRNAME));
    if let Some(dir) = exe_dir() {
        // the binary's directory itself plus a few ancestors: `target/release/arlecchino`
        // must reach the tree root, and there is no point going further up.
        let mut cur: Option<&Path> = Some(dir.as_path());
        for _ in 0..EXE_ANCESTORS {
            match cur {
                Some(d) => {
                    push(d.join(REPO_DIRNAME));
                    cur = d.parent();
                }
                None => break,
            }
        }
    }
    push(util::home_dir().join("Projects").join("arlecchino-midp-rt"));
    out
}

/// The first fork checkout that really has `build.sh`.
pub fn midp_rt_repo(root: Option<&Path>) -> Option<PathBuf> {
    midp_rt_repos(root).into_iter().find(|r| r.join("build.sh").is_file())
}

/// All places where a BUILT `midp-rt.jar` is looked for (in the same order as checkouts:
/// environment variable → engines root → next to the binary → checkouts).
pub fn midp_rt_jar_candidates(root: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Ok(v) = std::env::var("ARLECCHINO_MIDP_RT") {
        if !v.is_empty() {
            push(util::expanduser(&v));
        }
    }
    let engines = root.map(|r| r.to_path_buf()).unwrap_or_else(engines_root_default);
    push(engines.join(JAR_NAME));
    if let Some(dir) = exe_dir() {
        push(dir.join(JAR_NAME));
    }
    for repo in midp_rt_repos(root) {
        push(repo.join("build").join(JAR_NAME));
    }
    out
}

/// Main class of the runtime's window (separate process).
pub const MAIN_WINDOW: &str = "org.microemu.app.Main";
/// Main class of the runtime's headless mode (frame capture lives there too).
pub const MAIN_HEADLESS: &str = "org.microemu.app.Headless";

/// The "how to fix it" message for a missing midp-rt.jar.
/// `checked` empty → we list the standard places ourselves.
pub fn missing_hint(checked: &[PathBuf], root: Option<&Path>) -> String {
    let mut lines = vec![
        "midp-rt.jar not found — the MIDP runtime (LGPL fork of MicroEmulator) \
         is a separate artifact."
            .to_string(),
    ];
    match midp_rt_repo(root) {
        Some(repo) => lines.push(format!("Build it:  cd {} && ./build.sh", repo.display())),
        None => {
            lines.push(
                "No source checkout of the fork was found either. Put one at \
                 <engines root>/midp-rt, next to the arlecchino binary, or at \
                 ~/Projects/arlecchino-midp-rt — or set $ARLECCHINO_MIDP_RT_SRC."
                    .to_string(),
            );
            lines.push(
                "searched: ".to_string()
                    + &midp_rt_repos(root)
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
            );
        }
    }
    lines.push("or point at an existing jar with --midp-rt PATH / $ARLECCHINO_MIDP_RT.".to_string());
    let checked: Vec<PathBuf> = if checked.is_empty() {
        midp_rt_jar_candidates(root)
    } else {
        checked.to_vec()
    };
    lines.push(
        "checked: ".to_string()
            + &checked.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
    );
    lines.join("\n")
}

/// Find midp-rt.jar (order — in the module header); None when it is missing.
pub fn find_midp_rt(explicit: Option<&str>, root: Option<&Path>) -> Option<PathBuf> {
    if let Some(e) = explicit {
        let p = util::expanduser(e);
        return if p.exists() { Some(p) } else { None };
    }
    if let Ok(env) = std::env::var("ARLECCHINO_MIDP_RT") {
        let p = util::expanduser(&env);
        if !env.is_empty() && p.exists() {
            return Some(p);
        }
    }
    midp_rt_jar_candidates(root).into_iter().find(|cand| cand.exists())
}

/// midp-rt.jar plus the ASM jar(s) its MIDlet instrumentation needs
/// (`lib/asm*.jar` in the fork layout, or next to the jar itself).
pub fn runtime_classpath(midp_rt: &Path) -> Vec<PathBuf> {
    let mut cp = vec![midp_rt.to_path_buf()];
    let dirs = [
        // checkout layout: build/midp-rt.jar next to ../lib/asm-*.jar
        midp_rt.parent().and_then(|p| p.parent()).map(|p| p.join("lib")),
        // install layout: midp-rt.jar next to lib/asm-*.jar
        midp_rt.parent().map(|p| p.join("lib")),
        // everything piled into one directory
        midp_rt.parent().map(|p| p.to_path_buf()),
    ];
    for d in dirs.into_iter().flatten() {
        if !d.is_dir() {
            continue;
        }
        let mut jars: Vec<PathBuf> = std::fs::read_dir(&d)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .map(|n| {
                                let n = n.to_string_lossy();
                                n.starts_with("asm") && n.ends_with(".jar")
                            })
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        jars.sort();
        if !jars.is_empty() {
            cp.extend(jars);
            break;
        }
    }
    cp
}

// ───────────────────── parsing MIDlet-1 (autostart) ─────────────────────────
/// A minimal JAR manifest / .jad parser: `Key: value` with RFC-style
/// continuations (manifest lines starting with a single space).
pub fn fold_manifest_lines(text: &str) -> BTreeMap<String, String> {
    let mut attrs: BTreeMap<String, String> = BTreeMap::new();
    let mut key: Option<String> = None;
    for raw in text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)) {
        if raw.trim().is_empty() {
            key = None;
            continue;
        }
        if let Some(tail) = raw.strip_prefix(' ') {
            if let Some(k) = &key {
                let cont = tail.trim_end_matches('\r').to_string();
                attrs.entry(k.clone()).and_modify(|v| v.push_str(&cont));
            }
            continue;
        }
        match raw.find(':') {
            None => {
                key = None;
            }
            Some(i) => {
                let k = raw[..i].trim().to_string();
                let v = raw[i + 1..].trim().to_string();
                attrs.insert(k.clone(), v);
                key = Some(k);
            }
        }
    }
    attrs
}

/// `MIDlet-1 = 'Name, /icon.png, the.Class'` → `the.Class` (or None).
pub fn midlet1_class(attrs: &BTreeMap<String, String>) -> Option<String> {
    let entry = attrs.get("MIDlet-1")?;
    if entry.is_empty() {
        return None;
    }
    let parts: Vec<&str> = entry.split(',').collect();
    if parts.len() < 3 || parts[2].trim().is_empty() {
        return None;
    }
    Some(parts[2].trim().to_string())
}

/// The "by class name" invocation for a .jar or .jad:
/// `(midlet_class, app_jar, properties_jad)`.
///
/// `(None, None, None)` = "MIDlet-1 not found, pass the path to the runtime as is"
/// (its launcher list, Start by hand). Does not crash on malformed files.
pub fn autostart_target(game: &Path) -> (Option<String>, Option<PathBuf>, Option<PathBuf>) {
    let none = (None, None, None);
    let ext = game
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "jad" {
        let text = match util::read_text_lossy(game) {
            Ok(t) => t,
            Err(_) => return none,
        };
        let attrs = fold_manifest_lines(&text);
        let cls = match midlet1_class(&attrs) {
            Some(c) => c,
            None => return none,
        };
        let jar_ref = attrs.get("MIDlet-Jar-URL").cloned().unwrap_or_default();
        if jar_ref.is_empty() || jar_ref.contains("://") {
            return none;
        }
        let jar = util::absolutize(&game.parent().unwrap_or(Path::new(".")).join(&jar_ref));
        if !jar.exists() {
            return none;
        }
        return (Some(cls), Some(jar), Some(game.to_path_buf()));
    }
    let manifest = match util::zip_read_entry(game, "META-INF/MANIFEST.MF") {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => return none,
    };
    match midlet1_class(&fold_manifest_lines(&manifest)) {
        Some(cls) => (Some(cls), Some(game.to_path_buf()), None),
        None => none,
    }
}

// ───────────────────────── command line / launch ────────────────────────────
#[derive(Debug, Clone, Default)]
pub struct WindowOpts {
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub rms_dir: Option<PathBuf>,
    pub instrument: bool,
    pub trace: Option<PathBuf>,
}

fn pathsep_join(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(":")
}

/// Build (do NOT run) the java command line for a midp-rt window session.
///
/// `instrument = true` (the default) keeps the ASM-9 MIDlet instrumentation
/// on — in practice this is mandatory: the raw loader truncates classes at
/// 16 KB and turns the failure into a bare `NoClassDefFoundError`.
pub fn build_run_argv(game: &Path, midp_rt: &Path, o: &WindowOpts) -> Vec<String> {
    let java = util::tool("ARLECCHINO_JAVA", "java");
    let mut argv = vec![java];
    if let Some(t) = &o.trace {
        argv.push(format!("-Dmidprt.trace={}", t.display()));
    }
    if let Some(r) = &o.rms_dir {
        // MicroEmulator keeps its config and the file-based RMS under <user.home>/.microemulator/
        // (suite-<name>/<store>.rs). Overriding user.home is the only
        // non-invasive way to move that, and it also isolates the run from the user's live
        // ~/.microemulator.
        argv.push(format!("-Duser.home={}", r.display()));
    }
    argv.push("-cp".into());
    argv.push(pathsep_join(&runtime_classpath(midp_rt)));
    argv.push(MAIN_WINDOW.into());
    // file-based RMS so that saves outlive the process (FileRecordStoreManager)
    argv.push("--rms".into());
    argv.push("file".into());
    if !o.instrument {
        argv.push("--no-instrument".into());
    }
    if let (Some(w), Some(h)) = (o.width, o.height) {
        argv.push("--resizableDevice".into());
        argv.push(w.to_string());
        argv.push(h.to_string());
    }
    // Autostart (class-name mode) when MIDlet-1 can be recovered, otherwise the path as is.
    let (cls, app_jar, props_jad) = autostart_target(game);
    match (cls, app_jar) {
        (Some(c), Some(j)) => {
            argv.push("--appclasspath".into());
            argv.push(j.to_string_lossy().into_owned());
            if let Some(p) = props_jad {
                argv.push("--propertiesjad".into());
                argv.push(p.to_string_lossy().into_owned());
            }
            argv.push(c);
        }
        _ => argv.push(game.to_string_lossy().into_owned()),
    }
    argv
}

#[derive(Debug, Clone)]
pub struct HeadlessArgvOpts {
    pub frames_dir: PathBuf,
    pub frames: i64,
    pub period_ms: i64,
    pub boot_wait_ms: i64,
    pub inputs_file: Option<PathBuf>,
    pub rms_dir: Option<PathBuf>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub instrument: bool,
    pub home_dir: Option<PathBuf>,
}

/// Build (do NOT run) the java command line for a headless capture session
/// (the fork's `--frames-dir` mode: PNG frames + index.json).
///
/// The rewrite to a MIDlet-1 autostart happens INSIDE the runtime (the fork itself
/// reads the manifest/jad — the headless launcher list is never repainted, so
/// autostart is mandatory there); we pass the game path as is.
pub fn build_headless_argv(game: &Path, midp_rt: &Path, o: &HeadlessArgvOpts) -> Vec<String> {
    let java = util::tool("ARLECCHINO_JAVA", "java");
    let mut argv = vec![java, "-Djava.awt.headless=true".to_string()];
    if let Some(h) = &o.home_dir {
        argv.push(format!("-Duser.home={}", h.display()));
    }
    argv.push("-cp".into());
    argv.push(pathsep_join(&runtime_classpath(midp_rt)));
    argv.push(MAIN_HEADLESS.into());
    argv.push("--frames-dir".into());
    argv.push(o.frames_dir.to_string_lossy().into_owned());
    argv.push("--frames".into());
    argv.push(o.frames.to_string());
    argv.push("--period-ms".into());
    argv.push(o.period_ms.to_string());
    argv.push("--boot-wait-ms".into());
    argv.push(o.boot_wait_ms.to_string());
    if let Some(f) = &o.inputs_file {
        argv.push("--inputs".into());
        argv.push(f.to_string_lossy().into_owned());
    }
    if let Some(r) = &o.rms_dir {
        argv.push("--rms".into()); // a PATH value = flat file-based RMS directory
        argv.push(r.to_string_lossy().into_owned());
    }
    if !o.instrument {
        argv.push("--no-instrument".into());
    }
    if let (Some(w), Some(h)) = (o.width, o.height) {
        argv.push("--resizableDevice".into());
        argv.push(w.to_string());
        argv.push(h.to_string());
    }
    argv.push(game.to_string_lossy().into_owned());
    argv
}

/// Headless run of `jar` in midp-rt with frame capture — the twin of
/// `diffrun::run_headless`, with the same result shape so that difftest can
/// treat both engines the same way.
///
/// LGPL boundary = process boundary: a separate java process writes
/// frame_NNNN.png + index.json, we only spawn it and read the files back.
pub fn run_headless_midp_rt(
    jar: &Path,
    out_dir: &Path,
    inputs: Option<&[InputEvent]>,
    opts: &RunOpts,
    midp_rt: Option<&str>,
    instrument: bool,
) -> Result<HeadlessResult, String> {
    run_headless_midp_rt_sized(
        jar,
        out_dir,
        inputs,
        opts,
        midp_rt,
        instrument,
        Some((opts.width, opts.height)),
    )
}

/// Same as [`run_headless_midp_rt`], but the device size is given explicitly
/// instead of being taken from `opts`.
///
/// `size == None` passes NO `--resizableDevice` to midp-rt, so the emulator keeps
/// its own device default (176x220).  That is what the Python original does for
/// `arlecchino run --headless` without `--size`: there `width`/`height` stay
/// `None` all the way down.  `RunOpts` cannot express "unset" (difftest always
/// has a concrete -W/-H), hence the separate entry point.
pub fn run_headless_midp_rt_sized(
    jar: &Path,
    out_dir: &Path,
    inputs: Option<&[InputEvent]>,
    opts: &RunOpts,
    midp_rt: Option<&str>,
    instrument: bool,
    size: Option<(i64, i64)>,
) -> Result<HeadlessResult, String> {
    let rt = match find_midp_rt(midp_rt, None) {
        Some(p) => p,
        None => return Err(missing_hint(&[], None)),
    };
    let java = util::tool("ARLECCHINO_JAVA", "java");
    if which(&java).is_none() {
        return Err(format!("java executable not found ({java:?})"));
    }
    let jar = util::absolutize(jar);
    if !jar.exists() {
        return Err(format!("midlet jar not found: {}", jar.display()));
    }

    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let rms = opts.rms_dir.clone().unwrap_or_else(|| out_dir.join("rms"));
    std::fs::create_dir_all(&rms).map_err(|e| e.to_string())?;
    let home = out_dir.join("home");
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;

    let mut inputs_file = None;
    if let Some(ev) = inputs {
        if !ev.is_empty() {
            let f = out_dir.join("inputs.json");
            std::fs::write(&f, serde_json::to_string(ev).unwrap_or_else(|_| "[]".into()))
                .map_err(|e| e.to_string())?;
            inputs_file = Some(f);
        }
    }

    let argv = build_headless_argv(
        &jar,
        &rt,
        &HeadlessArgvOpts {
            frames_dir: out_dir.to_path_buf(),
            frames: opts.max_frames,
            period_ms: opts.frame_period_ms,
            boot_wait_ms: (opts.boot_wait_s * 1000.0) as i64,
            inputs_file,
            rms_dir: Some(rms.clone()),
            width: size.map(|(w, _)| w),
            height: size.map(|(_, h)| h),
            instrument,
            home_dir: Some(home),
        },
    );

    let stderr_path = out_dir.join("emulator_stderr.log");
    let timeout =
        opts.boot_wait_s + opts.max_frames as f64 * opts.frame_period_ms as f64 / 1000.0 + 120.0;
    let res = util::run_capture(&argv, Some(Duration::from_secs_f64(timeout)));
    let combined = format!("{}{}", res.stdout, res.stderr);
    let _ = std::fs::write(&stderr_path, &combined);
    if res.timed_out {
        return Err(format!(
            "midp-rt headless run timed out after {timeout:.0}s: {}",
            jar.display()
        ));
    }

    let index = out_dir.join("index.json");
    if !res.success() || !index.exists() {
        let tail: String = combined.chars().rev().take(800).collect::<String>().chars().rev().collect();
        return Err(format!(
            "midp-rt headless run failed (exit {}, index.json {}): {}\n{}",
            res.status.unwrap_or(-1),
            if index.exists() { "present" } else { "MISSING" },
            jar.display(),
            tail
        ));
    }

    let data: serde_json::Value = serde_json::from_str(&util::read_text_lossy(&index).unwrap_or_default())
        .map_err(|e| format!("bad index.json: {e}"))?;
    let mut result = HeadlessResult {
        frames: Vec::new(),
        stderr_log: stderr_path.to_string_lossy().into_owned(),
        rms_dir: rms.to_string_lossy().into_owned(),
    };
    if let Some(arr) = data.get("frames").and_then(|f| f.as_array()) {
        for f in arr {
            result.frames.push(FrameRecord {
                index: f.get("index").and_then(|v| v.as_i64()).unwrap_or(0),
                file: f.get("file").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                sha256: f.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                width: f.get("width").and_then(|v| v.as_i64()).unwrap_or(0),
                height: f.get("height").and_then(|v| v.as_i64()).unwrap_or(0),
                blank: f.get("blank").and_then(|v| v.as_bool()).unwrap_or(false),
            });
        }
    }
    Ok(result)
}

/// Spawn a separate java process (the LGPL boundary) and wait for it.
///
/// Frame/input bridge (allods-shell): when `$ALLODS_SHELL_SOCKET` is set,
/// the midp-rt fork's `ShellFrameBridge` streams the MIDP screen to an external Rust renderer over
/// that Unix socket and reads keyboard events from it. The bridge reads the
/// environment variable itself (it inherits our environment), we only show it to
/// the user; `allods-renderer` has to be started beforehand.
pub fn run_midlet(game: &Path, midp_rt: &Path, o: &WindowOpts) -> i32 {
    let argv = build_run_argv(game, midp_rt, o);
    if argv.last().map(|s| s == &game.to_string_lossy()).unwrap_or(false) {
        println!("  (no MIDlet-1 in manifest/jad — opening the runtime Launcher; press Start there)");
    }
    if let Ok(sock) = std::env::var("ALLODS_SHELL_SOCKET") {
        if !sock.is_empty() {
            println!("  allods-shell мост включён -> {sock} (запусти allods-renderer заранее)");
        }
    }
    println!("  launching: {}", argv.join(" "));
    util::run_inherit(&argv)
}

/// Run the fork's `./build.sh` (its own repository — nothing is
/// vendored here). None on success, a human-readable error string otherwise.
pub fn build_midp_rt(repo: Option<&Path>, root: Option<&Path>) -> Option<String> {
    let repo = match repo.map(|p| p.to_path_buf()).or_else(|| midp_rt_repo(root)) {
        Some(r) => r,
        None => {
            return Some(format!(
                "no fork checkout found (searched: {})",
                midp_rt_repos(root)
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    };
    let script = repo.join("build.sh");
    if !script.exists() {
        return Some(format!("no fork checkout at {}", repo.display()));
    }
    let out = std::process::Command::new("bash")
        .arg(&script)
        .current_dir(&repo)
        .output();
    match out {
        Err(e) => Some(format!("build.sh did not run: {e}")),
        Ok(o) if !o.status.success() => {
            let text = if o.stderr.is_empty() { &o.stdout } else { &o.stderr };
            let text = String::from_utf8_lossy(text);
            let tail: Vec<&str> = text.trim().lines().rev().take(3).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            Some(format!(
                "build.sh exit {}: {}",
                o.status.code().unwrap_or(-1),
                tail.join(" | ")
            ))
        }
        Ok(_) => None,
    }
}

/// The `arlecchino setup` step: find midp-rt.jar; if it is missing but a fork checkout
/// is present — BUILD it via its ./build.sh (a separate repository/process).
/// Gentle: a failure is a status line with a fix-it command, not a panic.
pub fn setup_midp_rt(root: Option<&Path>) -> String {
    let mut jar = find_midp_rt(None, root);
    let mut built = String::new();
    if jar.is_none() {
        let err = build_midp_rt(None, root);
        jar = if err.is_none() { find_midp_rt(None, root) } else { None };
        if jar.is_none() {
            return format!(
                "not found ({})\n{}",
                err.unwrap_or_else(|| "build.sh produced no jar".into()),
                missing_hint(&[], root)
            );
        }
        built = "built via build.sh; ".into();
    }
    let jar = jar.unwrap();
    // Install the runtime into the engines root: `<root>/midp-rt.jar` + `<root>/lib/asm-*.jar`.
    // After that the source checkout is no longer needed for anything except rebuilding.
    let mut installed = String::new();
    let root = root.map(|r| r.to_path_buf()).unwrap_or_else(engines_root_default);
    let target = root.join(JAR_NAME);
    let jar = if jar != target {
        let asm_src = runtime_classpath(&jar);
        let libdir = root.join("lib");
        let ok = std::fs::create_dir_all(&libdir).is_ok()
            && std::fs::copy(&jar, &target).is_ok()
            && asm_src.iter().skip(1).all(|a| {
                a.file_name()
                    .map(|n| std::fs::copy(a, libdir.join(n)).is_ok())
                    .unwrap_or(false)
            });
        if ok {
            installed = "installed -> ".to_string();
            target
        } else {
            jar
        }
    } else {
        jar
    };
    let asm: Vec<String> = runtime_classpath(&jar)
        .into_iter()
        .skip(1)
        .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    format!(
        "ok ({}{}{}; asm: {})",
        built,
        installed,
        jar.display(),
        if asm.is_empty() {
            "NOT FOUND — instrumentation needs lib/asm*.jar".to_string()
        } else {
            asm.join(", ")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_folding_and_midlet1() {
        let m = "Manifest-Version: 1.0\nMIDlet-1: Game, /i.png, com.x.\n Main\nMIDlet-Name: G\n";
        let a = fold_manifest_lines(m);
        assert_eq!(a.get("MIDlet-1").map(String::as_str), Some("Game, /i.png, com.x.Main"));
        assert_eq!(midlet1_class(&a).as_deref(), Some("com.x.Main"));
        let bad = fold_manifest_lines("MIDlet-1: Game, /i.png\n");
        assert_eq!(midlet1_class(&bad), None);
    }

    /// `run --headless` without `--size` must leave the device size UNSET, so
    /// midp-rt keeps its own default (176x220) like the Python original does.
    /// Forcing 240x320 changed every frame and its sha256.
    #[test]
    fn headless_argv_omits_device_size_when_unset() {
        let base = HeadlessArgvOpts {
            frames_dir: PathBuf::from("/out"),
            frames: 4,
            period_ms: 100,
            boot_wait_ms: 2000,
            inputs_file: None,
            rms_dir: None,
            width: None,
            height: None,
            instrument: true,
            home_dir: None,
        };
        let argv = build_headless_argv(Path::new("/g.jar"), Path::new("/midp-rt.jar"), &base);
        assert!(!argv.iter().any(|a| a == "--resizableDevice"), "{argv:?}");

        let sized = HeadlessArgvOpts {
            width: Some(240),
            height: Some(320),
            ..base
        };
        let argv = build_headless_argv(Path::new("/g.jar"), Path::new("/midp-rt.jar"), &sized);
        let i = argv.iter().position(|a| a == "--resizableDevice").expect("flag");
        assert_eq!(&argv[i + 1..i + 3], ["240".to_string(), "320".to_string()]);
    }
}
