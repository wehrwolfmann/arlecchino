//! Deobfuscation pass — optional `--deobf` flag.
//!
//! Two strategies, both heuristic:
//!   1. `jadx --deobf` re-run into a SEPARATE `deobf/` tree (the merged tree is left
//!      intact).
//!   2. a conservative heuristic that APPENDS a readability banner classifying each
//!      class by MIDP/string evidence (extends MIDlet -> "MIDlet", etc.) without
//!      renaming types (a safe project-wide rename needs a real refactoring engine).

use crate::util;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

static HINTS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"\bextends\s+MIDlet\b").unwrap(), "MIDlet"),
        (Regex::new(r"\bextends\s+(GameCanvas|Canvas)\b").unwrap(), "Canvas"),
        (
            Regex::new(r"\bextends\s+(Form|List|TextBox|Alert|Screen)\b").unwrap(),
            "Screen",
        ),
        (Regex::new(r"\bimplements\s+[^\{]*\bRunnable\b").unwrap(), "Worker"),
        (
            Regex::new(r"\bimplements\s+[^\{]*\bCommandListener\b").unwrap(),
            "UI",
        ),
        (
            Regex::new(r"\bnew\s+Player\b|MediaPlayer|javax\.microedition\.media").unwrap(),
            "Audio",
        ),
    ]
});

pub fn classify(text: &str) -> Option<&'static str> {
    for (rx, label) in HINTS.iter() {
        if rx.is_match(text) {
            return Some(label);
        }
    }
    None
}

/// Return [(relpath, suggested_label)] for classes we can classify (rglob order).
pub fn heuristic_report(srcdir: &Path) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for jf in util::rglob_ext(srcdir, "java") {
        if jf.file_name().map(|n| n == "_sources.txt").unwrap_or(false) {
            continue;
        }
        let text = match util::read_text_lossy(&jf) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Some(label) = classify(&text) {
            if let Ok(rel) = jf.strip_prefix(srcdir) {
                out.push((rel.to_string_lossy().into_owned(), label));
            }
        }
    }
    out
}

/// Add a `// arlecchino-deobf: ...` banner to each classified file (non-destructive).
pub fn annotate_in_place(srcdir: &Path) -> usize {
    let mut n = 0usize;
    for (rel, label) in heuristic_report(srcdir) {
        let jf = srcdir.join(&rel);
        let text = match util::read_text_lossy(&jf) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let banner = format!("// arlecchino-deobf: this class looks like a {label}\n");
        if text.contains(&banner) {
            continue;
        }
        if std::fs::write(&jf, format!("{banner}{text}")).is_ok() {
            n += 1;
        }
    }
    n
}

/// Run jadx with its deobfuscator into a separate tree.  Returns success.
pub fn emit_jadx_deobf(jar: &Path, jadx_bin: &Path, outdir: &Path) -> bool {
    if !jadx_bin.exists() {
        return false;
    }
    if std::fs::create_dir_all(outdir).is_err() {
        return false;
    }
    let argv = vec![
        jadx_bin.to_string_lossy().into_owned(),
        "--no-res".into(),
        "--deobf".into(),
        "--deobf-min".into(),
        "1".into(),
        "-d".into(),
        outdir.to_string_lossy().into_owned(),
        jar.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture(&argv, Some(std::time::Duration::from_secs(900)));
    // A run killed by the timeout is a FAILURE: the tree it left behind is partial.
    // The Python original raises TimeoutExpired here and reports False; reporting
    // success would print a "jadx --deobf tree -> ..." line pointing at a truncated
    // tree, and record it in report.json.
    r.spawn_err.is_none() && !r.timed_out
}
