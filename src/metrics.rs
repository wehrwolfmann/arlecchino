//! Quality metrics for a meta-decompilation run.
//!
//! Adapted from the three axes of "Java Decompiler Diversity and its Application to
//! Meta-decompilation" (Harrand, Soto-Valero, Monperrus, Baudry — arXiv:2005.11315).
//! Because Arlecchino reverse-engineers jars for which NO original source exists, the
//! paper's source-referenced metrics are re-cast into forms that need only the jar's
//! bytecode and the engines' own outputs:
//!
//!   1. **Syntactic correctness** — per-class javac-error count and the recompilable
//!      ratio of the assembled best-of tree (the paper's "syntactic distortion", but
//!      measured as compilability since there is no reference source to diff against).
//!
//!   2. **Cross-decompiler agreement** (distortion proxy) — with no original to
//!      measure distortion against, we measure how much the independent engines AGREE
//!      per class (mean pairwise weighted-Jaccard / Ruzicka similarity over comment-
//!      stripped token multisets).  High agreement across independent decompilers ⇒
//!      high confidence; the chosen fragment's "distortion" is its distance (1 −
//!      similarity) from that engine consensus.
//!
//!   3. **Behavioral / semantic equivalence** — (a) structural round-trip: does the
//!      assembled source recompile to bytecode with the same member/method structure
//!      as the original jar; (b) cross-engine bytecode consensus: for each class, of
//!      the engines whose variant compiles cleanly, how many agree on the compiled
//!      class's public structure (`javap -p`); and (c) a runtime probe recording
//!      whether a FreeJ2ME execution harness is available.  Full runtime input-diff
//!      execution is not wired into this crate and is reported as deferred.

use crate::core::{self, EngineData};
use crate::roundtrip::RoundTrip;
use crate::util;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

// ── metric 1: syntactic correctness ──────────────────────────────────────────
#[derive(Serialize)]
pub struct Syntactic {
    pub description: String,
    pub total_classes: usize,
    pub recompilable_classes: usize,
    pub recompilable_ratio: f64,
    pub total_isolated_errors: i64,
    pub whole_tree_errors: i64,
    pub per_class_errors: BTreeMap<String, i64>,
}

// ── metric 2: cross-decompiler agreement ─────────────────────────────────────
#[derive(Serialize)]
pub struct ClassAgreement {
    pub engines: usize,
    pub agreement: f64,
    pub chosen_distortion: f64,
}

#[derive(Serialize)]
pub struct Agreement {
    pub description: String,
    pub classes_scored: usize,
    pub mean_agreement: f64,
    pub chosen_mean_distortion: f64,
    pub per_class: BTreeMap<String, ClassAgreement>,
}

// ── metric 3: behavioral / semantic equivalence ──────────────────────────────
#[derive(Serialize)]
pub struct StructuralRt {
    pub total_original: usize,
    pub present: usize,
    pub structurally_matching: usize,
    pub match_ratio: f64,
}

#[derive(Serialize)]
pub struct ClassBytecode {
    pub clean_engines: usize,
    pub agreement: f64,
}

#[derive(Serialize)]
pub struct BytecodeConsensus {
    pub description: String,
    pub classes_scored: usize,
    pub mean_agreement: f64,
    pub per_class: BTreeMap<String, ClassBytecode>,
}

#[derive(Serialize)]
pub struct RuntimeProbe {
    pub executed: bool,
    pub emulator_available: bool,
    pub emulator: Option<String>,
    pub note: String,
}

#[derive(Serialize)]
pub struct Behavioral {
    pub description: String,
    pub structural_roundtrip: StructuralRt,
    pub cross_engine_bytecode_consensus: BytecodeConsensus,
    pub runtime: RuntimeProbe,
}

#[derive(Serialize)]
pub struct Metrics {
    pub syntactic_correctness: Syntactic,
    pub cross_decompiler_agreement: Agreement,
    pub behavioral_equivalence: Behavioral,
}

impl Metrics {
    /// One-line-per-metric summary for the console and the Markdown report.
    pub fn summary_lines(&self) -> Vec<String> {
        let s = &self.syntactic_correctness;
        let a = &self.cross_decompiler_agreement;
        let b = &self.behavioral_equivalence;
        vec![
            format!(
                "1. syntactic correctness : {}/{} classes recompilable ({:.1}%), \
                 isolated errors={}, whole-tree errors={}",
                s.recompilable_classes,
                s.total_classes,
                s.recompilable_ratio * 100.0,
                s.total_isolated_errors,
                s.whole_tree_errors,
            ),
            format!(
                "2. cross-decompiler agreement : mean={:.3} over {} class(es); \
                 chosen-fragment distortion={:.3}",
                a.mean_agreement, a.classes_scored, a.chosen_mean_distortion,
            ),
            format!(
                "3. behavioral equivalence : structural round-trip {}/{} match ({:.1}%); \
                 cross-engine bytecode consensus={:.3} over {} class(es); \
                 runtime executed={}",
                b.structural_roundtrip.structurally_matching,
                b.structural_roundtrip.total_original,
                b.structural_roundtrip.match_ratio * 100.0,
                b.cross_engine_bytecode_consensus.mean_agreement,
                b.cross_engine_bytecode_consensus.classes_scored,
                b.runtime.executed,
            ),
        ]
    }
}

// ── tokenisation & similarity ────────────────────────────────────────────────
/// Strip `//` line comments and `/* … */` block comments (not string-aware, but
/// engines emit well-formed source, so this is a fair, reproducible normaliser).
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Tokenise Java-ish source into a token multiset: maximal `[A-Za-z0-9_$]` runs plus
/// individual non-space punctuation characters.  Comments are stripped first.
fn token_multiset(src: &str) -> BTreeMap<String, u32> {
    let cleaned = strip_comments(src);
    let mut ms: BTreeMap<String, u32> = BTreeMap::new();
    let mut word = String::new();
    let flush = |word: &mut String, ms: &mut BTreeMap<String, u32>| {
        if !word.is_empty() {
            *ms.entry(std::mem::take(word)).or_insert(0) += 1;
        }
    };
    for ch in cleaned.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
            word.push(ch);
        } else {
            flush(&mut word, &mut ms);
            if !ch.is_whitespace() {
                *ms.entry(ch.to_string()).or_insert(0) += 1;
            }
        }
    }
    flush(&mut word, &mut ms);
    ms
}

/// Weighted Jaccard (Ruzicka) similarity of two token multisets: Σ min / Σ max.
/// 1.0 = identical token distribution, 0.0 = disjoint.
fn ruzicka(a: &BTreeMap<String, u32>, b: &BTreeMap<String, u32>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut inter: u64 = 0;
    let mut union: u64 = 0;
    let mut keys: std::collections::BTreeSet<&String> = a.keys().collect();
    keys.extend(b.keys());
    for k in keys {
        let x = *a.get(k).unwrap_or(&0) as u64;
        let y = *b.get(k).unwrap_or(&0) as u64;
        inter += x.min(y);
        union += x.max(y);
    }
    if union == 0 {
        1.0
    } else {
        inter as f64 / union as f64
    }
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

// ── javap structural signature (for metric 3b) ───────────────────────────────
/// Compile a single engine's variant of one class against `api_cp` (the original
/// jar's bytecode), then read the top-level class's `javap -p` declaration set.
/// Returns None if it did not compile cleanly or produced no matching `.class`.
fn compiled_signature(
    java_file: &Path,
    rel: &str,
    api_cp: &str,
    tmp_root: &Path,
) -> Option<Vec<String>> {
    let classdir = tmp_root.join("cls");
    let _ = std::fs::remove_dir_all(&classdir);
    let _ = std::fs::create_dir_all(&classdir);
    let javac = util::tool("ARLECCHINO_JAVAC", "javac");
    let argv = vec![
        javac,
        "-d".into(),
        classdir.to_string_lossy().into_owned(),
        "--release".into(),
        "8".into(),
        "-encoding".into(),
        "utf-8".into(),
        "-nowarn".into(),
        "-proc:none".into(),
        "-cp".into(),
        api_cp.to_string(),
        java_file.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture(&argv, None);
    if r.status != Some(0) {
        return None;
    }
    let stem = rel.strip_suffix(".java").unwrap_or(rel);
    if !classdir.join(format!("{stem}.class")).exists() {
        return None;
    }
    let dotted = stem.replace('/', ".");
    let jargv = vec![
        util::tool("ARLECCHINO_JAVAP", "javap"),
        "-p".into(),
        "-cp".into(),
        classdir.to_string_lossy().into_owned(),
        dotted,
    ];
    let jr = util::run_capture(&jargv, None);
    if jr.spawn_err.is_some() || jr.status != Some(0) {
        return None;
    }
    let mut sig: Vec<String> = jr
        .stdout
        .lines()
        .map(|l| l.trim())
        .filter(|s| {
            !s.is_empty() && !s.starts_with("Compiled") && !s.ends_with('{') && *s != "}"
        })
        .map(|s| s.to_string())
        .collect();
    sig.sort();
    Some(sig)
}

// ── driver ───────────────────────────────────────────────────────────────────
/// Compute all three metrics for a completed run.
///
/// * `data`        — per-engine decompile results.
/// * `merged_dir`  — the assembled best-of tree.
/// * `per_iso`     — per-class isolated error count of the chosen best-of fragment.
/// * `rt`          — round-trip result (if the round-trip stage ran).
/// * `orig_jar`    — the (possibly renamed) jar fed to the engines.
/// * `classpath`   — extra classpath (MIDP stub), if any.
/// * `engines_root`— engine install root (to probe for the FreeJ2ME emulator).
pub fn compute(
    data: &BTreeMap<String, EngineData>,
    merged_dir: &Path,
    per_iso: &BTreeMap<String, i64>,
    rt: Option<&RoundTrip>,
    orig_jar: &Path,
    classpath: Option<&str>,
    engines_root: &Path,
) -> Metrics {
    // ── metric 1 ─────────────────────────────────────────────────────────────
    let total_classes = per_iso.len();
    let recompilable_classes = per_iso.values().filter(|v| **v == 0).count();
    let total_isolated_errors: i64 = per_iso.values().sum();
    let (whole_tree_errors, _, _) = core::compile_dir(merged_dir, classpath);
    let syntactic = Syntactic {
        description:
            "per-class javac errors of the assembled best-of tree; recompilable = classes \
             with zero errors (no original source exists to diff against)."
                .into(),
        total_classes,
        recompilable_classes,
        recompilable_ratio: ratio(recompilable_classes, total_classes),
        total_isolated_errors,
        whole_tree_errors,
        per_class_errors: per_iso.clone(),
    };

    // names-typed engines only (mergeable, same class-name space)
    let names: Vec<&String> = data
        .iter()
        .filter(|(_, d)| d.type_ == "names")
        .map(|(n, _)| n)
        .collect();

    // ── metric 2: cross-decompiler agreement ─────────────────────────────────
    let mut per_class_agree: BTreeMap<String, ClassAgreement> = BTreeMap::new();
    let mut agree_vals: Vec<f64> = Vec::new();
    let mut distortion_vals: Vec<f64> = Vec::new();
    for rel in per_iso.keys() {
        // engine variants of this class
        let variants: Vec<BTreeMap<String, u32>> = names
            .iter()
            .filter_map(|n| data[*n].files.get(rel))
            .filter_map(|p| util::read_text_lossy(p).ok())
            .map(|t| token_multiset(&t))
            .collect();
        let n_eng = variants.len();
        // pairwise agreement among engines
        let mut pair: Vec<f64> = Vec::new();
        for i in 0..variants.len() {
            for j in (i + 1)..variants.len() {
                pair.push(ruzicka(&variants[i], &variants[j]));
            }
        }
        let agreement = if pair.is_empty() { 1.0 } else { mean(&pair) };
        // distortion of the chosen fragment vs the engine consensus
        let chosen = util::read_text_lossy(&merged_dir.join(rel))
            .map(|t| token_multiset(&t))
            .unwrap_or_default();
        let sims: Vec<f64> = variants.iter().map(|v| ruzicka(&chosen, v)).collect();
        let chosen_distortion = if sims.is_empty() { 0.0 } else { 1.0 - mean(&sims) };
        if n_eng >= 2 {
            agree_vals.push(agreement);
        }
        if n_eng >= 1 {
            distortion_vals.push(chosen_distortion);
        }
        per_class_agree.insert(
            rel.clone(),
            ClassAgreement { engines: n_eng, agreement, chosen_distortion },
        );
    }
    let agreement_metric = Agreement {
        description:
            "per class, mean pairwise weighted-Jaccard (Ruzicka) similarity of the engines' \
             comment-stripped token multisets; chosen_distortion = 1 − similarity of the \
             assembled fragment to that consensus."
                .into(),
        classes_scored: agree_vals.len(),
        mean_agreement: mean(&agree_vals),
        chosen_mean_distortion: mean(&distortion_vals),
        per_class: per_class_agree,
    };

    // ── metric 3a: structural round-trip ─────────────────────────────────────
    let structural = match rt {
        Some(r) => {
            let matching = r.present.saturating_sub(r.structural_mismatch.len());
            StructuralRt {
                total_original: r.total_original,
                present: r.present,
                structurally_matching: matching,
                match_ratio: ratio(matching, r.total_original),
            }
        }
        None => StructuralRt {
            total_original: 0,
            present: 0,
            structurally_matching: 0,
            match_ratio: 0.0,
        },
    };

    // ── metric 3b: cross-engine compiled-bytecode consensus ──────────────────
    let api_cp = core::build_api_classpath(orig_jar, classpath, merged_dir);
    let tmp = util::make_temp_dir("arl_metric_").unwrap_or_else(|_| std::env::temp_dir());
    let mut per_class_bc: BTreeMap<String, ClassBytecode> = BTreeMap::new();
    let mut bc_vals: Vec<f64> = Vec::new();
    for rel in per_iso.keys() {
        let mut sigs: Vec<Vec<String>> = Vec::new();
        for n in &names {
            if let Some(p) = data[*n].files.get(rel) {
                if let Some(sig) = compiled_signature(p, rel, &api_cp, &tmp) {
                    sigs.push(sig);
                }
            }
        }
        let clean = sigs.len();
        // agreement = size of the largest identical-signature group / clean count
        let mut agreement = 1.0;
        if clean >= 1 {
            let mut counts: BTreeMap<&Vec<String>, usize> = BTreeMap::new();
            for s in &sigs {
                *counts.entry(s).or_insert(0) += 1;
            }
            let largest = counts.values().copied().max().unwrap_or(0);
            agreement = largest as f64 / clean as f64;
        }
        if clean >= 2 {
            bc_vals.push(agreement);
        }
        per_class_bc.insert(rel.clone(), ClassBytecode { clean_engines: clean, agreement });
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let bytecode = BytecodeConsensus {
        description:
            "per class, of the engines whose variant recompiles cleanly, the fraction sharing \
             the majority `javap -p` public structure — independent engines converging on the \
             same compiled class is behavioral-equivalence evidence."
                .into(),
        classes_scored: bc_vals.len(),
        mean_agreement: mean(&bc_vals),
        per_class: per_class_bc,
    };

    // ── metric 3c: runtime probe (execution deferred) ────────────────────────
    let emu = crate::emulate::find_freej2me(None, Some(engines_root));
    let runtime = RuntimeProbe {
        executed: false,
        emulator_available: emu.is_some(),
        emulator: emu.map(|p| p.to_string_lossy().into_owned()),
        note:
            "FreeJ2ME headless MIDlet execution with an input-diff harness is not wired into \
             this crate; behavioral equivalence is scored statically (structural round-trip + \
             cross-engine bytecode consensus). Runtime input-diff execution is deferred."
                .into(),
    };

    let behavioral = Behavioral {
        description:
            "static behavioral-equivalence evidence: structural round-trip vs the original \
             bytecode, cross-engine compiled-bytecode consensus, and a runtime-harness probe."
                .into(),
        structural_roundtrip: structural,
        cross_engine_bytecode_consensus: bytecode,
        runtime,
    };

    Metrics {
        syntactic_correctness: syntactic,
        cross_decompiler_agreement: agreement_metric,
        behavioral_equivalence: behavioral,
    }
}

fn ratio(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_comments_removes_line_and_block() {
        let s = "int a; // trailing\n/* block\n comment */ int b;\n";
        assert_eq!(strip_comments(s), "int a; \n int b;\n");
    }

    #[test]
    fn ruzicka_identical_is_one_disjoint_is_zero() {
        let a = token_multiset("class A { int x; }");
        let b = token_multiset("class A { int x; }");
        assert!((ruzicka(&a, &b) - 1.0).abs() < 1e-9);
        let c = token_multiset("zzz");
        let d = token_multiset("qqq");
        assert_eq!(ruzicka(&c, &d), 0.0);
    }

    #[test]
    fn ruzicka_comments_do_not_affect_similarity() {
        // identical code, different comments -> perfect agreement
        let a = token_multiset("int x = 1; // engine A note");
        let b = token_multiset("int x = 1; /* engine B note */");
        assert!((ruzicka(&a, &b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ruzicka_partial_overlap_is_fractional() {
        let a = token_multiset("a a b");
        let b = token_multiset("a b b");
        // multiset {a:2,b:1} vs {a:1,b:2}: min-sum=2, max-sum=4 -> 0.5
        let s = ruzicka(&a, &b);
        assert!((s - 0.5).abs() < 1e-9, "got {s}");
    }
}
