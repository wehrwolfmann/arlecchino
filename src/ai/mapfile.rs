//! Name maps as Java-like `.map` declarations — the format of the Sporeflower
//! decompiler (`docs/mappings.md` there), so one file serves both tools:
//!
//! ```java
//! import javax.microedition.lcdui.Graphics;
//!
//! class GameCanvas /* was d */ {
//!     GameCanvas(Midlet owner) {}
//!     int score /* was a */;
//!     void drawHud(Graphics graphics, int x, int y) /* was b */; /* locals: v4=row, v5=label */
//! }
//! ```
//!
//! What Arlecchino reads from it: class, field and method names (`/* was … */` gives
//! the bytecode name), parameter names (by position), and local variable names in a
//! `/* locals: … */` comment after the method — a comment, so Sporeflower ignores it.
//! Locals are keyed by LABEL: `v<slot>` for the first type a JVM local slot holds,
//! `v<slot>b`, `v<slot>c` … for the others; the applier writes those labels into the
//! bytecode, so a labelled decompile shows exactly the keys to use.  A parameter or
//! local still called by its label (`v4`) counts as unnamed.
//!
//! Semantic annotations (`@Domain`, `@Flags`, domain interfaces, `@External` …) are
//! kept verbatim and counted; Sporeflower applies them, the bytecode applier cannot.
//!
//! Writing never rewrites what a person typed.  [`patch`] only
//! * fills names that are still unnamed (a member whose name equals its `was`, a
//!   parameter or local still called by its label),
//! * inserts declarations for symbols the files do not list yet,
//! * renames type references to a class it has just named,
//! and leaves every other byte alone.

use super::names::{self, Kind, NameEntry, Plan, World};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ── lexer ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tk {
    Ident(String),
    Punct(char),
    Was(String),
    Locals(String),
    Lit,
}

#[derive(Debug, Clone)]
struct Tok {
    t: Tk,
    start: usize,
    end: usize,
    line: usize,
}

fn lex(text: &str) -> Vec<Tok> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let start = i;
            let body_start = i + 2;
            let end = text[body_start..].find("*/").map(|k| body_start + k).unwrap_or(b.len());
            let body = text[body_start..end].trim();
            let l0 = line;
            line += text[start..end].matches('\n').count();
            i = (end + 2).min(b.len());
            if let Some(rest) = body.strip_prefix("was ") {
                out.push(Tok { t: Tk::Was(rest.trim().to_string()), start, end: i, line: l0 });
            } else if let Some(rest) = body.strip_prefix("locals:") {
                out.push(Tok { t: Tk::Locals(rest.trim().to_string()), start, end: i, line: l0 });
            }
            continue;
        }
        if c == b'"' || c == b'\'' {
            let start = i;
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            out.push(Tok { t: Tk::Lit, start, end: i, line });
            continue;
        }
        if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).map(|d| d.is_ascii_digit()).unwrap_or(false)) {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            out.push(Tok { t: Tk::Lit, start, end: i, line });
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            out.push(Tok { t: Tk::Ident(text[start..i].to_string()), start, end: i, line });
            continue;
        }
        out.push(Tok { t: Tk::Punct(c as char), start: i, end: i + 1, line });
        i += 1;
    }
    out
}

// ── the parsed model ─────────────────────────────────────────────────────────

/// A name token in the file: what it says and where.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    /// dotted base name as written (`Graphics`, `java.util.Vector`, `int`)
    pub base: Span,
    pub dims: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub ty: TypeRef,
    pub name: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MemberDecl {
    Field { ty: TypeRef, name: Span, was: Option<String>, line: usize },
    Method { ret: TypeRef, name: Span, params: Vec<Param>, was: Option<String>, locals: Vec<(String, Span)>, end: usize, line: usize },
    Ctor { params: Vec<Param>, locals: Vec<(String, Span)>, end: usize, line: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    pub name: Span,
    pub was: Option<String>,
    pub package: String,
    pub already_mapped: bool,
    pub members: Vec<MemberDecl>,
    /// byte offset of the closing `}`
    pub close: usize,
    pub line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct MapFile {
    pub path: PathBuf,
    pub text: String,
    pub package: String,
    /// simple name -> dotted name
    pub imports: BTreeMap<String, String>,
    pub classes: Vec<ClassDecl>,
    /// semantic annotations and domain/external declarations seen (kept verbatim)
    pub semantic: usize,
    /// every type-name token (for renaming references to a class)
    pub type_spans: Vec<Span>,
    pub errors: Vec<String>,
}

const MODIFIERS: &[&str] = &["public", "private", "protected", "static", "final", "abstract", "native", "synchronized", "transient", "volatile", "strictfp", "default"];
const SEMANTIC: &[&str] = &[
    "Domain", "Flags", "DomainValue", "DomainFromParameter", "DomainFromSlot", "DomainWhen", "CallDomain", "IndexDomain", "Slots", "Records", "Planes",
    "SlotValue", "BitField", "Elements", "Keys", "Values", "ClassName", "ValueDomain", "FlagDomain", "SlotDomain", "PackedDomain", "NumericDomain",
    "StringDomain", "External",
];

struct P<'a> {
    t: &'a [Tok],
    i: usize,
    f: &'a mut MapFile,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&'a Tk> {
        self.t.get(self.i).map(|t| &t.t)
    }
    fn tok(&self) -> Option<&'a Tok> {
        self.t.get(self.i)
    }
    fn is_p(&self, c: char) -> bool {
        self.peek() == Some(&Tk::Punct(c))
    }
    fn ident(&self) -> Option<&'a str> {
        match self.peek() {
            Some(Tk::Ident(s)) => Some(s.as_str()),
            _ => None,
        }
    }
    fn line(&self) -> usize {
        self.tok().map(|t| t.line).unwrap_or(0)
    }
    fn err(&mut self, msg: &str) {
        let l = self.line();
        let p = self.f.path.display().to_string();
        self.f.errors.push(format!("{p}:{l}: {msg}"));
    }
    /// skip a balanced group starting at the current opening bracket
    fn skip_group(&mut self, open: char, close: char) {
        let mut depth = 0;
        while let Some(t) = self.peek() {
            if *t == Tk::Punct(open) {
                depth += 1;
            } else if *t == Tk::Punct(close) {
                depth -= 1;
                if depth == 0 {
                    self.i += 1;
                    return;
                }
            }
            self.i += 1;
        }
    }
    /// skip to just past the next `;` at this level (or stop before a `}`)
    fn skip_statement(&mut self) {
        while let Some(t) = self.peek() {
            match t {
                Tk::Punct(';') => {
                    self.i += 1;
                    return;
                }
                Tk::Punct('}') => return,
                Tk::Punct('{') => {
                    self.skip_group('{', '}');
                    return;
                }
                Tk::Punct('(') => self.skip_group('(', ')'),
                _ => self.i += 1,
            }
        }
    }
    /// annotations -> (names)
    fn annotations(&mut self) -> Vec<String> {
        let mut v = Vec::new();
        while self.is_p('@') {
            self.i += 1;
            let mut name = String::new();
            while let Some(s) = self.ident() {
                name = s.to_string();
                self.i += 1;
                if self.is_p('.') {
                    self.i += 1;
                } else {
                    break;
                }
            }
            if name == "interface" {
                // `@interface` is not a map construct; treat as a type keyword
                self.i -= 1;
                break;
            }
            if self.is_p('(') {
                self.skip_group('(', ')');
            }
            v.push(name);
        }
        v
    }
    fn modifiers(&mut self) {
        while let Some(s) = self.ident() {
            if MODIFIERS.contains(&s) {
                self.i += 1;
            } else {
                break;
            }
        }
    }
    fn type_ref(&mut self) -> Option<TypeRef> {
        let first = self.tok()?;
        let Tk::Ident(_) = &first.t else { return None };
        let start = first.start;
        let mut end = first.end;
        self.i += 1;
        while self.is_p('.') {
            if let Some(Tk::Ident(_)) = self.t.get(self.i + 1).map(|t| &t.t) {
                end = self.t[self.i + 1].end;
                self.i += 2;
            } else {
                break;
            }
        }
        let mut dims = 0;
        while self.is_p('[') && self.t.get(self.i + 1).map(|t| t.t == Tk::Punct(']')).unwrap_or(false) {
            dims += 1;
            self.i += 2;
        }
        let text = self.f.text[start..end].split_whitespace().collect::<String>();
        let span = Span { text, start, end };
        self.f.type_spans.push(span.clone());
        Some(TypeRef { base: span, dims })
    }
    fn name_span(&mut self) -> Option<Span> {
        let t = self.tok()?;
        let Tk::Ident(s) = &t.t else { return None };
        self.i += 1;
        Some(Span { text: s.clone(), start: t.start, end: t.end })
    }
    fn params(&mut self) -> Vec<Param> {
        let mut v = Vec::new();
        if !self.is_p('(') {
            return v;
        }
        self.i += 1;
        loop {
            if self.is_p(')') {
                self.i += 1;
                break;
            }
            let ann = self.annotations();
            self.f.semantic += ann.iter().filter(|a| SEMANTIC.contains(&a.as_str())).count();
            self.modifiers();
            let Some(ty) = self.type_ref() else {
                self.err("expected a parameter type");
                self.skip_group_rest(')');
                break;
            };
            let Some(name) = self.name_span() else {
                self.err("every parameter needs a name");
                self.skip_group_rest(')');
                break;
            };
            v.push(Param { ty, name });
            if self.is_p(',') {
                self.i += 1;
            }
        }
        v
    }
    fn skip_group_rest(&mut self, close: char) {
        while let Some(t) = self.peek() {
            self.i += 1;
            if *t == Tk::Punct(close) {
                return;
            }
        }
    }
    fn was(&mut self) -> Option<String> {
        if let Some(Tk::Was(w)) = self.peek() {
            self.i += 1;
            return Some(w.clone());
        }
        None
    }
    fn locals(&mut self, out: &mut Vec<(String, Span)>, end: &mut usize) {
        while let Some(t) = self.tok() {
            let Tk::Locals(body) = &t.t else { break };
            // body offsets: locate each `label=name` inside the comment text
            let comment = &self.f.text[t.start..t.end];
            let body_off = t.start + comment.find("locals:").map(|k| k + "locals:".len()).unwrap_or(0);
            let raw = &self.f.text[body_off..t.end.saturating_sub(2)];
            let mut pos = 0;
            for part in raw.split(',') {
                let part_start = body_off + pos;
                pos += part.len() + 1;
                let trimmed = part.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let lead = part.len() - part.trim_start().len();
                let (label, name, name_start) = match trimmed.split_once('=') {
                    Some((l, n)) => {
                        let l = l.trim();
                        let n_raw_off = trimmed.find('=').unwrap() + 1;
                        let n_lead = trimmed[n_raw_off..].len() - trimmed[n_raw_off..].trim_start().len();
                        (l.to_string(), n.trim().to_string(), part_start + lead + n_raw_off + n_lead)
                    }
                    // bare label: an empty span right after it, so filling it inserts `=name`
                    None => (trimmed.to_string(), trimmed.to_string(), part_start + lead + trimmed.len()),
                };
                if !is_label(&label) {
                    let l = t.line;
                    let p = self.f.path.display().to_string();
                    self.f.errors.push(format!("{p}:{l}: `{label}` is not a local label (v<slot>, v<slot>b, …)"));
                    continue;
                }
                let bare = !trimmed.contains('=');
                let end = if bare { name_start } else { name_start + name.len() };
                out.push((label, Span { text: name.clone(), start: name_start, end }));
            }
            let _ = body;
            *end = t.end;
            self.i += 1;
        }
    }
    fn class_body(&mut self, cls: &mut ClassDecl) {
        // at '{'
        self.i += 1;
        loop {
            let Some(t) = self.tok() else {
                self.err("unterminated class body");
                cls.close = self.f.text.len();
                return;
            };
            if t.t == Tk::Punct('}') {
                cls.close = t.start;
                self.i += 1;
                return;
            }
            if t.t == Tk::Punct(';') {
                self.i += 1;
                continue;
            }
            if let Tk::Locals(_) = t.t {
                // a locals comment not after a method
                self.err("`/* locals: … */` must follow a method or constructor declaration");
                self.i += 1;
                continue;
            }
            let line = t.line;
            let ann = self.annotations();
            self.f.semantic += ann.iter().filter(|a| SEMANTIC.contains(&a.as_str())).count();
            self.modifiers();
            if matches!(self.ident(), Some("class" | "interface" | "enum")) {
                self.err("nested declarations are not supported in a map; declare `Outer$Inner` at top level");
                while !self.is_p('{') && self.peek().is_some() {
                    self.i += 1;
                }
                self.skip_group('{', '}');
                continue;
            }
            let Some(ty) = self.type_ref() else {
                self.err("expected a declaration");
                self.skip_statement();
                continue;
            };
            // constructor: `Name(params) {}`
            if self.is_p('(') && ty.dims == 0 && ty.base.text == cls.name.text {
                let params = self.params();
                if self.is_p('{') {
                    self.skip_group('{', '}');
                } else if self.is_p(';') {
                    self.i += 1;
                }
                let mut locals = Vec::new();
                let mut end = self.t.get(self.i.saturating_sub(1)).map(|t| t.end).unwrap_or(0);
                self.locals(&mut locals, &mut end);
                cls.members.push(MemberDecl::Ctor { params, locals, end, line });
                continue;
            }
            let Some(name) = self.name_span() else {
                self.err("expected a member name");
                self.skip_statement();
                continue;
            };
            if self.is_p('(') {
                let params = self.params();
                if self.ident() == Some("throws") {
                    while !self.is_p(';') && !matches!(self.peek(), Some(Tk::Was(_)) | None) {
                        self.i += 1;
                    }
                }
                let was = self.was();
                if self.is_p('{') {
                    self.err("method declarations end with `;`, not a body");
                    self.skip_group('{', '}');
                } else if self.is_p(';') {
                    self.i += 1;
                } else {
                    self.err("expected `;` after a method declaration");
                }
                let mut locals = Vec::new();
                let mut end = self.t.get(self.i.saturating_sub(1)).map(|t| t.end).unwrap_or(0);
                self.locals(&mut locals, &mut end);
                cls.members.push(MemberDecl::Method { ret: ty, name, params, was, locals, end, line });
            } else {
                let was = self.was();
                if self.is_p('=') {
                    // a source-only constant (domain interface) — keep verbatim
                    self.skip_statement();
                } else if self.is_p(';') {
                    self.i += 1;
                } else {
                    self.err("expected `;` after a field declaration");
                    self.skip_statement();
                }
                cls.members.push(MemberDecl::Field { ty, name, was, line });
            }
        }
    }
}

pub fn is_label(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('v') else { return false };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return false;
    }
    let tail = &rest[digits.len()..];
    tail.is_empty() || (tail.len() == 1 && tail.chars().all(|c| ('b'..='z').contains(&c)))
}

/// A parameter or local name that means "not named yet".
pub fn is_placeholder(s: &str) -> bool {
    let num = |p: &str| s.strip_prefix(p).map(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())).unwrap_or(false);
    is_label(s) || num("var") || num("arg") || num("p") || num("param")
}

pub fn parse(path: &Path, text: &str) -> MapFile {
    let toks = lex(text);
    let mut f = MapFile { path: path.to_path_buf(), text: text.to_string(), ..Default::default() };
    let mut p = P { t: &toks, i: 0, f: &mut f };
    let mut package = String::new();
    while let Some(t) = p.tok() {
        match &t.t {
            Tk::Ident(s) if s == "package" => {
                p.i += 1;
                let mut name = String::new();
                while let Some(s) = p.ident() {
                    name.push_str(s);
                    p.i += 1;
                    if p.is_p('.') {
                        name.push('.');
                        p.i += 1;
                    }
                }
                package = name;
                if p.f.package.is_empty() {
                    p.f.package = package.clone();
                }
                p.skip_statement();
            }
            Tk::Ident(s) if s == "import" => {
                p.i += 1;
                if p.ident() == Some("static") {
                    p.i += 1;
                }
                let mut name = String::new();
                while let Some(s) = p.ident() {
                    name.push_str(s);
                    p.i += 1;
                    if p.is_p('.') {
                        name.push('.');
                        p.i += 1;
                    }
                }
                if let Some(simple) = name.rsplit('.').next() {
                    p.f.imports.insert(simple.to_string(), name.clone());
                }
                p.skip_statement();
            }
            Tk::Punct('@') | Tk::Ident(_) => {
                let line = t.line;
                let ann = p.annotations();
                let sem = ann.iter().filter(|a| SEMANTIC.contains(&a.as_str())).count();
                p.modifiers();
                let kw = p.ident().map(str::to_string);
                match kw.as_deref() {
                    Some("class") | Some("interface") | Some("enum") => {
                        let is_iface = kw.as_deref() != Some("class");
                        p.i += 1;
                        let Some(name) = p.name_span() else {
                            p.err("expected a class name");
                            p.skip_statement();
                            continue;
                        };
                        let was = p.was();
                        while !p.is_p('{') && p.peek().is_some() {
                            p.i += 1;
                        }
                        let external = ann.iter().any(|a| a == "External");
                        if (is_iface && was.is_none()) || external {
                            // a domain declaration or an API binding: verbatim, not a rename
                            p.f.semantic += sem.max(1);
                            p.skip_group('{', '}');
                            continue;
                        }
                        p.f.semantic += sem;
                        let already = ann.iter().any(|a| a == "AlreadyMapped");
                        let mut cls = ClassDecl { name, was, package: package.clone(), already_mapped: already, members: vec![], close: 0, line };
                        if p.is_p('{') {
                            p.class_body(&mut cls);
                        }
                        p.f.classes.push(cls);
                    }
                    _ => {
                        p.err("expected `class`, `interface`, `package` or `import`");
                        p.skip_statement();
                        if p.is_p('}') {
                            p.i += 1;
                        }
                    }
                }
            }
            _ => {
                p.err("unexpected text");
                p.i += 1;
            }
        }
    }
    f
}

/// Every `.map` file of `path` (a file, or the `.map` files directly inside a
/// directory, in name order).
pub fn load(path: &Path) -> Result<Vec<MapFile>, String> {
    let mut files = Vec::new();
    if path.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "map").unwrap_or(false) && p.is_file())
            .collect();
        v.sort();
        for p in v {
            let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            files.push(parse(&p, &text));
        }
    } else if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        files.push(parse(path, &text));
    }
    Ok(files)
}

// ── resolving declarations against the jar ───────────────────────────────────

/// Names for the type resolver: mapped simple name -> internal, plus originals.
struct Types<'a> {
    world: &'a World<'a>,
    /// new simple name (as declared in the map) -> internal (bytecode) name
    mapped: BTreeMap<String, String>,
}

const PRIMS: &[(&str, &str)] = &[("int", "I"), ("boolean", "Z"), ("byte", "B"), ("char", "C"), ("short", "S"), ("long", "J"), ("float", "F"), ("double", "D"), ("void", "V")];

impl<'a> Types<'a> {
    fn internal(&self, f: &MapFile, base: &str) -> Option<String> {
        if let Some((_, d)) = PRIMS.iter().find(|(n, _)| *n == base) {
            return Some(d.to_string());
        }
        let as_class = |n: &str| format!("L{n};");
        if let Some(i) = self.mapped.get(base) {
            return Some(as_class(i));
        }
        if base.contains('.') {
            let slashed = base.replace('.', "/");
            // a project class written with its package, or an external type
            if let Some(i) = self.mapped.iter().find(|(n, i)| {
                let p = names::package_of(i);
                (if p.is_empty() { (*n).clone() } else { format!("{p}/{n}") }) == slashed
            }) {
                return Some(as_class(i.1));
            }
            return Some(as_class(&slashed));
        }
        if let Some(d) = f.imports.get(base) {
            return Some(as_class(&d.replace('.', "/")));
        }
        // an unmapped project class under its own name (same package as the file)
        let pkg = f.package.replace('.', "/");
        let own = if pkg.is_empty() { base.to_string() } else { format!("{pkg}/{base}") };
        if self.world.in_jar(&own) {
            return Some(as_class(&own));
        }
        let jl = format!("java/lang/{base}");
        if self.world.ext.known.contains_key(&jl) || ["String", "Object", "Thread", "Runnable", "Integer", "StringBuffer", "Class", "Throwable", "Exception", "System", "Math", "Long", "Character", "Boolean", "Byte", "Short", "Error", "RuntimeException"].contains(&base) {
            return Some(as_class(&jl));
        }
        // a referenced external type known only by its simple name
        let mut hits: Vec<&String> = self.world.ext.known.keys().filter(|k| names::simple_name(k) == base).collect();
        hits.dedup();
        if hits.len() == 1 {
            return Some(as_class(hits[0]));
        }
        None
    }
    fn desc(&self, f: &MapFile, t: &TypeRef) -> Option<String> {
        Some(format!("{}{}", "[".repeat(t.dims), self.internal(f, &t.base.text)?))
    }
}

/// `was` text -> internal name of a jar class.
fn resolve_owner(world: &World, f: &MapFile, was: &str) -> Option<String> {
    let w = was.trim();
    let slashed = w.replace('.', "/");
    if world.in_jar(&slashed) {
        return Some(slashed);
    }
    let pkg = f.package.replace('.', "/");
    if !pkg.is_empty() {
        let rel = format!("{pkg}/{slashed}");
        if world.in_jar(&rel) {
            return Some(rel);
        }
    }
    // simple name, unique in the jar
    let hits: Vec<&String> = world.classes.keys().filter(|k| k.rsplit('/').next() == Some(w)).collect();
    if hits.len() == 1 {
        return Some(hits[0].clone());
    }
    None
}

fn slots_of(desc: &str, is_static: bool) -> Vec<usize> {
    let mut out = Vec::new();
    let mut slot = if is_static { 0 } else { 1 };
    let inner = &desc[1..desc.find(')').unwrap_or(1)];
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() {
        out.push(slot);
        let wide = b[i] == b'J' || b[i] == b'D';
        while b[i] == b'[' {
            i += 1;
        }
        if b[i] == b'L' {
            while b[i] != b';' {
                i += 1;
            }
        }
        i += 1;
        slot += if wide && (i < 2 || b[i - 2] != b'[') { 2 } else { 1 };
    }
    out
}

/// Label of the `k`-th parameter (by declaration order) of a method.
pub fn param_label(desc: &str, is_static: bool, k: usize) -> Option<String> {
    slots_of(desc, is_static).get(k).map(|s| format!("v{s}"))
}

/// What the files say, as name entries (origin `map`).  Declarations that do not
/// match the jar are reported in the returned list, never guessed.
pub fn entries(world: &World, files: &[MapFile]) -> (Vec<NameEntry>, Vec<String>) {
    let mut out = Vec::new();
    let mut errs: Vec<String> = files.iter().flat_map(|f| f.errors.iter().cloned()).collect();
    // pass 1: classes
    let mut mapped: BTreeMap<String, String> = BTreeMap::new();
    let mut owners: Vec<Vec<Option<String>>> = Vec::new();
    for f in files {
        let mut v = Vec::new();
        for c in &f.classes {
            if c.already_mapped {
                v.push(None);
                continue;
            }
            let owner = match &c.was {
                Some(w) => resolve_owner(world, f, w),
                None => resolve_owner(world, f, &c.name.text),
            };
            match &owner {
                Some(o) => {
                    mapped.insert(c.name.text.clone(), o.clone());
                    if c.name.text != names::simple_name(o) && c.was.is_some() {
                        out.push(entry(Kind::Class, o, "", "", &c.name.text, ""));
                    }
                }
                None => errs.push(format!("{}:{}: class `{}` (was {}) is not in the jar", f.path.display(), c.line, c.name.text, c.was.as_deref().unwrap_or("?"))),
            }
            v.push(owner);
        }
        owners.push(v);
    }
    let types = Types { world, mapped };
    // pass 2: members, parameters, locals
    for (fi, f) in files.iter().enumerate() {
        for (ci, c) in f.classes.iter().enumerate() {
            let Some(owner) = owners[fi][ci].clone() else { continue };
            let Some(info) = world.classes.get(&owner) else { continue };
            for m in &c.members {
                match m {
                    MemberDecl::Field { ty, name, was, line } => {
                        let old = was.clone().unwrap_or_else(|| name.text.clone());
                        let cands: Vec<_> = info.fields.iter().filter(|x| x.name == old).collect();
                        let want = types.desc(f, ty);
                        let hit = match (cands.len(), &want) {
                            (0, _) => None,
                            (1, None) => Some(cands[0]),
                            (_, Some(d)) => cands.iter().copied().find(|x| &x.desc == d),
                            _ => None,
                        };
                        match hit {
                            Some(fd) if fd.name != name.text => out.push(entry(Kind::Field, &owner, &fd.name, &fd.desc, &name.text, "")),
                            Some(_) => {}
                            None => errs.push(format!("{}:{line}: field {}.{old} of type {} is not in the jar", f.path.display(), owner, ty.base.text)),
                        }
                    }
                    MemberDecl::Method { ret, name, params, was, locals, line, .. } => {
                        let old = was.clone().unwrap_or_else(|| name.text.clone());
                        let want: Option<String> = (|| {
                            let mut d = String::from("(");
                            for p in params {
                                d.push_str(&types.desc(f, &p.ty)?);
                            }
                            d.push(')');
                            d.push_str(&types.desc(f, ret)?);
                            Some(d)
                        })();
                        let cands: Vec<_> = info.methods.iter().filter(|x| x.name == old).collect();
                        let hit = match &want {
                            Some(d) => cands.iter().copied().find(|x| &x.desc == d),
                            None => {
                                let same_arity: Vec<_> = cands.iter().copied().filter(|x| slots_of(&x.desc, true).len() == params.len()).collect();
                                if same_arity.len() == 1 {
                                    Some(same_arity[0])
                                } else {
                                    None
                                }
                            }
                        };
                        let Some(md) = hit else {
                            errs.push(format!(
                                "{}:{line}: method {}.{old}{} is not in the jar{}",
                                f.path.display(),
                                owner,
                                want.as_deref().unwrap_or("(…)"),
                                if cands.is_empty() { String::new() } else { format!(" (candidates: {})", cands.iter().map(|c| c.desc.as_str()).collect::<Vec<_>>().join(", ")) }
                            ));
                            continue;
                        };
                        if md.name != name.text {
                            out.push(entry(Kind::Method, &owner, &md.name, &md.desc, &name.text, ""));
                        }
                        push_locals(&mut out, &owner, &md.name, &md.desc, md.is_static(), params, locals);
                    }
                    MemberDecl::Ctor { params, locals, line, .. } => {
                        let want: Option<String> = (|| {
                            let mut d = String::from("(");
                            for p in params {
                                d.push_str(&types.desc(f, &p.ty)?);
                            }
                            d.push_str(")V");
                            Some(d)
                        })();
                        let ctors: Vec<_> = info.methods.iter().filter(|x| x.name == "<init>").collect();
                        let hit = match &want {
                            Some(d) => ctors.iter().copied().find(|x| &x.desc == d),
                            None => ctors.iter().copied().find(|x| slots_of(&x.desc, true).len() == params.len()).filter(|_| ctors.len() == 1),
                        };
                        match hit {
                            Some(md) => push_locals(&mut out, &owner, "<init>", &md.desc, false, params, locals),
                            None => errs.push(format!("{}:{line}: constructor {}{} is not in the jar", f.path.display(), owner, want.as_deref().unwrap_or("(…)"))),
                        }
                    }
                }
            }
        }
    }
    (out, errs)
}

fn push_locals(out: &mut Vec<NameEntry>, owner: &str, m: &str, d: &str, is_static: bool, params: &[Param], locals: &[(String, Span)]) {
    for (k, p) in params.iter().enumerate() {
        if is_placeholder(&p.name.text) {
            continue;
        }
        if let Some(l) = param_label(d, is_static, k) {
            out.push(entry(Kind::Local, owner, m, d, &p.name.text, &l));
        }
    }
    for (label, name) in locals {
        if !is_placeholder(&name.text) {
            out.push(entry(Kind::Local, owner, m, d, &name.text, label));
        }
    }
}

fn entry(kind: Kind, owner: &str, name: &str, desc: &str, new: &str, local: &str) -> NameEntry {
    NameEntry {
        kind,
        owner: owner.into(),
        name: name.into(),
        desc: desc.into(),
        local: local.into(),
        new: new.into(),
        origin: "map".into(),
        status: String::new(),
    }
}

// ── labels written by the applier ────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub owner: String,
    pub method: String,
    pub desc: String,
    pub label: String,
    /// JVM descriptor of the value the label holds
    pub ty: String,
    pub param: bool,
}

pub fn read_labels(path: &Path) -> Vec<Label> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\t').collect();
            (p.len() >= 6).then(|| Label { owner: p[0].into(), method: p[1].into(), desc: p[2].into(), label: p[3].into(), ty: p[4].into(), param: p[5] == "1" })
        })
        .collect()
}

// ── writing ──────────────────────────────────────────────────────────────────

/// Java source spelling of a descriptor, with project classes under their final
/// names and external classes fully qualified unless `imports` has them.
fn java_of(desc: &str, plan: &Plan, imports: &BTreeMap<String, String>, world: &World) -> String {
    let dims = desc.chars().take_while(|c| *c == '[').count();
    let base = &desc[dims..];
    let b = match base.as_bytes().first() {
        Some(b'L') => {
            let int = &base[1..base.len() - 1];
            if world.in_jar(int) {
                let fin = plan.classes.get(int).cloned().unwrap_or_else(|| int.to_string());
                names::simple_name(&fin).to_string().replace('$', ".")
            } else {
                let dotted = int.replace(['/', '$'], ".");
                let simple = names::simple_name(int);
                if int.starts_with("java/lang/") && !int[10..].contains('/') {
                    simple.to_string()
                } else if imports.get(simple).map(|d| *d == dotted).unwrap_or(false) {
                    simple.to_string()
                } else {
                    dotted
                }
            }
        }
        Some(c) => PRIMS.iter().find(|(_, d)| d.as_bytes()[0] == *c).map(|(n, _)| n.to_string()).unwrap_or("?".into()),
        None => "?".into(),
    };
    format!("{b}{}", "[]".repeat(dims))
}

fn args_of(desc: &str) -> Vec<String> {
    let inner = &desc[1..desc.find(')').unwrap_or(1)];
    let mut v = Vec::new();
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let s = i;
        while b[i] == b'[' {
            i += 1;
        }
        if b[i] == b'L' {
            while b[i] != b';' {
                i += 1;
            }
        }
        i += 1;
        v.push(inner[s..i].to_string());
    }
    v
}

/// What the map should say about one symbol after this run.
#[derive(Debug, Clone, Default)]
pub struct Wanted {
    /// class internal -> final simple name (only classes that look obfuscated or are named)
    pub classes: BTreeMap<String, String>,
    /// (owner, name, desc) -> final name, for fields (desc = field desc)
    pub fields: BTreeMap<(String, String, String), String>,
    pub methods: BTreeMap<(String, String, String), String>,
    /// (owner, method, desc) -> label -> name (or the label itself when unnamed)
    pub locals: BTreeMap<(String, String, String), BTreeMap<String, String>>,
}

/// The applied names from `plan` for every symbol worth listing: obfuscated-looking
/// classes and members (named or not, so the file is a complete worksheet) and every
/// labelled local.
pub fn wanted(world: &World, plan: &Plan, labels: &[Label]) -> Wanted {
    let mut w = Wanted::default();
    for (c, ci) in world.classes {
        let simple = names::simple_name(c);
        let named = plan.classes.get(c).map(|f| names::simple_name(f).to_string()).filter(|n| n != simple);
        if named.is_some() || names::looks_obfuscated(simple) {
            w.classes.insert(c.clone(), named.unwrap_or_else(|| simple.to_string()));
        }
        for f in ci.fields.iter().filter(|f| !f.is_synthetic()) {
            let n = plan.fields.get(&(c.clone(), f.name.clone())).cloned();
            if n.is_some() || names::looks_obfuscated(&f.name) {
                w.fields.insert((c.clone(), f.name.clone(), f.desc.clone()), n.unwrap_or_else(|| f.name.clone()));
            }
        }
        for m in ci.methods.iter().filter(|m| !m.is_synthetic() && m.name != "<clinit>") {
            if m.name == "<init>" {
                continue;
            }
            let n = plan.methods.get(&(c.clone(), m.name.clone(), m.desc.clone())).cloned();
            if n.is_some() || names::looks_obfuscated(&m.name) {
                w.methods.insert((c.clone(), m.name.clone(), m.desc.clone()), n.unwrap_or_else(|| m.name.clone()));
            }
        }
    }
    for l in labels {
        w.locals.entry((l.owner.clone(), l.method.clone(), l.desc.clone())).or_default().insert(l.label.clone(), l.label.clone());
    }
    for ((o, m, d, l), n) in &plan.locals {
        w.locals.entry((o.clone(), m.clone(), d.clone())).or_default().insert(l.clone(), n.clone());
    }
    // a method listed only for its locals is declared too
    for (o, m, d) in w.locals.keys() {
        if m != "<init>" {
            let n = plan.methods.get(&(o.clone(), m.clone(), d.clone())).cloned().unwrap_or_else(|| m.clone());
            w.methods.entry((o.clone(), m.clone(), d.clone())).or_insert(n);
        }
        let simple = names::simple_name(o).to_string();
        let fin = plan.classes.get(o).map(|f| names::simple_name(f).to_string()).unwrap_or(simple);
        w.classes.entry(o.clone()).or_insert(fin);
    }
    w
}

/// Bring the files in `dir_or_file` up to `want`, touching only unnamed tokens and
/// inserting what is missing.  `auto` marks inserted or filled lines `// auto`
/// (model output that nobody reviewed yet).  Returns (path, new text) per file that
/// changed or must be created.
pub fn patch(world: &World, plan: &Plan, files: &[MapFile], want: &Wanted, target: &Path, auto: &BTreeSet<String>) -> Vec<(PathBuf, String)> {
    // where is each class declared already?
    let mut declared: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (fi, f) in files.iter().enumerate() {
        for (ci, c) in f.classes.iter().enumerate() {
            if let Some(o) = c.was.as_deref().and_then(|w| resolve_owner(world, f, w)) {
                declared.entry(o).or_insert((fi, ci));
            }
        }
    }
    // edits per file: (start, end, replacement)
    let mut edits: Vec<Vec<(usize, usize, String)>> = vec![Vec::new(); files.len()];
    let mut renamed_classes: BTreeMap<String, String> = BTreeMap::new(); // old simple -> new simple
    let mut new_blocks: BTreeMap<String, Vec<String>> = BTreeMap::new(); // package -> class blocks

    let fq = BTreeMap::new();
    for (owner, cname) in &want.classes {
        let methods: Vec<(&(String, String, String), &String)> = want.methods.iter().filter(|(k, _)| &k.0 == owner).collect();
        let fields: Vec<(&(String, String, String), &String)> = want.fields.iter().filter(|(k, _)| &k.0 == owner).collect();
        let ctors: Vec<(&(String, String, String), &BTreeMap<String, String>)> = want.locals.iter().filter(|(k, _)| &k.0 == owner && k.1 == "<init>").collect();
        match declared.get(owner) {
            Some(&(fi, ci)) => {
                let f = &files[fi];
                let c = &f.classes[ci];
                let old_simple = names::simple_name(owner);
                if c.name.text == old_simple && cname != old_simple {
                    edits[fi].push((c.name.start, c.name.end, cname.clone()));
                    renamed_classes.insert(old_simple.to_string(), cname.clone());
                }
                let mut inserts = String::new();
                for ((_, mname, mdesc), new) in &methods {
                    let existing = c.members.iter().find(|m| match m {
                        MemberDecl::Method { name, was, .. } => was.as_deref().unwrap_or(&name.text) == mname.as_str() && member_desc_matches(world, f, m, mdesc, &files_types(world, files)),
                        _ => false,
                    });
                    let lw = want.locals.get(&(owner.clone(), mname.clone(), mdesc.clone()));
                    match existing {
                        Some(MemberDecl::Method { name, params, locals, end, was, .. }) => {
                            if was.as_deref() == Some(name.text.as_str()) && new.as_str() != name.text {
                                edits[fi].push((name.start, name.end, new.to_string()));
                            }
                            patch_locals(&mut edits[fi], world, owner, mname, mdesc, params, locals, *end, lw);
                        }
                        _ => {
                            let mark = if auto.contains(&format!("m {owner} {mname}{mdesc}")) { " // auto" } else { "" };
                            inserts.push_str(&format!("    {}{mark}\n", method_line(world, plan, &fq, owner, mname, mdesc, new, lw)));
                        }
                    }
                }
                for ((_, fname, fdesc), new) in &fields {
                    let existing = c.members.iter().find(|m| matches!(m, MemberDecl::Field { name, was, .. } if was.as_deref().unwrap_or(&name.text) == fname.as_str()));
                    match existing {
                        Some(MemberDecl::Field { name, was, .. }) => {
                            if was.as_deref() == Some(name.text.as_str()) && new.as_str() != name.text {
                                edits[fi].push((name.start, name.end, new.to_string()));
                            }
                        }
                        _ => {
                            let mark = if auto.contains(&format!("f {owner} {fname}")) { " // auto" } else { "" };
                            inserts.push_str(&format!("    {} {new} /* was {fname} */;{mark}\n", java_of(fdesc, plan, &fq, world)));
                        }
                    }
                }
                for ((_, _, d), lw) in &ctors {
                    let existing = c.members.iter().find(|m| matches!(m, MemberDecl::Ctor { params, .. } if params.len() == args_of(d).len()));
                    match existing {
                        Some(MemberDecl::Ctor { params, locals, end, .. }) => patch_locals(&mut edits[fi], world, owner, "<init>", d, params, locals, *end, Some(lw)),
                        _ => inserts.push_str(&format!("    {}\n", ctor_line(world, plan, &fq, owner, cname, d, lw))),
                    }
                }
                if !inserts.is_empty() {
                    edits[fi].push((c.close, c.close, inserts));
                }
            }
            None => {
                let mut b = format!("class {cname} /* was {} */ {{\n", names::simple_name(owner));
                for ((_, _, d), lw) in &ctors {
                    b.push_str(&format!("    {}\n", ctor_line(world, plan, &fq, owner, cname, d, lw)));
                }
                for ((_, fname, fdesc), new) in &fields {
                    let mark = if auto.contains(&format!("f {owner} {fname}")) { " // auto" } else { "" };
                    b.push_str(&format!("    {} {new} /* was {fname} */;{mark}\n", java_of(fdesc, plan, &fq, world)));
                }
                for ((_, mname, mdesc), new) in &methods {
                    let lw = want.locals.get(&(owner.clone(), mname.clone(), mdesc.clone()));
                    let mark = if auto.contains(&format!("m {owner} {mname}{mdesc}")) { " // auto" } else { "" };
                    b.push_str(&format!("    {}{mark}\n", method_line(world, plan, &fq, owner, mname, mdesc, new, lw)));
                }
                b.push_str("}\n");
                let pkg = names::package_of(owner).replace('/', ".");
                new_blocks.entry(pkg).or_default().push(b);
            }
        }
    }
    // references to classes renamed in place
    if !renamed_classes.is_empty() {
        for (fi, f) in files.iter().enumerate() {
            let declared_new: BTreeSet<&str> = f.classes.iter().map(|c| c.name.text.as_str()).collect();
            for s in &f.type_spans {
                if let Some(n) = renamed_classes.get(&s.text) {
                    if !declared_new.contains(s.text.as_str()) || f.classes.iter().any(|c| c.name.start == s.start) {
                        if !edits[fi].iter().any(|e| e.0 == s.start) {
                            edits[fi].push((s.start, s.end, n.clone()));
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (fi, f) in files.iter().enumerate() {
        if edits[fi].is_empty() {
            continue;
        }
        let mut e = std::mem::take(&mut edits[fi]);
        e.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let mut text = f.text.clone();
        for (s, en, r) in e {
            text.replace_range(s..en, &r);
        }
        out.push((f.path.clone(), text));
    }
    // new classes: into the file of their package, or a new one
    for (pkg, blocks) in new_blocks {
        let existing = files.iter().position(|f| f.package == pkg);
        match existing {
            Some(fi) => {
                let base = out.iter().position(|(p, _)| *p == files[fi].path);
                let text = match base {
                    Some(k) => &mut out[k].1,
                    None => {
                        out.push((files[fi].path.clone(), files[fi].text.clone()));
                        &mut out.last_mut().unwrap().1
                    }
                };
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                for b in blocks {
                    text.push('\n');
                    text.push_str(&b);
                }
            }
            None => {
                let path = if target.extension().map(|e| e == "map").unwrap_or(false) {
                    if pkg.is_empty() {
                        target.to_path_buf()
                    } else {
                        target.with_file_name(format!("{pkg}.map"))
                    }
                } else if pkg.is_empty() {
                    target.join("default.map")
                } else {
                    target.join(format!("{pkg}.map"))
                };
                let mut text = String::from(HEADER);
                if !pkg.is_empty() {
                    text.push_str(&format!("package {pkg};\n\n"));
                }
                text.push_str(&blocks.join("\n"));
                out.push((path, text));
            }
        }
    }
    out
}

const HEADER: &str = "// Name map (Sporeflower .map format). Edit names freely: a name that differs\n\
// from its `was` is never overwritten. Parameters and locals still called v<N> are\n\
// unnamed; name locals in `/* locals: v4=name */`. `// auto` = proposed, not reviewed.\n\n";

fn files_types<'a>(world: &'a World<'a>, files: &[MapFile]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for f in files {
        for c in &f.classes {
            if let Some(o) = c.was.as_deref().and_then(|w| resolve_owner(world, f, w)) {
                m.insert(c.name.text.clone(), o);
            }
        }
    }
    m
}

fn member_desc_matches(world: &World, f: &MapFile, m: &MemberDecl, desc: &str, mapped: &BTreeMap<String, String>) -> bool {
    let MemberDecl::Method { ret, params, .. } = m else { return false };
    let t = Types { world, mapped: mapped.clone() };
    let mut d = String::from("(");
    for p in params {
        match t.desc(f, &p.ty) {
            Some(x) => d.push_str(&x),
            None => return args_of(desc).len() == params.len(),
        }
    }
    d.push(')');
    match t.desc(f, ret) {
        Some(x) => d.push_str(&x),
        None => return args_of(desc).len() == params.len(),
    }
    d == desc
}

#[allow(clippy::too_many_arguments)]
fn patch_locals(
    edits: &mut Vec<(usize, usize, String)>,
    world: &World,
    owner: &str,
    mname: &str,
    mdesc: &str,
    params: &[Param],
    locals: &[(String, Span)],
    end: usize,
    lw: Option<&BTreeMap<String, String>>,
) {
    let Some(lw) = lw else { return };
    let is_static = world.classes.get(owner).and_then(|c| c.methods.iter().find(|m| m.name == mname && m.desc == mdesc)).map(|m| m.is_static()).unwrap_or(false);
    let mut covered: BTreeSet<String> = BTreeSet::new();
    for (k, p) in params.iter().enumerate() {
        let Some(l) = param_label(mdesc, is_static, k) else { continue };
        covered.insert(l.clone());
        if let Some(n) = lw.get(&l) {
            if is_placeholder(&p.name.text) && n != &p.name.text && !is_placeholder(n) {
                edits.push((p.name.start, p.name.end, n.clone()));
            }
        }
    }
    for (l, span) in locals {
        covered.insert(l.clone());
        if let Some(n) = lw.get(l) {
            if is_placeholder(&span.text) && n != &span.text && !is_placeholder(n) {
                if span.start == span.end {
                    edits.push((span.start, span.end, format!("={n}")));
                } else {
                    edits.push((span.start, span.end, n.clone()));
                }
            }
        }
    }
    let missing: Vec<String> = lw.iter().filter(|(l, _)| !covered.contains(*l)).map(|(l, n)| if l == n { l.clone() } else { format!("{l}={n}") }).collect();
    if !missing.is_empty() {
        edits.push((end, end, format!(" /* locals: {} */", missing.join(", "))));
    }
}

fn local_list(lw: Option<&BTreeMap<String, String>>, skip: &BTreeSet<String>) -> String {
    let Some(lw) = lw else { return String::new() };
    let v: Vec<String> = lw
        .iter()
        .filter(|(l, _)| !skip.contains(*l))
        .map(|(l, n)| if l == n { l.clone() } else { format!("{l}={n}") })
        .collect();
    if v.is_empty() {
        String::new()
    } else {
        format!(" /* locals: {} */", v.join(", "))
    }
}

#[allow(clippy::too_many_arguments)]
fn method_line(world: &World, plan: &Plan, imports: &BTreeMap<String, String>, owner: &str, mname: &str, mdesc: &str, new: &str, lw: Option<&BTreeMap<String, String>>) -> String {
    let is_static = world.classes.get(owner).and_then(|c| c.methods.iter().find(|m| m.name == mname && m.desc == mdesc)).map(|m| m.is_static()).unwrap_or(false);
    let (params, skip) = param_list(mdesc, is_static, lw, plan, imports, world);
    let ret = java_of(&mdesc[mdesc.find(')').unwrap() + 1..], plan, imports, world);
    let st = if is_static { "static " } else { "" };
    format!("{st}{ret} {new}({params}) /* was {mname} */;{}", local_list(lw, &skip))
}

fn ctor_line(world: &World, plan: &Plan, imports: &BTreeMap<String, String>, _owner: &str, cname: &str, d: &str, lw: &BTreeMap<String, String>) -> String {
    let (params, skip) = param_list(d, false, Some(lw), plan, imports, world);
    format!("{cname}({params}) {{}}{}", local_list(Some(lw), &skip))
}

fn param_list(d: &str, is_static: bool, lw: Option<&BTreeMap<String, String>>, plan: &Plan, imports: &BTreeMap<String, String>, world: &World) -> (String, BTreeSet<String>) {
    let mut skip = BTreeSet::new();
    let ps: Vec<String> = args_of(d)
        .iter()
        .enumerate()
        .map(|(k, a)| {
            let l = param_label(d, is_static, k).unwrap_or_else(|| format!("v{k}"));
            skip.insert(l.clone());
            let n = lw.and_then(|m| m.get(&l)).cloned().unwrap_or_else(|| l.clone());
            format!("{} {n}", java_of(a, plan, imports, world))
        })
        .collect();
    (ps.join(", "), skip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_placeholders() {
        assert!(is_label("v4") && is_label("v12b") && is_label("v1z"));
        assert!(!is_label("v") && !is_label("v04") && !is_label("v4a") && !is_label("v4bb") && !is_label("value"));
        assert!(is_placeholder("var3") && is_placeholder("arg0") && is_placeholder("p2") && !is_placeholder("player"));
    }

    #[test]
    fn param_slots_count_wide_types_twice() {
        assert_eq!(slots_of("(IJLjava/lang/String;[JD)V", false), vec![1, 2, 4, 5, 6]);
        assert_eq!(slots_of("(I[[Lx;)V", true), vec![0, 1]);
        assert_eq!(param_label("(JI)V", true, 1).as_deref(), Some("v2"));
    }

    #[test]
    fn parses_declarations_comments_and_semantics() {
        let text = "package p.q;\nimport javax.microedition.lcdui.Graphics;\n\
            @ValueDomain interface Dir { int LEFT = 1; }\n\
            class Game /* was d */ {\n    Game(Midlet owner) {} /* locals: v2=tmp */\n\
            @Domain(Dir.class) int dir /* was a */;\n\
            void draw(Graphics g, int v2) /* was b */; /* locals: v3=row, v4b */\n}\n";
        let f = parse(Path::new("x.map"), text);
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert_eq!(f.package, "p.q");
        assert_eq!(f.imports["Graphics"], "javax.microedition.lcdui.Graphics");
        assert_eq!(f.semantic, 2);
        let c = &f.classes[0];
        assert_eq!((c.name.text.as_str(), c.was.as_deref()), ("Game", Some("d")));
        assert_eq!(c.members.len(), 3);
        match &c.members[2] {
            MemberDecl::Method { name, params, was, locals, .. } => {
                assert_eq!(name.text, "draw");
                assert_eq!(was.as_deref(), Some("b"));
                assert_eq!(params[0].name.text, "g");
                assert_eq!(locals.iter().map(|(l, s)| format!("{l}={}", s.text)).collect::<Vec<_>>(), vec!["v3=row", "v4b=v4b"]);
                assert_eq!(&text[locals[0].1.start..locals[0].1.end], "row");
                assert_eq!(locals[1].1.start, locals[1].1.end, "a bare label has an empty name span");
                assert_eq!(&text[locals[1].1.start - 3..locals[1].1.start], "v4b");
            }
            m => panic!("{m:?}"),
        }
        match &c.members[0] {
            MemberDecl::Ctor { params, locals, .. } => assert_eq!((params.len(), locals.len()), (1, 1)),
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn bad_input_is_reported_with_a_line() {
        let f = parse(Path::new("x.map"), "class A /* was a */ {\n  int ;\n  void m() /* was b */ {}\n}\n");
        assert!(f.errors.iter().any(|e| e.starts_with("x.map:2:")), "{:?}", f.errors);
        assert!(f.errors.iter().any(|e| e.contains("end with `;`")), "{:?}", f.errors);
    }
}
