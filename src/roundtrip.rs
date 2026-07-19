//! Round-trip / bytecode-structure check.
//!
//! After merging we have a source tree; round-trip means: compile it back to .class
//! and compare against the ORIGINAL jar's bytecode, catching silent mismatches.
//!
//!   * Coverage: does the merge produce a .class for EVERY class of the original jar?
//!   * Coarse bytecode diff: for each class present in both, compare the number of
//!     declared members / methods (read from `javap -p`).
//!
//! This does NOT prove semantic equivalence — it is a cheap structural smoke test.

use crate::util;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Clone)]
pub struct Mismatch {
    pub class: String,
    pub orig: (i64, i64),
    pub merged: (i64, i64),
}

#[derive(Serialize, Clone)]
pub struct RoundTrip {
    pub total_original: usize,
    pub present: usize,
    pub missing: Vec<String>,
    pub coverage_pct: f64,
    pub structural_mismatch: Vec<Mismatch>,
    pub excluded_broken: Vec<String>,
}

fn jar_classes(jar: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    if let Ok(names) = util::zip_namelist(jar) {
        for n in names {
            if let Some(stem) = n.strip_suffix(".class") {
                out.insert(stem.to_string());
            }
        }
    }
    out
}

/// Return (#members, #methods-ish) for a class via `javap -p`.
fn javap_counts(classpath: &Path, binary_name: &str) -> Option<(i64, i64)> {
    let cls = binary_name.replace('/', ".");
    let argv = vec![
        "javap".to_string(),
        "-p".to_string(),
        "-cp".to_string(),
        classpath.to_string_lossy().into_owned(),
        cls,
    ];
    let r = util::run_capture(&argv, None);
    if r.spawn_err.is_some() || r.status != Some(0) {
        return None;
    }
    let mut methods = 0i64;
    let mut members = 0i64;
    for line in r.stdout.lines() {
        let s = line.trim();
        if s.is_empty() || s.starts_with("Compiled") || s.ends_with('{') || s == "}" {
            continue;
        }
        members += 1;
        if s.contains('(') && s.contains(')') {
            methods += 1;
        }
    }
    Some((members, methods))
}

/// Compare original jar classes vs compiled merged classes.
/// `merged_classes_dir` is the `_classes` output of the final compile.
pub fn check(jar: &Path, merged_classes_dir: &Path) -> RoundTrip {
    let orig = jar_classes(jar);
    let mut present = 0usize;
    let mut missing: Vec<String> = Vec::new();
    let mut method_mismatch: Vec<Mismatch> = Vec::new();

    // extract original .class to a temp dir for javap
    let tmp = util::make_temp_dir("arl_rt_").unwrap_or_else(|_| std::env::temp_dir());
    let _ = util::zip_extract_all(jar, &tmp);

    for bn in &orig {
        let compiled = merged_classes_dir.join(format!("{bn}.class"));
        if !compiled.exists() {
            missing.push(bn.clone());
            continue;
        }
        present += 1;
        let oc = javap_counts(&tmp, bn);
        let mc = javap_counts(merged_classes_dir, bn);
        if let (Some(o), Some(m)) = (oc, mc) {
            if o != m {
                method_mismatch.push(Mismatch {
                    class: bn.clone(),
                    orig: o,
                    merged: m,
                });
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);

    let total = orig.len();
    let pct = (100.0 * present as f64 / (total.max(1)) as f64 * 10.0).round() / 10.0;
    RoundTrip {
        total_original: total,
        present,
        missing,
        coverage_pct: pct,
        structural_mismatch: method_mismatch,
        excluded_broken: Vec::new(),
    }
}
