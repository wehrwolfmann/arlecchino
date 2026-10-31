//! `ai-annotate`: short Javadoc explanations for classes and non-obvious methods.
//!
//! The model sees the source with line numbers and answers with
//! `{"docs": [{"line": N, "text": "..."}]}`.  Each answer is checked before it is
//! used — the line must exist in the window shown, must look like a class or method
//! declaration, and must not already carry a Javadoc — and the text is sanitised so
//! it cannot end the comment early (`*/`) or smuggle in a Java unicode escape
//! (`\u000a` is processed even inside comments).  Comments are only INSERTED as
//! whole new lines; [`annotate_text`] proves it by removing them again and comparing
//! with the input byte for byte, so the code itself cannot change.

use super::provider::{estimate_tokens, extract_json, CACHE_MISS_OFFLINE};
use crate::ai_model::{ChatMessage, ChatModel, ChatRequest};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const PROMPT_VERSION: &str = "ai-annotate/1";

pub const SYSTEM_PROMPT: &str = "You document decompiled Java ME (J2ME/MIDP) code for a \
reader porting it. You get one source file (or a window of it) with line numbers. \
Write a short Javadoc explanation (1-3 sentences, plain prose, no code, no HTML, no \
@tags) for the class declaration and for each method whose purpose is NOT obvious \
from its name and a glance at its body. Explain what it does and why it matters in \
the program, not how Java works; do not restate the signature. Skip trivial getters, \
setters and one-liners. Reply with ONE JSON object and nothing else: \
{\"docs\": [{\"line\": <line number of the declaration>, \"text\": \"...\"}]}.";

#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub line: usize, // 1-based
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct FileResult {
    pub inserted: usize,
    pub rejected: Vec<String>,
    pub error: Option<String>,
    pub not_cached: bool,
}

fn decl_regexes() -> (Regex, Regex) {
    (
        Regex::new(r"^\s*(?:(?:public|protected|private|abstract|static|final|strictfp)\s+)*(?:class|interface|enum)\s+[\w$]+").unwrap(),
        Regex::new(r"^\s*(?:(?:public|protected|private|abstract|static|final|synchronized|native|strictfp)\s+)*(?:<[^>]*>\s*)?[\w$.\[\]<>, ]+\s+[\w$]+\s*\([^;]*$").unwrap(),
    )
}

const NOT_A_DECL: &[&str] = &["if", "for", "while", "switch", "catch", "return", "new", "else", "synchronized", "throw", "do", "try", "case"];

/// Is this line a class or method declaration header?
pub fn is_declaration(line: &str) -> bool {
    let (cls, meth) = decl_regexes();
    if cls.is_match(line) {
        return true;
    }
    let first = line.split_whitespace().next().unwrap_or("");
    let first = first.split('(').next().unwrap_or("");
    !NOT_A_DECL.contains(&first) && meth.is_match(line) && !line.contains('=')
}

/// Make model text safe inside `/** … */`.
pub fn sanitize(text: &str) -> String {
    let t: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .replace('\\', "&#92;")
        .replace("*/", "*&#47;")
        .replace("/*", "&#47;*");
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + w.len() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Insert the docs into `src`.  Returns the new text plus a reason for every doc
/// that was not used.  Guarantees (asserted): deleting the inserted lines gives back
/// `src` exactly.
pub fn annotate_text(src: &str, docs: &[Doc]) -> (String, usize, Vec<String>) {
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let mut by_line: BTreeMap<usize, String> = BTreeMap::new();
    let mut rejected = Vec::new();
    for d in docs {
        if d.line == 0 || d.line > lines.len() {
            rejected.push(format!("line {}: out of range", d.line));
            continue;
        }
        let l = lines[d.line - 1];
        if !is_declaration(l) {
            rejected.push(format!("line {}: not a class/method declaration", d.line));
            continue;
        }
        // insertion point: above any annotations directly over the declaration
        let mut at = d.line - 1;
        while at > 0 && lines[at - 1].trim_start().starts_with('@') {
            at -= 1;
        }
        let prev = lines[..at].iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim_end()).unwrap_or("");
        if prev.ends_with("*/") {
            rejected.push(format!("line {}: already has a comment", d.line));
            continue;
        }
        let text = sanitize(&d.text);
        if text.is_empty() {
            rejected.push(format!("line {}: empty text", d.line));
            continue;
        }
        if by_line.contains_key(&at) {
            rejected.push(format!("line {}: duplicate", d.line));
            continue;
        }
        by_line.insert(at, text);
    }
    let mut out = String::with_capacity(src.len() + by_line.len() * 120);
    let mut inserted_ranges: Vec<(usize, usize)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(text) = by_line.get(&i) {
            let indent: String = l.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let nl = if l.ends_with("\r\n") { "\r\n" } else { "\n" };
            let start = out.len();
            out.push_str(&format!("{indent}/**{nl}"));
            for w in wrap(text, 88usize.saturating_sub(indent.len()).max(40)) {
                out.push_str(&format!("{indent} * {w}{nl}"));
            }
            out.push_str(&format!("{indent} */{nl}"));
            inserted_ranges.push((start, out.len()));
        }
        out.push_str(l);
    }
    // proof that only comment lines were added
    let mut back = String::with_capacity(src.len());
    let mut pos = 0;
    for (a, b) in &inserted_ranges {
        back.push_str(&out[pos..*a]);
        pos = *b;
    }
    back.push_str(&out[pos..]);
    assert_eq!(back, src, "annotate_text changed the code");
    (out, by_line.len(), rejected)
}

/// Parse `{"docs": [...]}` (pure; unit-tested).
pub fn parse_reply(reply: &str) -> Result<Vec<Doc>, String> {
    let v = extract_json(reply)?;
    let arr = v.get("docs").and_then(|d| d.as_array()).ok_or("reply has no `docs` array")?;
    let mut out = Vec::new();
    for d in arr {
        let line = d.get("line").and_then(|l| l.as_u64().or_else(|| l.as_str().and_then(|s| s.parse().ok())));
        let text = d.get("text").and_then(|t| t.as_str());
        if let (Some(line), Some(text)) = (line, text) {
            out.push(Doc { line: line as usize, text: text.to_string() });
        }
    }
    Ok(out)
}

/// Line windows `[start, end)` (0-based) of at most `budget` tokens each.
pub fn windows(src: &str, budget: usize) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (i, l) in lines.iter().enumerate() {
        let t = estimate_tokens(l) + 3;
        if i > start && used + t > budget {
            out.push((start, i));
            start = i;
            used = 0;
        }
        used += t;
    }
    if start < lines.len() {
        out.push((start, lines.len()));
    }
    out
}

pub fn build_request(rel: &str, src: &str, win: (usize, usize), total: usize, reply_tokens: u32) -> ChatRequest {
    let mut user = format!("FILE {rel} (lines {}-{} of {total})\n", win.0 + 1, win.1);
    for (i, l) in src.lines().enumerate().skip(win.0).take(win.1 - win.0) {
        user.push_str(&format!("{:>5}| {l}\n", i + 1));
    }
    ChatRequest {
        system: format!("{SYSTEM_PROMPT}\n[{PROMPT_VERSION}]"),
        messages: vec![ChatMessage::user(user)],
        max_tokens: reply_tokens,
    }
}

/// Annotate one file's text.
pub fn annotate_file(model: &dyn ChatModel, rel: &str, src: &str, budget: usize, reply_tokens: u32) -> (String, FileResult) {
    let total = src.lines().count();
    let mut docs = Vec::new();
    let mut res = FileResult::default();
    for w in windows(src, budget) {
        let req = build_request(rel, src, w, total, reply_tokens);
        match model.chat(&req).and_then(|r| parse_reply(&r)) {
            Ok(ds) => {
                for d in ds {
                    if d.line < w.0 + 1 || d.line > w.1 {
                        res.rejected.push(format!("line {}: outside the window shown", d.line));
                    } else {
                        docs.push(d);
                    }
                }
            }
            Err(e) if e == CACHE_MISS_OFFLINE => res.not_cached = true,
            Err(e) => res.error = Some(e),
        }
    }
    let (out, n, rej) = annotate_text(src, &docs);
    res.inserted = n;
    res.rejected.extend(rej);
    (out, res)
}

/// All `.java` files under `dir`, relative, sorted.
pub fn java_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = crate::util::rglob_ext(dir, "java")
        .into_iter()
        .filter_map(|p| p.strip_prefix(dir).ok().map(|r| r.to_path_buf()))
        .collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations() {
        assert!(is_declaration("public class a extends Canvas {"));
        assert!(is_declaration("    protected void a(int n, String[] s) {"));
        assert!(is_declaration("  static final int[] b(c x) throws Exception {"));
        assert!(!is_declaration("    if (x(1)) {"));
        assert!(!is_declaration("    foo(1);"));
        assert!(!is_declaration("    int x = y(2);"));
        assert!(!is_declaration("    return a(b);"));
    }

    #[test]
    fn sanitize_blocks_comment_escape() {
        let s = sanitize("ends */ here \\u000a and /* nested");
        assert!(!s.contains("*/") && !s.contains("/*") && !s.contains('\\'));
    }

    #[test]
    fn inserts_only_comments() {
        let src = "class a {\n    @Override\n    public void b() {\n    }\n    /** old */\n    void c() {}\n}\n";
        let docs = vec![
            Doc { line: 1, text: "Main game loop.".into() },
            Doc { line: 3, text: "Draws a frame.".into() },
            Doc { line: 6, text: "dup".into() },
            Doc { line: 4, text: "not a decl".into() },
        ];
        let (out, n, rej) = annotate_text(src, &docs);
        assert_eq!(n, 2);
        assert_eq!(rej.len(), 2);
        assert!(out.contains("    /**\n     * Draws a frame.\n     */\n    @Override\n"));
        assert!(out.starts_with("/**\n * Main game loop.\n */\nclass a {"));
    }

    #[test]
    fn windows_cover_all_lines() {
        let src: String = (0..100).map(|i| format!("line number {i} with some text\n")).collect();
        let w = windows(&src, 100);
        assert!(w.len() > 1);
        assert_eq!(w.first().unwrap().0, 0);
        assert_eq!(w.last().unwrap().1, 100);
        for p in w.windows(2) {
            assert_eq!(p[0].1, p[1].0);
        }
    }
}
