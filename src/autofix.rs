//! Auto-fix common decompiler artifacts, as a post-merge pass.
//!
//! These are universal, semantics-preserving rewrites that turn legal-bytecode /
//! illegal-Java constructs back into something javac accepts.  Each fix is applied
//! file-by-file and only KEPT if it strictly reduces that file's javac error count
//! (verified by the caller), so a fix can never make a file worse.

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

/// Heuristic: an interface whose body contains a `static {}` block (only legal in a
/// class) — the signature of a decompiled constant interface.
fn is_constant_interface(text: &str, m_end: usize) -> bool {
    STATIC_INIT.is_match(&text[m_end..])
}

/// Return (kw_start, body_start, body_end) for the brace block following the first
/// occurrence of `start_kw`, or None.  Byte offsets; brace bytes are ASCII.
fn brace_block(text: &str, start_kw: &str) -> Option<(usize, usize, usize)> {
    let si = text.find(start_kw)?;
    let i = text[si..].find('{').map(|p| si + p)?;
    let bytes = text.as_bytes();
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

/// Keep a constant interface AS an interface but make it legal by folding its
/// `static {}` block's `FIELD = value;` assignments into inline field initialisers.
pub fn fold_interface_static_init(text: &str) -> String {
    let caps = match INTERFACE_DECL.captures(text) {
        Some(c) => c,
        None => return text.to_string(),
    };
    let m_end = caps.get(0).unwrap().end();
    if !is_constant_interface(text, m_end) {
        return text.to_string();
    }
    let blk = match brace_block(text, "static {") {
        Some(b) => b,
        None => return text.to_string(),
    };
    let (blk_si, body_start, body_end) = blk;
    let body = &text[body_start + 1..body_end];

    // collect declared, uninitialised interface fields (name -> (start,end,whole))
    let mut decls: Vec<(String, usize, usize, String)> = Vec::new();
    let mut decl_names = std::collections::HashSet::new();
    for fm in IFACE_FIELD.captures_iter(&text[..blk_si]) {
        let whole = fm.get(0).unwrap();
        let name = fm.name("name").unwrap().as_str().to_string();
        // dict semantics: last declaration of a given name wins on lookup, but every
        // declaration is rewritten. We keep them all; the name set is what matters.
        decl_names.insert(name.clone());
        decls.push((name, whole.start(), whole.end(), whole.as_str().to_string()));
    }
    if decls.is_empty() {
        return text.to_string();
    }

    // collect `NAME = value;` assignments from the static block body
    let mut inits: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in body.split('\n') {
        if let Some(am) = ASSIGN_LINE.captures(line) {
            let n = am.get(1).unwrap().as_str().to_string();
            if decl_names.contains(&n) {
                inits.insert(n, am.get(2).unwrap().as_str().to_string());
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
    let mut ordered = decls.clone();
    ordered.sort_by(|a, b| b.1.cmp(&a.1)); // by start desc
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
    let caps = match INTERFACE_DECL.captures(text) {
        Some(c) => c,
        None => return text.to_string(),
    };
    let whole = caps.get(0).unwrap();
    if !is_constant_interface(text, whole.end()) {
        return text.to_string();
    }
    let pre = caps.name("pre").map(|m| m.as_str()).unwrap_or("");
    let name = caps.name("name").unwrap().as_str();
    let tail = caps.name("tail").map(|m| m.as_str()).unwrap_or("");
    if EXTENDS.is_match(tail) {
        return text.to_string();
    }
    let repl = format!("{pre}final class {name}{tail}{{");
    format!("{}{}{}", &text[..whole.start()], repl, &text[whole.end()..])
}

/// Drop a `static` modifier on a deeply-nested (local) declaration.
pub fn illegal_static_local(text: &str) -> String {
    STATIC_LOCAL.replace_all(text, "${ind}${c}").into_owned()
}

/// Repair a synthetic decompiler scratch temp whose array DECLARATION type conflicts
/// with its initialiser's array type, when that initialiser is a DEAD store.
pub fn scratch_temp_dead_array_type(text: &str) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for m in SCRATCH_DEAD_DECL.captures_iter(text) {
        let whole = m.get(0).unwrap();
        let dtype = m.name("dtype").unwrap().as_str();
        let itype = m.name("itype").unwrap().as_str();
        if dtype == itype {
            continue; // declared and initialiser element types agree -> legal, skip
        }
        let var = m.name("var").unwrap().as_str();
        // next non-blank line after this declaration must reassign the same temp
        let nl = text[whole.start()..].find('\n').map(|p| whole.start() + p);
        let rest = match nl {
            Some(n) => &text[n + 1..],
            None => "",
        };
        let nxt = rest
            .split('\n')
            .map(|l| l.trim())
            .find(|l| !l.is_empty());
        let reassign = Regex::new(&format!(r"^{}\s*=", regex::escape(var))).unwrap();
        match nxt {
            Some(n) if reassign.is_match(n) => {}
            _ => continue,
        }
        let seg = &text[whole.start()..whole.end()];
        let pat = format!("{dtype}[] {var} =");
        let rep = format!("Object {var} =");
        let seg_new = replacen(seg, &pat, &rep, 1);
        if seg_new != seg {
            edits.push((whole.start(), whole.end(), seg_new));
        }
    }
    let mut out = text.to_string();
    for (start, end, seg_new) in edits.into_iter().rev() {
        out = format!("{}{}{}", &out[..start], seg_new, &out[end..]);
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

/// `str::replacen`-equivalent (Python `str.replace(a, b, count)`).
fn replacen(hay: &str, needle: &str, rep: &str, count: usize) -> String {
    hay.replacen(needle, rep, count)
}
