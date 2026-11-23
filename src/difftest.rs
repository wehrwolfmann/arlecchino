//! Stage A5 (PLAN_runtime.md, block A) — the behavioural comparison harness.
//!
//! Port of `arlecchino/difftest.py`.
//!
//! `arlecchino difftest <orig.jar> <rebuilt.jar>` runs BOTH jars headless in
//! FreeJ2ME-plus (stage A2, `diffrun`) with the same input script (stage A3
//! format) and compares their behaviour.
//!
//! The A2 investigation set the comparison model this module is built around:
//!
//!   * frame CONTENT is deterministic — every distinguishable screen state gives
//!     the same sha256 from run to run;
//!   * frame TIMING is not: state transitions drift by ±1 frame (live clock).
//!     The optional A7 mode `--fixed-clock` (`fixedclock`) virtualises the
//!     MIDlet's clock/RNG and removes almost all of this drift.
//!
//! So we never compare frame i with frame i: what is compared is the **structure
//! of screen states** — the set of unique hashes, the order of their first
//! appearance and the first-appearance indices (with a shift tolerance), plus RMS
//! side effects (stage A4) and crash signs from the emulator's stderr.
//!
//! Noise calibration (`--calibrate`, and by default inside difftest): THE SAME
//! jar is run twice and compared with itself; this baseline tells how much of the
//! divergence is pure emulator non-determinism, and the "original vs rebuild"
//! verdict fails only on signals STRONGER than this noise.
//!
//! The pure comparison logic (everything that does not need the emulator) lives in
//! the upper half of the module and is covered by tests without java; run
//! orchestration is below.

use crate::diffrun::{self, HeadlessResult, InputEvent, RunOpts};
use crate::midprun;
use crate::util;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Defaults. `shift_tol` is IN FRAMES: the investigation measured ±1, we use 3 for margin;
/// the effective tolerance is additionally widened by what the calibration measured.
pub const DEFAULT_FRAMES: i64 = 30;
pub const DEFAULT_SHIFT_TOL: i64 = 3;
pub const DEFAULT_JACCARD_MARGIN: f64 = 0.15;

/// Headless engines difftest can drive (B4': midp-rt — our LGPL fork of
/// MicroEmulator in a separate process — is the second independent reference next
/// to FreeJ2ME-plus).
pub const ENGINES: [&str; 2] = ["freej2me", "midp-rt"];
/// Environment problems (engine jar / java not installed) are passed up so that
/// the caller reports a SKIP rather than recording a behavioural failure.
const ENV_ERROR_MARKERS: [&str; 3] =
    ["freej2me-lr.jar not found", "java executable", "midp-rt.jar not found"];

/// Files inside the RMS directory that are NOT game record stores
/// (FreeJ2ME-plus puts its own per-game config next to them).
const RMS_ENGINE_FILES: [&str; 1] = ["game.conf"];

// ───────────────────────── pure comparison logic ────────────────────────────
/// Collapse per-frame hashes into (state, number_of_consecutive_frames).
pub fn collapse_states(hashes: &[String]) -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = Vec::new();
    for h in hashes {
        match out.last_mut() {
            Some(last) if last.0 == *h => last.1 += 1,
            _ => out.push((h.clone(), 1)),
        }
    }
    out
}

/// Distinct states in order of FIRST appearance (robust to ±1-frame drift,
/// unlike the raw RLE sequence, which may lose or gain a trailing blink
/// cycle).
pub fn unique_in_order(hashes: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    let mut out = Vec::new();
    for h in hashes {
        if seen.insert(h) {
            out.push(h.clone());
        }
    }
    out
}

/// state hash -> index of the frame where it first appeared
pub fn first_seen(hashes: &[String]) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for (i, h) in hashes.iter().enumerate() {
        out.entry(h.clone()).or_insert(i as i64);
    }
    out
}

/// Structural comparison of the screen-state sequences of two runs.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqComparison {
    /// |unique_a ∩ unique_b| / |unique_a ∪ unique_b|
    pub jaccard: f64,
    /// states seen only in run a
    pub only_a: Vec<String>,
    pub only_b: Vec<String>,
    /// shared states first appear in the same order
    pub order_ok: bool,
    /// max |first_seen_a - first_seen_b| over shared states
    pub max_shift: i64,
    /// match by index (informational only — timing-sensitive)
    pub frame_match_rate: f64,
}

impl SeqComparison {
    pub fn sets_equal(&self) -> bool {
        self.only_a.is_empty() && self.only_b.is_empty()
    }
}

pub fn compare_sequences(hashes_a: &[String], hashes_b: &[String]) -> SeqComparison {
    let ua = unique_in_order(hashes_a);
    let ub = unique_in_order(hashes_b);
    let sa: BTreeSet<String> = ua.iter().cloned().collect();
    let sb: BTreeSet<String> = ub.iter().cloned().collect();
    let shared: BTreeSet<String> = sa.intersection(&sb).cloned().collect();
    let union: BTreeSet<String> = sa.union(&sb).cloned().collect();
    let fa = first_seen(hashes_a);
    let fb = first_seen(hashes_b);
    let max_shift = shared
        .iter()
        .map(|h| (fa[h] - fb[h]).abs())
        .max()
        .unwrap_or(0);
    let order_a: Vec<&String> = ua.iter().filter(|h| shared.contains(*h)).collect();
    let order_b: Vec<&String> = ub.iter().filter(|h| shared.contains(*h)).collect();
    let n = hashes_a.len().min(hashes_b.len());
    let matches = hashes_a.iter().zip(hashes_b.iter()).filter(|(x, y)| x == y).count();
    SeqComparison {
        jaccard: if union.is_empty() {
            1.0
        } else {
            shared.len() as f64 / union.len() as f64
        },
        only_a: sa.difference(&sb).cloned().collect(),
        only_b: sb.difference(&sa).cloned().collect(),
        order_ok: order_a == order_b,
        max_shift,
        frame_match_rate: if n == 0 { 1.0 } else { matches as f64 / n as f64 },
    }
}

/// Everything the verdict needs to know about one headless run.
#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub label: String,
    pub out_dir: String,
    pub hashes: Vec<String>,
    pub nonblank: bool,
    /// relative path -> sha256 (after the run)
    pub rms: BTreeMap<String, String>,
    /// exception class names found in stderr
    pub exceptions: BTreeSet<String>,
    /// the run itself failed
    pub error: Option<String>,
}

pub fn rms_paths(s: &RunSummary) -> BTreeSet<String> {
    s.rms.keys().cloned().collect()
}

#[derive(Debug, Clone)]
pub struct Verdict {
    pub passed: bool,
    /// why it FAILED (empty on success)
    pub reasons: Vec<String>,
    /// informational observations
    pub notes: Vec<String>,
    /// original vs original (noise); None if calibration was skipped
    pub baseline: Option<SeqComparison>,
    /// original vs rebuild
    pub cross: SeqComparison,
}

fn empty_seq() -> SeqComparison {
    SeqComparison {
        jaccard: 0.0,
        only_a: vec![],
        only_b: vec![],
        order_ok: false,
        max_shift: 0,
        frame_match_rate: 0.0,
    }
}

fn pct0(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

/// A5 verdict: FAIL only on signals stronger than the calibrated noise.
///
/// `orig_b` is the second run of the original (noise baseline); None means judging
/// by fixed thresholds without calibration. A pure function over RunSummary.
pub fn decide(
    orig_a: &RunSummary,
    orig_b: Option<&RunSummary>,
    rebuilt: &RunSummary,
    shift_tol: i64,
    jaccard_margin: f64,
) -> Verdict {
    let mut reasons: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // 0) the runs must have happened at all
    if let Some(e) = &orig_a.error {
        return Verdict {
            passed: false,
            reasons: vec![format!("original run failed: {e}")],
            notes,
            baseline: None,
            cross: empty_seq(),
        };
    }
    if let Some(e) = &rebuilt.error {
        return Verdict {
            passed: false,
            reasons: vec![format!("rebuilt run failed: {e}")],
            notes,
            baseline: None,
            cross: empty_seq(),
        };
    }

    let baseline = match orig_b {
        Some(b) if b.error.is_none() => Some(compare_sequences(&orig_a.hashes, &b.hashes)),
        _ => None,
    };
    let cross = compare_sequences(&orig_a.hashes, &rebuilt.hashes);

    // 1) crash signs unique to the rebuilt jar
    let mut orig_exc = orig_a.exceptions.clone();
    if let Some(b) = orig_b {
        orig_exc.extend(b.exceptions.iter().cloned());
    }
    let new_exc: Vec<String> = rebuilt.exceptions.difference(&orig_exc).cloned().collect();
    if !new_exc.is_empty() {
        reasons.push(format!("new exceptions in rebuilt stderr: {}", new_exc.join(", ")));
    }
    if !orig_a.exceptions.is_empty() {
        notes.push(format!(
            "original itself logs exceptions (ignored as baseline): {}",
            orig_a.exceptions.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    // 2) blank screen
    if orig_a.nonblank && !rebuilt.nonblank {
        reasons.push("rebuilt rendered ONLY blank frames (original did not)".into());
    }

    // 3) screen-state structure, judged against the noise baseline
    let base_j = baseline.as_ref().map(|b| b.jaccard).unwrap_or(1.0);
    let min_jaccard = (base_j - jaccard_margin).max(0.0);
    if cross.jaccard < min_jaccard {
        reasons.push(match &baseline {
            Some(b) => format!(
                "state-set similarity {:.2} below noise floor {:.2} (baseline {:.2} - margin {:.2})",
                cross.jaccard, min_jaccard, b.jaccard, jaccard_margin
            ),
            None => format!(
                "state-set similarity {:.2} below threshold {:.2}",
                cross.jaccard, min_jaccard
            ),
        });
    }
    // order: require only what the baseline itself sustains
    if !cross.order_ok {
        if baseline.as_ref().map(|b| b.order_ok).unwrap_or(true) {
            reasons.push("shared screen states appear in a DIFFERENT order".into());
        } else {
            notes.push(
                "state order differs, but the noise baseline already shows order \
                 instability — not held against rebuilt"
                    .into(),
            );
        }
    }
    // time shift: tolerance widened by what the baseline measured
    let eff_tol = shift_tol.max(baseline.as_ref().map(|b| b.max_shift).unwrap_or(0) + 1);
    if cross.max_shift > eff_tol {
        reasons.push(format!(
            "state timing shifted by {} frames (tolerance ±{eff_tol})",
            cross.max_shift
        ));
    }

    // 4) RMS side effects
    if rms_paths(orig_a) != rms_paths(rebuilt) {
        let pa = rms_paths(orig_a);
        let pr = rms_paths(rebuilt);
        let only_o: Vec<String> = pa.difference(&pr).cloned().collect();
        let only_r: Vec<String> = pr.difference(&pa).cloned().collect();
        let paths_stable = match (&baseline, orig_b) {
            (None, _) => true,
            (Some(_), Some(b)) => pa == rms_paths(b),
            (Some(_), None) => true,
        };
        if paths_stable {
            reasons.push(format!(
                "RMS stores differ (only-orig: {}, only-rebuilt: {})",
                fmt_list(&only_o),
                fmt_list(&only_r)
            ));
        } else {
            notes.push(
                "RMS store sets differ, but they differ between the two original runs \
                 too (noise)"
                    .into(),
            );
        }
    } else {
        let content_noise = orig_b.map(|b| orig_a.rms != b.rms).unwrap_or(false);
        if orig_a.rms != rebuilt.rms {
            if content_noise {
                notes.push(
                    "RMS contents differ byte-wise (within noise — original runs differ too)"
                        .into(),
                );
            } else if orig_b.is_none() {
                notes.push("RMS contents differ byte-wise (no baseline to judge by)".into());
            } else {
                reasons.push(
                    "RMS contents differ byte-wise while two original runs were \
                     byte-identical"
                        .into(),
                );
            }
        }
    }

    notes.push(format!(
        "frame-by-frame index match (timing-sensitive, info only): orig-vs-rebuilt {}{}",
        pct0(cross.frame_match_rate),
        match &baseline {
            Some(b) => format!(", baseline {}", pct0(b.frame_match_rate)),
            None => String::new(),
        }
    ));
    Verdict {
        passed: reasons.is_empty(),
        reasons,
        notes,
        baseline,
        cross,
    }
}

/// Python's `[...] or '-'`: an empty list prints as `-`, otherwise as repr.
fn fmt_list(v: &[String]) -> String {
    if v.is_empty() {
        "-".to_string()
    } else {
        format!("['{}']", v.join("', '"))
    }
}

// ───────────────────── cross-engine comparison (the A↔B link) ───────────────
// PLAN_runtime.md "The A↔B link": THE SAME jar in FreeJ2ME-plus versus midp-rt
// with one input script. The engines draw with different skins/fonts, so pixel
// hashes will NEVER match between engines — the comparison is deliberately SOFTER
// than the A5 verdict: it compares the STRUCTURE of state changes, RMS behaviour
// (store NAMES) and crash signs. The pixel Jaccard is informational only.

/// Frame indices where the screen CHANGES state (the hash differs from the previous one) —
/// the engine-independent "shape" of a run.
pub fn transition_frames(hashes: &[String]) -> Vec<i64> {
    (1..hashes.len())
        .filter(|i| hashes[*i] != hashes[*i - 1])
        .map(|i| i as i64)
        .collect()
}

/// RMS store names normalised for display: base name, `.rs`/`.rms` stripped,
/// engine service files (`game.conf`) excluded. NOTE: names are NOT comparable
/// BETWEEN engines — FreeJ2ME-plus mangles them on disk — so the cross-engine
/// verdict only compares RMS ACTIVITY.
pub fn rms_store_names(s: &RunSummary) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for rel in s.rms.keys() {
        let name = Path::new(rel)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel.clone());
        if RMS_ENGINE_FILES.contains(&name.as_str()) {
            continue;
        }
        let mut name = name;
        for suf in [".rs", ".rms"] {
            if let Some(stripped) = name.strip_suffix(suf) {
                name = stripped.to_string();
                break;
            }
        }
        names.insert(name);
    }
    names
}

/// Soft structural comparison of one jar on two engines.
#[derive(Debug, Clone)]
pub struct CrossEngineComparison {
    pub agree: bool,
    /// signs of divergence (empty when they agree)
    pub reasons: Vec<String>,
    /// informational (including the pixel Jaccard)
    pub notes: Vec<String>,
    /// unique screen states per engine
    pub states: (i64, i64),
    /// frame indices of state changes
    pub transitions: (Vec<i64>, Vec<i64>),
    /// informational ONLY — ~0 is expected between engines
    pub pixel_jaccard: f64,
}

fn fmt_sorted(v: &BTreeSet<String>) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        format!("['{}']", v.iter().cloned().collect::<Vec<_>>().join("', '"))
    }
}

fn fmt_ints(v: &[i64]) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        format!("[{}]", v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", "))
    }
}

/// A pure function over two RunSummary values.
///
/// Divergence (reasons) on: a failed run, "blank vs drawing", unique-state /
/// transition counts differing by more than `state_tol`, exceptions in only one
/// engine, different sets of RMS store names.
pub fn compare_cross_engine(
    run_a: &RunSummary,
    run_b: &RunSummary,
    state_tol: i64,
) -> CrossEngineComparison {
    let mut reasons: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let ta = transition_frames(&run_a.hashes);
    let tb = transition_frames(&run_b.hashes);
    let na = run_a.hashes.iter().collect::<BTreeSet<_>>().len() as i64;
    let nb = run_b.hashes.iter().collect::<BTreeSet<_>>().len() as i64;

    for r in [run_a, run_b] {
        if let Some(e) = &r.error {
            reasons.push(format!("run {} failed: {e}", r.label));
        }
    }
    if reasons.is_empty() {
        if run_a.nonblank != run_b.nonblank {
            let blank = if !run_a.nonblank { run_a } else { run_b };
            reasons.push(format!(
                "{} rendered ONLY blank frames (the other engine did not)",
                blank.label
            ));
        }
        if (na - nb).abs() > state_tol {
            reasons.push(format!(
                "unique screen-state counts diverge: {}={na} vs {}={nb} (tolerance ±{state_tol})",
                run_a.label, run_b.label
            ));
        }
        if (ta.len() as i64 - tb.len() as i64).abs() > state_tol {
            reasons.push(format!(
                "state-change counts diverge: {}={} vs {}={} (tolerance ±{state_tol})",
                run_a.label,
                ta.len(),
                run_b.label,
                tb.len()
            ));
        }
        let only_a: BTreeSet<String> =
            run_a.exceptions.difference(&run_b.exceptions).cloned().collect();
        let only_b: BTreeSet<String> =
            run_b.exceptions.difference(&run_a.exceptions).cloned().collect();
        if !only_a.is_empty() || !only_b.is_empty() {
            reasons.push(format!(
                "exceptions logged by only one engine: {}: {}, {}: {}",
                run_a.label,
                fmt_sorted(&only_a),
                run_b.label,
                fmt_sorted(&only_b)
            ));
        }
        // RMS: only ACTIVITY is equally meaningful across engines
        let sa = rms_store_names(run_a);
        let sb = rms_store_names(run_b);
        if sa.is_empty() != sb.is_empty() {
            let (wrote, silent) = if !sa.is_empty() { (run_a, run_b) } else { (run_b, run_a) };
            reasons.push(format!(
                "RMS activity differs: {} wrote record stores, {} wrote none",
                wrote.label, silent.label
            ));
        } else if !sa.is_empty() {
            notes.push(format!(
                "both engines wrote RMS (store names per engine, NOT comparable across): \
                 {}: {}; {}: {}",
                run_a.label,
                fmt_sorted(&sa),
                run_b.label,
                fmt_sorted(&sb)
            ));
        }
    }

    let pixel = compare_sequences(&run_a.hashes, &run_b.hashes).jaccard;
    notes.push(format!(
        "pixel-level state Jaccard (info ONLY — different skins/fonts make ~0.00 \
         expected): {pixel:.2}"
    ));
    notes.push(format!(
        "state-change frames {}: {}; {}: {}",
        run_a.label,
        fmt_ints(&ta),
        run_b.label,
        fmt_ints(&tb)
    ));
    CrossEngineComparison {
        agree: reasons.is_empty(),
        reasons,
        notes,
        states: (na, nb),
        transitions: (ta, tb),
        pixel_jaccard: pixel,
    }
}

// ───────────────────────── run orchestration ────────────────────────────────
/// Java exception/error class names mentioned in the emulator's stderr.
///
/// We compare SETS, so noise that the original prints too (e.g. a MIDlet that
/// routinely logs an IOException) never fails the verdict.
pub fn scan_stderr(path: &Path) -> BTreeSet<String> {
    let text = match util::read_text_lossy(path) {
        Ok(t) => t,
        Err(_) => return BTreeSet::new(),
    };
    let re = Regex::new(
        r"\b((?:[A-Za-z_$][\w$]*\.)*[A-Za-z_$][\w$]*(?:Exception|Error))\b",
    )
    .unwrap();
    re.captures_iter(&text)
        .map(|c| c[1].rsplit('.').next().unwrap_or("").to_string())
        .collect()
}

/// One headless run -> RunSummary. A failed run records `error` instead of
/// panicking, so the verdict can report it (a missing emulator is still returned
/// as Err — that is an environment problem, the caller skips).
pub fn summarize_run(
    jar: &Path,
    out_dir: &Path,
    label: &str,
    inputs: Option<&[InputEvent]>,
    engine: &str,
    opts: &RunOpts,
) -> Result<RunSummary, String> {
    if !ENGINES.contains(&engine) {
        return Err(format!(
            "unknown difftest engine {engine:?} (choose from {ENGINES:?})"
        ));
    }
    let mut summary = RunSummary {
        label: label.to_string(),
        out_dir: out_dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let res: Result<HeadlessResult, String> = if engine == "midp-rt" {
        // midp-rt clocks itself — it is already deterministic and does not take such a flag
        let mut o = opts.clone();
        o.deterministic_capture = false;
        midprun::run_headless_midp_rt(jar, out_dir, inputs, &o, None, true)
    } else {
        diffrun::run_headless(jar, out_dir, inputs, opts, None)
    };
    match res {
        Err(e) => {
            if ENV_ERROR_MARKERS.iter().any(|m| e.contains(m)) {
                return Err(e); // environment problem, not a behavioural signal
            }
            summary.error = Some(e);
            return Ok(summary);
        }
        Ok(r) => {
            summary.hashes = r.hashes();
            summary.nonblank = r.any_nonblank();
            summary.rms = diffrun::snapshot_rms(Path::new(&r.rms_dir));
            summary.exceptions = scan_stderr(Path::new(&r.stderr_log));
        }
    }
    Ok(summary)
}

/// A7: when `enabled`, return an A7-instrumented copy of `jar` under
/// `out_root/_fixedclock`, otherwise `jar` unchanged. If instrumentation fails we
/// fall back to the original jar (a run is still possible, just non-deterministic).
fn fixed_clock_jar(jar: &Path, out_root: &Path, enabled: bool) -> PathBuf {
    if !enabled {
        return jar.to_path_buf();
    }
    crate::fixedclock::maybe_transform(jar, &out_root.join("_fixedclock"), None, true)
}

/// `--calibrate`: run ONE jar twice with one input script and measure the
/// baseline divergence = pure emulator non-determinism (the A2 method).
pub fn calibrate(
    jar: &Path,
    out_root: &Path,
    inputs: Option<&[InputEvent]>,
    engine: &str,
    fixed_clock: bool,
    opts: &RunOpts,
) -> Result<(SeqComparison, RunSummary, RunSummary), String> {
    std::fs::create_dir_all(out_root).map_err(|e| e.to_string())?;
    let jar = fixed_clock_jar(jar, out_root, fixed_clock);
    let a = summarize_run(&jar, &out_root.join("calib_a"), "calib_a", inputs, engine, opts)?;
    let b = summarize_run(&jar, &out_root.join("calib_b"), "calib_b", inputs, engine, opts)?;
    let cmp = compare_sequences(&a.hashes, &b.hashes);
    Ok((cmp, a, b))
}

/// The full A5 harness: (optional) calibration on the original, a run of the rebuilt
/// jar, a verdict. Writes `difftest_report.{md,json}` into out_root.
#[allow(clippy::too_many_arguments)]
pub fn difftest(
    orig_jar: &Path,
    rebuilt_jar: &Path,
    out_root: &Path,
    inputs: Option<&[InputEvent]>,
    shift_tol: i64,
    jaccard_margin: f64,
    with_baseline: bool,
    engine: &str,
    fixed_clock: bool,
    opts: &RunOpts,
) -> Result<Verdict, String> {
    std::fs::create_dir_all(out_root).map_err(|e| e.to_string())?;
    let orig_jar = fixed_clock_jar(orig_jar, &out_root.join("_fc_orig"), fixed_clock);
    let rebuilt_jar = fixed_clock_jar(rebuilt_jar, &out_root.join("_fc_rebuilt"), fixed_clock);
    let orig_a = summarize_run(&orig_jar, &out_root.join("orig_a"), "orig_a", inputs, engine, opts)?;
    let orig_b = if with_baseline {
        Some(summarize_run(&orig_jar, &out_root.join("orig_b"), "orig_b", inputs, engine, opts)?)
    } else {
        None
    };
    let rebuilt =
        summarize_run(&rebuilt_jar, &out_root.join("rebuilt"), "rebuilt", inputs, engine, opts)?;
    let verdict = decide(&orig_a, orig_b.as_ref(), &rebuilt, shift_tol, jaccard_margin);
    write_reports(
        out_root,
        &orig_jar,
        &rebuilt_jar,
        &orig_a,
        orig_b.as_ref(),
        &rebuilt,
        &verdict,
        engine,
    );
    Ok(verdict)
}

/// Run ONE jar on both engines with one input script and compare softly.
/// Writes `cross_engine_report.{md,json}` into out_root.
pub fn cross_engine(
    jar: &Path,
    out_root: &Path,
    inputs: Option<&[InputEvent]>,
    engines: (&str, &str),
    state_tol: i64,
    fixed_clock: bool,
    opts: &RunOpts,
) -> Result<(CrossEngineComparison, RunSummary, RunSummary), String> {
    std::fs::create_dir_all(out_root).map_err(|e| e.to_string())?;
    let jar = fixed_clock_jar(jar, out_root, fixed_clock);
    let a = summarize_run(&jar, &out_root.join(engines.0), engines.0, inputs, engines.0, opts)?;
    let b = summarize_run(&jar, &out_root.join(engines.1), engines.1, inputs, engines.1, opts)?;
    let cmp = compare_cross_engine(&a, &b, state_tol);
    write_cross_reports(out_root, &jar, &a, &b, &cmp);
    Ok((cmp, a, b))
}

// ───────────────────────────── reports ───────────────────────────────────────
fn seq_json(c: Option<&SeqComparison>) -> serde_json::Value {
    match c {
        None => serde_json::Value::Null,
        Some(c) => serde_json::json!({
            "jaccard": round4(c.jaccard),
            "sets_equal": c.sets_equal(),
            "only_a": c.only_a,
            "only_b": c.only_b,
            "order_ok": c.order_ok,
            "max_shift_frames": c.max_shift,
            "frame_match_rate": round4(c.frame_match_rate),
        }),
    }
}

/// Round to 4 digits, like `round(x, 4)` in Python.
fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

fn run_json(r: &RunSummary) -> serde_json::Value {
    serde_json::json!({
        "label": r.label,
        "out_dir": r.out_dir,
        "frames": r.hashes.len(),
        "unique_states": r.hashes.iter().collect::<BTreeSet<_>>().len(),
        "nonblank": r.nonblank,
        "rms_files": r.rms.keys().cloned().collect::<Vec<_>>(),
        "exceptions": r.exceptions.iter().cloned().collect::<Vec<_>>(),
        "error": r.error,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn render_report(
    orig_jar: &Path,
    rebuilt_jar: &Path,
    orig_a: &RunSummary,
    orig_b: Option<&RunSummary>,
    rebuilt: &RunSummary,
    verdict: &Verdict,
    engine: &str,
) -> String {
    let mut lines: Vec<String> = vec![
        "# difftest report".into(),
        String::new(),
        format!("- original: `{}`", orig_jar.display()),
        format!("- rebuilt:  `{}`", rebuilt_jar.display()),
        format!("- engine:   `{engine}`"),
        String::new(),
        format!("## VERDICT: {}", if verdict.passed { "PASS" } else { "FAIL" }),
        String::new(),
    ];
    for r in &verdict.reasons {
        lines.push(format!("- ✗ {r}"));
    }
    for n in &verdict.notes {
        lines.push(format!("- ℹ {n}"));
    }
    lines.extend([
        String::new(),
        "## Runs".into(),
        String::new(),
        "| run | frames | unique states | non-blank | RMS files | exceptions |".into(),
        "|---|---|---|---|---|---|".into(),
    ]);
    let mut runs: Vec<&RunSummary> = vec![orig_a];
    if let Some(b) = orig_b {
        runs.push(b);
    }
    runs.push(rebuilt);
    for r in runs {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} |",
            r.label,
            r.hashes.len(),
            r.hashes.iter().collect::<BTreeSet<_>>().len(),
            if r.nonblank { "yes" } else { "NO" },
            r.rms.len(),
            if r.exceptions.is_empty() {
                "-".to_string()
            } else {
                r.exceptions.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        ));
    }
    lines.extend(seq_block("Noise baseline (orig vs orig)", verdict.baseline.as_ref()));
    lines.extend(seq_block("Orig vs rebuilt", Some(&verdict.cross)));
    lines.join("\n") + "\n"
}

fn seq_block(title: &str, c: Option<&SeqComparison>) -> Vec<String> {
    match c {
        None => vec![String::new(), format!("## {title}"), String::new(), "(skipped)".into()],
        Some(c) => vec![
            String::new(),
            format!("## {title}"),
            String::new(),
            format!(
                "- state-set similarity (Jaccard): {:.2}{}",
                c.jaccard,
                if c.sets_equal() {
                    " (sets EQUAL)".to_string()
                } else {
                    format!(" (only-a: {}, only-b: {})", c.only_a.len(), c.only_b.len())
                }
            ),
            format!(
                "- first-appearance order: {}",
                if c.order_ok { "same" } else { "DIFFERS" }
            ),
            format!("- max timing shift: {} frame(s)", c.max_shift),
            format!("- frame-by-frame match (info): {}", pct0(c.frame_match_rate)),
        ],
    }
}

#[allow(clippy::too_many_arguments)]
fn write_reports(
    out_root: &Path,
    orig_jar: &Path,
    rebuilt_jar: &Path,
    orig_a: &RunSummary,
    orig_b: Option<&RunSummary>,
    rebuilt: &RunSummary,
    verdict: &Verdict,
    engine: &str,
) {
    let _ = std::fs::write(
        out_root.join("difftest_report.md"),
        render_report(orig_jar, rebuilt_jar, orig_a, orig_b, rebuilt, verdict, engine),
    );
    let mut runs = vec![run_json(orig_a)];
    if let Some(b) = orig_b {
        runs.push(run_json(b));
    }
    runs.push(run_json(rebuilt));
    let doc = serde_json::json!({
        "orig_jar": orig_jar.to_string_lossy(),
        "rebuilt_jar": rebuilt_jar.to_string_lossy(),
        "engine": engine,
        "passed": verdict.passed,
        "reasons": verdict.reasons,
        "notes": verdict.notes,
        "baseline": seq_json(verdict.baseline.as_ref()),
        "cross": seq_json(Some(&verdict.cross)),
        "runs": runs,
    });
    let _ = std::fs::write(
        out_root.join("difftest_report.json"),
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    );
}

pub fn render_cross_report(
    jar: &Path,
    run_a: &RunSummary,
    run_b: &RunSummary,
    cmp: &CrossEngineComparison,
) -> String {
    let mut lines: Vec<String> = vec![
        "# cross-engine report (A↔B link)".into(),
        String::new(),
        format!("- jar: `{}`", jar.display()),
        format!("- engines: `{}` vs `{}`", run_a.label, run_b.label),
        String::new(),
        format!(
            "## VERDICT: {} (soft, structural — pixels are NOT compared across engines)",
            if cmp.agree { "AGREE" } else { "DIVERGE" }
        ),
        String::new(),
    ];
    for r in &cmp.reasons {
        lines.push(format!("- ✗ {r}"));
    }
    for n in &cmp.notes {
        lines.push(format!("- ℹ {n}"));
    }
    lines.extend([
        String::new(),
        "## Runs".into(),
        String::new(),
        "| engine | frames | unique states | state changes | non-blank | RMS stores | exceptions |"
            .into(),
        "|---|---|---|---|---|---|---|".into(),
    ]);
    for (r, t) in [(run_a, &cmp.transitions.0), (run_b, &cmp.transitions.1)] {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.label,
            r.hashes.len(),
            r.hashes.iter().collect::<BTreeSet<_>>().len(),
            t.len(),
            if r.nonblank { "yes" } else { "NO" },
            {
                let n = rms_store_names(r);
                if n.is_empty() {
                    "-".to_string()
                } else {
                    n.iter().cloned().collect::<Vec<_>>().join(", ")
                }
            },
            if r.exceptions.is_empty() {
                "-".to_string()
            } else {
                r.exceptions.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        ));
    }
    lines.join("\n") + "\n"
}

fn write_cross_reports(
    out_root: &Path,
    jar: &Path,
    run_a: &RunSummary,
    run_b: &RunSummary,
    cmp: &CrossEngineComparison,
) {
    let _ = std::fs::write(
        out_root.join("cross_engine_report.md"),
        render_cross_report(jar, run_a, run_b, cmp),
    );
    let doc = serde_json::json!({
        "jar": jar.to_string_lossy(),
        "engines": [run_a.label.clone(), run_b.label.clone()],
        "agree": cmp.agree,
        "reasons": cmp.reasons,
        "notes": cmp.notes,
        "unique_states": [cmp.states.0, cmp.states.1],
        "transition_frames": [cmp.transitions.0.clone(), cmp.transitions.1.clone()],
        "pixel_jaccard_info": round4(cmp.pixel_jaccard),
        "runs": [run_json(run_a), run_json(run_b)],
    });
    let _ = std::fs::write(
        out_root.join("cross_engine_report.json"),
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    );
}

// ───────────────────── packaging the rebuilt jar ────────────────────────────
/// Build a RUNNABLE rebuilt jar: manifest and resources from `src_jar`,
/// `.class` files from `classes_dir` (the round-trip `merged/_classes`, which is complete:
/// classes that are still broken are seeded there with the original bytecode).
pub fn pack_rebuilt_jar(src_jar: &Path, classes_dir: &Path, out_jar: &Path) -> Result<PathBuf, String> {
    if let Some(p) = out_jar.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::open(src_jar).map_err(|e| e.to_string())?;
    let mut zin = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let outf = std::fs::File::create(out_jar).map_err(|e| e.to_string())?;
    let mut zout = zip::ZipWriter::new(outf);
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for i in 0..zin.len() {
        let mut e = zin.by_index(i).map_err(|e| e.to_string())?;
        let name = e.name().to_string();
        if name.ends_with('/') || name.ends_with(".class") {
            continue; // resources and manifest only; classes come rebuilt
        }
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut e, &mut buf).map_err(|e| e.to_string())?;
        zout.start_file(name, opts).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut zout, &buf).map_err(|e| e.to_string())?;
    }
    let mut classes: Vec<PathBuf> = util::walk_files(classes_dir)
        .into_iter()
        .filter(|p| p.extension().map(|e| e == "class").unwrap_or(false))
        .collect();
    classes.sort();
    for cls in classes {
        let rel = cls.strip_prefix(classes_dir).map_err(|e| e.to_string())?;
        let data = std::fs::read(&cls).map_err(|e| e.to_string())?;
        zout.start_file(rel.to_string_lossy().into_owned(), opts)
            .map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut zout, &data).map_err(|e| e.to_string())?;
    }
    zout.finish().map_err(|e| e.to_string())?;
    Ok(out_jar.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn collapse_and_unique() {
        assert_eq!(
            collapse_states(&h(&["a", "a", "b", "b", "b", "a"])),
            vec![("a".into(), 2), ("b".into(), 3), ("a".into(), 1)]
        );
        assert!(collapse_states(&[]).is_empty());
        let hs = h(&["splash", "splash", "menu", "blinkA", "blinkB", "blinkA"]);
        assert_eq!(unique_in_order(&hs), h(&["splash", "menu", "blinkA", "blinkB"]));
        assert_eq!(first_seen(&hs).get("blinkB"), Some(&4));
    }

    #[test]
    fn timing_shift_tolerated() {
        let a = h(&["s", "s", "s", "m", "m", "a", "b", "a"]);
        let b = h(&["s", "s", "s", "s", "m", "a", "b", "a"]);
        let c = compare_sequences(&a, &b);
        assert!(c.sets_equal() && c.order_ok);
        assert_eq!(c.max_shift, 1);
        assert!(c.frame_match_rate < 1.0);
    }
}
