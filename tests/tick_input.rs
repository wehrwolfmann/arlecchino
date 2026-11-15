//! Deterministic input in midp-rt (`Headless --ticks N`, see the deterministic-input
//! addendum of `docs/determinism-fixed-clock.md`).
//!
//! The runtime is compiled from THIS checkout's `midp-rt/` sources (not the
//! installed jar), so the test pins the code under review.  The fixture is the
//! looping synthetic MIDlet: a game-style thread ticks with repaint +
//! serviceRepaints, and every frame records the paint index at which the last key
//! press arrived (`fillRect <index> 0 1 1`).  Against it:
//!
//!   * two runs with the same tick scenario produce byte-identical traces and
//!     byte-identical frame indexes — no wall clock, no scheduler in the result;
//!   * the press scheduled for tick N is delivered right after `F N` and the game
//!     sees it on paint N exactly (and not one frame earlier or later);
//!   * a time-based (`t_ms`) scenario is refused in tick mode, so a stale
//!     scenario cannot silently fall back to wall-clock delivery.
//!
//! Needs `javac`, `java` and `midp-rt/lib/asm-*.jar`; skips cleanly without them.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

const KEY_NUM5: i32 = 53;
const PRESS_TICK: usize = 5;
const RELEASE_TICK: usize = 9;
const TICKS: usize = 16;

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn midp_rt_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("midp-rt")
}

fn asm_jar() -> Option<PathBuf> {
    let lib = midp_rt_src().join("lib");
    let mut jars: Vec<PathBuf> = std::fs::read_dir(&lib)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with("asm") && n.ends_with(".jar")
        })
        .collect();
    jars.sort();
    jars.pop()
}

/// Compile midp-rt from the checkout into `<work>/midp-rt-classes` (classes +
/// resources, run straight off the directory).
fn build_midp_rt(work: &Path, asm: &Path) -> Result<PathBuf, String> {
    let out = work.join("midp-rt-classes");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let modules = [
        "microemu-cldc",
        "microemu-midp",
        "microemu-javase",
        "microemu-javase-swing",
        "microemu-extensions/microemu-nokiaui",
        "microemu-extensions/microemu-siemensapi",
    ];
    let mut srcs = Vec::new();
    for m in modules {
        let base = midp_rt_src().join(m).join("src").join("main");
        let mut v = arlecchino::util::rglob_ext(&base.join("java"), "java");
        srcs.append(&mut v);
        let res = base.join("resources");
        if res.is_dir() {
            for f in arlecchino::util::walk_files(&res) {
                let dst = out.join(f.strip_prefix(&res).unwrap());
                std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::copy(&f, &dst).map_err(|e| e.to_string())?;
            }
        }
    }
    srcs.sort();
    let list = work.join("midp-rt-sources.txt");
    std::fs::write(&list, srcs.iter().map(|p| s(p)).collect::<Vec<_>>().join("\n"))
        .map_err(|e| e.to_string())?;
    common::run(
        &[
            common::javac(),
            "--release".into(),
            "17".into(),
            "-encoding".into(),
            "UTF-8".into(),
            "-nowarn".into(),
            "-cp".into(),
            s(asm),
            "-d".into(),
            s(&out),
            format!("@{}", s(&list)),
        ],
        "javac (midp-rt)",
    )?;
    Ok(out)
}

struct Fixture {
    work: PathBuf,
    rt_cp: String,
    jar: PathBuf,
}

/// The runtime classpath, compiled once per test binary.
fn runtime() -> Result<String, String> {
    static RT: std::sync::OnceLock<Result<String, String>> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        if !common::have(&common::javac()) || !common::have(&common::java()) {
            return Err("javac/java not on PATH".into());
        }
        let asm = asm_jar().ok_or("midp-rt/lib/asm-*.jar missing")?;
        let classes = build_midp_rt(&common::workdir("ticks-rt"), &asm)?;
        let sep = if cfg!(windows) { ";" } else { ":" };
        Ok(format!("{}{sep}{}", s(&classes), s(&asm)))
    })
    .clone()
}

fn fixture(tag: &str) -> Result<Fixture, String> {
    let rt_cp = runtime()?;
    let work = common::workdir(tag);
    let jar = common::build_synth_midlet_looping(&work, 0x00CC66)?;
    Ok(Fixture { rt_cp, work, jar })
}

struct Run {
    code: Option<i32>,
    trace: String,
    index: String,
    stderr: String,
}

fn run(f: &Fixture, name: &str, inputs: &Path) -> Run {
    let dir = f.work.join(name);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let trace = dir.join("trace.txt");
    let frames = dir.join("frames");
    let out = Command::new(common::java())
        .arg("-Djava.awt.headless=true")
        .arg(format!("-Duser.home={}", s(&dir.join("home"))))
        .arg(format!("-Dmidprt.trace={}", s(&trace)))
        .arg("-cp")
        .arg(&f.rt_cp)
        .arg("org.microemu.app.Headless")
        .args(["--ticks", &TICKS.to_string(), "--tick-timeout-ms", "20000"])
        .arg("--inputs")
        .arg(inputs)
        .arg("--frames-dir")
        .arg(&frames)
        .args(["--resizableDevice", "240", "320"])
        .arg(&f.jar)
        .output()
        .expect("spawn java");
    Run {
        code: out.status.code(),
        trace: std::fs::read_to_string(&trace).unwrap_or_default(),
        index: std::fs::read_to_string(frames.join("index.json")).unwrap_or_default(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn scenario(f: &Fixture) -> PathBuf {
    let p = f.work.join("ticks.json");
    std::fs::write(
        &p,
        format!(
            "[\n  {{\"tick\": {PRESS_TICK}, \"key\": \"NUM5\", \"action\": \"press\"}},\n  \
             {{\"tick\": {RELEASE_TICK}, \"key\": \"NUM5\", \"action\": \"release\"}}\n]\n"
        ),
    )
    .unwrap();
    p
}

/// Trace lines between `F n` and the next frame boundary.
fn frame_block(trace: &str, n: usize) -> Vec<&str> {
    let head = format!("F {n}");
    trace
        .lines()
        .skip_while(|l| *l != head)
        .skip(1)
        .take_while(|l| !l.starts_with("F ") && !l.starts_with("# histogram"))
        .collect()
}

#[test]
fn two_runs_with_the_same_tick_scenario_are_byte_identical() {
    let f = match fixture("ticks-same") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let inputs = scenario(&f);
    let a = run(&f, "a", &inputs);
    let b = run(&f, "b", &inputs);
    assert_eq!(a.code, Some(0), "run a failed: {}", a.stderr);
    assert_eq!(b.code, Some(0), "run b failed: {}", b.stderr);
    assert!(!a.trace.is_empty(), "no trace written");
    let frames = a.trace.lines().filter(|l| l.starts_with("F ")).count();
    assert_eq!(frames, TICKS, "exactly --ticks frame boundaries must be traced");
    assert!(a.trace == b.trace, "the traces of two identical tick runs differ");
    assert!(!a.index.is_empty() && a.index == b.index, "the frame indexes differ");
    assert_eq!(a.index.matches("\"sha256\"").count(), TICKS, "one captured frame per tick");
    let _ = std::fs::remove_dir_all(&f.work);
}

#[test]
fn the_press_is_seen_by_the_game_exactly_on_its_tick() {
    let f = match fixture("ticks-exact") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let r = run(&f, "run", &scenario(&f));
    assert_eq!(r.code, Some(0), "run failed: {}", r.stderr);
    // delivery: right after its frame boundary, before the frame is drawn
    let press = frame_block(&r.trace, PRESS_TICK);
    assert_eq!(press.first().copied(), Some(format!("keyPressed {KEY_NUM5}").as_str()), "{press:?}");
    let release = frame_block(&r.trace, RELEASE_TICK);
    assert_eq!(release.first().copied(), Some(format!("keyReleased {KEY_NUM5}").as_str()));
    assert_eq!(r.trace.lines().filter(|l| l.starts_with("keyPressed ")).count(), 1);
    // the game's own view: no press before PRESS_TICK, then "seen on paint PRESS_TICK"
    let seen = format!("fillRect {PRESS_TICK} 0 1 1");
    for n in 0..TICKS {
        let block = frame_block(&r.trace, n);
        if n < PRESS_TICK {
            assert!(block.contains(&"fillRect -1 0 1 1"), "frame {n} already saw a press: {block:?}");
        } else {
            assert!(block.contains(&seen.as_str()), "frame {n} does not show the press of tick {PRESS_TICK}");
        }
    }
    let _ = std::fs::remove_dir_all(&f.work);
}

#[test]
fn a_wall_clock_scenario_is_refused_in_tick_mode() {
    let f = match fixture("ticks-tms") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let p = f.work.join("tms.json");
    std::fs::write(&p, "[{\"t_ms\": 300, \"key\": \"NUM5\", \"action\": \"tap\"}]").unwrap();
    let r = run(&f, "tms", &p);
    assert_eq!(r.code, Some(2), "a t_ms scenario must be rejected: {}", r.stderr);
    assert!(r.stderr.contains("tick"), "the error must name the tick field: {}", r.stderr);
    let _ = std::fs::remove_dir_all(&f.work);
}
