//! `ai-fix` — optional model-assisted repair of the classes that still fail to
//! compile after the best-of merge and autofix.
//!
//! The deterministic pipeline (8 engines, per-class best-of, autofix, per-method
//! merge) usually leaves a small residue of classes that no engine got right.  This
//! stage hands each of them to a text model together with everything the pipeline
//! already knows about the class, and keeps an answer ONLY when it is verified:
//!
//!   1. the candidate compiles with zero javac errors, isolated, against the
//!      original jar's bytecode (the same scoring the merge uses);
//!   2. it still declares the same package and top-level type;
//!   3. when `javap` is available and the class exists in the jar, the semantic
//!      round-trip gate (see `semdiff`) finds no method whose normalised symbolic
//!      reference set drifted from the original bytecode — a model cannot "fix" a
//!      class by deleting or stubbing out code;
//!   4. after all substitutions the whole merged tree must not have MORE javac
//!      errors than before; an accepted class that makes it worse is reverted.
//!
//! A rejected candidate is fed back (its new errors, or the drifting methods) and
//! the model tries again, up to `max_attempts` times.  Everything is journaled.
//!
//! Context given to the model per class (no whole-program dump):
//!   * the javac errors of the current merged variant;
//!   * the current merged variant itself;
//!   * the OTHER engines' variants of this class, reduced to the hunks where they
//!     diverge from it, nearest-to-an-error first;
//!   * the member signatures the original bytecode declares for this class, and
//!     the bytecode of the methods that hold errors;
//!   * member signatures of the in-jar classes this class refers to.
//!
//! The approach follows the published idea of compiler-feedback repair loops for
//! decompiler output (e.g. "Refining Decompiled C Code with LLMs", arXiv:2310.06530;
//! DecLLM, ISSTA 2025) — the code is an independent implementation.
//!
//! The stage is OFF by default and talks to a model only through
//! [`crate::ai_model::ChatModel`].

use crate::ai_model::{ChatMessage, ChatModel, ChatRequest};
use crate::{autofix, core, permethod, semdiff, util};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static ERR_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(.*?\.java):(\d+): error: ").unwrap());
static PACKAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*package\s+([\w.$]+)\s*;").unwrap());
static TYPE_DECL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:class|interface|enum)\s+([A-Za-z_$][\w$]*)").unwrap()
});
static IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*").unwrap());
static FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```[A-Za-z]*[ \t]*\r?\n(.*?)```").unwrap());

pub const SYSTEM_PROMPT: &str = "\
You repair Java source code produced by decompilers so that it compiles again.
The class was decompiled from an existing program; the original bytecode is the
ground truth. It is compiled with `javac --release 8` against the original
program's classes, so every other class it uses already exists exactly as the
signatures given to you describe.

Rules:
- Keep the package, the type name, every field and every method with the same
  name, signature and modifiers. Do not add or remove members.
- Preserve behaviour exactly: the same calls, field accesses, constants, object
  creations and control flow as the original bytecode. Do not delete statements,
  do not replace bodies with stubs or `throw`, do not wrap code in try/catch to
  silence errors.
- Fix only what the compiler rejects: casts, generic or primitive types, variable
  declarations and scopes, labels and loops, synthetic accessors, name clashes,
  unrepresentable constructs. When another decompiler's variant of a region is
  correct, prefer it.
- Reply with the COMPLETE corrected compilation unit in a single ```java code
  block, and nothing else.";

/// Knobs for one `ai-fix` run.
#[derive(Debug, Clone)]
pub struct Options {
    /// Model calls per class before giving up.
    pub max_attempts: usize,
    /// Stop after this many classes (None = all failing classes).
    pub max_classes: Option<usize>,
    /// Restrict to these relative paths (empty = every failing class).
    pub only: Vec<String>,
    /// Apply the semdiff gate when javap is available (on by default).
    pub semantic_gate: bool,
    /// Rough character budget for the variant diffs in one prompt.
    pub diff_budget: usize,
    /// Rough character budget for referenced-class signatures.
    pub signature_budget: usize,
    /// Build and save the prompts, but never call the model.
    pub dry_run: bool,
    pub max_tokens: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_attempts: 3,
            max_classes: None,
            only: Vec::new(),
            semantic_gate: true,
            diff_budget: 12_000,
            signature_budget: 8_000,
            dry_run: false,
            max_tokens: 16_000,
        }
    }
}

/// Where the stage reads from and writes to.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Merged source tree (`<out>/merged`), patched in place.
    pub merged: PathBuf,
    /// Jar whose bytecode the pipeline compiled against (the renamed jar when the
    /// member renamer ran).  Source of truth for signatures and the semantic gate.
    pub jar: PathBuf,
    /// Extra classpath (MIDP stub), if any.
    pub classpath: Option<String>,
    /// Renamer mapping, for the semantic gate's name reconciliation.
    pub mapping: Option<PathBuf>,
    /// Engine source trees in config order: (engine name, sources dir).
    pub engines: Vec<(String, PathBuf)>,
    /// Journal / attempts / prompts go here (`<out>/ai_fix`).
    pub work: PathBuf,
}

impl Workspace {
    pub fn api_classpath(&self) -> String {
        core::build_api_classpath(&self.jar, self.classpath.as_deref(), &self.work)
    }
}

// ───────────────────────────── compiling one class ──────────────────────────

/// Result of compiling one source file in isolation.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub errors: i64,
    /// javac's stderr with the scratch path stripped, so lines read `a/B.java:12: ...`
    pub log: String,
    /// 1-based source lines that carry an error.
    pub error_lines: Vec<usize>,
    /// Directory that holds the produced .class files (complete only if errors == 0).
    pub classes: PathBuf,
}

/// Compile `text` as the file `rel` against `api_cp`, inside `dir` (wiped first).
pub fn compile_one(text: &str, rel: &str, api_cp: &str, dir: &Path) -> Compiled {
    let _ = std::fs::remove_dir_all(dir);
    let src_root = dir.join("src");
    let classes = dir.join("classes");
    let src = src_root.join(rel);
    if let Some(p) = src.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::create_dir_all(&classes);
    let _ = std::fs::write(&src, text);
    let argv = vec![
        util::tool("ARLECCHINO_JAVAC", "javac"),
        "-d".into(),
        classes.to_string_lossy().into_owned(),
        "--release".into(),
        "8".into(),
        "-encoding".into(),
        "utf-8".into(),
        "-nowarn".into(),
        "-proc:none".into(),
        "-Xmaxerrs".into(),
        "200".into(),
        "-cp".into(),
        api_cp.to_string(),
        src.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture_in(&argv, None, Some(dir));
    let prefix = format!("{}/", src_root.to_string_lossy());
    let log = r.stderr.replace(&prefix, "");
    let mut errors = 0i64;
    let mut error_lines = Vec::new();
    for c in ERR_LINE_RE.captures_iter(&log) {
        errors += 1;
        if let Ok(n) = c[2].parse::<usize>() {
            error_lines.push(n);
        }
    }
    if errors == 0 && !r.success() {
        // javac failed without a parsable error line (crash, bad classpath, ...)
        errors = 1;
    }
    error_lines.sort_unstable();
    error_lines.dedup();
    Compiled { errors, log, error_lines, classes }
}

// ───────────────────────────── context: variant diffs ───────────────────────

/// One region where an engine's variant diverges from the current text.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// 1-based line range in the CURRENT text (start inclusive, end exclusive).
    pub cur_start: usize,
    pub cur_end: usize,
    pub other_start: usize,
    pub other_end: usize,
    /// Rendered lines prefixed with ' ', '-' (current only) or '+' (variant only).
    pub body: String,
}

/// Line-level LCS diff of `cur` against `other`, grouped into hunks with `ctx`
/// lines of context.  Large inputs fall back to one hunk spanning the differing
/// middle after the common prefix/suffix is trimmed.
pub fn divergent_hunks(cur: &str, other: &str, ctx: usize) -> Vec<Hunk> {
    let a: Vec<&str> = cur.lines().collect();
    let b: Vec<&str> = other.lines().collect();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre].trim_end() == b[pre].trim_end() {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre
        && suf < b.len() - pre
        && a[a.len() - 1 - suf].trim_end() == b[b.len() - 1 - suf].trim_end()
    {
        suf += 1;
    }
    let am = &a[pre..a.len() - suf];
    let bm = &b[pre..b.len() - suf];
    if am.is_empty() && bm.is_empty() {
        return Vec::new();
    }

    // ops over the trimmed middle: (tag, a_index, b_index) in absolute line indices
    #[derive(Clone, Copy, PartialEq)]
    enum Op {
        Eq,
        Del,
        Add,
    }
    let mut ops: Vec<(Op, usize, usize)> = Vec::new();
    const MAX_CELLS: usize = 6_000_000;
    if am.len().saturating_mul(bm.len()) <= MAX_CELLS {
        let (n, m) = (am.len(), bm.len());
        let mut dp = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i * (m + 1) + j] = if am[i].trim_end() == bm[j].trim_end() {
                    dp[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    dp[(i + 1) * (m + 1) + j].max(dp[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i].trim_end() == bm[j].trim_end() {
                ops.push((Op::Eq, pre + i, pre + j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || dp[(i + 1) * (m + 1) + j] >= dp[i * (m + 1) + j + 1]) {
                // deletions first, so a replaced region reads "- old / + new"
                ops.push((Op::Del, pre + i, pre + j));
                i += 1;
            } else {
                ops.push((Op::Add, pre + i, pre + j));
                j += 1;
            }
        }
    } else {
        for i in 0..am.len() {
            ops.push((Op::Del, pre + i, pre));
        }
        for j in 0..bm.len() {
            ops.push((Op::Add, pre + am.len(), pre + j));
        }
    }

    // group changed ops (plus ctx lines of the trimmed prefix/suffix and equal runs)
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| o.0 != Op::Eq)
        .map(|(k, _)| k)
        .collect();
    let mut groups: Vec<(usize, usize)> = Vec::new(); // op index ranges [s, e]
    for k in changed {
        match groups.last_mut() {
            Some(g) if k <= g.1 + 2 * ctx + 1 => g.1 = k,
            _ => groups.push((k, k)),
        }
    }
    let mut hunks = Vec::new();
    for (s, e) in groups {
        let s2 = s.saturating_sub(ctx);
        let e2 = (e + ctx).min(ops.len() - 1);
        let mut body = String::new();
        // leading context from the common prefix when the hunk starts at the middle
        let (a0, b0) = (ops[s2].1, ops[s2].2);
        let lead = if s2 == 0 { ctx.min(pre) } else { 0 };
        for line in &a[a0 - lead..a0] {
            body.push_str(&format!("  {line}\n"));
        }
        let (mut a_last, mut b_last) = (a0, b0);
        for &(op, i, j) in &ops[s2..=e2] {
            match op {
                Op::Eq => {
                    body.push_str(&format!("  {}\n", a[i]));
                    a_last = i + 1;
                    b_last = j + 1;
                }
                Op::Del => {
                    body.push_str(&format!("- {}\n", a[i]));
                    a_last = i + 1;
                }
                Op::Add => {
                    body.push_str(&format!("+ {}\n", b[j]));
                    b_last = j + 1;
                }
            }
        }
        // trailing context from the common suffix when the hunk ends the middle
        if e2 == ops.len() - 1 {
            let tail_end = (a_last + ctx).min(a.len());
            for line in &a[a_last..tail_end] {
                body.push_str(&format!("  {line}\n"));
            }
        }
        hunks.push(Hunk {
            cur_start: a0 - lead + 1,
            cur_end: a_last.max(a0) + 1,
            other_start: b0 - lead + 1,
            other_end: b_last.max(b0) + 1,
            body,
        });
    }
    hunks
}

/// Render every other engine's divergent hunks within `budget` characters,
/// hunks closest to an error line first.  Engines whose text is identical to the
/// current one, or to an engine already shown, are folded into one heading.
pub fn render_variant_diffs(
    current: &str,
    variants: &[(String, String)],
    error_lines: &[usize],
    budget: usize,
) -> String {
    // dedup identical variants
    let mut groups: Vec<(Vec<String>, &str)> = Vec::new();
    let mut same_as_current: Vec<String> = Vec::new();
    for (eng, text) in variants {
        if text.trim() == current.trim() {
            same_as_current.push(eng.clone());
            continue;
        }
        match groups.iter_mut().find(|(_, t)| t.trim() == text.trim()) {
            Some(g) => g.0.push(eng.clone()),
            None => groups.push((vec![eng.clone()], text.as_str())),
        }
    }
    let mut out = String::new();
    if !same_as_current.is_empty() {
        out.push_str(&format!(
            "(identical to the current variant: {})\n",
            same_as_current.join(", ")
        ));
    }
    if groups.is_empty() {
        return out;
    }
    let per = budget / groups.len();
    for (engs, text) in groups {
        let mut hunks = divergent_hunks(current, text, 2);
        let dist = |h: &Hunk| -> usize {
            error_lines
                .iter()
                .map(|&l| {
                    if l >= h.cur_start && l < h.cur_end.max(h.cur_start + 1) {
                        0
                    } else if l < h.cur_start {
                        h.cur_start - l
                    } else {
                        l - h.cur_end
                    }
                })
                .min()
                .unwrap_or(usize::MAX)
        };
        hunks.sort_by_key(|h| (dist(h), h.cur_start));
        out.push_str(&format!(
            "### {} ({} divergent region(s))\n",
            engs.join(" = "),
            hunks.len()
        ));
        let mut used = 0usize;
        let mut shown = 0usize;
        for h in &hunks {
            let head = format!(
                "@@ current {}-{} | variant {}-{} @@\n",
                h.cur_start,
                h.cur_end.saturating_sub(1).max(h.cur_start),
                h.other_start,
                h.other_end.saturating_sub(1).max(h.other_start)
            );
            if used > 0 && used + head.len() + h.body.len() > per {
                break;
            }
            let mut body = h.body.clone();
            if body.len() > per {
                let mut cut = per;
                while !body.is_char_boundary(cut) {
                    cut -= 1;
                }
                body.truncate(cut);
                body.push_str("\n  [... hunk truncated ...]\n");
            }
            used += head.len() + body.len();
            out.push_str(&head);
            out.push_str(&body);
            shown += 1;
        }
        if shown < hunks.len() {
            out.push_str(&format!("  [... {} more region(s) omitted ...]\n", hunks.len() - shown));
        }
    }
    out
}

// ───────────────────────────── context: signatures ──────────────────────────

/// Binary names (`a/b/C`) of every class in `jar`.
pub fn jar_classes(jar: &Path) -> BTreeSet<String> {
    util::zip_namelist(jar)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| n.strip_suffix(".class").map(|s| s.to_string()))
        .collect()
}

/// In-jar classes `text` (the file `rel`) refers to, most-mentioned first, self
/// and own inner classes excluded.  Literals and comments are ignored.
pub fn referenced_classes(text: &str, rel: &str, jar_classes: &BTreeSet<String>) -> Vec<String> {
    let me = rel.strip_suffix(".java").unwrap_or(rel);
    let my_pkg = me.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let masked = autofix::mask_literals(text);
    let imports: BTreeSet<String> = masked
        .lines()
        .filter_map(|l| l.trim().strip_prefix("import "))
        .map(|l| l.trim_end_matches(';').trim().replace('.', "/"))
        .collect();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for m in IDENT_RE.find_iter(&masked) {
        let tok = m.as_str();
        // dotted fully-qualified reference
        let as_bin = tok.replace('.', "/");
        let mut hit: Option<String> = None;
        if tok.contains('.') && jar_classes.contains(&as_bin) {
            hit = Some(as_bin);
        } else {
            let first = tok.split('.').next().unwrap_or(tok);
            let local = if my_pkg.is_empty() { first.to_string() } else { format!("{my_pkg}/{first}") };
            if jar_classes.contains(&local) {
                hit = Some(local);
            } else if let Some(imp) = imports.iter().find(|i| i.rsplit('/').next() == Some(first)) {
                if jar_classes.contains(imp) {
                    hit = Some(imp.clone());
                }
            }
        }
        if let Some(h) = hit {
            if h != me && !h.starts_with(&format!("{me}$")) {
                *counts.entry(h).or_insert(0) += 1;
            }
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    v.into_iter().map(|(n, _)| n).collect()
}

fn javap(args: &[&str], cp: &str, classes: &[String]) -> Option<String> {
    if classes.is_empty() || !semdiff::javap_available() {
        return None;
    }
    let mut argv: Vec<String> = vec!["javap".into()];
    argv.extend(args.iter().map(|s| s.to_string()));
    argv.push("-cp".into());
    argv.push(cp.to_string());
    argv.extend(classes.iter().map(|c| c.replace('/', ".")));
    let r = util::run_capture(&argv, None);
    if r.spawn_err.is_some() || r.stdout.trim().is_empty() {
        return None;
    }
    Some(r.stdout.replace("Compiled from \"", "// compiled from \"").replace("\r", ""))
}

/// `javap -p` of `classes`, stopping before `budget` characters.
pub fn signatures(cp: &str, classes: &[String], budget: usize) -> String {
    let mut out = String::new();
    for c in classes {
        let Some(s) = javap(&["-p"], cp, std::slice::from_ref(c)) else { continue };
        if !out.is_empty() && out.len() + s.len() > budget {
            out.push_str("// [... further referenced classes omitted ...]\n");
            break;
        }
        out.push_str(&s);
    }
    out
}

/// Bytecode (`javap -c -p`) of the methods of `binary` whose names are in `names`,
/// at most `max_lines` lines in total.
pub fn method_bytecode(cp: &str, binary: &str, names: &BTreeSet<String>, max_lines: usize) -> String {
    let Some(dis) = javap(&["-c", "-p"], cp, &[binary.to_string()]) else { return String::new() };
    let mut out = String::new();
    let mut lines_out = 0usize;
    let mut take = false;
    for line in dis.lines() {
        let t = line.trim_start();
        let indent = line.len() - t.len();
        // member headers sit at indent 2 and end with ';' (javap -c style)
        if indent == 2 && t.ends_with(';') {
            take = names.iter().any(|n| {
                let n = if n == "<init>" { binary.rsplit('/').next().unwrap_or(n) } else { n.as_str() };
                t.contains(&format!(" {n}(")) || t.contains(&format!(".{n}(")) || t.starts_with(&format!("{n}("))
            });
        }
        if take {
            if lines_out >= max_lines {
                out.push_str("    [... bytecode truncated ...]\n");
                break;
            }
            out.push_str(line);
            out.push('\n');
            lines_out += 1;
        }
    }
    out
}

/// Names of the methods whose bodies contain any of `error_lines`.
pub fn methods_with_errors(text: &str, error_lines: &[usize]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (key, s, e) in permethod::method_spans(text) {
        if error_lines.iter().any(|&l| l >= s && l <= e) {
            let name = key.split('(').next().unwrap_or(&key).to_string();
            out.insert(name);
        }
    }
    out
}

// ───────────────────────────── prompt / reply ───────────────────────────────

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n[... truncated ...]\n", &s[..cut])
}

/// One class to repair.
#[derive(Debug, Clone)]
pub struct ClassInput {
    pub rel: String,
    /// Current merged variant.
    pub current: String,
    /// Every engine's variant of this class, config order.
    pub variants: Vec<(String, String)>,
}

/// Build the first user message for `input`, given its current compile result.
pub fn initial_prompt(ws: &Workspace, input: &ClassInput, comp: &Compiled, opts: &Options) -> String {
    let binary = input.rel.strip_suffix(".java").unwrap_or(&input.rel);
    let jar_cp = ws.jar.to_string_lossy().into_owned();
    let all = jar_classes(&ws.jar);
    let mut p = String::new();
    p.push_str(&format!(
        "Repair the decompiled class `{}` (file `{}`) so that it compiles.\n\n",
        binary.replace('/', "."),
        input.rel
    ));
    p.push_str("## javac errors of the current variant\n```\n");
    p.push_str(&truncate_chars(comp.log.trim(), 8_000));
    p.push_str("\n```\n\n## Current variant (best of all decompilers after autofix)\n```java\n");
    p.push_str(&input.current);
    if !input.current.ends_with('\n') {
        p.push('\n');
    }
    p.push_str("```\n\n");
    let diffs = render_variant_diffs(&input.current, &input.variants, &comp.error_lines, opts.diff_budget);
    if !diffs.trim().is_empty() {
        p.push_str(
            "## Other decompilers' variants of this class (only the regions that differ; \
             '-' = current, '+' = that decompiler)\n```diff\n",
        );
        p.push_str(&diffs);
        p.push_str("```\n\n");
    }
    if all.contains(binary) {
        let own = signatures(&jar_cp, &[binary.to_string()], 6_000);
        if !own.is_empty() {
            p.push_str("## Members declared by the ORIGINAL bytecode of this class\n```\n");
            p.push_str(&own);
            p.push_str("```\n\n");
        }
        let names = methods_with_errors(&input.current, &comp.error_lines);
        let bc = method_bytecode(&jar_cp, binary, &names, 400);
        if !bc.trim().is_empty() {
            p.push_str("## Original bytecode of the methods that fail to compile\n```\n");
            p.push_str(&bc);
            p.push_str("```\n\n");
        }
    }
    let refs = referenced_classes(&input.current, &input.rel, &all);
    let sigs = signatures(&jar_cp, &refs[..refs.len().min(30)], opts.signature_budget);
    if !sigs.trim().is_empty() {
        p.push_str("## Signatures of the classes it refers to\n```\n");
        p.push_str(&sigs);
        p.push_str("```\n\n");
    }
    p.push_str("Reply with the complete corrected file in one ```java block.\n");
    p
}

/// Pull the Java compilation unit out of a model reply: the longest fenced block,
/// or the bare reply when it already looks like a Java file.
pub fn extract_java(reply: &str) -> Option<String> {
    let best = FENCE_RE
        .captures_iter(reply)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .max_by_key(|s| s.len());
    let text = match best {
        Some(b) => b,
        None => {
            let t = reply.trim();
            if TYPE_DECL_RE.is_match(t) && t.ends_with('}') {
                t.to_string()
            } else {
                return None;
            }
        }
    };
    let mut text = text.trim_matches('\n').to_string();
    text.push('\n');
    Some(text)
}

fn package_of(text: &str) -> String {
    let masked = autofix::mask_literals(text);
    PACKAGE_RE.captures(&masked).map(|c| c[1].to_string()).unwrap_or_default()
}

/// Type names declared in `text` (masked, so literals/comments cannot fake one).
fn declared_types(text: &str) -> BTreeSet<String> {
    let masked = autofix::mask_literals(text);
    TYPE_DECL_RE.captures_iter(&masked).map(|c| c[1].to_string()).collect()
}

/// Does `cand` still declare the package and the file's top-level type?
pub fn same_unit(rel: &str, current: &str, cand: &str) -> Result<(), String> {
    if package_of(current) != package_of(cand) {
        return Err(format!(
            "package changed ('{}' -> '{}')",
            package_of(current),
            package_of(cand)
        ));
    }
    let stem = Path::new(rel).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if !declared_types(cand).contains(&stem) {
        return Err(format!("top-level type `{stem}` is no longer declared"));
    }
    Ok(())
}

// ───────────────────────────── semantic gate ────────────────────────────────

/// Outcome of comparing a compiled candidate with the original bytecode.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Gate {
    /// Every method's normalised reference set matches the original.
    Passed { classes_checked: usize },
    /// Some methods drifted; human-readable descriptions.
    Drift { details: Vec<String> },
    /// No javap, class not in the jar, or the gate is disabled.
    Unavailable { why: String },
}

/// Compare every class the original jar has for `rel` (top-level and `$` inner
/// classes) with the ones compiled into `classes`.
pub fn semantic_gate(ws: &Workspace, rel: &str, classes: &Path, enabled: bool) -> Gate {
    if !enabled {
        return Gate::Unavailable { why: "disabled (--no-semantic-gate)".into() };
    }
    if !semdiff::javap_available() {
        return Gate::Unavailable { why: "javap not found".into() };
    }
    let binary = rel.strip_suffix(".java").unwrap_or(rel);
    let all = jar_classes(&ws.jar);
    if !all.contains(binary) {
        return Gate::Unavailable { why: "class not present in the jar".into() };
    }
    let rev = semdiff::load_reverse_map(ws.mapping.as_deref());
    let orig_cp = ws.jar.to_string_lossy().into_owned();
    let cand_cp = classes.to_string_lossy().into_owned();
    let mut details = Vec::new();
    let mut checked = 0usize;
    let prefix = format!("{binary}$");
    for bn in all.iter().filter(|b| b.as_str() == binary || b.starts_with(&prefix)) {
        if !classes.join(format!("{bn}.class")).exists() {
            details.push(format!("class {bn}: not produced by the candidate"));
            continue;
        }
        let Some(a) = semdiff::disassemble(&orig_cp, bn, &rev) else {
            return Gate::Unavailable { why: format!("javap failed on original {bn}") };
        };
        let Some(b) = semdiff::disassemble(&cand_cp, bn, &rev) else {
            details.push(format!("class {bn}: javap failed on the candidate"));
            continue;
        };
        checked += 1;
        for f in semdiff::compare_maps(bn, &a, &b).flagged {
            let mut d = format!("{bn}: {} — {}", f.method, f.reason);
            match (&f.only_orig, &f.only_recompiled) {
                (Some(semdiff::OnlyOrig::Refs(o)), Some(r)) => {
                    if !o.is_empty() {
                        d.push_str(&format!("; missing vs original: {}", o.join(", ")));
                    }
                    if !r.is_empty() {
                        d.push_str(&format!("; not in original: {}", r.join(", ")));
                    }
                }
                (Some(semdiff::OnlyOrig::Side(true)), _) => d.push_str(" (only in the original)"),
                (Some(semdiff::OnlyOrig::Side(false)), _) => d.push_str(" (only in the candidate)"),
                _ => {}
            }
            details.push(d);
        }
    }
    if details.is_empty() {
        Gate::Passed { classes_checked: checked }
    } else {
        Gate::Drift { details }
    }
}

// ───────────────────────────── repair loop ──────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Attempt {
    pub attempt: usize,
    /// Saved candidate (relative to the work dir), if the reply held one.
    pub candidate: Option<String>,
    /// javac errors of the candidate (None when no candidate was extracted).
    pub errors: Option<i64>,
    pub gate: Option<Gate>,
    /// accepted | compile_errors | drift | not_same_unit | no_code | model_error
    pub outcome: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassLog {
    pub class: String,
    pub errors_before: i64,
    pub errors_after: i64,
    /// accepted | rejected | dry_run | reverted_tree_regression
    pub status: String,
    pub model: String,
    pub prompt_chars: usize,
    pub attempts: Vec<Attempt>,
}

fn feedback_errors(comp: &Compiled) -> String {
    format!(
        "Your version still does not compile ({} error(s)):\n```\n{}\n```\n\
         Fix these and reply with the complete corrected file in one ```java block.",
        comp.errors,
        truncate_chars(comp.log.trim(), 6_000)
    )
}

fn feedback_drift(details: &[String]) -> String {
    let mut list = details.iter().take(40).cloned().collect::<Vec<_>>().join("\n");
    if details.len() > 40 {
        list.push_str(&format!("\n... and {} more", details.len() - 40));
    }
    format!(
        "Your version compiles, but its bytecode no longer matches the original \
         program — behaviour changed in these methods (normalised call/field/new \
         references compared with the original bytecode):\n```\n{list}\n```\n\
         Restore the original behaviour (do not drop or add calls, field accesses or \
         object creations) and reply with the complete corrected file in one ```java block."
    )
}

fn rel_to_flat(rel: &str) -> String {
    rel.replace(['/', '\\'], "__")
}

/// Repair one class.  Returns the journal entry and, when accepted, the new text.
pub fn repair_class(
    model: &dyn ChatModel,
    ws: &Workspace,
    input: &ClassInput,
    opts: &Options,
) -> (ClassLog, Option<String>) {
    let api_cp = ws.api_classpath();
    let scratch = ws.work.join("_scratch");
    let before = compile_one(&input.current, &input.rel, &api_cp, &scratch.join("before"));
    let mut log = ClassLog {
        class: input.rel.clone(),
        errors_before: before.errors,
        errors_after: before.errors,
        status: "rejected".into(),
        model: model.id(),
        prompt_chars: 0,
        attempts: Vec::new(),
    };
    if before.errors == 0 {
        log.status = "already_compiles".into();
        return (log, None);
    }
    let first = initial_prompt(ws, input, &before, opts);
    log.prompt_chars = first.len();
    let flat = rel_to_flat(&input.rel);
    let prompts_dir = ws.work.join("prompts");
    let _ = std::fs::create_dir_all(&prompts_dir);
    let _ = std::fs::write(prompts_dir.join(format!("{flat}.prompt.md")), &first);
    if opts.dry_run {
        log.status = "dry_run".into();
        return (log, None);
    }

    let attempts_dir = ws.work.join("attempts");
    let _ = std::fs::create_dir_all(&attempts_dir);
    let mut last_reply: Option<String> = None;
    let mut last_feedback: Option<String> = None;
    for k in 1..=opts.max_attempts.max(1) {
        let mut messages = vec![ChatMessage::user(first.clone())];
        if let (Some(r), Some(f)) = (&last_reply, &last_feedback) {
            messages.push(ChatMessage::assistant(r.clone()));
            messages.push(ChatMessage::user(f.clone()));
        }
        let req = ChatRequest { system: SYSTEM_PROMPT.into(), messages, max_tokens: opts.max_tokens };
        let reply = match model.chat(&req) {
            Ok(r) => r,
            Err(e) => {
                log.attempts.push(Attempt {
                    attempt: k,
                    candidate: None,
                    errors: None,
                    gate: None,
                    outcome: "model_error".into(),
                    note: e,
                });
                // a transport/key failure will not heal by itself; stop this class
                break;
            }
        };
        let Some(cand) = extract_java(&reply) else {
            log.attempts.push(Attempt {
                attempt: k,
                candidate: None,
                errors: None,
                gate: None,
                outcome: "no_code".into(),
                note: "reply holds no Java code block".into(),
            });
            last_feedback = Some(
                "Your reply contained no Java code. Reply with the complete corrected \
                 file in one ```java block."
                    .into(),
            );
            last_reply = Some(reply);
            continue;
        };
        let cand_name = format!("{flat}.attempt{k}.java");
        let _ = std::fs::write(attempts_dir.join(&cand_name), &cand);
        let cand_rel = Some(format!("attempts/{cand_name}"));

        if let Err(why) = same_unit(&input.rel, &input.current, &cand) {
            log.attempts.push(Attempt {
                attempt: k,
                candidate: cand_rel,
                errors: None,
                gate: None,
                outcome: "not_same_unit".into(),
                note: why.clone(),
            });
            last_feedback = Some(format!(
                "Your version is a different compilation unit: {why}. Keep the package \
                 and the type of file `{}`; reply with the complete corrected file in one \
                 ```java block.",
                input.rel
            ));
            last_reply = Some(reply);
            continue;
        }

        let comp = compile_one(&cand, &input.rel, &api_cp, &scratch.join(format!("try{k}")));
        if comp.errors > 0 {
            log.attempts.push(Attempt {
                attempt: k,
                candidate: cand_rel,
                errors: Some(comp.errors),
                gate: None,
                outcome: "compile_errors".into(),
                note: String::new(),
            });
            last_feedback = Some(feedback_errors(&comp));
            last_reply = Some(reply);
            continue;
        }
        let gate = semantic_gate(ws, &input.rel, &comp.classes, opts.semantic_gate);
        if let Gate::Drift { details } = &gate {
            last_feedback = Some(feedback_drift(details));
            log.attempts.push(Attempt {
                attempt: k,
                candidate: cand_rel,
                errors: Some(0),
                gate: Some(gate.clone()),
                outcome: "drift".into(),
                note: String::new(),
            });
            last_reply = Some(reply);
            continue;
        }
        log.attempts.push(Attempt {
            attempt: k,
            candidate: cand_rel,
            errors: Some(0),
            gate: Some(gate),
            outcome: "accepted".into(),
            note: String::new(),
        });
        log.status = "accepted".into();
        log.errors_after = 0;
        let _ = std::fs::remove_dir_all(&scratch);
        return (log, Some(cand));
    }
    let _ = std::fs::remove_dir_all(&scratch);
    (log, None)
}

// ───────────────────────────── whole run ────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub model: String,
    pub dry_run: bool,
    pub semantic_gate: bool,
    pub max_attempts: usize,
    /// classes failing (isolated) before / after
    pub failing_before: usize,
    pub failing_after: usize,
    /// sum of isolated javac errors over the attempted classes
    pub isolated_errors_before: i64,
    pub isolated_errors_after: i64,
    /// javac errors of the whole merged tree, before / after
    pub tree_errors_before: i64,
    pub tree_errors_after: i64,
    pub accepted: usize,
    pub rejected: usize,
    /// model replies consumed (cache hits included)
    pub attempts_total: usize,
    pub classes: Vec<ClassLog>,
}

/// Classes of `merged` with isolated errors, restricted to `candidates` when given.
pub fn failing_classes(ws: &Workspace, candidates: Option<&[String]>) -> Vec<(String, i64)> {
    let api_cp = ws.api_classpath();
    let rels: Vec<String> = match candidates {
        Some(c) => c.to_vec(),
        None => core::collect_java(&ws.merged).into_keys().filter(|r| !r.starts_with('_')).collect(),
    };
    let mut out = Vec::new();
    for rel in rels {
        let f = ws.merged.join(&rel);
        let Ok(text) = util::read_text_lossy(&f) else { continue };
        let c = compile_one(&text, &rel, &api_cp, &ws.work.join("_scan"));
        if c.errors > 0 {
            out.push((rel, c.errors));
        }
    }
    let _ = std::fs::remove_dir_all(ws.work.join("_scan"));
    out
}

/// Classes the merge log of `report.json` marked as still failing.
pub fn failing_from_report(report: &serde_json::Value) -> Option<Vec<String>> {
    let log = report.get("merge_log")?.as_array()?;
    let re = Regex::new(r"^\s*(\S+\.java): .*isolated errors=([1-9]\d*)").unwrap();
    let mut v: Vec<String> = log
        .iter()
        .filter_map(|l| l.as_str())
        .filter_map(|l| re.captures(l).map(|c| c[1].to_string()))
        .collect();
    v.sort();
    v.dedup();
    Some(v)
}

/// Run the stage over `ws.merged`.  `candidates` narrows the scan (e.g. the merge
/// log's failing classes); None scans every merged class.
pub fn run(model: &dyn ChatModel, ws: &Workspace, candidates: Option<&[String]>, opts: &Options) -> Summary {
    let _ = std::fs::create_dir_all(&ws.work);
    let mut failing = failing_classes(ws, candidates);
    if !opts.only.is_empty() {
        failing.retain(|(r, _)| opts.only.iter().any(|o| o == r));
    }
    if let Some(n) = opts.max_classes {
        failing.truncate(n);
    }
    let failing_before_all = failing.len();
    let (tree_before, _, _) = core::compile_dir(&ws.merged, ws.classpath.as_deref());

    let backup = ws.work.join("before");
    let mut logs: Vec<ClassLog> = Vec::new();
    let mut accepted_texts: Vec<(String, String)> = Vec::new(); // (rel, previous text)
    for (rel, errs) in &failing {
        let path = ws.merged.join(rel);
        let current = util::read_text_lossy(&path).unwrap_or_default();
        let mut variants = Vec::new();
        for (eng, dir) in &ws.engines {
            let f = dir.join(rel);
            if f.exists() {
                if let Ok(t) = util::read_text_lossy(&f) {
                    variants.push((eng.clone(), t));
                }
            }
        }
        println!("  {rel}: {errs} error(s) — asking {}", if opts.dry_run { "nobody (dry run)".to_string() } else { model.id() });
        let input = ClassInput { rel: rel.clone(), current: current.clone(), variants };
        let (log, fixed) = repair_class(model, ws, &input, opts);
        if let Some(text) = fixed {
            let b = backup.join(rel);
            if let Some(p) = b.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(&b, &current);
            let _ = std::fs::write(&path, &text);
            accepted_texts.push((rel.clone(), current));
        }
        println!(
            "    -> {} after {} attempt(s)",
            log.status,
            log.attempts.len()
        );
        logs.push(log);
    }

    // tree-level guard: substitutions must not raise the whole-tree error count
    let (mut tree_after, _, _) = core::compile_dir(&ws.merged, ws.classpath.as_deref());
    if tree_after > tree_before {
        for (rel, prev) in &accepted_texts {
            let path = ws.merged.join(rel);
            let fixed = util::read_text_lossy(&path).unwrap_or_default();
            let _ = std::fs::write(&path, prev);
            let (t, _, _) = core::compile_dir(&ws.merged, ws.classpath.as_deref());
            if t < tree_after {
                tree_after = t;
                if let Some(l) = logs.iter_mut().find(|l| &l.class == rel) {
                    l.status = "reverted_tree_regression".into();
                    l.errors_after = l.errors_before;
                }
                println!("  {rel}: reverted (whole-tree errors would rise)");
            } else {
                let _ = std::fs::write(&path, fixed);
            }
            if tree_after <= tree_before {
                break;
            }
        }
    }

    let accepted = logs.iter().filter(|l| l.status == "accepted").count();
    let summary = Summary {
        model: model.id(),
        dry_run: opts.dry_run,
        semantic_gate: opts.semantic_gate && semdiff::javap_available(),
        max_attempts: opts.max_attempts,
        failing_before: failing_before_all,
        failing_after: failing_before_all - accepted,
        isolated_errors_before: logs.iter().map(|l| l.errors_before).sum(),
        isolated_errors_after: logs.iter().map(|l| l.errors_after).sum(),
        tree_errors_before: tree_before,
        tree_errors_after: tree_after,
        accepted,
        rejected: logs.iter().filter(|l| l.status == "rejected").count(),
        attempts_total: logs.iter().map(|l| l.attempts.len()).sum(),
        classes: logs,
    };
    write_journal(ws, &summary);
    summary
}

fn write_journal(ws: &Workspace, s: &Summary) {
    let _ = std::fs::write(
        ws.work.join("summary.json"),
        serde_json::to_string_pretty(s).unwrap_or_default(),
    );
    let jl: Vec<String> = s
        .classes
        .iter()
        .map(|c| serde_json::to_string(c).unwrap_or_default())
        .collect();
    // the journal accumulates across runs; summary.json is the latest run only
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ws.work.join("journal.jsonl"))
    {
        for l in jl {
            let _ = writeln!(f, "{l}");
        }
    }
    let mut md = vec![
        "# ai-fix".to_string(),
        String::new(),
        format!("- model: `{}`{}", s.model, if s.dry_run { " (dry run — nothing sent)" } else { "" }),
        format!("- semantic gate: {}", if s.semantic_gate { "on" } else { "off / unavailable" }),
        format!("- failing classes: {} -> {}", s.failing_before, s.failing_after),
        format!("- isolated errors (attempted classes): {} -> {}", s.isolated_errors_before, s.isolated_errors_after),
        format!("- whole-tree errors: {} -> {}", s.tree_errors_before, s.tree_errors_after),
        format!("- accepted {}, rejected {}, attempts {}", s.accepted, s.rejected, s.attempts_total),
        String::new(),
        "| class | before | after | status | attempts |".into(),
        "|---|---|---|---|---|".into(),
    ];
    for c in &s.classes {
        let trail: Vec<String> = c
            .attempts
            .iter()
            .map(|a| match a.errors {
                Some(e) => format!("{}:{}({e})", a.attempt, a.outcome),
                None => format!("{}:{}", a.attempt, a.outcome),
            })
            .collect();
        md.push(format!(
            "| {} | {} | {} | {} | {} |",
            c.class,
            c.errors_before,
            c.errors_after,
            c.status,
            trail.join(" ")
        ));
    }
    let _ = std::fs::write(ws.work.join("report.md"), md.join("\n") + "\n");
}

// ───────────────────────────── CLI: `arlecchino ai-fix` ─────────────────────

/// `arlecchino ai-fix [OUT]` — runs over the output directory of a previous
/// `decompile` (its `merged/`, engine trees and `report.json`).
#[derive(clap::Args, Debug)]
pub struct AiFixArgs {
    /// output directory of a previous `arlecchino decompile`
    #[arg(default_value = "arlecchino_out")]
    pub out: String,
    /// dependency classpath used by the decompile run (e.g. a MIDP stub)
    #[arg(long = "classpath")]
    pub classpath: Option<String>,
    /// path to a MIDP/CLDC stub jar (default: auto-find, as `decompile` does)
    #[arg(long = "midp-stub")]
    pub midp_stub: Option<String>,
    #[arg(long = "no-midp-stub")]
    pub no_midp_stub: bool,
    /// engines config used by the decompile run (to locate each engine's tree)
    #[arg(long = "config")]
    pub config: Option<String>,
    /// model calls per class before giving up
    #[arg(long = "attempts", default_value_t = 3)]
    pub attempts: usize,
    /// repair at most this many classes
    #[arg(long = "max-classes")]
    pub max_classes: Option<usize>,
    /// only this class (relative path such as `a/B.java`); repeatable
    #[arg(long = "class")]
    pub class: Vec<String>,
    /// accept a compiling fix even when its bytecode drifts from the original
    #[arg(long = "no-semantic-gate")]
    pub no_semantic_gate: bool,
    /// build and save the prompts (ai_fix/prompts/), send nothing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
    /// do not ask before sending code to a remote endpoint
    #[arg(long = "yes", short = 'y')]
    pub yes: bool,
    /// anthropic | openai (any OpenAI-compatible endpoint, e.g. a local server) |
    /// claude-code (an installed, logged-in Claude Code CLI)
    #[arg(long = "provider")]
    pub provider: Option<String>,
    #[arg(long = "model")]
    pub model: Option<String>,
    #[arg(long = "base-url")]
    pub base_url: Option<String>,
    /// name of the environment variable that holds the API key
    #[arg(long = "api-key-env")]
    pub api_key_env: Option<String>,
    /// TOML file with an `[ai]` table (provider, model, base_url, ...)
    #[arg(long = "ai-config")]
    pub ai_config: Option<String>,
    #[arg(long = "effort")]
    pub effort: Option<String>,
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u32>,
    /// response cache directory (default: OUT/ai_fix/cache)
    #[arg(long = "cache-dir")]
    pub cache_dir: Option<String>,
    #[arg(long = "no-cache")]
    pub no_cache: bool,
}

fn confirm_remote(base_url: &str) -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("Send decompiled code to {base_url}? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

pub fn cmd_ai_fix(a: &AiFixArgs) -> i32 {
    use crate::ai::provider::{CachedModel, ProviderConfig, ProviderOverrides};

    let out = util::absolutize(Path::new(&a.out));
    let merged = out.join("merged");
    if !merged.is_dir() {
        println!("ai-fix: {} has no merged/ tree — run `arlecchino decompile` first", out.display());
        return 2;
    }
    let report: serde_json::Value = util::read_text_lossy(&out.join("report.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null);
    let jar = match report.get("jar").and_then(|v| v.as_str()) {
        Some(j) if Path::new(j).exists() => PathBuf::from(j),
        _ => {
            println!("ai-fix: report.json in {} names no existing jar", out.display());
            return 2;
        }
    };
    let mapping = report
        .pointer("/member_rename/mapping")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .filter(|p| p.exists());

    let (engines, _, root) = match crate::config::load_engines(a.config.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            println!("config error: {e}");
            return 2;
        }
    };
    let mut classpath = a.classpath.clone();
    if classpath.is_none() && !a.no_midp_stub {
        classpath = crate::emulate::auto_api_classpath(a.midp_stub.as_deref(), &root);
    }
    let engine_dirs: Vec<(String, PathBuf)> = engines
        .iter()
        .filter(|e| e.enabled && e.type_ == "names")
        .map(|e| (e.name.clone(), e.sources_dir(&out.join(&e.name))))
        .filter(|(_, d)| d.is_dir())
        .collect();

    let cfg = match ProviderConfig::resolve(&ProviderOverrides {
        provider: a.provider.clone(),
        model: a.model.clone(),
        base_url: a.base_url.clone(),
        api_key_env: a.api_key_env.clone(),
        max_tokens: a.max_tokens,
        effort: a.effort.clone(),
        config: a.ai_config.as_ref().map(PathBuf::from),
    }) {
        Ok(c) => c,
        Err(e) => {
            println!("ai-fix: {e}");
            return 2;
        }
    };

    println!("== Arlecchino ai-fix :: {} ==", out.display());
    println!("   jar: {}", jar.display());
    println!("   engines: {}", engine_dirs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "));
    println!("   model: {} @ {}", cfg.id(), cfg.endpoint());
    if !a.dry_run && cfg.is_remote() {
        println!(
            "\n   WARNING: ai-fix sends the source of every failing class, its javac errors,\n   \
             the other decompilers' variants and bytecode/signature excerpts of the jar to\n   \
             {} — a third-party service. Use --dry-run to inspect the prompts first,\n   \
             or --provider openai --base-url http://localhost:... to keep code local.\n",
            cfg.endpoint()
        );
        if !a.yes && !confirm_remote(&cfg.endpoint()) {
            println!("ai-fix: not confirmed (pass --yes to send without asking) — nothing sent");
            return 1;
        }
    }

    let ws = Workspace {
        merged,
        jar,
        classpath,
        mapping,
        engines: engine_dirs,
        work: out.join("ai_fix"),
    };
    let opts = Options {
        max_attempts: a.attempts.max(1),
        max_classes: a.max_classes,
        only: a.class.clone(),
        semantic_gate: !a.no_semantic_gate,
        dry_run: a.dry_run,
        max_tokens: cfg.max_tokens,
        ..Options::default()
    };
    let candidates = failing_from_report(&report);
    let cache_dir = a
        .cache_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| ws.work.join("cache"));

    let summary = if a.dry_run {
        let offline = CachedModel::offline(&cfg.id(), &cache_dir);
        run(&offline, &ws, candidates.as_deref(), &opts)
    } else {
        let live = match cfg.build() {
            Ok(m) => m,
            Err(e) => {
                println!("ai-fix: {e}");
                return 2;
            }
        };
        if a.no_cache {
            run(&live, &ws, candidates.as_deref(), &opts)
        } else {
            let cached = CachedModel::new(&live, &cache_dir);
            run(&cached, &ws, candidates.as_deref(), &opts)
        }
    };

    println!("\n== ai-fix RESULT ==");
    println!("   failing classes: {} -> {}", summary.failing_before, summary.failing_after);
    println!(
        "   isolated errors: {} -> {}   whole-tree errors: {} -> {}",
        summary.isolated_errors_before,
        summary.isolated_errors_after,
        summary.tree_errors_before,
        summary.tree_errors_after
    );
    println!(
        "   accepted {}, rejected {}, attempts {}",
        summary.accepted, summary.rejected, summary.attempts_total
    );
    if a.dry_run {
        println!("   dry run: prompts in {}", ws.work.join("prompts").display());
    }
    println!("   journal: {}", ws.work.join("report.md").display());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_prefers_longest_fence() {
        let r = "Here:\n```java\nclass A {}\n```\nand\n```java\nclass A { int x; }\n```\n";
        assert_eq!(extract_java(r).unwrap(), "class A { int x; }\n");
        assert_eq!(extract_java("class B { }").unwrap(), "class B { }\n");
        assert!(extract_java("sorry, I cannot").is_none());
    }

    #[test]
    fn hunks_cover_only_the_divergence() {
        let a = "l1\nl2\nl3\nOLD\nl5\nl6\nl7\n";
        let b = "l1\nl2\nl3\nNEW\nl5\nl6\nl7\n";
        let h = divergent_hunks(a, b, 1);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].body, "  l3\n- OLD\n+ NEW\n  l5\n");
        assert_eq!(h[0].cur_start, 3);
        assert!(divergent_hunks(a, a, 2).is_empty());
    }

    #[test]
    fn identical_variants_are_folded() {
        let cur = "x\ny\n";
        let v = vec![
            ("cfr".to_string(), "x\ny\n".to_string()),
            ("jadx".to_string(), "x\nz\n".to_string()),
            ("procyon".to_string(), "x\nz\n".to_string()),
        ];
        let r = render_variant_diffs(cur, &v, &[2], 10_000);
        assert!(r.contains("identical to the current variant: cfr"));
        assert!(r.contains("### jadx = procyon"));
    }

    #[test]
    fn same_unit_rejects_renamed_type_and_package() {
        let cur = "package p;\nclass Foo {}\n";
        assert!(same_unit("p/Foo.java", cur, "package p;\nclass Foo { int a; }\n").is_ok());
        assert!(same_unit("p/Foo.java", cur, "package q;\nclass Foo {}\n").is_err());
        assert!(same_unit("p/Foo.java", cur, "package p;\nclass Bar {}\n// class Foo\n").is_err());
    }

    #[test]
    fn references_resolve_default_and_named_packages() {
        let jar: BTreeSet<String> =
            ["a", "b", "c", "pk/Util", "pk/Me", "pk/Me$1"].iter().map(|s| s.to_string()).collect();
        let text = "package pk;\nclass Me { a f; Util u; String s = \"b\"; Me$1 x; }\n";
        let r = referenced_classes(text, "pk/Me.java", &jar);
        // `a` is a default-package class, not visible from pk by simple name
        assert_eq!(r, vec!["pk/Util".to_string()]);
        let text2 = "class c { a x; a y; b z; }\n";
        assert_eq!(referenced_classes(text2, "c.java", &jar), vec!["a".to_string(), "b".to_string()]);
    }
}
