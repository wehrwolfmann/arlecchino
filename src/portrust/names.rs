//! Rust names for Java names, and the name maps that feed them.
//!
//! The translation keeps every name of the input program.  Only the casing changes,
//! to what Rust expects:
//!
//! * classes become `CamelCase` structs in a `snake_case` module (`GameEngine` →
//!   `game_engine::GameEngine`);
//! * methods, instance fields, parameters and locals become `snake_case`
//!   (`frameCounter` → `frame_counter`);
//! * static fields become `SCREAMING_SNAKE_CASE` (`maxLives` → `MAX_LIVES`).
//!
//! A name that is a Rust keyword becomes a raw identifier (`type` → `r#type`); the
//! four keywords that cannot be raw (`self`, `Self`, `super`, `crate`), names Rust
//! code relies on (`std`, `core`, `Default`…) and names of the generator's own
//! items get a trailing `_`.  Two Java names that fold to the same Rust name in
//! one scope (`getX` and `get_x`) are told apart by a numeric suffix (`get_x_2`),
//! in declaration order, so the output is deterministic.
//!
//! [`NameMap`] reads the names part of a Sporeflower `.map` file (classes, fields,
//! methods and their parameters with `/* was … */` markers) and lets the generator
//! apply it directly, without a separate remap pass over the Java source.

use std::collections::{BTreeMap, HashMap, HashSet};

pub const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "continue", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "gen", "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "priv", "pub", "ref",
    "return", "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual",
    "where", "while", "yield",
];

/// Keywords that cannot be written as raw identifiers.
const NOT_RAW: &[&str] = &["self", "Self", "super", "crate", "_"];

/// Prelude and std names a translated module uses unqualified; a struct or module
/// with one of these names would shadow it.
const SHADOWING_TYPES: &[&str] = &[
    "Default", "Option", "Some", "None", "Result", "Ok", "Err", "Vec", "Box", "String", "Clone", "Copy", "Send",
    "Sync", "Sized", "Drop", "Fn", "FnMut", "FnOnce", "Iterator", "IntoIterator", "Extend", "ToString", "ToOwned",
    "Into", "From", "TryFrom", "TryInto", "AsRef", "AsMut", "Eq", "PartialEq", "Ord", "PartialOrd", "Debug",
    "Hash", "Unpin", "FromIterator", "DoubleEndedIterator", "ExactSizeIterator",
];
const SHADOWING_MODULES: &[&str] = &["std", "core", "alloc", "jrt", "proc_macro", "test"];

fn is_word_start(prev: Option<char>, c: char, next: Option<char>) -> bool {
    if !c.is_uppercase() {
        return false;
    }
    match prev {
        None | Some('_') => false,
        Some(p) if p.is_lowercase() => true,
        Some(p) if p.is_ascii_digit() || p.is_uppercase() => next.is_some_and(|n| n.is_lowercase()),
        _ => false,
    }
}

/// `frameCounter` → `frame_counter`, `HTTPServer` → `http_server`, `Outer$Inner`
/// → `outer_inner`, `MAX_LIVES` → `max_lives`.
pub fn snake_case(java: &str) -> String {
    let chars: Vec<char> = java.chars().map(|c| if c == '$' { '_' } else { c }).collect();
    let mut out = String::with_capacity(chars.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        let prev = if i == 0 { None } else { Some(chars[i - 1]) };
        if is_word_start(prev, c, chars.get(i + 1).copied()) {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// `game_engine` → `GameEngine`, `a` → `A`, `Outer$Inner` → `OuterInner`.
pub fn camel_case(java: &str) -> String {
    let mut out = String::with_capacity(java.len());
    for piece in java.split(['_', '$']).filter(|p| !p.is_empty()) {
        let mut cs = piece.chars();
        if let Some(f) = cs.next() {
            out.extend(f.to_uppercase());
            out.push_str(cs.as_str());
        }
    }
    if out.is_empty() {
        out.push_str("Class");
    }
    out
}

/// `maxLives` → `MAX_LIVES`.  A name with no letter at all gets an `S_` prefix, so
/// that it can never equal a `snake_case` local (a `let` may not shadow a static).
pub fn screaming_case(java: &str) -> String {
    let s = if java.chars().any(|c| c.is_lowercase()) { snake_case(java) } else { java.replace('$', "_") };
    let up = s.to_uppercase();
    if up.chars().any(|c| c.is_alphabetic()) {
        up
    } else {
        format!("S_{up}")
    }
}

/// Makes `name` usable as a Rust identifier: raw form for keywords, a trailing
/// `_` for the few words that cannot be raw.
pub fn legal(name: &str) -> String {
    if NOT_RAW.contains(&name) {
        format!("{name}_")
    } else if RUST_KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

/// The names already taken in one Rust scope.
#[derive(Default, Clone)]
pub struct Scope {
    used: HashSet<String>,
}

impl Scope {
    pub fn with_reserved(names: &[&str]) -> Self {
        Scope { used: names.iter().map(|s| s.to_string()).collect() }
    }

    /// Takes `base` (already cased), or `base_2`, `base_3`… if it is taken.
    /// Returns the legal identifier.
    pub fn claim(&mut self, base: &str) -> String {
        let mut candidate = base.to_string();
        let mut n = 2;
        while self.used.contains(&candidate) {
            candidate = format!("{base}_{n}");
            n += 1;
        }
        self.used.insert(candidate.clone());
        legal(&candidate)
    }
}

/// Module name for a class (simple name, `snake_case`).
pub fn module_base(simple: &str) -> String {
    let s = snake_case(simple);
    if SHADOWING_MODULES.contains(&s.as_str()) || RUST_KEYWORDS.contains(&s.as_str()) || NOT_RAW.contains(&s.as_str()) {
        format!("{s}_")
    } else {
        s
    }
}

/// Struct name for a class (simple name, `CamelCase`).
pub fn struct_base(simple: &str, reserved: &[&str]) -> String {
    let s = camel_case(simple);
    if SHADOWING_TYPES.contains(&s.as_str()) || reserved.contains(&s.as_str()) || NOT_RAW.contains(&s.as_str()) {
        format!("{s}_")
    } else {
        s
    }
}

/// Local, parameter, field or method name (`snake_case`).  Leading `__` is kept
/// free for the generator's temporaries.
pub fn value_base(java: &str) -> String {
    let s = snake_case(java);
    if s.starts_with("__") {
        format!("j{s}")
    } else {
        s
    }
}

/// Simple name of a flat Java class name: `pkg.Outer$Inner` → `Outer$Inner`.
pub fn simple_of(flat: &str) -> &str {
    flat.rsplit('.').next().unwrap_or(flat)
}

// ---------------------------------------------------------------- name maps

/// One class of a `.map` file, names only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapClass {
    /// Declared (new) simple name.
    pub name: String,
    /// `/* was … */` owner: simple, slash-separated or dotted; `None` when the
    /// class keeps its name (`@AlreadyMapped`, generated domains).
    pub was: Option<String>,
    pub fields: Vec<MapField>,
    pub methods: Vec<MapMethod>,
    pub nested: Vec<MapClass>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapField {
    pub name: String,
    pub was: String,
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapMethod {
    /// New name; the class name for a constructor.
    pub name: String,
    /// `None` for a constructor.
    pub was: Option<String>,
    /// `(type, name)` as written.
    pub params: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Was(String),
    Punct(char),
}

fn tokenize(text: &str) -> Vec<Tok> {
    let cs: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && cs.get(i + 1) == Some(&'/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && cs.get(i + 1) == Some(&'*') {
            let start = i + 2;
            let mut j = start;
            while j + 1 < cs.len() && !(cs[j] == '*' && cs[j + 1] == '/') {
                j += 1;
            }
            let body: String = cs[start..j.min(cs.len())].iter().collect();
            let body = body.trim();
            if let Some(w) = body.strip_prefix("was").filter(|w| w.starts_with(char::is_whitespace)) {
                out.push(Tok::Was(w.trim().to_string()));
            }
            i = j + 2;
        } else if c == '"' || c == '\'' {
            let q = c;
            i += 1;
            while i < cs.len() && cs[i] != q {
                if cs[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            out.push(Tok::Word("<lit>".into()));
        } else if c.is_alphanumeric() || c == '_' || c == '$' || c == '.' {
            let start = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || matches!(cs[i], '_' | '$' | '.')) {
                i += 1;
            }
            out.push(Tok::Word(cs[start..i].iter().collect()));
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

const MODIFIERS: &[&str] = &[
    "public", "protected", "private", "static", "final", "abstract", "native", "synchronized", "transient",
    "volatile", "strictfp", "default",
];

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn is_punct(&self, c: char) -> bool {
        self.peek() == Some(&Tok::Punct(c))
    }
    fn word(&self) -> Option<&str> {
        match self.peek() {
            Some(Tok::Word(w)) => Some(w),
            _ => None,
        }
    }
    /// Skips a balanced `(...)`, `{...}` or `[...]` starting at the current token.
    fn skip_group(&mut self) {
        let (open, close) = match self.peek() {
            Some(Tok::Punct('(')) => ('(', ')'),
            Some(Tok::Punct('{')) => ('{', '}'),
            Some(Tok::Punct('[')) => ('[', ']'),
            _ => return,
        };
        let mut depth = 0;
        while let Some(t) = self.next() {
            if t == Tok::Punct(open) {
                depth += 1;
            } else if t == Tok::Punct(close) {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
        }
    }
    /// Skips annotations (`@Name`, `@Name(...)`) and modifiers.
    fn skip_decorations(&mut self) {
        loop {
            if self.is_punct('@') {
                self.next();
                if self.word() == Some("interface") {
                    return; // `@interface` declaration: let the caller see `interface`
                }
                self.next();
                if self.is_punct('(') {
                    self.skip_group();
                }
            } else if self.word().is_some_and(|w| MODIFIERS.contains(&w)) {
                self.next();
            } else {
                return;
            }
        }
    }
    /// A type: `int`, `a.b.C`, `C[]`, `C<...>` (generic arguments are dropped).
    fn ty(&mut self) -> Option<String> {
        self.skip_decorations();
        let mut t = match self.next()? {
            Tok::Word(w) => w,
            _ => return None,
        };
        if self.is_punct('<') {
            let mut depth = 0;
            while let Some(x) = self.next() {
                match x {
                    Tok::Punct('<') => depth += 1,
                    Tok::Punct('>') => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        while self.is_punct('[') {
            self.next();
            if self.is_punct(']') {
                self.next();
            }
            t.push_str("[]");
        }
        if self.is_punct('.') {
            // varargs `...` tokenizes as dots inside a word; nothing to do here
        }
        Some(t)
    }
    fn skip_to_semicolon(&mut self) {
        while let Some(t) = self.peek() {
            match t {
                Tok::Punct(';') => {
                    self.next();
                    return;
                }
                Tok::Punct('{') | Tok::Punct('(') | Tok::Punct('[') => self.skip_group(),
                _ => {
                    self.next();
                }
            }
        }
    }

    fn class_body(&mut self, class: &mut MapClass) {
        // at '{'
        self.next();
        loop {
            self.skip_decorations();
            match self.peek() {
                None => return,
                Some(Tok::Punct('}')) => {
                    self.next();
                    return;
                }
                Some(Tok::Punct(';')) => {
                    self.next();
                    continue;
                }
                Some(Tok::Punct('{')) => {
                    self.skip_group(); // initializer block
                    continue;
                }
                _ => {}
            }
            if matches!(self.word(), Some("class" | "interface" | "enum")) {
                if let Some(n) = self.type_decl() {
                    class.nested.push(n);
                }
                continue;
            }
            let start = self.pos;
            // constructor: Name ( ... )
            if let (Some(Tok::Word(w)), Some(Tok::Punct('('))) = (self.toks.get(self.pos), self.toks.get(self.pos + 1)) {
                if *w == class.name {
                    self.next();
                    let params = self.params();
                    class.methods.push(MapMethod { name: class.name.clone(), was: None, params });
                    if self.is_punct('{') {
                        self.skip_group();
                    } else {
                        self.skip_to_semicolon();
                    }
                    continue;
                }
            }
            let Some(ty) = self.ty() else {
                self.pos = start + 1;
                continue;
            };
            let name = match self.next() {
                Some(Tok::Word(w)) => w,
                _ => {
                    self.skip_to_semicolon();
                    continue;
                }
            };
            if self.is_punct('(') {
                let params = self.params();
                let mut was = None;
                // `/* was x */` may stand before or after a `throws` clause
                while let Some(t) = self.peek() {
                    match t {
                        Tok::Was(w) => {
                            was = Some(w.clone());
                            self.next();
                        }
                        Tok::Punct(';') => {
                            self.next();
                            break;
                        }
                        Tok::Punct('{') => {
                            self.skip_group();
                            break;
                        }
                        _ => {
                            self.next();
                        }
                    }
                }
                class.methods.push(MapMethod { name, was, params });
            } else {
                let mut was = None;
                if let Some(Tok::Was(w)) = self.peek() {
                    was = Some(w.clone());
                    self.next();
                }
                self.skip_to_semicolon();
                if let Some(was) = was {
                    class.fields.push(MapField { name, was, ty });
                }
            }
        }
    }

    fn params(&mut self) -> Vec<(String, String)> {
        // at '('
        self.next();
        let mut out = Vec::new();
        loop {
            if self.is_punct(')') {
                self.next();
                return out;
            }
            if self.peek().is_none() {
                return out;
            }
            let Some(ty) = self.ty() else { continue };
            let name = match self.next() {
                Some(Tok::Word(w)) => w,
                _ => String::new(),
            };
            out.push((ty, name));
            if self.is_punct(',') {
                self.next();
            }
        }
    }

    /// At `class`/`interface`/`enum`.
    fn type_decl(&mut self) -> Option<MapClass> {
        self.next();
        let name = match self.next()? {
            Tok::Word(w) => w,
            _ => return None,
        };
        let mut class = MapClass { name, ..Default::default() };
        while let Some(t) = self.peek() {
            match t {
                Tok::Was(w) => {
                    class.was = Some(w.clone());
                    self.next();
                }
                Tok::Punct('{') => break,
                Tok::Punct('<') => {
                    self.next();
                }
                _ => {
                    self.next();
                }
            }
        }
        if self.is_punct('{') {
            self.class_body(&mut class);
        }
        Some(class)
    }
}

/// Reads the class/field/method/parameter names of one `.map` file.  Semantic
/// annotations, domains without a `was` marker and anything else are skipped.
pub fn parse_map(text: &str) -> Vec<MapClass> {
    let mut p = Parser { toks: tokenize(text), pos: 0 };
    let mut out = Vec::new();
    while p.peek().is_some() {
        if matches!(p.word(), Some("package" | "import")) {
            p.skip_to_semicolon();
            continue;
        }
        p.skip_decorations();
        if matches!(p.word(), Some("class" | "interface" | "enum")) {
            if let Some(c) = p.type_decl() {
                out.push(c);
            }
        } else {
            p.next();
        }
    }
    out
}

/// What the generator knows about a class of the input, for resolving a map.
pub struct InputClass<'a> {
    pub flat: &'a str,
    /// `(name, type)` of every field.
    pub fields: Vec<(&'a str, &'a str)>,
    /// `(name, parameter types)` of every method; constructors are `<init>`.
    pub methods: Vec<(&'a str, Vec<&'a str>)>,
}

/// New Java names for one method.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MethodNames {
    pub name: Option<String>,
    pub params: Vec<String>,
}

/// A name map resolved against the input program.
#[derive(Debug, Default)]
pub struct NameMap {
    classes: HashMap<String, String>,
    fields: HashMap<(String, String), String>,
    methods: HashMap<(String, String, Vec<String>), MethodNames>,
    pub applied: usize,
    pub warnings: Vec<String>,
}

fn strip_array(t: &str) -> (&str, usize) {
    let mut t = t.trim();
    let mut dims = 0;
    while let Some(x) = t.strip_suffix("[]") {
        t = x;
        dims += 1;
    }
    (t, dims)
}

/// Last segment of a type name (`javax.microedition.lcdui.Graphics` → `Graphics`), arrays kept.
fn simple_type(t: &str) -> String {
    let (base, dims) = strip_array(t);
    format!("{}{}", simple_of(base), "[]".repeat(dims))
}

impl NameMap {
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty() && self.fields.is_empty() && self.methods.is_empty()
    }

    /// Resolves parsed map classes against the input program.
    pub fn resolve(maps: &[MapClass], input: &[InputClass]) -> NameMap {
        let mut nm = NameMap::default();
        let by_flat: BTreeMap<&str, &InputClass> = input.iter().map(|c| (c.flat, c)).collect();
        let mut flat_list = Vec::new();
        fn flatten<'m>(c: &'m MapClass, out: &mut Vec<&'m MapClass>) {
            out.push(c);
            for n in &c.nested {
                flatten(n, out);
            }
        }
        for m in maps {
            flatten(m, &mut flat_list);
        }
        // 1. classes: original flat name for every map class
        let mut owner_of: Vec<Option<String>> = Vec::new();
        for m in &flat_list {
            let key = m.was.clone().unwrap_or_else(|| m.name.clone()).replace('/', ".");
            let found: Vec<&str> = if by_flat.contains_key(key.as_str()) {
                vec![by_flat[key.as_str()].flat]
            } else {
                input.iter().filter(|c| simple_of(c.flat) == key).map(|c| c.flat).collect()
            };
            match found.as_slice() {
                [one] => {
                    if m.was.is_some() && m.name != simple_of(one) {
                        nm.classes.insert(one.to_string(), m.name.clone());
                        nm.applied += 1;
                    }
                    owner_of.push(Some(one.to_string()));
                }
                [] => {
                    if m.was.is_some() {
                        nm.warnings.push(format!("class {} (was {key}): not in the input", m.name));
                    }
                    owner_of.push(None);
                }
                _ => {
                    nm.warnings.push(format!("class {} (was {key}): ambiguous, {} candidates", m.name, found.len()));
                    owner_of.push(None);
                }
            }
        }
        // reverse map: new simple class name -> original simple name, for parameter types
        let reverse: HashMap<String, String> =
            nm.classes.iter().map(|(orig, new)| (new.clone(), simple_of(orig).to_string())).collect();
        let original_type = |t: &str| -> String {
            let (base, dims) = strip_array(t);
            let s = simple_of(base);
            let s = reverse.get(s).map(String::as_str).unwrap_or(s);
            format!("{s}{}", "[]".repeat(dims))
        };
        // 2. members
        for (m, owner) in flat_list.iter().zip(&owner_of) {
            let Some(owner) = owner else { continue };
            let ic = by_flat[owner.as_str()];
            for f in &m.fields {
                let cands: Vec<&(&str, &str)> = ic.fields.iter().filter(|(n, _)| *n == f.was).collect();
                let pick = if cands.len() > 1 {
                    let want = original_type(&f.ty);
                    cands.into_iter().filter(|(_, t)| simple_type(t) == want).collect::<Vec<_>>()
                } else {
                    cands
                };
                match pick.as_slice() {
                    [(n, _)] => {
                        if f.name != *n {
                            nm.fields.insert((owner.clone(), n.to_string()), f.name.clone());
                            nm.applied += 1;
                        }
                    }
                    [] => nm.warnings.push(format!("field {}.{} (was {}): not in the input", m.name, f.name, f.was)),
                    _ => nm.warnings.push(format!("field {}.{} (was {}): ambiguous", m.name, f.name, f.was)),
                }
            }
            for mm in &m.methods {
                let old = mm.was.clone().unwrap_or_else(|| "<init>".to_string());
                let want: Vec<String> = mm.params.iter().map(|(t, _)| original_type(t)).collect();
                let hit = ic.methods.iter().find(|(n, ps)| {
                    *n == old && ps.len() == want.len() && ps.iter().zip(&want).all(|(p, w)| simple_type(p) == *w)
                });
                let Some((n, ps)) = hit else {
                    let what = if mm.was.is_some() { format!("method {}.{}", m.name, mm.name) } else { format!("constructor {}", m.name) };
                    nm.warnings.push(format!("{what}({}) (was {old}): not in the input", want.join(", ")));
                    continue;
                };
                let names = MethodNames {
                    name: if mm.was.is_some() && mm.name != *n { Some(mm.name.clone()) } else { None },
                    params: mm.params.iter().map(|(_, p)| p.clone()).collect(),
                };
                nm.applied += 1;
                let key = (owner.clone(), n.to_string(), ps.iter().map(|p| simple_type(p)).collect());
                nm.methods.insert(key, names);
            }
        }
        nm
    }

    /// New simple name of a class, if the map renames it.
    pub fn class(&self, flat: &str) -> Option<&str> {
        self.classes.get(flat).map(String::as_str)
    }

    pub fn field(&self, owner: &str, name: &str) -> Option<&str> {
        self.fields.get(&(owner.to_string(), name.to_string())).map(String::as_str)
    }

    /// New names of a method, matched on the simple names of its parameter types.
    pub fn method(&self, owner: &str, name: &str, ptypes: &[String]) -> Option<&MethodNames> {
        let key = (owner.to_string(), name.to_string(), ptypes.iter().map(|p| simple_type(p)).collect());
        self.methods.get(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casing() {
        assert_eq!(snake_case("frameCounter"), "frame_counter");
        assert_eq!(snake_case("HTTPServer"), "http_server");
        assert_eq!(snake_case("Outer$Inner"), "outer_inner");
        assert_eq!(snake_case("MAX_LIVES"), "max_lives");
        assert_eq!(snake_case("level2Score"), "level2_score");
        assert_eq!(snake_case("get2D"), "get2d");
        assert_eq!(snake_case("b_3c1de266"), "b_3c1de266");
        assert_eq!(camel_case("a"), "A");
        assert_eq!(camel_case("game_engine"), "GameEngine");
        assert_eq!(camel_case("SynthCanvas"), "SynthCanvas");
        assert_eq!(screaming_case("maxLives"), "MAX_LIVES");
        assert_eq!(screaming_case("PATTERN_COLOR"), "PATTERN_COLOR");
        assert_eq!(screaming_case("_1"), "S__1");
        assert_eq!(value_base("__old"), "j__old");
    }

    #[test]
    fn keywords_and_collisions() {
        assert_eq!(legal("type"), "r#type");
        assert_eq!(legal("self"), "self_");
        assert_eq!(module_base("Std"), "std_");
        assert_eq!(struct_base("Box", &[]), "Box_");
        let mut s = Scope::with_reserved(&["peer"]);
        assert_eq!(s.claim("get_x"), "get_x");
        assert_eq!(s.claim("get_x"), "get_x_2");
        assert_eq!(s.claim("peer"), "peer_2");
        assert_eq!(s.claim("match"), "r#match");
    }

    #[test]
    fn parses_names_part_of_a_map() {
        let text = r#"
            package sample.game;
            import javax.microedition.lcdui.Graphics;
            // a comment
            @ValueDomain interface Direction { byte LEFT = 2; }
            class GameEngine /* was af */ {
                GameEngine(int width, int height) {}
                @Domain(Direction.class) int frameCounter /* was a */;
                java.util.Hashtable cache /* was b */;
                void draw(Graphics graphics, int x, int y) /* was a */;
                int tick(@Domain(Direction.class) int frame, long delta) /* was c */;
            }
        "#;
        let m = parse_map(text);
        assert_eq!(m.len(), 2);
        let g = &m[1];
        assert_eq!(g.name, "GameEngine");
        assert_eq!(g.was.as_deref(), Some("af"));
        assert_eq!(g.fields.len(), 2);
        assert_eq!(g.fields[0], MapField { name: "frameCounter".into(), was: "a".into(), ty: "int".into() });
        assert_eq!(g.methods.len(), 3);
        assert_eq!(g.methods[0].was, None);
        assert_eq!(g.methods[1].name, "draw");
        assert_eq!(g.methods[1].params[0], ("Graphics".to_string(), "graphics".to_string()));
        assert_eq!(g.methods[2].params[0], ("int".to_string(), "frame".to_string()));

        let input = [
            InputClass {
                flat: "af",
                fields: vec![("a", "int"), ("b", "java.util.Hashtable")],
                methods: vec![
                    ("<init>", vec!["int", "int"]),
                    ("a", vec!["javax.microedition.lcdui.Graphics", "int", "int"]),
                    ("c", vec!["int", "long"]),
                ],
            },
            InputClass { flat: "Midlet", fields: vec![], methods: vec![] },
        ];
        let nm = NameMap::resolve(&m, &input);
        assert!(nm.warnings.is_empty(), "{:?}", nm.warnings);
        assert_eq!(nm.class("af"), Some("GameEngine"));
        assert_eq!(nm.field("af", "a"), Some("frameCounter"));
        let ptypes = vec!["javax.microedition.lcdui.Graphics".to_string(), "int".into(), "int".into()];
        let mn = nm.method("af", "a", &ptypes).unwrap();
        assert_eq!(mn.name.as_deref(), Some("draw"));
        assert_eq!(mn.params, vec!["graphics", "x", "y"]);
        let ctor = nm.method("af", "<init>", &["int".to_string(), "int".to_string()]).unwrap();
        assert_eq!(ctor.params, vec!["width", "height"]);
    }
}
