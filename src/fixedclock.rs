//! Stage A7 (PLAN_runtime.md, block A) — deterministic frame-by-frame replay
//! via instrumentation of the MIDlet bytecode (path *(b)*: we instrument the MIDlet
//! and do NOT fork the GPL emulator).
//!
//! Port of `arlecchino/fixedclock.py` — orchestration around
//! `assets/DeterministicClockTransformer.java`. The transformer itself stays in Java:
//! it works through ASM, and by project decision the Rust/Java boundary = the process boundary.
//! The transformer rewrites the MIDlet jar so that `System.currentTimeMillis()`/
//! `nanoTime()` become a virtual clock (a counter with a fixed step), and
//! `new Random(...)` gets a fixed seed — the MIDlet's rendered state becomes a
//! pure function of the SEQUENCE of its clock/RNG calls and does not depend on the
//! host clock. Two replays of the same input then give byte-identical frames
//! even on FreeJ2ME-plus (its non-determinism gave a baseline Jaccard of
//! 0.86 on Allods).
//!
//! It uses THE SAME ASM infrastructure as the member renamer
//! (`rename`): the ASM jars under `<engines>/asm` and the directory of compiled
//! helpers `_classes/`.
//!
//! Everything degrades gracefully: no ASM jars or no compiled helper —
//! `transform` says so and returns None, and the caller falls back to the
//! uninstrumented jar (the normal non-deterministic MVP mode).
//!
//! ON REQUEST ONLY. Nothing here runs without an explicit `difftest --fixed-clock`
//! or the `ARLECCHINO_FIXED_CLOCK` variable.

use crate::rename;
use crate::util;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const HELPER_CLASS: &str = "DeterministicClockTransformer";
pub const HELPER_SOURCE: &str = include_str!("../assets/DeterministicClockTransformer.java");

/// Environment variable: if true, difftest instruments both jars before the
/// run. The CLI `--fixed-clock` turns on the same thing explicitly.
pub const ENV_FLAG: &str = "ARLECCHINO_FIXED_CLOCK";

pub fn env_enabled() -> bool {
    matches!(
        std::env::var(ENV_FLAG).unwrap_or_default().trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn helper_class_file(root: Option<&Path>) -> PathBuf {
    rename::helper_classes_dir(root).join(format!("{HELPER_CLASS}.class"))
}

/// Whether the A7 transformer is compiled.
pub fn helper_compiled(root: Option<&Path>) -> bool {
    helper_class_file(root).exists()
}

/// true only if both the ASM jars and the compiled A7 transformer are present.
pub fn available(root: Option<&Path>) -> bool {
    !rename::asm_jars(root).is_empty() && helper_compiled(root)
}

/// javac compilation of `DeterministicClockTransformer.java` against the ASM jars
/// (idempotent). Used by the `setup` command next to the member renamer.
pub fn compile_helper(root: Option<&Path>) -> String {
    let jars = rename::asm_jars(root);
    if jars.is_empty() {
        return "skip (no ASM jars; run setup with network access)".into();
    }
    let dir = rename::asm_dir(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return format!("FAILED (cannot create {})", dir.display());
    }
    let src = dir.join(format!("{HELPER_CLASS}.java"));
    if std::fs::write(&src, HELPER_SOURCE).is_err() {
        return format!("FAILED (cannot write helper source: {})", src.display());
    }
    let outdir = rename::helper_classes_dir(root);
    let _ = std::fs::create_dir_all(&outdir);
    let cp = jars.iter().map(|j| j.to_string_lossy().into_owned()).collect::<Vec<_>>().join(util::PATHSEP);
    let argv = vec![
        util::tool("ARLECCHINO_JAVAC", "javac"),
        "-nowarn".into(),
        "-proc:none".into(),
        "-cp".into(),
        cp,
        "-d".into(),
        outdir.to_string_lossy().into_owned(),
        src.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture(&argv, Some(Duration::from_secs(300)));
    if r.success() && helper_compiled(root) {
        format!("ok ({})", helper_class_file(root).display())
    } else {
        let tail: String = r.stderr.trim().lines().rev().take(2).collect::<Vec<_>>().join(" | ");
        format!("FAILED ({tail})")
    }
}

/// Jars carrying the MIDP API (`javax.microedition.*`) — passed to the transformer
/// as `--cp` so that recomputing stack-map frames can resolve the MIDlet's supertypes
/// (Canvas, MIDlet, RecordStore, …).
///
/// Only the fallback frame-RECOMPUTATION path needs them (a class whose frames
/// were stripped); the normal path keeps the jar's own frames and resolves nothing.
/// So the absence of these jars is not an error.
pub fn api_classpath(root: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(rt) = crate::midprun::find_midp_rt(None, root) {
        out.push(rt);
    }
    let roots: Vec<PathBuf> = match root {
        Some(r) => vec![r.to_path_buf()],
        None => vec![crate::config::engines_root_default()],
    };
    for r in roots {
        for name in ["freej2me-lr.jar", "freej2me.jar"] {
            let cand = r.join(name);
            if cand.exists() {
                out.push(cand);
                break;
            }
        }
    }
    out
}

/// Call the ASM transformer: rewrite `jar` -> `out_jar` with a virtual clock and a
/// fixed-seed RNG, injecting the self-contained runtime class
/// `arlecchino/det/Det`. Returns out_jar on success, None on any failure
/// (with a printed message), so the caller falls back to the original jar.
pub fn transform(
    jar: &Path,
    out_jar: &Path,
    root: Option<&Path>,
    recompute_frames: bool,
) -> Option<PathBuf> {
    let jars = rename::asm_jars(root);
    if jars.is_empty() || !helper_compiled(root) {
        println!(
            "   fixed-clock: ASM/transformer unavailable — skipping \
             (run `arlecchino setup`). Using original jar (nondeterministic)."
        );
        return None;
    }
    let mut cp: Vec<String> = jars.iter().map(|j| j.to_string_lossy().into_owned()).collect();
    cp.push(rename::helper_classes_dir(root).to_string_lossy().into_owned());
    if let Some(p) = out_jar.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let mut argv = vec![
        util::tool("ARLECCHINO_JAVA", "java"),
        "-cp".into(),
        cp.join(util::PATHSEP),
        HELPER_CLASS.into(),
    ];
    let api = api_classpath(root);
    if !api.is_empty() {
        argv.push("--cp".into());
        argv.push(
            api.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(util::PATHSEP),
        );
    }
    if recompute_frames {
        argv.push("--recompute-frames".into());
    }
    argv.push(jar.to_string_lossy().into_owned());
    argv.push(out_jar.to_string_lossy().into_owned());

    let r = util::run_capture(&argv, Some(Duration::from_secs(600)));
    if let Some(e) = &r.spawn_err {
        println!("   fixed-clock: launch error ({e}); using original jar.");
        return None;
    }
    if !r.success() || !out_jar.exists() {
        let tail: String = r.stderr.trim().chars().rev().take(300).collect::<String>().chars().rev().collect();
        println!("   fixed-clock: FAILED ({tail}); using original jar.");
        return None;
    }
    println!("   fixed-clock: {}", r.stdout.trim());
    // Show the helper's warnings (e.g. types it could not resolve while recomputing
    // frames) instead of letting them vanish in the pipe.
    for line in r.stderr.trim().lines() {
        if !line.trim().is_empty() {
            println!("   fixed-clock: {}", line.trim());
        }
    }
    Some(out_jar.to_path_buf())
}

/// Decide on and perform the A7 instrumentation.
///
/// `enabled` (from `--fixed-clock` or `env_enabled`): when false, return the original
/// `jar` unchanged. When true, rewrite into `<stem>.fixedclock.jar` inside
/// `workdir`; on any failure fall back to the original jar.
pub fn maybe_transform(jar: &Path, workdir: &Path, root: Option<&Path>, enabled: bool) -> PathBuf {
    if !enabled {
        return jar.to_path_buf();
    }
    let _ = std::fs::create_dir_all(workdir);
    let stem = jar.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let out_jar = workdir.join(format!("{stem}.fixedclock.jar"));
    transform(jar, &out_jar, root, false).unwrap_or_else(|| jar.to_path_buf())
}
