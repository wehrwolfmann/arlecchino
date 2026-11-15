//! What a game sees of the screen and of its record store under midp-rt
//! `--resizableDevice W H`, built from this checkout's `midp-rt/` sources.
//!
//! The fixture (`tests/fixtures/canvassize/SizeMidlet.java`) writes every answer of
//! `Canvas.getWidth()/getHeight()` into the trace as a `fillRect`, and makes a fixed
//! set of RMS calls.  Against it:
//!
//!   * a normal canvas is the screen minus the soft-key bar (W × (H − 16): the
//!     resizable device's soft keys are 16 px high), a full-screen canvas is the
//!     whole W × H screen, the captured frame is W × H either way, and the size
//!     asked for before `setFullScreenMode` does not stick after it;
//!   * the trace carries exactly the RMS calls the game made — one
//!     `RecordStore.getNumRecords` — and none of the ones midp-rt makes for itself
//!     while it enumerates the records and saves the store to disk; the records
//!     themselves are traced byte for byte.
//!
//! Needs `javac`, `java` and `midp-rt/lib/asm-*.jar`; skips cleanly without them.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

/// Height of the soft-key bar of midp-rt's resizable device (`resizable/device.xml`).
const SOFT_KEYS: i64 = 16;

const MANIFEST: &str = "Manifest-Version: 1.0\n\
                        MIDlet-Name: ArlecchinoSize\n\
                        MIDlet-Vendor: Arlecchino\n\
                        MIDlet-Version: 1.0\n\
                        MIDlet-1: Size, , SizeMidlet\n\
                        MicroEdition-Configuration: CLDC-1.1\n\
                        MicroEdition-Profile: MIDP-2.0\n";

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn build_midlet(work: &Path, rt_cp: &str, full: bool) -> Result<PathBuf, String> {
    let tag = if full { "full" } else { "normal" };
    let src_dir = work.join(format!("src_{tag}"));
    let cls_dir = work.join(format!("classes_{tag}"));
    std::fs::create_dir_all(&src_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&cls_dir).map_err(|e| e.to_string())?;
    let template = std::fs::read_to_string(common::fixtures_dir().join("canvassize").join("SizeMidlet.java"))
        .map_err(|e| e.to_string())?;
    let src = src_dir.join("SizeMidlet.java");
    std::fs::write(&src, template.replace("@FULL@", if full { "true" } else { "false" })).map_err(|e| e.to_string())?;
    common::run(
        &[
            common::javac(),
            "--release".into(),
            "8".into(),
            "-nowarn".into(),
            "-proc:none".into(),
            "-cp".into(),
            rt_cp.to_string(),
            "-d".into(),
            s(&cls_dir),
            s(&src),
        ],
        "javac (SizeMidlet)",
    )?;
    let jar = work.join(format!("size_{tag}.jar"));
    common::pack_jar(&cls_dir, &jar, Some(MANIFEST));
    Ok(jar)
}

struct Run {
    trace: String,
    index: String,
    rms: PathBuf,
}

fn run(rt_cp: &str, jar: &Path, dir: &Path, size: (i64, i64)) -> Run {
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let trace = dir.join("trace.txt");
    let frames = dir.join("frames");
    let rms = dir.join("rms");
    let out = Command::new(common::java())
        .arg("-Djava.awt.headless=true")
        .arg(format!("-Duser.home={}", s(&dir.join("home"))))
        .arg(format!("-Dmidprt.trace={}", s(&trace)))
        .arg("-cp")
        .arg(rt_cp)
        .arg("org.microemu.app.Headless")
        .args(["--frames", "3", "--period-ms", "100", "--boot-wait-ms", "800"])
        .arg("--frames-dir")
        .arg(&frames)
        .arg("--rms")
        .arg(&rms)
        .args(["--resizableDevice", &size.0.to_string(), &size.1.to_string()])
        .arg(jar)
        .output()
        .expect("spawn java");
    assert!(out.status.success(), "midp-rt failed: {}", String::from_utf8_lossy(&out.stderr));
    Run {
        trace: std::fs::read_to_string(&trace).unwrap_or_default(),
        index: std::fs::read_to_string(frames.join("index.json")).unwrap_or_default(),
        rms,
    }
}

/// The size the game saw: `fillRect <x> <x> W H` with the given marker `x`.
fn seen(trace: &str, marker: i64) -> Option<(i64, i64)> {
    let head = format!("fillRect {marker} {marker} ");
    trace.lines().find_map(|l| {
        let p: Vec<&str> = l.strip_prefix(&head)?.split(' ').collect();
        Some((p.first()?.parse().ok()?, p.get(1)?.parse().ok()?))
    })
}

fn check_size(full: bool) {
    let rt_cp = match common::midp_rt_from_checkout() {
        Ok(v) => v,
        Err(e) => return common::skip(&e),
    };
    let work = common::workdir(if full { "canvas-full" } else { "canvas-normal" });
    let jar = build_midlet(&work, &rt_cp, full).expect("SizeMidlet builds");
    for size in [(240, 320), (128, 160)] {
        let r = run(&rt_cp, &jar, &work.join(format!("{}x{}", size.0, size.1)), size);
        let normal = (size.0, size.1 - SOFT_KEYS);
        let want_after = if full { size } else { normal };
        assert_eq!(seen(&r.trace, 1), Some(normal), "size before setFullScreenMode at {size:?}:\n{}", r.trace);
        assert_eq!(seen(&r.trace, 2), Some(want_after), "size right after setFullScreenMode({full}) at {size:?}");
        assert_eq!(seen(&r.trace, 0), Some(want_after), "size in paint() with setFullScreenMode({full}) at {size:?}");
        let dims = format!("\"width\": {}, \"height\": {},", size.0, size.1);
        assert!(r.index.contains(&dims), "the frame is not {}x{}: {}", size.0, size.1, r.index);
    }
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn a_normal_canvas_is_the_screen_minus_the_soft_keys() {
    check_size(false);
}

#[test]
fn a_full_screen_canvas_is_the_whole_screen() {
    check_size(true);
}

#[test]
fn the_trace_holds_only_the_rms_calls_of_the_game() {
    let rt_cp = match common::midp_rt_from_checkout() {
        Ok(v) => v,
        Err(e) => return common::skip(&e),
    };
    let work = common::workdir("canvas-rms");
    let jar = build_midlet(&work, &rt_cp, false).expect("SizeMidlet builds");
    let r = run(&rt_cp, &jar, &work.join("run"), (240, 320));
    let rms: Vec<&str> = r
        .trace
        .lines()
        .filter(|l| l.starts_with("RecordStore") || l.starts_with("RecordEnumeration"))
        .collect();
    assert_eq!(
        rms,
        vec![
            "RecordStore.open \"sizestore\"",
            "RecordStore.addRecord \"sizestore\" len=3 hex=010203",
            "RecordStore.addRecord \"sizestore\" len=2 hex=0405",
            "RecordEnumeration.nextRecord \"sizestore\" len=3 hex=010203",
            "RecordEnumeration.nextRecord \"sizestore\" len=2 hex=0405",
            "RecordStore.getNumRecords \"sizestore\" 2",
            "RecordStore.close \"sizestore\"",
        ],
        "{}",
        r.trace
    );
    // the store really went to disk, i.e. the runtime's own save path ran
    let saved = std::fs::read_dir(&r.rms).map(|d| d.count()).unwrap_or(0);
    assert!(saved > 0, "no record store written to {}", r.rms.display());
    let _ = std::fs::remove_dir_all(&work);
}
