//! Auto-fix common decompiler artifacts, as a post-merge pass.
//!
//! These are universal, semantics-preserving rewrites that turn legal-bytecode /
//! illegal-Java constructs back into something javac accepts.  Each fix is applied
//! file-by-file and only KEPT if it strictly reduces that file's javac error count
//! (verified by the caller), so a fix can never make a file worse.
//!
//! All STRUCTURAL scanning (brace matching, keyword search, the regex fixes) runs on
//! a length-preserving MASKED view of the source produced by [`mask_literals`], so a
//! `{`, `}`, `;` or a keyword hiding inside a string literal, a char literal or a
//! comment can neither move a block boundary nor trigger a spurious edit.  Because
//! the mask preserves every byte offset, positions computed on it index the ORIGINAL
//! text unchanged, and the edits are always applied to the original.

use crate::util::split_keepends;
use regex::Regex;
use std::sync::LazyLock;

static INTERFACE_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<pre>\bpublic\s+|\bfinal\s+|\babstract\s+|\bstrictfp\s+)*\binterface\s+(?P<name>[A-Za-z_$][\w$]*)\b(?P<tail>[^\{]*)\{",
    )
    .unwrap()
});
static STATIC_INIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bstatic\s*\{").unwrap());
static IMPORT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*import\s+[\w.$*]+\s*;[ \t]*$").unwrap());
static IFACE_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+static\s+final\s+[\w$.]+(?:\s*\[\s*\])*\s+(?P<name>[A-Za-z_$][\w$]*)\s*;")
        .unwrap()
});
static ASSIGN_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([A-Za-z_$][\w$]*)\s*=\s*(.+);\s*$").unwrap());
static EXTENDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bextends\b").unwrap());
static STATIC_LOCAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(?P<ind>[ \t]{8,})static\s+(?P<c>[A-Za-z_$])").unwrap());
static SCRATCH_DEAD_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<ind>[ \t]*)(?P<dtype>[A-Za-z_$][\w$.]*)\s*\[\s*\]\s+(?P<var>var\d+)\s*=\s*new\s+(?P<itype>[A-Za-z_$][\w$.]*)\s*\[\s*\]\s*\{",
    )
    .unwrap()
});
static CAST_BEFORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([A-Za-z_$][\w$.]*)\)\s*$").unwrap());
static TRAIL_IDENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([A-Za-z_$][\w$]*)$").unwrap());

const UNREP: &str = "(<unrepresentable>)";

// ── length-preserving literal / comment mask ─────────────────────────────────

/// Return a length-preserving "masked" view of `text` in which the *contents* of
/// string literals, char literals, line comments and block comments are replaced by
/// spaces, while every BYTE offset is preserved 1:1 with the original.
///
/// This lets the brace-matcher and the regex fixes scan a view where a stray `{`,
/// `}`, `;` (or anything else) hiding INSIDE a literal/comment can no longer be
/// mis-counted, yet any edit position computed on the masked view still lands on the
/// correct byte of the ORIGINAL text (byte lengths are identical).
///
/// What is masked (inner chars -> spaces; the delimiters themselves are kept so the
/// masked text stays lexically shaped like Java):
///   * `"..."`     string literals (with `\"` / `\\` escapes handled)
///   * `'...'`     char literals (with escapes handled)
///   * `// ...`    line comments (to end of line; the newline is preserved)
///   * `/* ... */` block comments (including any newlines inside -> spaces)
///
/// Newlines are always preserved (never blanked) so line-oriented logic keeps
/// identical line boundaries on the masked and the original text.
///
/// A non-ASCII char is replaced by AS MANY spaces as it occupies bytes, so the mask
/// stays byte-for-byte aligned with the original (and remains valid UTF-8).
pub fn mask_literals(text: &str) -> String {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut out: Vec<u8> = text.as_bytes().to_vec();

    // blank char #k (a newline is always kept, so line boundaries never move)
    fn blank(out: &mut [u8], chars: &[(usize, char)], k: usize) {
        let (off, c) = chars[k];
        if c == '\n' {
            return;
        }
        for b in 0..c.len_utf8() {
            out[off + b] = b' ';
        }
    }

    let mut i = 0usize;
    while i < n {
        let c = chars[i].1;
        if c == '"' || c == '\'' {
            // string / char literal: the delimiters stay, the contents are blanked
            let quote = c;
            i += 1;
            while i < n {
                let ch = chars[i].1;
                if ch == '\\' && i + 1 < n {
                    blank(&mut out, &chars, i);
                    blank(&mut out, &chars, i + 1);
                    i += 2;
                    continue;
                }
                if ch == quote {
                    i += 1;
                    break;
                }
                blank(&mut out, &chars, i);
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1].1 == '/' {
            // line comment
            blank(&mut out, &chars, i);
            blank(&mut out, &chars, i + 1);
            i += 2;
            while i < n && chars[i].1 != '\n' {
                blank(&mut out, &chars, i);
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1].1 == '*' {
            // block comment
            blank(&mut out, &chars, i);
            blank(&mut out, &chars, i + 1);
            i += 2;
            while i < n {
                if chars[i].1 == '*' && i + 1 < n && chars[i + 1].1 == '/' {
                    blank(&mut out, &chars, i);
                    blank(&mut out, &chars, i + 1);
                    i += 2;
                    break;
                }
                blank(&mut out, &chars, i);
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    String::from_utf8(out).expect("mask only ever writes ASCII spaces")
}

/// Snap a mask-derived byte offset down to the nearest char boundary of `text`.
/// A masked multi-byte char becomes several spaces, so a regex can in principle
/// report an offset that is a boundary in the mask but not in the original; every
/// structural token we match on is ASCII, so this is a guard, not a routine fixup.
fn snap(text: &str, mut i: usize) -> usize {
    while i > 0 && i < text.len() && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Slice the ORIGINAL text with offsets computed on its mask.
fn cut(text: &str, start: usize, end: usize) -> &str {
    &text[snap(text, start)..snap(text, end)]
}

/// Heuristic: an interface whose body contains a `static {}` block (only legal in a
/// class) — the signature of a decompiled constant interface.
///
/// The `static {` probe runs on the masked view, so a `static {` token sitting in a
/// string literal or a comment cannot masquerade as a real static-init block.
fn is_constant_interface(mask: &str, m_end: usize) -> bool {
    STATIC_INIT.is_match(&mask[m_end..])
}

/// Return (kw_start, body_start, body_end) for the brace block following the first
/// occurrence of `start_kw`, or None.  Byte offsets into the ORIGINAL text.
///
/// Both the `start_kw` search AND the `{`/`}` depth counting run on `mask`, so
/// braces, semicolons or the keyword text appearing inside a string/char literal or
/// a comment are ignored.  The mask is length-preserving, so the returned offsets
/// index the original text unchanged.
fn brace_block_masked(mask: &str, start_kw: &str) -> Option<(usize, usize, usize)> {
    let si = mask.find(start_kw)?;
    let i = mask[si..].find('{').map(|p| si + p)?;
    let bytes = mask.as_bytes();
    let mut depth: i32 = 0;
    let mut j = i;
    while j < bytes.len() {
        match bytes[j] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((si, i, j));
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// [`brace_block_masked`] on a freshly computed mask of `text`.
pub fn brace_block(text: &str, start_kw: &str) -> Option<(usize, usize, usize)> {
    brace_block_masked(&mask_literals(text), start_kw)
}

/// Keep a constant interface AS an interface but make it legal by folding its
/// `static {}` block's `FIELD = value;` assignments into inline field initialisers.
pub fn fold_interface_static_init(text: &str) -> String {
    let mask = mask_literals(text);
    let caps = match INTERFACE_DECL.captures(&mask) {
        Some(c) => c,
        None => return text.to_string(),
    };
    let m_end = caps.get(0).unwrap().end();
    if !is_constant_interface(&mask, m_end) {
        return text.to_string();
    }
    let blk = match brace_block_masked(&mask, "static {") {
        Some(b) => b,
        None => return text.to_string(),
    };
    let (blk_si, body_start, body_end) = blk;
    // masked body: assignments hiding inside a comment/string must not be folded.
    let body = &mask[body_start + 1..body_end];
    let real_body = cut(text, body_start + 1, body_end);

    // collect declared, uninitialised interface fields (name -> (start,end,whole)).
    // Matched on the masked view so a field-shaped line inside a comment/string is
    // ignored; the spans index the ORIGINAL text (the mask is length-preserving).
    // Dict semantics (one entry per NAME): when a class declares the same field name
    // twice with different descriptors — the `o.java` case — only the LAST
    // declaration is recorded, and therefore only that one is rewritten.  Folding a
    // value into every same-named declaration would hand javac a type mismatch.
    let mut decls: std::collections::HashMap<String, (usize, usize, String)> =
        std::collections::HashMap::new();
    for fm in IFACE_FIELD.captures_iter(&mask[..blk_si]) {
        let whole = fm.get(0).unwrap();
        let name = fm.name("name").unwrap().as_str().to_string();
        let real = cut(text, whole.start(), whole.end()).to_string();
        decls.insert(name, (whole.start(), whole.end(), real));
    }
    if decls.is_empty() {
        return text.to_string();
    }
    let decl_names: std::collections::HashSet<String> = decls.keys().cloned().collect();

    // Walk the masked and the original body lines in lock-step (the mask is length-
    // preserving -> same line count, same per-line byte lengths).  The assignment is
    // RECOGNISED on the masked line (so a `name = ...;` hiding in a comment/string is
    // ignored), but the VALUE is sliced from the ORIGINAL line at the same offsets —
    // so the real expression text is folded in, not blanked-out chars.  A trailing
    // line comment after the `;` is masked to spaces, so `;\s*$` still anchors on the
    // real `;`.
    let mut inits: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (mline, rline) in body.split('\n').zip(real_body.split('\n')) {
        if let Some(am) = ASSIGN_LINE.captures(mline) {
            let n = am.get(1).unwrap().as_str().to_string();
            if decl_names.contains(&n) {
                let v = am.get(2).unwrap();
                inits.insert(n, cut(rline, v.start(), v.end()).to_string());
            }
        }
    }
    // require every declared field to be initialised (no data loss)
    let init_names: std::collections::HashSet<String> = inits.keys().cloned().collect();
    if init_names != decl_names {
        return text.to_string();
    }

    // rewrite each declaration to include its initialiser (back-to-front)
    let mut new = text.to_string();
    let mut ordered: Vec<(String, usize, usize, String)> = decls
        .into_iter()
        .map(|(n, (s, e, w))| (n, s, e, w))
        .collect();
    ordered.sort_by_key(|x| std::cmp::Reverse(x.1)); // by start desc
    for (name, start, end, whole) in ordered {
        // whole ends with ';'
        let mut nd = whole[..whole.len() - 1].to_string();
        nd = nd.trim_end().to_string();
        nd = nd.trim_end_matches(';').to_string();
        nd = nd.trim_end().to_string();
        nd.push_str(&format!(" = {};", inits[&name]));
        new = format!("{}{}{}", &new[..start], nd, &new[end..]);
    }
    // remove the now-empty static block (recompute its span in `new`)
    if let Some((ks, _, be)) = brace_block(&new, "static {") {
        let end = be + 1;
        new = format!("{}{}", &new[..ks], &new[end..]);
    }
    new
}

/// Convert the FIRST top-level constant interface to a final class.
pub fn const_interface_to_class(text: &str) -> String {
    let mask = mask_literals(text);
    let caps = match INTERFACE_DECL.captures(&mask) {
        Some(c) => c,
        None => return text.to_string(),
    };
    let whole = caps.get(0).unwrap();
    if !is_constant_interface(&mask, whole.end()) {
        return text.to_string();
    }
    // rebuild the groups from the ORIGINAL text at the matched spans (the mask is
    // length-preserving, so the same offsets hold).
    let pre = caps
        .name("pre")
        .map(|m| cut(text, m.start(), m.end()))
        .unwrap_or("");
    let nm = caps.name("name").unwrap();
    let name = cut(text, nm.start(), nm.end());
    let tail_m = caps.name("tail");
    let tail = tail_m.map(|m| cut(text, m.start(), m.end())).unwrap_or("");
    if EXTENDS.is_match(tail_m.map(|m| m.as_str()).unwrap_or("")) {
        return text.to_string();
    }
    let repl = format!("{pre}final class {name}{tail}{{");
    format!(
        "{}{}{}",
        cut(text, 0, whole.start()),
        repl,
        &text[snap(text, whole.end())..]
    )
}

/// Drop a `static` modifier on a deeply-nested (local) declaration.
///
/// The match is computed on the masked view so a `static` token inside a string
/// literal or a comment is never stripped; the edits apply to the original text.
pub fn illegal_static_local(text: &str) -> String {
    let mask = mask_literals(text);
    let hits: Vec<(usize, usize, usize)> = STATIC_LOCAL
        .captures_iter(&mask)
        .map(|m| {
            let whole = m.get(0).unwrap();
            let ind = m.name("ind").unwrap();
            (whole.start(), whole.end(), ind.end())
        })
        .collect();
    let mut out = text.to_string();
    // apply back-to-front so earlier offsets stay valid after the (shorter) replacement
    for (start, end, ind_end) in hits.into_iter().rev() {
        let keep_ind = cut(text, start, ind_end).to_string();
        // the trailing `[A-Za-z_$]` the regex consumed is put back with the indent
        let kept_char = cut(text, end - 1, end).to_string();
        out = format!(
            "{}{}{}{}",
            &out[..snap(&out, start)],
            keep_ind,
            kept_char,
            &out[snap(&out, end)..]
        );
    }
    out
}

/// Repair a synthetic decompiler scratch temp whose array DECLARATION type conflicts
/// with its initialiser's array type, when that initialiser is a DEAD store.
pub fn scratch_temp_dead_array_type(text: &str) -> String {
    // Declarations and the dead-store proof are matched on the masked view (so a
    // declaration-shaped fragment or a `var = ` line hiding in a string/comment is
    // ignored); the spans index the original text.
    let mask = mask_literals(text);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for m in SCRATCH_DEAD_DECL.captures_iter(&mask) {
        let whole = m.get(0).unwrap();
        let dtype = m.name("dtype").unwrap().as_str();
        let itype = m.name("itype").unwrap().as_str();
        if dtype == itype {
            continue; // declared and initialiser element types agree -> legal, skip
        }
        let var = m.name("var").unwrap().as_str();
        // next non-blank line after this declaration must reassign the same temp
        let nl = mask[whole.start()..].find('\n').map(|p| whole.start() + p);
        let rest = match nl {
            Some(n) => &mask[n + 1..],
            None => "",
        };
        let nxt = rest.split('\n').map(|l| l.trim()).find(|l| !l.is_empty());
        let reassign = Regex::new(&format!(r"^{}\s*=", regex::escape(var))).unwrap();
        match nxt {
            Some(n) if reassign.is_match(n) => {}
            _ => continue,
        }
        let seg = cut(text, whole.start(), whole.end());
        let pat = format!("{dtype}[] {var} =");
        let rep = format!("Object {var} =");
        let seg_new = seg.replacen(&pat, &rep, 1);
        if seg_new != seg {
            edits.push((whole.start(), whole.end(), seg_new));
        }
    }
    let mut out = text.to_string();
    for (start, end, seg_new) in edits.into_iter().rev() {
        out = format!(
            "{}{}{}",
            &out[..snap(&out, start)],
            seg_new,
            &out[snap(&out, end)..]
        );
    }
    out
}

/// Replace Vineflower's `(<unrepresentable>)` placeholder casts with the real cast
/// type recovered from another engine's rendering of the *same* expression.
pub fn resolve_unrepresentable_casts(text: &str, donor_texts: &[String]) -> String {
    if !text.contains(UNREP) {
        return text.to_string();
    }
    let mut out = String::new();
    for line in split_keepends(text) {
        if !line.contains(UNREP) {
            out.push_str(line);
            continue;
        }
        let idx = line.find(UNREP).unwrap();
        let after = &line[idx + UNREP.len()..];
        let mut tail = after.trim().to_string();
        tail = match tail.split_once('=') {
            Some((before, _)) if tail.contains('=') => before.to_string(),
            _ => tail,
        };
        tail = tail.trim_end_matches(';').trim().to_string();
        let anchor: String = match tail.find('.') {
            Some(dot) => tail[dot..].to_string(),
            None => tail.clone(),
        };
        let mut repl_type: Option<String> = None;
        if anchor.chars().count() >= 6 {
            for dt in donor_texts {
                let mut search_from = 0usize;
                while let Some(rel) = dt[search_from..].find(&anchor) {
                    if repl_type.is_some() {
                        break;
                    }
                    let pos = search_from + rel;
                    let pre = dt[..pos].trim_end();
                    if let Some(vm) = TRAIL_IDENT.find(pre) {
                        let before_var = pre[..vm.start()].trim_end();
                        if let Some(cm) = CAST_BEFORE.captures(before_var) {
                            let t = cm.get(1).unwrap().as_str();
                            if t != "<unrepresentable>" {
                                repl_type = Some(t.to_string());
                            }
                        }
                    }
                    search_from = pos + 1;
                }
                if repl_type.is_some() {
                    break;
                }
            }
        }
        if let Some(t) = repl_type {
            out.push_str(&line.replace(UNREP, &format!("({t})")));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Drop duplicate `import a.b.C;` lines (keep the first).
pub fn duplicate_imports(text: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    for line in split_keepends(text) {
        let stripped_nl = line.trim_end_matches('\n');
        if IMPORT_LINE.is_match(stripped_nl) {
            let key = line.trim().to_string();
            if seen.contains(&key) {
                continue; // drop the duplicate import line entirely
            }
            seen.insert(key);
        }
        out.push_str(line);
    }
    out
}

/// Ordered list of (name, function); each is tried independently.
type FixFn = fn(&str) -> String;
pub fn fixes() -> Vec<(&'static str, FixFn)> {
    vec![
        ("fold_interface_static_init", fold_interface_static_init),
        ("const_interface_to_class", const_interface_to_class),
        ("scratch_temp_dead_array_type", scratch_temp_dead_array_type),
        ("duplicate_imports", duplicate_imports),
        ("illegal_static_local", illegal_static_local),
    ]
}

/// Apply every fix in sequence (best-effort).  Returns (new_text, applied names).
pub fn apply_all(text: &str) -> (String, Vec<String>) {
    let mut applied = Vec::new();
    let mut cur = text.to_string();
    for (name, f) in fixes() {
        let nxt = f(&cur);
        if nxt != cur {
            applied.push(name.to_string());
            cur = nxt;
        }
    }
    (cur, applied)
}
