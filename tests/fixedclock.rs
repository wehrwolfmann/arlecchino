//! Deterministic-clock instrumentation (`--fixed-clock`).
//!
//! Ported from the reference suite's `tests/test_fixedclock.py`.
//!
//! A tiny fixture class reads the wall clock and an unseeded RNG; after instrumentation
//! it must (a) contain no `System.currentTimeMillis` / `nanoTime` / `Math.random` and
//! carry the injected `arlecchino/det/Det` runtime, (b) actually RUN deterministically —
//! two separate JVM launches produce byte-identical stdout — and (c) leave the default
//! path untouched.
//!
//! The branchy fixture pins a real regression: a jar rebuilt by a modern javac is class
//! major >= 50, where every branch target needs a StackMapTable entry.  Instrumenting it
//! used to strip that table and the emulator then died with `VerifyError: Expecting a
//! stackmap frame at branch target N`.  Both the preserving path and the COMPUTE_FRAMES
//! fallback are pinned, and so is the frameless path a major <= 49 class still takes.
//!
//! javac + java + ASM + the compiled transformer helper; skips cleanly without them.

mod common;

use arlecchino::fixedclock;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Output depends ENTIRELY on the wall clock, an unseeded RNG and Math.random — i.e.
/// nondeterministic until instrumented.
const CLOCKY_SRC: &str = r#"import java.util.Random;
public class Clocky {
    public static void main(String[] a) {
        long t0 = System.currentTimeMillis();
        long n0 = System.nanoTime();
        long t1 = System.currentTimeMillis();
        Random r1 = new Random();          // time-seeded by the JDK
        Random r2 = new Random(99L);       // explicit seed
        double m = Math.random();
        System.out.println("dt=" + (t1 - t0));
        System.out.println("nano0=" + n0);
        System.out.println("r1=" + r1.nextInt(1000000));
        System.out.println("r2=" + r2.nextInt(1000000));
        System.out.println("math=" + (long) (m * 1000000));
    }
}
"#;

/// Deliberately BRANCHY (loop + if/else + try/catch): on a modern javac that means a
/// StackMapTable with an entry at every branch target, and the rewrite fires inside
/// those branches.
const BRANCHY_SRC: &str = r#"import java.util.Random;
public class Branchy {
    static long spin(int n) {
        long acc = 0;
        for (int i = 0; i < n; i++) {                 // branch target
            if ((i & 1) == 0) acc += System.currentTimeMillis();
            else acc -= System.nanoTime();
        }
        return acc;
    }
    public static void main(String[] a) {
        Random r = new Random();                      // rewritten to a fixed seed
        long acc = spin(8);
        try {
            if (r.nextInt(4) > 100) throw new IllegalStateException("never");
            acc += (long) (Math.random() * 1000);
        } catch (RuntimeException e) {                // handler => another frame
            acc = -1;
        }
        System.out.println("acc=" + acc);
        System.out.println("r=" + r.nextInt(1000000));
    }
}
"#;

/// Same shape, minus string concatenation: javac compiles `"x=" + n` to an
/// invokedynamic, and a class stamped major 49 may not carry one.  Original phone jars
/// are exactly that — pre-indy, pre-stackmap — so the legacy fixture has to be too.
const LEGACY_SRC: &str = r#"import java.util.Random;
public class Legacy {
    static long spin(int n) {
        long acc = 0;
        for (int i = 0; i < n; i++) {
            if ((i & 1) == 0) acc += System.currentTimeMillis();
            else acc -= System.nanoTime();
        }
        return acc;
    }
    public static void main(String[] a) {
        Random r = new Random();
        long acc = spin(8);
        try {
            if (r.nextInt(4) > 100) throw new IllegalStateException();
            acc += (long) (Math.random() * 1000);
        } catch (RuntimeException e) {
            acc = -1;
        }
        System.out.println(acc);
        System.out.println(r.nextInt(1000000));
    }
}
"#;

fn prerequisites() -> Result<(), String> {
    common::need_java_and_asm()?;
    if !fixedclock::available(None) {
        return Err("fixed-clock transformer helper not compiled (run `arlecchino setup`)".into());
    }
    Ok(())
}

/// Compile one source into a runnable single-class jar under `w`.
fn build_jar(w: &Path, name: &str, src_text: &str) -> Result<PathBuf, String> {
    let src_dir = w.join(format!("{name}_src"));
    let cls_dir = w.join(format!("{name}_classes"));
    std::fs::create_dir_all(&src_dir).map_err(|e| e.to_string())?;
    std::fs::write(src_dir.join(format!("{name}.java")), src_text).map_err(|e| e.to_string())?;
    common::javac_dir(&src_dir, &cls_dir, None, &format!("javac ({name})"))?;
    let jar = w.join(format!("{}.jar", name.to_lowercase()));
    common::pack_jar(
        &cls_dir,
        &jar,
        Some(&format!("Manifest-Version: 1.0\nMain-Class: {name}\n\n")),
    );
    Ok(jar)
}

fn run_jar_stdout(jar: &Path) -> String {
    let argv = vec![common::java(), "-jar".into(), jar.to_string_lossy().into_owned()];
    let r = arlecchino::util::run_capture(&argv, Some(Duration::from_secs(120)));
    assert!(r.success(), "java -jar failed: {}", r.stderr);
    r.stdout
}

/// Does any method of `entry` actually carry a StackMapTable ATTRIBUTE?
///
/// A plain substring test would lie: the writer copies the source constant pool, so the
/// UTF-8 string "StackMapTable" survives even when no method emits the attribute.  Match
/// the attribute header line instead.
fn has_stackmap(jar: &Path, entry: &str, w: &Path) -> Option<bool> {
    let cls = w.join("_javap_target.class");
    std::fs::write(&cls, arlecchino::util::zip_read_entry(jar, entry).unwrap()).unwrap();
    let argv = vec![
        arlecchino::util::tool("ARLECCHINO_JAVAP", "javap"),
        "-v".into(),
        "-p".into(),
        cls.to_string_lossy().into_owned(),
    ];
    let r = arlecchino::util::run_capture(&argv, Some(Duration::from_secs(120)));
    if !r.success() {
        return None;
    }
    Some(r.stdout.lines().any(|l| l.trim().starts_with("StackMapTable:")))
}

// ───────────────────────────── straight-line fixture ────────────────────────

#[test]
fn default_path_returns_the_original_jar_unchanged() {
    let w = common::workdir("fc-default");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let jar = match build_jar(&w, "Clocky", CLOCKY_SRC) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    // enabled = false must be a byte-identical no-op
    let out = fixedclock::maybe_transform(&jar, &w.join("_off"), None, false);
    assert_eq!(out, jar);
    assert!(!common::jar_entries(&jar).contains("arlecchino/det/Det.class"));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn transform_rewrites_time_and_rng_calls() {
    let w = common::workdir("fc-rewrite");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let jar = match build_jar(&w, "Clocky", CLOCKY_SRC) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let out = w.join("clocky.fixedclock.jar");
    assert_eq!(fixedclock::transform(&jar, &out, None, false).as_deref(), Some(out.as_path()));
    assert!(
        common::jar_entries(&out).contains("arlecchino/det/Det.class"),
        "the self-contained Det runtime must ship inside the jar"
    );

    let cls = w.join("Clocky.class");
    std::fs::write(&cls, arlecchino::util::zip_read_entry(&out, "Clocky.class").unwrap()).unwrap();
    let argv = vec![
        arlecchino::util::tool("ARLECCHINO_JAVAP", "javap"),
        "-p".into(),
        "-c".into(),
        cls.to_string_lossy().into_owned(),
    ];
    let r = arlecchino::util::run_capture(&argv, Some(Duration::from_secs(120)));
    if !r.success() {
        return common::skip("javap unavailable");
    }
    let text = r.stdout;
    for gone in ["java/lang/System.currentTimeMillis", "java/lang/System.nanoTime", "java/lang/Math.random"] {
        assert!(!text.contains(gone), "{gone} survived the rewrite");
    }
    for present in ["arlecchino/det/Det.currentTimeMillis", "arlecchino/det/Det.nanoTime", "arlecchino/det/Det.random"] {
        assert!(text.contains(present), "{present} redirect missing");
    }
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn instrumented_jar_runs_deterministically() {
    let w = common::workdir("fc-determinism");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let jar = match build_jar(&w, "Clocky", CLOCKY_SRC) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let out = w.join("clocky.fixedclock.jar");
    assert!(fixedclock::transform(&jar, &out, None, false).is_some());
    // the whole point: two SEPARATE JVM launches must agree byte for byte
    let a = run_jar_stdout(&out);
    let b = run_jar_stdout(&out);
    assert_eq!(a, b, "instrumented runs differ");
    assert!(a.contains("dt="), "{a}");
    assert!(a.lines().any(|l| l.starts_with("nano0=")), "{a}");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn the_fixture_is_genuinely_nondeterministic_before_instrumentation() {
    // Otherwise the determinism test above would be vacuous.
    let w = common::workdir("fc-sanity");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let jar = match build_jar(&w, "Clocky", CLOCKY_SRC) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    assert_ne!(
        run_jar_stdout(&jar),
        run_jar_stdout(&jar),
        "fixture unexpectedly deterministic without instrumentation"
    );
    let _ = std::fs::remove_dir_all(&w);
}

// ──────────────────── stackmap frames (class major >= 50) ───────────────────

#[test]
fn a_modern_classfile_keeps_its_stackmap_frames() {
    let w = common::workdir("fc-frames");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let jar = match build_jar(&w, "Branchy", BRANCHY_SRC) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    if common::class_major(&jar, "Branchy.class") < 50 {
        return common::skip("javac produced a pre-stackmap class file (major < 50)");
    }
    let out = w.join("branchy.fixedclock.jar");
    assert!(fixedclock::transform(&jar, &out, None, false).is_some());
    assert_eq!(common::class_major(&out, "Branchy.class"), common::class_major(&jar, "Branchy.class"));
    match has_stackmap(&out, "Branchy.class", &w) {
        None => return common::skip("javap unavailable"),
        Some(v) => assert!(v, "StackMapTable was dropped — the class will not verify"),
    }
    // the verifier is the real judge: running it is what used to blow up
    assert!(run_jar_stdout(&out).contains("acc="));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn the_recompute_frames_fallback_also_verifies() {
    let w = common::workdir("fc-recompute");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let jar = match build_jar(&w, "Branchy", BRANCHY_SRC) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    if common::class_major(&jar, "Branchy.class") < 50 {
        return common::skip("javac produced a pre-stackmap class file (major < 50)");
    }
    let out = w.join("branchy.recomputed.jar");
    assert!(fixedclock::transform(&jar, &out, None, true).is_some());
    match has_stackmap(&out, "Branchy.class", &w) {
        None => return common::skip("javap unavailable"),
        Some(v) => assert!(v, "the recompute path emitted no frames"),
    }
    assert!(run_jar_stdout(&out).contains("acc="));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn both_frame_strategies_produce_the_same_behaviour() {
    // Frames are metadata, not semantics: the instrumented program must be identical.
    let w = common::workdir("fc-agree");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let jar = match build_jar(&w, "Branchy", BRANCHY_SRC) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let kept = w.join("b_kept.jar");
    let recomputed = w.join("b_recomputed.jar");
    assert!(fixedclock::transform(&jar, &kept, None, false).is_some());
    assert!(fixedclock::transform(&jar, &recomputed, None, true).is_some());
    assert_eq!(run_jar_stdout(&kept), run_jar_stdout(&recomputed));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn a_legacy_classfile_takes_the_frameless_path() {
    // Original phone jars are major 48/49 — the old inferencing verifier, no frames
    // required.  The transformer must behave there exactly as it always did.
    let w = common::workdir("fc-legacy");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let src_dir = w.join("legacy_src");
    let cls_dir = w.join("legacy_classes");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("Legacy.java"), LEGACY_SRC).unwrap();
    if let Err(e) = common::javac_dir(&src_dir, &cls_dir, None, "javac (Legacy)") {
        return common::skip(&e);
    }
    // stamp it as Java 5 — the pre-stackmap world
    let cls = cls_dir.join("Legacy.class");
    let mut data = std::fs::read(&cls).unwrap();
    data[6] = 0;
    data[7] = 49;
    std::fs::write(&cls, &data).unwrap();
    let jar = w.join("legacy49.jar");
    common::pack_jar(&cls_dir, &jar, Some("Manifest-Version: 1.0\nMain-Class: Legacy\n\n"));
    assert_eq!(common::class_major(&jar, "Legacy.class"), 49);

    let out = w.join("legacy49.fixedclock.jar");
    assert!(fixedclock::transform(&jar, &out, None, false).is_some());
    assert_eq!(common::class_major(&out, "Legacy.class"), 49, "the class version must be untouched");
    match has_stackmap(&out, "Legacy.class", &w) {
        None => return common::skip("javap unavailable"),
        Some(v) => assert!(!v, "the legacy path unexpectedly started emitting frames"),
    }
    assert!(!run_jar_stdout(&out).trim().is_empty(), "instrumented legacy class produced no output");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn instrumentation_is_reproducible() {
    // Otherwise a "deterministic" run would start from a different input each time.
    let w = common::workdir("fc-reproducible");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let jar = match build_jar(&w, "Branchy", BRANCHY_SRC) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let a = w.join("rep_a.jar");
    let b = w.join("rep_b.jar");
    assert!(fixedclock::transform(&jar, &a, None, false).is_some());
    assert!(fixedclock::transform(&jar, &b, None, false).is_some());
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    let _ = std::fs::remove_dir_all(&w);
}
