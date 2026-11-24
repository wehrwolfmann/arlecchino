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

// ---- pixels ------------------------------------------------------------------

const RENDER_TICKS: i32 = 24;

/// The test's own images: RGBA with three alpha levels, and a palette with tRNS.
fn render_images(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let (w, h) = (12u32, 10u32);
    let mut rgba = vec![];
    for y in 0..h {
        for x in 0..w {
            let a = match (x + y) % 3 {
                0 => 255,
                1 => 128,
                _ => 0,
            };
            rgba.extend_from_slice(&[(x * 20) as u8, (y * 25) as u8, 200, a]);
        }
    }
    let write = |name: &str, color: png::ColorType, data: &[u8], palette: Option<(&[u8], &[u8])>| {
        let f = std::fs::File::create(dir.join(name)).unwrap();
        let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        e.set_color(color);
        e.set_depth(png::BitDepth::Eight);
        if let Some((pal, trns)) = palette {
            e.set_palette(pal.to_vec());
            e.set_trns(trns.to_vec());
        }
        e.write_header().unwrap().write_image_data(data).unwrap();
    };
    write("rgba.png", png::ColorType::Rgba, &rgba, None);
    let pal: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 250, 250, 40];
    let trns: Vec<u8> = vec![255, 0, 255, 255];
    let idx: Vec<u8> = (0..w * h).map(|i| ((i / 3 + i / w) % 4) as u8).collect();
    write("pal.png", png::ColorType::Indexed, &idx, Some((&pal, &trns)));
}

/// `(jar, source dir, MIDP stub, resource dir)` of the rendering MIDlet.
fn render_midlet(w: &Path) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), String> {
    if !common::have(&common::java()) || !common::have(&common::javac()) {
        return Err("java/javac not on PATH".into());
    }
    let stub = common::full_midp_stub(w)?;
    let src = w.join("render_src");
    let classes = w.join("render_classes");
    let res = w.join("render_res");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::copy(common::fixtures_dir().join("render").join("RenderMidlet.java"), src.join("RenderMidlet.java")).unwrap();
    std::fs::create_dir_all(&classes).unwrap();
    let ps = |p: &Path| p.to_string_lossy().into_owned();
    common::run(
        &[
            common::javac(),
            "--release".into(),
            "8".into(),
            "-nowarn".into(),
            "-proc:none".into(),
            "-cp".into(),
            ps(&stub),
            "-d".into(),
            ps(&classes),
            ps(&src.join("RenderMidlet.java")),
        ],
        "javac (RenderMidlet)",
    )?;
    render_images(&res);
    // the jar: classes, the two images, a manifest
    let jar = w.join("render.jar");
    {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
        let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        z.start_file("META-INF/MANIFEST.MF", o).unwrap();
        z.write_all(
            b"Manifest-Version: 1.0\nMIDlet-Name: ArlecchinoRender\nMIDlet-Vendor: Arlecchino\nMIDlet-Version: 1.0\n\
              MIDlet-1: Render, , RenderMidlet\nMicroEdition-Configuration: CLDC-1.1\nMicroEdition-Profile: MIDP-2.0\n",
        )
        .unwrap();
        for dir in [&classes, &res] {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
            files.sort();
            for p in files {
                z.start_file(p.file_name().unwrap().to_string_lossy(), o).unwrap();
                z.write_all(&std::fs::read(&p).unwrap()).unwrap();
            }
        }
        z.finish().unwrap();
    }
    Ok((jar, src, stub, res))
}

#[test]
fn translated_frames_match_midp_rt_pixel_for_pixel() {
    let w = common::workdir("port-rust-render");
    let (jar, src, stub, res) = match render_midlet(&w) {
        Ok(v) => v,
        Err(e) => return common::skip(&e),
    };
    let crate_dir = w.join("render-port");
    let t = Translation {
        sources: src.clone(),
        crate_dir: crate_dir.clone(),
        crate_name: "render-port".into(),
        classpath: stub.to_string_lossy().into_owned(),
        maps: vec![],
        api_table: portrust::API_TABLE.to_string(),
        jrt_dependency: portrust::jrt_dependency(None),
    };
    let r = portrust::translate(&t).unwrap_or_else(|e| panic!("port-rust failed: {e}"));
    assert_eq!(r.outcome.stats.todo_sites, 0, "{}", r.outcome.report);
    // its own target dir: the trace test builds another crate's `trace` binary in
    // port-rust-test, and the two tests run in parallel
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("port-rust-test").join("render");
    let out = std::process::Command::new(cargo())
        .args(["build", "--quiet", "--manifest-path"])
        .arg(crate_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .args(["--bin", "trace"])
        .output()
        .expect("cargo");
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        if stderr.contains("failed to fetch") || stderr.contains("failed to load source") || stderr.contains("Could not resolve") {
            return common::skip("jrt dependencies not available offline");
        }
        panic!("the translated crate does not build:\n{stderr}");
    }

    // one key press on tick 7, so the frames also show input arriving on its tick
    let inputs = w.join("ticks.json");
    std::fs::write(&inputs, "[{\"tick\": 7, \"key\": \"NUM5\", \"action\": \"tap\"}]\n").unwrap();
    let port_frames = w.join("port-frames");
    let port_trace = w.join("port-trace.txt");
    let st = std::process::Command::new(target.join("debug").join("trace"))
        .arg(&res)
        .arg(&port_trace)
        .args(["240", "320", &RENDER_TICKS.to_string(), "--inputs"])
        .arg(&inputs)
        .arg("--frames-dir")
        .arg(&port_frames)
        .status()
        .expect("trace binary");
    assert!(st.success());
    let port = read(&port_trace);
    assert!(!port.contains("# STOPPED"), "the translation stopped early:\n{port}");
    assert_eq!(portrust::frames::list_frames(&port_frames).unwrap().len(), RENDER_TICKS as usize);

    let rt_cp = match common::midp_rt_from_checkout() {
        Ok(v) => v,
        Err(e) => return common::skip(&format!("midp-rt not buildable ({e}): port frames not compared")),
    };
    let ref_frames = w.join("ref-frames");
    let ref_trace = w.join("ref-trace.txt");
    std::fs::create_dir_all(w.join("home")).unwrap();
    let out = std::process::Command::new(common::java())
        .arg("-Djava.awt.headless=true")
        .arg(format!("-Duser.home={}", w.join("home").display()))
        .arg(format!("-Dmidprt.trace={}", ref_trace.display()))
        .arg(format!("-Dmidprt.trace.frames={RENDER_TICKS}"))
        .arg("-cp")
        .arg(&rt_cp)
        .arg("org.microemu.app.Headless")
        .args(["--ticks", &RENDER_TICKS.to_string(), "--tick-timeout-ms", "20000", "--inputs"])
        .arg(&inputs)
        .arg("--frames-dir")
        .arg(&ref_frames)
        .args(["--resizableDevice", "240", "320"])
        .arg(&jar)
        .output()
        .expect("spawn java");
    if out.status.code() != Some(0) {
        return common::skip(&format!("headless midp-rt run failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let d = portrust::frames::compare_dirs(&ref_frames, &port_frames, 0, Some(&w.join("diff"))).unwrap();
    assert_eq!(d.frames.len(), RENDER_TICKS as usize);
    let bad: Vec<String> = d.frames.iter().filter(|f| f.differing != 0).map(|f| format!("frame {}: {} px {:?}", f.index, f.differing, f.bbox)).collect();
    assert!(bad.is_empty(), "frames differ from midp-rt (diff pictures in {}):\n{}", w.join("diff").display(), bad.join("\n"));
    let td = trace::compare(&read(&ref_trace), &port, &[], true);
    assert_eq!(td.differing, 0, "--- midp-rt\n{}\n--- port\n{}", td.reference.lines.join("\n"), td.port.lines.join("\n"));
}
