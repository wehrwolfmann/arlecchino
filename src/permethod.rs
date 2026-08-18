//! Per-method merge — EXPERIMENTAL.
//!
//! When no single engine compiles a class cleanly, try to assemble the class body
//! from the methods of different engines: keep each method's text from whichever
//! engine yields the fewest errors for the whole reconstructed file.  A splice is
//! kept ONLY if it strictly reduces the file's isolated javac error count.
//!
//! Parity note vs the Python original: Python located method/constructor boundaries
//! with the optional `javalang` library (an optional, non-vendored pip dependency;
//! absent on a default install, in which case Python skips this pass entirely).  This
//! port ships a small BUILT-IN Java signature scanner (`method_spans`) instead, so the
//! feature works out of the box with no optional dependency.  Method keys are computed
//! identically on both the base and donor sides, so cross-engine matching stays
//! consistent; the scanner recognises single-line method/constructor headers with a
//! body (the common decompiler-output shape).

use crate::core::{compile_file_isolated, EngineData};
use crate::util::{self, split_keepends};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

static HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[ \t]*(?:(?:public|private|protected|static|final|abstract|synchronized|native|strictfp|default)\s+)*(?:[\w$.<>\[\], ]+?\s+)?(?P<name>[A-Za-z_$][\w$]*)\s*\((?P<params>[^)]*)\)[^{;]*\{[ \t]*$",
    )
    .unwrap()
});

const STMT_KEYWORDS: &[&str] = &[
    "if", "for", "while", "switch", "catch", "do", "return", "throw", "else", "new", "assert",
];

fn first_token(s: &str) -> &str {
    let t = s.trim_start();
    let end = t.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).unwrap_or(t.len());
    &t[..end]
}

/// Simple erased type name for a single parameter declaration (`int a` -> `int`,
/// `a.b.C[] x` -> `C`).  Used only for internally-consistent method keys.
fn param_type(p: &str) -> String {
    let p = p.trim();
    if p.is_empty() {
        return String::new();
    }
    let mut tok = p.split_whitespace().next().unwrap_or("").to_string();
    // strip generics
    if let Some(i) = tok.find('<') {
        tok.truncate(i);
    }
    // strip array brackets
    tok = tok.replace("[]", "").replace(['[', ']'], "");
    // last dotted segment
    if let Some(pos) = tok.rfind('.') {
        tok = tok[pos + 1..].to_string();
    }
    tok
}

fn make_key(name: &str, params: &str) -> String {
    let ps: Vec<String> = if params.trim().is_empty() {
        Vec::new()
    } else {
        params.split(',').map(param_type).collect()
    };
    format!("{name}({})", ps.join(","))
}

/// Return [(key, start_line, end_line)] for single-line method/constructor headers
/// with a body.  Lines are 1-based; end is the line of the matching closing brace.
pub fn method_spans(text: &str) -> Vec<(String, usize, usize)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut spans = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let ft = first_token(line);
        if STMT_KEYWORDS.contains(&ft) {
            continue;
        }
        if let Some(cap) = HEADER_RE.captures(line) {
            let name = cap.name("name").unwrap().as_str();
            if STMT_KEYWORDS.contains(&name) {
                continue;
            }
            let params = cap.name("params").unwrap().as_str();
            let key = make_key(name, params);
            let start = idx + 1;
            let end = match_end(&lines, idx);
            spans.push((key, start, end));
        }
    }
    spans
}

fn match_end(lines: &[&str], start_idx: usize) -> usize {
    let mut depth: i32 = 0;
    let mut open_found = false;
    for (i, line) in lines.iter().enumerate().skip(start_idx) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    open_found = true;
                }
                '}' => {
                    depth -= 1;
                    if open_found && depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
        }
    }
    start_idx + 1
}

fn extract(text: &str, start: usize, end: usize) -> String {
    let lines = split_keepends(text);
    lines[start - 1..end.min(lines.len())].concat()
}

fn replace_lines(text: &str, start: usize, end: usize, repl: &str) -> String {
    let lines = split_keepends(text);
    let mut out = String::new();
    for l in &lines[..start - 1] {
        out.push_str(l);
    }
    if repl.ends_with('\n') {
        out.push_str(repl);
    } else {
        out.push_str(repl);
        out.push('\n');
    }
    for l in &lines[end.min(lines.len())..] {
        out.push_str(l);
    }
    out
}

fn spans_map(text: &str) -> BTreeMap<String, (usize, usize)> {
    let mut m = BTreeMap::new();
    for (k, s, e) in method_spans(text) {
        m.insert(k, (s, e)); // last wins, mirroring the Python dict comprehension
    }
    m
}

/// Try to lower the error count of one file by swapping individual methods from donor
/// engines.  `compile_one(text) -> error_count`.  Returns (best_text, log).
pub fn per_method_merge<F: Fn(&str) -> i64>(
    rel: &str,
    base_text: &str,
    donor_texts: &[(String, String)],
    compile_one: F,
) -> (String, Vec<String>) {
    let mut log: Vec<String> = Vec::new();
    let mut cur = base_text.to_string();
    let mut cur_err = compile_one(&cur);
    if cur_err == 0 {
        return (cur, log);
    }
    let base_spans = spans_map(&cur);
    for key in base_spans.keys() {
        for (dn, dtext) in donor_texts {
            let dspans = spans_map(dtext);
            let (ds, de) = match dspans.get(key) {
                Some(v) => *v,
                None => continue,
            };
            let donor_method = extract(dtext, ds, de);
            let cur_spans = spans_map(&cur);
            let (cs, ce) = match cur_spans.get(key) {
                Some(v) => *v,
                None => break,
            };
            let cand = replace_lines(&cur, cs, ce, donor_method.trim_end_matches('\n'));
            let t2 = compile_one(&cand);
            if t2 < cur_err {
                cur = cand;
                cur_err = t2;
                log.push(format!("  {rel}::{key} <- {dn} (errors->{cur_err})"));
                break;
            }
        }
        if cur_err == 0 {
            break;
        }
    }
    (cur, log)
}

/// Run per-method merge, in place, over files that still fail in ISOLATION.
pub fn merge_per_method(
    engine_data: &BTreeMap<String, EngineData>,
    merge_dir: &Path,
    per_iso: &mut BTreeMap<String, i64>,
    api_classpath: &str,
    tmp: &Path,
) -> Vec<String> {
    let _ = std::fs::create_dir_all(tmp);
    let mut failing: Vec<String> = per_iso
        .iter()
        .filter(|(_, e)| **e != 0)
        .map(|(r, _)| r.clone())
        .collect();
    failing.sort();
    let iso_sum: i64 = per_iso.values().sum();
    let mut log = vec![format!(
        "per-method start: {} failing class(es), isolated-sum={iso_sum}",
        failing.len()
    )];

    let compile_one = |text: &str, rel: &str| -> i64 {
        let tf = tmp.join(rel);
        if let Some(p) = tf.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(&tf, text);
        compile_file_isolated(&tf, api_classpath, &tmp.join("_o"))
    };

    let names: Vec<&String> = engine_data
        .iter()
        .filter(|(_, d)| d.type_ == "names")
        .map(|(n, _)| n)
        .collect();
    let mut improved = 0usize;
    for rel in &failing {
        let target = merge_dir.join(rel);
        if !target.exists() {
            continue;
        }
        let base_text = util::read_text_lossy(&target).unwrap_or_default();
        let mut donor_texts: Vec<(String, String)> = Vec::new();
        for n in &names {
            if let Some(f) = engine_data[*n].files.get(rel) {
                donor_texts.push(((*n).clone(), util::read_text_lossy(f).unwrap_or_default()));
            }
        }
        let (best, sub) =
            per_method_merge(rel, &base_text, &donor_texts, |t| compile_one(t, rel));
        if best != base_text && compile_one(&best, rel) < per_iso[rel] {
            let _ = std::fs::write(&target, &best);
            per_iso.insert(rel.clone(), compile_one(&best, rel));
            log.extend(sub);
            improved += 1;
        }
    }
    let iso_sum2: i64 = per_iso.values().sum();
    log.push(format!(
        "per-method result: improved {improved} class(es), isolated-sum={iso_sum2}"
    ));
    log
}
