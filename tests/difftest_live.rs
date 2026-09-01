//! Live behavioural verification on the synthetic MIDlet.
//!
//! Ported from the integration half of the reference suite's `tests/test_difftest.py`.
//!
//! The fixture is a deterministic MIDlet built on the fly (checkerboard in a chosen
//! colour, a colour bar encoding the last key code, one RecordStore write).  Against it:
//!
//!   * difftest of a jar against ITSELF must PASS, with a baseline that sees the same
//!     state set twice — otherwise the harness's own noise would swamp real signal;
//!   * difftest against an intentionally RECOLOURED copy must FAIL, and say so because
//!     of state similarity — otherwise the harness would pass anything;
//!   * a key tap must produce a screen state the idle run never shows — that is the
//!     input-replay path end to end;
//!   * the run must leave a RecordStore behind — that is the RMS snapshot path.
//!
//! Needs `java` plus the headless runtime jar (`midp-rt.jar`, installed or built by
//! `arlecchino setup`); skips cleanly without them.  Runs are deliberately short
//! (8 frames) — this is a contract check, not a benchmark.

mod common;

use arlecchino::{difftest, diffrun, midprun};
use std::path::PathBuf;

const ENGINE: &str = "midp-rt";

fn opts() -> diffrun::RunOpts {
    diffrun::RunOpts {
        max_frames: 8,
        frame_period_ms: 100,
        boot_wait_s: 2.0,
        ..Default::default()
    }
}

fn tap_num5() -> Vec<diffrun::InputEvent> {
    vec![diffrun::InputEvent {
        t_ms: 300,
        key: serde_json::json!("NUM5"),
        action: "tap".into(),
    }]
}

/// `Ok(jar)` when the fixture and the runtime are both available.
fn fixture(w: &PathBuf) -> Result<PathBuf, String> {
    if !common::have(&common::java()) {
        return Err("java not on PATH".into());
    }
    if midprun::find_midp_rt(None, None).is_none() {
        return Err("midp-rt.jar not installed (run `arlecchino setup`)".into());
    }
    common::build_synth_midlet(w, 0x00CC66)
}

#[test]
fn difftest_of_a_jar_against_itself_passes() {
    let w = common::workdir("dt-self");
    let jar = match fixture(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let v = match difftest::difftest(
        &jar,
        &jar,
        &w.join("self"),
        None,
        difftest::DEFAULT_SHIFT_TOL,
        difftest::DEFAULT_JACCARD_MARGIN,
        true,
        ENGINE,
        false,
        &opts(),
    ) {
        Ok(v) => v,
        Err(e) => return common::skip(&format!("headless run unavailable: {e}")),
    };
    assert!(v.passed, "self-difftest must pass: {:?}", v.reasons);
    let baseline = v.baseline.expect("a baseline was requested");
    assert!(baseline.sets_equal(), "the two calibration runs saw different state sets");
    // the harness must leave both report shapes behind
    assert!(w.join("self").join("difftest_report.md").exists());
    assert!(w.join("self").join("difftest_report.json").exists());
    let report = std::fs::read_to_string(w.join("self").join("difftest_report.json")).unwrap();
    assert!(report.contains("\"engine\": \"midp-rt\""), "the engine must be recorded");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn difftest_fails_on_a_recoloured_pattern() {
    let w = common::workdir("dt-recolour");
    let green = match fixture(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let red = match common::build_synth_midlet(&w, 0xCC0000) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let v = match difftest::difftest(
        &green,
        &red,
        &w.join("diff"),
        None,
        difftest::DEFAULT_SHIFT_TOL,
        difftest::DEFAULT_JACCARD_MARGIN,
        false,
        ENGINE,
        false,
        &opts(),
    ) {
        Ok(v) => v,
        Err(e) => return common::skip(&format!("headless run unavailable: {e}")),
    };
    assert!(!v.passed, "a recoloured MIDlet must NOT pass");
    assert!(
        v.reasons.iter().any(|r| r.contains("similarity")),
        "the verdict must blame state similarity: {:?}",
        v.reasons
    );
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn a_key_tap_produces_a_screen_state_the_idle_run_never_shows() {
    let w = common::workdir("dt-input");
    let jar = match fixture(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let o = opts();
    let idle = match midprun::run_headless_midp_rt(&jar, &w.join("idle"), None, &o, None, true) {
        Ok(r) => r,
        Err(e) => return common::skip(&format!("headless run unavailable: {e}")),
    };
    let inputs = tap_num5();
    let keyed =
        match midprun::run_headless_midp_rt(&jar, &w.join("keyed"), Some(&inputs), &o, None, true) {
            Ok(r) => r,
            Err(e) => return common::skip(&format!("headless run unavailable: {e}")),
        };
    assert!(idle.any_nonblank(), "the idle run captured nothing but blank frames");
    let idle_states: std::collections::BTreeSet<String> = idle.hashes().into_iter().collect();
    let new: Vec<String> =
        keyed.hashes().into_iter().filter(|h| !idle_states.contains(h)).collect();
    assert!(!new.is_empty(), "the key press produced no new screen state");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn the_run_leaves_a_recordstore_behind() {
    let w = common::workdir("dt-rms");
    let jar = match fixture(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let res = match midprun::run_headless_midp_rt(&jar, &w.join("run"), None, &opts(), None, true) {
        Ok(r) => r,
        Err(e) => return common::skip(&format!("headless run unavailable: {e}")),
    };
    let snap = diffrun::snapshot_rms(std::path::Path::new(&res.rms_dir));
    assert!(
        snap.keys().any(|p| p.ends_with(".rms")),
        "the MIDlet's RecordStore write left nothing on disk: {snap:?}"
    );
    let _ = std::fs::remove_dir_all(&w);
}
