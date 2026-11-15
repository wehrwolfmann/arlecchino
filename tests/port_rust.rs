//! `port-rust` end to end on the synthetic MIDlet (`tests/fixtures/synthjar/midlet`).
//!
//! * the translation leaves nothing untranslated and keeps the Java names, in Rust
//!   casing (`SynthCanvas`, `last_key`, `PATTERN_COLOR`, `key_pressed`);
//! * a `.map` name map is applied while generating (renamed class, field and
//!   parameters show up in the Rust);
//! * the generated crate builds, its headless run writes a call trace, and that
//!   trace equals the committed golden one (`tests/golden/port_rust_synth.trace`,
//!   240×320) and the trace midp-rt (built from this checkout) writes for the
//!   original jar, line for line after canonicalisation.
//!
//! Needs `javac` and `java` (JDK 17+, for the AST helper); the build test also
//! needs `cargo` and the `jrt` dependencies (fetched once, then cached).  Skips
//! cleanly without them.

mod common;

use arlecchino::portrust::{self, trace, Translation};
use std::path::{Path, PathBuf};

const COLOR: u32 = 0x00CC66;
const FRAMES: i32 = 20;

/// `(jar, source dir, MIDP stub)` of the synthetic MIDlet.
fn synth(w: &Path) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    if !common::have(&common::java()) || !common::have(&common::javac()) {
        return Err("java/javac not on PATH".into());
    }
    let jar = common::build_synth_midlet(w, COLOR)?;
    let src = w.join(format!("midlet_src_{:06x}", COLOR));
    let stub = common::full_midp_stub(w)?;
    Ok((jar, src, stub))
}

fn translate(w: &Path, src: &Path, stub: &Path, maps: Vec<PathBuf>) -> Result<(portrust::Translated, PathBuf), String> {
    let crate_dir = w.join("synth-port");
    let t = Translation {
        sources: src.to_path_buf(),
        crate_dir: crate_dir.clone(),
        crate_name: "synth-port".into(),
        classpath: stub.to_string_lossy().into_owned(),
        maps,
        api_table: portrust::API_TABLE.to_string(),
        jrt_dependency: portrust::jrt_dependency(None),
    };
    portrust::translate(&t).map(|r| (r, crate_dir))
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn synth_midlet_translates_completely_with_its_own_names() {
    let w = common::workdir("port-rust-names");
    let (_, src, stub) = match synth(&w) {
        Ok(v) => v,
        Err(e) => return common::skip(&e),
    };
    let (r, dir) = match translate(&w, &src, &stub, vec![]) {
        Ok(v) => v,
        Err(e) => panic!("port-rust failed: {e}"),
    };
    let st = &r.outcome.stats;
    assert_eq!(r.javac_errors, 0);
    assert_eq!(st.todo_sites, 0, "untranslated: {:?}\n{}", r.outcome.residue, r.outcome.report);
    assert_eq!(st.methods, st.methods_clean);
    assert!(st.methods >= 6, "{}", r.outcome.report);

    let lib = read(&dir.join("src/lib.rs"));
    assert!(lib.contains("pub mod synth_midlet;") && lib.contains("pub mod synth_canvas;"), "{lib}");
    let canvas = read(&dir.join("src/synth_canvas.rs"));
    for needle in [
        "pub struct SynthCanvas {",
        "pub last_key: i32,",
        "pub static PATTERN_COLOR:",
        "pub fn paint(this: &jrt::JRef<SynthCanvas>, mut g: jrt::api::Graphics)",
        "pub fn key_pressed(this: &jrt::JRef<SynthCanvas>, mut key_code: i32)",
        "jrt::put!(this, last_key, key_code)",
    ] {
        assert!(canvas.contains(needle), "missing `{needle}` in:\n{canvas}");
    }
    let midlet = read(&dir.join("src/synth_midlet.rs"));
    for needle in ["pub struct SynthMidlet {", "pub canvas: jrt::JRef<crate::synth_canvas::SynthCanvas>,", "pub fn start_app("] {
        assert!(midlet.contains(needle), "missing `{needle}` in:\n{midlet}");
    }
    // locals keep their names too
    assert!(midlet.contains("let mut rs: jrt::api::RecordStore"), "{midlet}");
}

#[test]
fn name_map_is_applied_while_generating() {
    let w = common::workdir("port-rust-map");
    let (_, src, stub) = match synth(&w) {
        Ok(v) => v,
        Err(e) => return common::skip(&e),
    };
    let map = w.join("synth.map");
    std::fs::write(
        &map,
        "import javax.microedition.lcdui.Graphics;\n\
         class CheckerCanvas /* was SynthCanvas */ {\n\
         \x20   static final int squareColor /* was PATTERN_COLOR */;\n\
         \x20   int lastKeyCode /* was lastKey */;\n\
         \x20   void paint(Graphics screen) /* was paint */;\n\
         \x20   void keyPressed(int code) /* was keyPressed */;\n\
         }\n\
         class SynthMidlet /* was SynthMidlet */ {\n\
         \x20   CheckerCanvas board /* was canvas */;\n\
         }\n",
    )
    .unwrap();
    let (r, dir) = match translate(&w, &src, &stub, vec![map]) {
        Ok(v) => v,
        Err(e) => panic!("port-rust failed: {e}"),
    };
    assert!(r.map_warnings.is_empty(), "{:?}", r.map_warnings);
    assert_eq!(r.outcome.stats.todo_sites, 0, "{}", r.outcome.report);
    let lib = read(&dir.join("src/lib.rs"));
    assert!(lib.contains("pub mod checker_canvas;") && !lib.contains("synth_canvas"), "{lib}");
    let canvas = read(&dir.join("src/checker_canvas.rs"));
    for needle in [
        "pub struct CheckerCanvas {",
        "pub last_key_code: i32,",
        "pub static SQUARE_COLOR:",
        "pub fn paint(this: &jrt::JRef<CheckerCanvas>, mut screen: jrt::api::Graphics)",
        "pub fn key_pressed(this: &jrt::JRef<CheckerCanvas>, mut code: i32)",
    ] {
        assert!(canvas.contains(needle), "missing `{needle}` in:\n{canvas}");
    }
    let midlet = read(&dir.join("src/synth_midlet.rs"));
    assert!(midlet.contains("pub board: jrt::JRef<crate::checker_canvas::CheckerCanvas>,"), "{midlet}");
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

#[test]
fn translated_crate_builds_and_matches_the_reference_trace() {
    let w = common::workdir("port-rust-trace");
    let (jar, src, stub) = match synth(&w) {
        Ok(v) => v,
        Err(e) => return common::skip(&e),
    };
    let (r, dir) = match translate(&w, &src, &stub, vec![]) {
        Ok(v) => v,
        Err(e) => panic!("port-rust failed: {e}"),
    };
    assert_eq!(r.outcome.stats.todo_sites, 0);
    // build on disk, next to this crate's own target (never in a RAM-backed temp dir)
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("port-rust-test");
    let argv = vec![
        cargo(),
        "build".to_string(),
        "--quiet".into(),
        "--manifest-path".into(),
        dir.join("Cargo.toml").to_string_lossy().into_owned(),
        "--target-dir".into(),
        target.to_string_lossy().into_owned(),
        "--bin".into(),
        "trace".into(),
    ];
    let out = std::process::Command::new(&argv[0]).args(&argv[1..]).output().expect("cargo");
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        if stderr.contains("failed to fetch") || stderr.contains("failed to load source") || stderr.contains("Could not resolve") {
            return common::skip(&format!("jrt dependencies not available offline: {}", stderr.lines().next().unwrap_or("")));
        }
        panic!("the translated crate does not build:\n{stderr}");
    }
    assert!(!stderr.contains("error"), "{stderr}");

    let res = w.join("res");
    std::fs::create_dir_all(&res).unwrap();
    let bin = target.join("debug").join("trace");
    let run_port = |size: (i64, i64), tag: &str| -> String {
        let out = w.join(format!("port-trace-{tag}.txt"));
        let st = std::process::Command::new(&bin)
            .args([res.to_str().unwrap(), out.to_str().unwrap(), &size.0.to_string(), &size.1.to_string(), &FRAMES.to_string()])
            .status()
            .expect("trace binary");
        assert!(st.success());
        let t = read(&out);
        assert!(!t.contains("# STOPPED"), "the translation stopped early:\n{t}");
        t
    };
    let port = run_port((240, 320), "240x320");

    let golden_path = common::golden_dir().join("port_rust_synth.trace");
    let golden = read(&golden_path);
    let canon = trace::canonical(&port, &Default::default());
    let want: Vec<String> = golden.lines().map(str::to_string).collect();
    assert_eq!(
        trace::differing_lines(&want, &canon.lines),
        0,
        "port trace differs from {}:\n{}",
        golden_path.display(),
        canon.lines.join("\n")
    );

    // the reference comes from midp-rt built from this checkout, not from an installed
    // jar that may predate the runtime fixes this comparison relies on
    let midp_rt = match common::midp_rt_checkout_build() {
        Ok(p) => p,
        Err(e) => return common::skip(&format!("midp-rt not buildable ({e}): compared with the golden trace only")),
    };
    let refw = w.join("midp-rt");
    std::fs::create_dir_all(&refw).unwrap();
    let reference = match trace::midp_rt_trace(&jar, &midp_rt, &refw, FRAMES as i64, (240, 320)) {
        Ok(t) => t,
        Err(e) => return common::skip(&format!("headless midp-rt run unavailable: {e}")),
    };
    // midp-rt answers Canvas.getWidth()/getHeight() with the canvas area: for a normal
    // (not full-screen) canvas that is the --resizableDevice screen minus the soft-key
    // bar. The fixture's first call fills the whole canvas, so the reference itself
    // says which size to give the port.
    let size = reference
        .lines()
        .find_map(|l| {
            let p: Vec<&str> = l.strip_prefix("fillRect 0 0 ")?.split(' ').collect();
            Some((p.first()?.parse().ok()?, p.get(1)?.parse().ok()?))
        })
        .expect("the reference fills the canvas");
    let port_same_size = if size == (240, 320) { port.clone() } else { run_port(size, "reference-size") };
    let d = trace::compare(&reference, &port_same_size, &[], true);
    assert_eq!(
        d.differing, 0,
        "midp-rt and the translation disagree\n--- midp-rt\n{}\n--- port\n{}",
        d.reference.lines.join("\n"),
        d.port.lines.join("\n")
    );
}
