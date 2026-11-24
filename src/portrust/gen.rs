//! The Rust generator: javac-attributed AST (JSON from `assets/JavacAstDump.java`)
//! → a Cargo crate on top of the `jrt` runtime.
//!
//! * a class becomes a module with a `struct` of its instance fields; a method a
//!   free function `fn m(this: &jrt::JRef<K>, …)`; a static field a `thread_local!`
//!   cell read with `.val()` and written with `.set()`;
//! * arithmetic keeps Java semantics (wrapping, `idiv`/`irem` with
//!   `ArithmeticException`, masked shifts, narrowing by `as`) — the operand types
//!   come from javac, so implicit widening is explicit in the output;
//! * `switch` becomes a `match`, or the general fall-through form; labelled
//!   `break`/`continue` become labelled blocks; `try`/`catch` a closure under
//!   `jrt::catch` with `jrt::Flow` carrying `return`/`break`/`continue` out of it;
//! * platform calls are bound by the API table (`assets/port-rust-api.tsv`); an
//!   unknown call is never guessed.
//!
//! Every construct it cannot translate becomes a typed `jrt::todo_ai!(T; why, java)`
//! (so the crate still compiles), a line in `residue.json`, and a count in
//! `report.txt`.

use super::names::{self, NameMap, Scope};
use serde_json::Value as J;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------- JSON helpers

fn s<'a>(v: &'a J, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}
fn b(v: &J, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or(false)
}
fn arr<'a>(v: &'a J, k: &str) -> &'a [J] {
    v.get(k).and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[])
}
fn int(v: &J, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or(0)
}
fn strings(v: &J, k: &str) -> Vec<String> {
    arr(v, k).iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()
}

fn is_prim(t: &str) -> bool {
    matches!(t, "int" | "byte" | "short" | "char" | "boolean" | "long" | "float" | "double")
}
fn is_ref(t: &str) -> bool {
    !is_prim(t) && t != "void"
}
fn prim_rs(t: &str) -> &'static str {
    match t {
        "int" => "i32",
        "byte" => "i8",
        "short" => "i16",
        "char" => "u16",
        "boolean" => "bool",
        "long" => "i64",
        "float" => "f32",
        "double" => "f64",
        _ => "()",
    }
}

/// Numeric conversion with Java semantics (Rust `as` matches the JVM's
/// i2b/i2c/i2s/l2i/f2i… for every case emitted).
fn conv(code: String, from: &str, to: &str) -> String {
    if from == to || !is_prim(from) || !is_prim(to) || from == "boolean" || to == "boolean" {
        return code;
    }
    format!("({code} as {})", prim_rs(to))
}

// ---------------------------------------------------------------- API table

/// `assets/port-rust-api.tsv`: `key<TAB>template` per platform member, and
/// `type<TAB>java.type<TAB>rust::Type` per platform type.  In a template `{r}` is
/// `&receiver` and `{0}`, `{1}`… are the converted arguments.
#[derive(Default)]
pub struct ApiTable {
    calls: HashMap<String, String>,
    types: HashMap<String, String>,
}

impl ApiTable {
    pub fn parse(text: &str) -> ApiTable {
        let mut t = ApiTable::default();
        for l in text.lines() {
            if l.trim().is_empty() || l.starts_with('#') {
                continue;
            }
            let mut p = l.splitn(3, '\t');
            let (k0, k1, k2) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""));
            if k0 == "type" {
                t.types.insert(k1.to_string(), k2.to_string());
            } else {
                t.calls.insert(k0.to_string(), k1.to_string());
            }
        }
        t
    }
    pub fn len(&self) -> usize {
        self.calls.len()
    }
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

// ---------------------------------------------------------------- model

#[derive(Clone)]
struct MethodInfo {
    /// Rust function name (for a constructor: the one that allocates).
    rname: String,
    /// Constructor only: the function that runs the body on an allocated object.
    construct: String,
    is_static: bool,
    /// Parameter names from the name map (empty: keep the Java ones).
    param_names: Vec<String>,
}

struct ClassInfo {
    flat: String,
    sup: String,
    ifaces: Vec<String>,
    module: String,
    strukt: String,
    /// Java field name → Rust name (instance: snake_case, static: SCREAMING).
    fields: HashMap<String, String>,
    methods: HashMap<(String, String), MethodInfo>,
}

/// Counts for the report.
#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub methods: usize,
    pub methods_clean: usize,
    pub stmts: usize,
    pub todo_sites: usize,
    lines: BTreeSet<(String, i64)>,
    todo_lines: BTreeSet<(String, i64)>,
    pub why: BTreeMap<String, usize>,
    pub api_used: BTreeMap<String, usize>,
    pub api_missing: BTreeMap<String, usize>,
}

impl Stats {
    pub fn source_lines(&self) -> usize {
        self.lines.len()
    }
    pub fn todo_source_lines(&self) -> usize {
        self.todo_lines.intersection(&self.lines).count()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Residue {
    pub class: String,
    pub method: String,
    pub why: String,
    pub line: i64,
    pub java: String,
}

#[derive(Clone, Copy, PartialEq)]
enum TKind {
    While,
    /// for / do: `continue` = break out of the inner `'c` block
    LoopWithCont,
    Switch,
    Block,
}

struct TryFrame {
    declared: HashSet<i64>,
    escapes: BTreeSet<(char, i64)>, // ('b'|'c', target)
}

struct Ctx {
    class: String,
    method: String,
    ret: String,
    /// Java local/parameter name → Rust name.
    locals: HashMap<String, String>,
    kinds: HashMap<i64, TKind>,
    tries: Vec<TryFrame>,
    tmp: usize,
    todo_here: usize,
    line: i64,
}

impl Ctx {
    fn new(class: &str, method: &str, ret: &str) -> Ctx {
        Ctx {
            class: class.into(),
            method: method.into(),
            ret: ret.into(),
            locals: HashMap::new(),
            kinds: HashMap::new(),
            tries: vec![],
            tmp: 0,
            todo_here: 0,
            line: 0,
        }
    }
    fn local(&self, java: &str) -> String {
        self.locals.get(java).cloned().unwrap_or_else(|| names::legal(&names::value_base(java)))
    }
}

struct Gen<'a> {
    classes: HashMap<String, ClassInfo>,
    api: &'a ApiTable,
    stats: Stats,
    residue: Vec<Residue>,
}

const CALLBACKS: &[(&str, &str, &str)] = &[
    ("run", "()void", "fn run(&self) { {m}(&self.0) }"),
    ("paint", "(javax.microedition.lcdui.Graphics)void", "fn paint(&self, g: jrt::api::Graphics) { {m}(&self.0, g) }"),
    ("keyPressed", "(int)void", "fn key_pressed(&self, k: i32) { {m}(&self.0, k) }"),
    ("keyReleased", "(int)void", "fn key_released(&self, k: i32) { {m}(&self.0, k) }"),
    ("keyRepeated", "(int)void", "fn key_repeated(&self, k: i32) { {m}(&self.0, k) }"),
    ("showNotify", "()void", "fn show_notify(&self) { {m}(&self.0) }"),
    ("hideNotify", "()void", "fn hide_notify(&self) { {m}(&self.0) }"),
    ("sizeChanged", "(int,int)void", "fn size_changed(&self, w: i32, h: i32) { {m}(&self.0, w, h) }"),
    ("startApp", "()void", "fn start_app(&self) { {m}(&self.0) }"),
    ("pauseApp", "()void", "fn pause_app(&self) { {m}(&self.0) }"),
    ("destroyApp", "(boolean)void", "fn destroy_app(&self, u: bool) { {m}(&self.0, u) }"),
];

/// Platform types a user object may be handed to (it then travels as a `jrt::Peer`).
const PEER_TYPES: &[&str] = &[
    "java.lang.Runnable",
    "javax.microedition.lcdui.Displayable",
    "javax.microedition.lcdui.Canvas",
    "javax.microedition.lcdui.game.GameCanvas",
    "javax.microedition.midlet.MIDlet",
    "javax.microedition.lcdui.CommandListener",
    "java.lang.Thread",
];

/// Name of the per-class glue struct that implements `jrt::Callbacks`.
const PEER_STRUCT: &str = "PlatformPeer";
/// Function names every module defines itself.
const MODULE_FNS: &[&str] = &["static_init", "init_fields", "peer"];

impl Gen<'_> {
    fn rty(&mut self, t: &str) -> String {
        if let Some(e) = t.strip_suffix("[]") {
            return format!("jrt::JArr<{}>", self.rty(e));
        }
        if is_prim(t) {
            return prim_rs(t).into();
        }
        if t == "void" {
            return "()".into();
        }
        if t == "java.lang.String" {
            return "jrt::JStr".into();
        }
        if let Some(ci) = self.classes.get(t) {
            return format!("jrt::JRef<crate::{}::{}>", ci.module, ci.strukt);
        }
        if let Some(r) = self.api.types.get(t) {
            return r.clone();
        }
        *self.stats.api_missing.entry(format!("type {t}")).or_default() += 1;
        "jrt::Unknown".into()
    }

    fn todo(&mut self, cx: &mut Ctx, why: &str, java: &str, line: i64) -> String {
        let line = if line == 0 { cx.line } else { line };
        self.stats.todo_sites += 1;
        cx.todo_here += 1;
        *self.stats.why.entry(why.to_string()).or_default() += 1;
        let nl = java.matches('\n').count() as i64;
        for l in line..=line + nl {
            self.stats.todo_lines.insert((cx.class.clone(), l));
        }
        self.residue.push(Residue {
            class: cx.class.clone(),
            method: cx.method.clone(),
            why: why.into(),
            line,
            java: java.into(),
        });
        let short: String = java.chars().take(160).collect();
        format!("jrt::todo_ai!({:?}, {:?})", why, short)
    }

    fn find_method(&self, owner: &str, name: &str, desc: &str) -> Option<(String, MethodInfo)> {
        let mut c = owner.to_string();
        loop {
            let ci = self.classes.get(&c)?;
            if let Some(m) = ci.methods.get(&(name.to_string(), desc.to_string())) {
                return Some((c, m.clone()));
            }
            c = ci.sup.clone();
        }
    }

    /// Does any user class that derives from `owner` redeclare the method?
    fn overridden(&self, owner: &str, name: &str, desc: &str) -> bool {
        self.classes.values().any(|ci| {
            if ci.flat == owner {
                return false;
            }
            let mut c = ci.sup.clone();
            while let Some(p) = self.classes.get(&c) {
                if p.flat == owner {
                    return ci.methods.contains_key(&(name.to_string(), desc.to_string()));
                }
                c = p.sup.clone();
            }
            false
        })
    }

    fn module(&self, class: &str) -> String {
        self.classes[class].module.clone()
    }

    /// Rust name of a field declared in user class `owner`.
    fn field(&self, owner: &str, name: &str) -> String {
        self.classes
            .get(owner)
            .and_then(|c| c.fields.get(name))
            .cloned()
            .unwrap_or_else(|| names::legal(&names::value_base(name)))
    }

    fn static_field(&self, owner: &str, name: &str) -> String {
        format!("crate::{}::{}", self.module(owner), self.field(owner, name))
    }

    /// Converts an argument value of Java type `from` for a parameter of type `to`.
    fn arg(&mut self, code: String, from: &str, to: &str) -> String {
        if self.classes.contains_key(from) && PEER_TYPES.contains(&to) {
            return format!("crate::{}::peer({code})", self.module(from));
        }
        conv(code, from, to)
    }

    // ------------------------------------------------------------ expressions

    fn lit(&mut self, e: &J) -> String {
        let kind = s(e, "kind");
        match kind {
            "string" => format!("jrt::JStr::lit({:?})", s(e, "v")),
            "bool" => format!("{}", b(e, "v")),
            "char" => {
                let v = int(e, "v");
                match char::from_u32(v as u32) {
                    Some(c) if (' '..='~').contains(&c) && c != '\'' && c != '\\' => format!("('{c}' as u16)"),
                    _ => format!("{v}u16"),
                }
            }
            "float" => {
                let bits: u32 = s(e, "bits").parse::<i64>().unwrap_or(0) as u32;
                let f = f32::from_bits(bits);
                if f.is_finite() {
                    format!("{f:?}f32")
                } else {
                    format!("f32::from_bits({bits:#x})")
                }
            }
            "double" => {
                let bits: u64 = s(e, "bits").parse::<i64>().unwrap_or(0) as u64;
                let f = f64::from_bits(bits);
                if f.is_finite() {
                    format!("{f:?}f64")
                } else {
                    format!("f64::from_bits({bits:#x})")
                }
            }
            "long" => {
                let v = int(e, "v");
                if v == i64::MIN {
                    "i64::MIN".into()
                } else if v < 0 {
                    format!("({v}i64)")
                } else {
                    format!("{v}i64")
                }
            }
            "int" => {
                let v = int(e, "v");
                if v == i32::MIN as i64 {
                    "i32::MIN".into()
                } else if v < 0 {
                    format!("({v})")
                } else {
                    format!("{v}")
                }
            }
            "byte" | "short" => format!("({}{})", int(e, "v"), prim_rs(kind)),
            _ => format!("/*?lit*/{}", s(e, "v")),
        }
    }

    fn expr(&mut self, e: &J, cx: &mut Ctx) -> String {
        let code = self.expr0(e, cx);
        let t = s(e, "t");
        if code.starts_with("jrt::todo_ai!(\"") && !t.is_empty() && t != "void" && t != "<nulltype>" {
            let rt = self.rty(t);
            return code.replacen("jrt::todo_ai!(", &format!("jrt::todo_ai!({rt}; "), 1);
        }
        code
    }

    fn field_read(&mut self, e: &J, cx: &mut Ctx, target: Option<&J>) -> String {
        let name = s(e, "name");
        let owner = s(e, "owner");
        if b(e, "static") {
            if self.classes.contains_key(owner) {
                format!("{}.val()", self.static_field(owner, name))
            } else {
                self.api_field(owner, name, cx, e)
            }
        } else if self.classes.contains_key(owner) {
            let f = self.field(owner, name);
            match target {
                Some(t) => {
                    let o = self.expr(t, cx);
                    format!("jrt::get!({o}, {f})")
                }
                None => format!("jrt::get!(this, {f})"),
            }
        } else {
            let why = if target.is_some() { "platform instance field" } else { "inherited platform field" };
            self.todo(cx, why, s(e, "src"), 0)
        }
    }

    fn expr0(&mut self, e: &J, cx: &mut Ctx) -> String {
        let k = s(e, "k");
        let t = s(e, "t").to_string();
        match k {
            "lit" => self.lit(e),
            "null" => "Default::default()".into(),
            "ident" => {
                let name = s(e, "name");
                match s(e, "sym") {
                    "local" => {
                        let n = cx.local(name);
                        if is_ref(&t) {
                            format!("{n}.clone()")
                        } else {
                            n
                        }
                    }
                    "field" if name == "this" => "this.clone()".into(),
                    "field" => self.field_read(e, cx, None),
                    _ if name == "this" => "this.clone()".into(),
                    other => self.todo(cx, &format!("ident {other}"), s(e, "src"), 0),
                }
            }
            "select" => {
                let name = s(e, "name");
                let target = &e["target"];
                if name == "length" && s(target, "t").ends_with("[]") {
                    let a = self.expr(target, cx);
                    return format!("{a}.len()");
                }
                match s(e, "sym") {
                    "field" => self.field_read(e, cx, Some(target)),
                    _ if name == "this" => "this.clone()".into(),
                    other => self.todo(cx, &format!("select {other}"), s(e, "src"), 0),
                }
            }
            "call" => self.call(e, cx),
            "new" => self.new_obj(e, cx),
            "newarray" => {
                let elem_t = t.strip_suffix("[]").unwrap_or("int").to_string();
                let dims = arr(e, "dims");
                if let Some(elems) = e.get("elems").and_then(|x| x.as_array()) {
                    let ert = self.rty(&elem_t);
                    let mut parts = Vec::new();
                    for x in elems {
                        let c = self.expr(x, cx);
                        parts.push(self.arg(c, s(x, "t"), &elem_t));
                    }
                    return format!("jrt::JArr::<{ert}>::from_vec(vec![{}])", parts.join(", "));
                }
                if dims.len() == 1 {
                    let n = self.expr(&dims[0], cx);
                    let ert = self.rty(&elem_t);
                    let n = conv(n, s(&dims[0], "t"), "int");
                    format!("jrt::JArr::<{ert}>::new({n})")
                } else if dims.len() == 2 {
                    let inner = elem_t.strip_suffix("[]").unwrap_or("int").to_string();
                    let irt = self.rty(&inner);
                    let a0 = self.expr(&dims[0], cx);
                    let a1 = self.expr(&dims[1], cx);
                    let a0 = conv(a0, s(&dims[0], "t"), "int");
                    let a1 = conv(a1, s(&dims[1], "t"), "int");
                    format!("jrt::JArr::<jrt::JArr<{irt}>>::new_2d({a0}, {a1})")
                } else {
                    self.todo(cx, "array with 3+ dims", s(e, "src"), 0)
                }
            }
            "bin" => self.binary(e, cx),
            "unary" => self.unary(e, cx, false),
            "assign" | "assignop" => self.assign(e, cx, false),
            "cond" => {
                let c = self.expr(&e["c"], cx);
                let a = self.expr(&e["a"], cx);
                let a = self.arg(a, s(&e["a"], "t"), &t);
                let bb = self.expr(&e["b"], cx);
                let bb = self.arg(bb, s(&e["b"], "t"), &t);
                format!("(if {c} {{ {a} }} else {{ {bb} }})")
            }
            "cast" => {
                let inner = &e["e"];
                let it = s(inner, "t").to_string();
                let c = self.expr(inner, cx);
                if is_prim(&t) {
                    conv(c, &it, &t)
                } else if it == t || it == "<nulltype>" {
                    c
                } else if t == "java.lang.Object" {
                    // only ever handed to System.out.println, a no-op in the port
                    format!("{{ let _ = {c}; jrt::Unknown::default() }}")
                } else if self.rty(&it) == self.rty(&t) {
                    c
                } else {
                    self.todo(cx, "reference cast", s(e, "src"), 0)
                }
            }
            "index" => {
                let a = self.expr(&e["a"], cx);
                let i = self.expr(&e["i"], cx);
                let i = conv(i, s(&e["i"], "t"), "int");
                format!("{a}.get({i})")
            }
            "instanceof" => self.todo(cx, "instanceof", s(e, "src"), 0),
            other => self.todo(cx, &format!("expr {other}"), s(e, "src"), 0),
        }
    }

    fn api_field(&mut self, owner: &str, name: &str, cx: &mut Ctx, e: &J) -> String {
        let key = format!("{owner}.{name}");
        if let Some(tpl) = self.api.calls.get(&key).cloned() {
            *self.stats.api_used.entry(key).or_default() += 1;
            tpl
        } else {
            *self.stats.api_missing.entry(key.clone()).or_default() += 1;
            self.todo(cx, &format!("api {key}"), s(e, "src"), 0)
        }
    }

    fn args(&mut self, e: &J, cx: &mut Ctx) -> Vec<String> {
        let ptypes = strings(e, "ptypes");
        let mut out = Vec::new();
        for (i, a) in arr(e, "args").iter().enumerate() {
            let c = self.expr(a, cx);
            let to = ptypes.get(i).cloned().unwrap_or_default();
            out.push(self.arg(c, s(a, "t"), &to));
        }
        out
    }

    fn fill_template(tpl: &str, recv: Option<String>, args: &[String]) -> String {
        let mut out = tpl.to_string();
        if let Some(r) = recv {
            out = out.replace("{r}", &format!("(&{r})"));
        }
        for (i, a) in args.iter().enumerate() {
            out = out.replace(&format!("{{{i}}}"), a);
        }
        out
    }

    fn call(&mut self, e: &J, cx: &mut Ctx) -> String {
        let owner = platform_alias(s(e, "owner")).to_string();
        let name = s(e, "name").to_string();
        let desc = s(e, "desc").to_string();
        let implicit = e.get("implicit").and_then(|x| x.as_str());
        let args = self.args(e, cx);
        // this(...) / super(...)
        if implicit == Some("super") {
            if self.classes.contains_key(&owner) {
                return self.todo(cx, "super constructor of a user class", s(e, "src"), 0);
            }
            return "() /* super() of a platform class */".into();
        }
        if implicit == Some("this") {
            if let Some((_, m)) = self.find_method(&owner, "<init>", &desc) {
                let mut all = vec!["this".to_string()];
                all.extend(args);
                return format!("crate::{}::{}({})", self.module(&owner), m.construct, all.join(", "));
            }
        }
        if self.classes.contains_key(&owner) {
            let Some((decl_owner, m)) = self.find_method(&owner, &name, &desc) else {
                return self.todo(cx, "unresolved user method", s(e, "src"), 0);
            };
            if !m.is_static && self.overridden(&decl_owner, &name, &desc) {
                return self.todo(cx, "virtual call with overrides", s(e, "src"), 0);
            }
            let path = format!("crate::{}::{}", self.module(&decl_owner), m.rname);
            if m.is_static {
                return format!("{path}({})", args.join(", "));
            }
            let recv = match e.get("recv") {
                Some(r) => {
                    if s(r, "k") == "ident" && s(r, "name") == "super" {
                        return self.todo(cx, "super.method()", s(e, "src"), 0);
                    }
                    self.expr(r, cx)
                }
                None => "this.clone()".into(),
            };
            let mut all = vec![format!("&{recv}")];
            all.extend(args);
            return format!("{path}({})", all.join(", "));
        }
        // platform API
        let key = format!("{owner}.{name}{desc}");
        let Some(tpl) = self.api.calls.get(&key).cloned() else {
            *self.stats.api_missing.entry(key.clone()).or_default() += 1;
            return self.todo(cx, &format!("api {key}"), s(e, "src"), 0);
        };
        *self.stats.api_used.entry(key).or_default() += 1;
        let recv = if tpl.contains("{r}") {
            Some(match e.get("recv") {
                Some(r) => self.expr(r, cx),
                None => "this.clone()".into(),
            })
        } else {
            None
        };
        Self::fill_template(&tpl, recv, &args)
    }

    fn new_obj(&mut self, e: &J, cx: &mut Ctx) -> String {
        let class = s(e, "class").to_string();
        if b(e, "anon") {
            return self.todo(cx, "anonymous class", s(e, "src"), 0);
        }
        let args = self.args(e, cx);
        let desc = format!("({})void", strings(e, "ptypes").join(","));
        if self.classes.contains_key(&class) {
            let Some((_, m)) = self.find_method(&class, "<init>", &desc) else {
                return self.todo(cx, "unresolved constructor", s(e, "src"), 0);
            };
            return format!("crate::{}::{}({})", self.module(&class), m.rname, args.join(", "));
        }
        let key = format!("{class}.<init>{desc}");
        let Some(tpl) = self.api.calls.get(&key).cloned() else {
            *self.stats.api_missing.entry(key.clone()).or_default() += 1;
            return self.todo(cx, &format!("api {key}"), s(e, "src"), 0);
        };
        *self.stats.api_used.entry(key).or_default() += 1;
        Self::fill_template(&tpl, None, &args)
    }

    fn concat_pieces(&mut self, e: &J, cx: &mut Ctx, out: &mut Vec<String>) {
        if s(e, "k") == "bin" && s(e, "op") == "PLUS" && s(e, "t") == "java.lang.String" {
            self.concat_pieces(&e["l"], cx, out);
            self.concat_pieces(&e["r"], cx, out);
            return;
        }
        let t = s(e, "t").to_string();
        let c = self.expr(e, cx);
        let p = match t.as_str() {
            "java.lang.String" => format!("jrt::Piece::S({c})"),
            "int" | "byte" | "short" | "long" => format!("jrt::Piece::I({c} as i64)"),
            "char" => format!("jrt::Piece::C({c})"),
            "boolean" => format!("jrt::Piece::B({c})"),
            "java.lang.Exception" | "java.lang.Throwable" | "java.io.IOException" => {
                format!("jrt::Piece::S(jrt::exc_string(&{c}))")
            }
            "java.lang.Object" => "jrt::Piece::S(jrt::JStr::lit(\"<object>\"))".to_string(),
            "java.lang.StringBuffer" => format!("jrt::Piece::S(jrt::api::string_buffer::to_string(&{c}))"),
            _ => self.todo(cx, &format!("string concat of {t}"), s(e, "src"), 0),
        };
        out.push(p);
    }

    fn binary(&mut self, e: &J, cx: &mut Ctx) -> String {
        let op = s(e, "op");
        let t = s(e, "t").to_string();
        if op == "PLUS" && t == "java.lang.String" {
            let mut p = Vec::new();
            self.concat_pieces(e, cx, &mut p);
            return format!("jrt::concat(vec![{}])", p.join(", "));
        }
        let lt = s(e, "lt").to_string();
        let rt = s(e, "rt").to_string();
        let (le, re) = (&e["l"], &e["r"]);
        // reference equality
        if (op == "EQ" || op == "NE") && is_ref(&lt) {
            let neg = if op == "NE" { "!" } else { "" };
            if s(re, "k") == "null" {
                let l = self.expr(le, cx);
                return format!("{neg}{l}.is_null()");
            }
            if s(le, "k") == "null" {
                let r = self.expr(re, cx);
                return format!("{neg}{r}.is_null()");
            }
            let l = self.expr(le, cx);
            let r = self.expr(re, cx);
            return format!("{neg}jrt::same(&{l}, &{r})");
        }
        let l = self.expr(le, cx);
        let l = conv(l, s(le, "t"), &lt);
        let r = self.expr(re, cx);
        let r = conv(r, s(re, "t"), &rt);
        let integral = matches!(lt.as_str(), "int" | "long");
        let long = lt == "long";
        let l = if integral { typed_lit(l, long) } else { l };
        match arith(op, &l, &r, integral, long, &rt) {
            Some(code) => code,
            None => match op {
                "AND" => format!("({l} && {r})"),
                "OR" => format!("({l} || {r})"),
                "EQ" => format!("({l} == {r})"),
                "NE" => format!("({l} != {r})"),
                "LT" => format!("({l} < {r})"),
                "GT" => format!("({l} > {r})"),
                "LE" => format!("({l} <= {r})"),
                "GE" => format!("({l} >= {r})"),
                other => self.todo(cx, &format!("binary {other}"), s(e, "src"), 0),
            },
        }
    }

    fn unary(&mut self, e: &J, cx: &mut Ctx, stmt: bool) -> String {
        let op = s(e, "op");
        let t = s(e, "t").to_string();
        match op {
            "PREINC" | "PREDEC" | "POSTINC" | "POSTDEC" => {
                let target = &e["e"];
                let lvt = s(target, "t").to_string();
                let Some(lv) = self.lvalue(target, cx) else {
                    return self.todo(cx, "inc/dec of unsupported lvalue", s(e, "src"), 0);
                };
                let wide = match lvt.as_str() {
                    "long" | "float" | "double" => lvt.clone(),
                    _ => "int".to_string(),
                };
                let one = match wide.as_str() {
                    "long" => "1i64",
                    "float" => "1.0f32",
                    "double" => "1.0f64",
                    _ => "1",
                };
                let f = if op.ends_with("INC") { "add" } else { "sub" };
                let calc = |old: &str| {
                    let w = conv(old.to_string(), &lvt, &wide);
                    let r = if wide == "int" || wide == "long" {
                        format!("{w}.wrapping_{f}({one})")
                    } else {
                        format!("({w} {} {one})", if f == "add" { "+" } else { "-" })
                    };
                    conv(r, &wide, &lvt)
                };
                if stmt {
                    if lv.pre.is_empty() && lv.simple {
                        return (lv.write)(&calc(&lv.read));
                    }
                    return format!("{{ {} let __old = {}; {}; }}", lv.pre.join(" "), lv.read, (lv.write)(&calc("__old")));
                }
                let result = if op.starts_with("PRE") { "__new" } else { "__old" };
                format!(
                    "{{ {} let __old = {}; let __new = {}; {}; {result} }}",
                    lv.pre.join(" "),
                    lv.read,
                    calc("__old"),
                    (lv.write)("__new")
                )
            }
            _ => {
                let inner = &e["e"];
                let ot = s(e, "ot").to_string();
                let c = self.expr(inner, cx);
                let c = conv(c, s(inner, "t"), if ot.is_empty() { &t } else { &ot });
                match op {
                    "NEG" if t == "int" || t == "long" => format!("{}.wrapping_neg()", typed_lit(c, t == "long")),
                    "NEG" => format!("(-{c})"),
                    "POS" => c,
                    "NOT" | "COMPL" => format!("(!{c})"),
                    other => self.todo(cx, &format!("unary {other}"), s(e, "src"), 0),
                }
            }
        }
    }

    fn lvalue(&mut self, e: &J, cx: &mut Ctx) -> Option<LValue> {
        let k = s(e, "k");
        let name = s(e, "name").to_string();
        let t = s(e, "t").to_string();
        let clone = if is_ref(&t) { ".clone()" } else { "" };
        match (k, s(e, "sym")) {
            ("ident", "local") => {
                let n = cx.local(&name);
                let n2 = n.clone();
                Some(LValue { pre: vec![], read: format!("{n}{clone}"), write: Box::new(move |v| format!("{n2} = {v}")), simple: true })
            }
            ("ident", "field") | ("select", "field") => {
                let owner = s(e, "owner").to_string();
                if !self.classes.contains_key(&owner) {
                    return None;
                }
                if b(e, "static") {
                    let p = self.static_field(&owner, &name);
                    let p2 = p.clone();
                    return Some(LValue { pre: vec![], read: format!("{p}.val()"), write: Box::new(move |v| format!("{p2}.set({v})")), simple: true });
                }
                let fname = self.field(&owner, &name);
                let f2 = fname.clone();
                if k == "ident" {
                    return Some(LValue {
                        pre: vec![],
                        read: format!("jrt::get!(this, {fname})"),
                        write: Box::new(move |v| format!("jrt::put!(this, {f2}, {v})")),
                        simple: true,
                    });
                }
                let o = self.expr(&e["target"], cx);
                cx.tmp += 1;
                let tmp = format!("__o{}", cx.tmp);
                let t2 = tmp.clone();
                Some(LValue {
                    pre: vec![format!("let {tmp} = {o};")],
                    read: format!("jrt::get!({tmp}, {fname})"),
                    write: Box::new(move |v| format!("jrt::put!({t2}, {f2}, {v})")),
                    simple: false,
                })
            }
            ("index", _) => {
                let a = self.expr(&e["a"], cx);
                let i = self.expr(&e["i"], cx);
                let i = conv(i, s(&e["i"], "t"), "int");
                cx.tmp += 1;
                let n = cx.tmp;
                let (ta, ti) = (format!("__a{n}"), format!("__i{n}"));
                let (ta2, ti2) = (ta.clone(), ti.clone());
                Some(LValue {
                    pre: vec![format!("let {ta} = {a};"), format!("let {ti} = {i};")],
                    read: format!("{ta}.get({ti})"),
                    write: Box::new(move |v| format!("{ta2}.set({ti2}, {v})")),
                    simple: false,
                })
            }
            _ => None,
        }
    }

    fn assign(&mut self, e: &J, cx: &mut Ctx, stmt: bool) -> String {
        let l = &e["l"];
        let r = &e["r"];
        let lt = s(l, "t").to_string();
        let compound = s(e, "k") == "assignop";
        let Some(lv) = self.lvalue(l, cx) else {
            return self.todo(cx, "assignment to unsupported lvalue", s(e, "src"), 0);
        };
        let value = if compound {
            let op = s(e, "op").trim_end_matches("_ASG").to_string();
            let olt = s(e, "lt").to_string();
            let ort = s(e, "rt").to_string();
            let res = s(e, "res").to_string();
            if lt == "java.lang.String" {
                let mut p = vec!["jrt::Piece::S(__old.clone())".to_string()];
                self.concat_pieces(r, cx, &mut p);
                format!("jrt::concat(vec![{}])", p.join(", "))
            } else {
                let rc = self.expr(r, cx);
                let rc = conv(rc, s(r, "t"), &ort);
                let oldc = conv("__old".to_string(), &lt, &olt);
                let integral = matches!(olt.as_str(), "int" | "long");
                let long = olt == "long";
                let Some(calc) = arith(&op, &oldc, &rc, integral, long, &ort) else {
                    return self.todo(cx, &format!("compound {op}"), s(e, "src"), 0);
                };
                conv(calc, &res, &lt)
            }
        } else {
            let rc = self.expr(r, cx);
            self.arg(rc, s(r, "t"), &lt)
        };
        let old = if compound { format!("let __old = {};", lv.read) } else { String::new() };
        if stmt {
            if lv.pre.is_empty() && !compound {
                return (lv.write)(&value);
            }
            return format!("{{ {} {old} let __v = {value}; {}; }}", lv.pre.join(" "), (lv.write)("__v"));
        }
        format!("{{ {} {old} let __v = {value}; {}; __v }}", lv.pre.join(" "), (lv.write)("__v.clone()"))
    }

    // ------------------------------------------------------------ statements

    fn collect_kinds(&self, st: &J, kinds: &mut HashMap<i64, TKind>) {
        match s(st, "k") {
            "while" => {
                kinds.insert(int(st, "id"), TKind::While);
            }
            "for" | "do" => {
                kinds.insert(int(st, "id"), TKind::LoopWithCont);
            }
            "switch" => {
                kinds.insert(int(st, "id"), TKind::Switch);
            }
            "labeled" if !matches!(s(&st["body"], "k"), "while" | "for" | "do" | "switch") => {
                kinds.insert(int(st, "id"), TKind::Block);
            }
            _ => {}
        }
        for_each_child_stmt(st, &mut |x| self.collect_kinds(x, kinds));
    }

    fn jump(&mut self, cx: &mut Ctx, brk: bool, target: i64, st: &J) -> String {
        let Some(kind) = cx.kinds.get(&target).copied() else {
            return self.todo(cx, "jump to unknown target", s(st, "src"), int(st, "line"));
        };
        // leaving a try closure?
        if let Some(fr) = cx.tries.last_mut() {
            if !fr.declared.contains(&target) {
                fr.escapes.insert((if brk { 'b' } else { 'c' }, target));
                return format!("return jrt::Flow::{}({target})", if brk { "Brk" } else { "Cont" });
            }
        }
        jump_code(kind, brk, target)
    }

    fn block(&mut self, sts: &[J], cx: &mut Ctx, out: &mut String, ind: usize) {
        for x in sts {
            self.stmt(x, cx, out, ind);
        }
    }

    fn body_of(&mut self, st: &J, cx: &mut Ctx, out: &mut String, ind: usize) {
        if s(st, "k") == "block" {
            self.block(arr(st, "stmts"), cx, out, ind);
        } else {
            self.stmt(st, cx, out, ind);
        }
    }

    fn stmt(&mut self, st: &J, cx: &mut Ctx, out: &mut String, ind: usize) {
        let pad = "    ".repeat(ind);
        let k = s(st, "k");
        let line = int(st, "line");
        if line != 0 {
            cx.line = line;
        }
        if k != "block" {
            self.stats.stmts += 1;
            self.stats.lines.insert((cx.class.clone(), line));
        }
        let src = s(st, "src").to_string();
        match k {
            "block" => {
                let _ = writeln!(out, "{pad}{{");
                self.block(arr(st, "stmts"), cx, out, ind + 1);
                let _ = writeln!(out, "{pad}}}");
            }
            "empty" => {}
            "local" => {
                let t = s(st, "t").to_string();
                let rt = self.rty(&t);
                let init = match st.get("init") {
                    Some(i) => {
                        let c = self.expr(i, cx);
                        self.arg(c, s(i, "t"), &t)
                    }
                    None => "Default::default()".into(),
                };
                let _ = writeln!(out, "{pad}let mut {}: {rt} = {init};", cx.local(s(st, "name")));
            }
            "expr" => {
                let e = &st["e"];
                let code = match s(e, "k") {
                    "assign" | "assignop" => self.assign(e, cx, true),
                    "unary" => self.unary(e, cx, true),
                    _ => self.expr(e, cx),
                };
                let _ = writeln!(out, "{pad}{code};");
            }
            "if" => {
                let c = self.expr(&st["cond"], cx);
                let _ = writeln!(out, "{pad}if {c} {{");
                self.body_of(&st["then"], cx, out, ind + 1);
                if let Some(e) = st.get("else") {
                    if s(e, "k") == "if" {
                        let _ = write!(out, "{pad}}} else ");
                        let mut inner = String::new();
                        self.stmt(e, cx, &mut inner, ind);
                        out.push_str(inner.trim_start());
                        return;
                    }
                    let _ = writeln!(out, "{pad}}} else {{");
                    self.body_of(e, cx, out, ind + 1);
                }
                let _ = writeln!(out, "{pad}}}");
            }
            "while" => {
                let idn = int(st, "id");
                let c = self.expr(&st["cond"], cx);
                let _ = writeln!(out, "{pad}'l{idn}: while {c} {{");
                self.body_of(&st["body"], cx, out, ind + 1);
                let _ = writeln!(out, "{pad}}}");
            }
            "do" => {
                let idn = int(st, "id");
                let _ = writeln!(out, "{pad}'l{idn}: loop {{");
                let _ = writeln!(out, "{pad}    'c{idn}: {{");
                self.body_of(&st["body"], cx, out, ind + 2);
                let _ = writeln!(out, "{pad}    }}");
                let c = self.expr(&st["cond"], cx);
                let _ = writeln!(out, "{pad}    if !{c} {{ break; }}");
                let _ = writeln!(out, "{pad}}}");
            }
            "for" => {
                let idn = int(st, "id");
                let _ = writeln!(out, "{pad}{{");
                for i in arr(st, "init") {
                    self.stmt(i, cx, out, ind + 1);
                }
                let c = match st.get("cond") {
                    Some(c) => self.expr(c, cx),
                    None => "true".into(),
                };
                let _ = writeln!(out, "{pad}    'l{idn}: while {c} {{");
                let _ = writeln!(out, "{pad}        'c{idn}: {{");
                self.body_of(&st["body"], cx, out, ind + 3);
                let _ = writeln!(out, "{pad}        }}");
                for u in arr(st, "update") {
                    self.stmt(u, cx, out, ind + 2);
                }
                let _ = writeln!(out, "{pad}    }}");
                let _ = writeln!(out, "{pad}}}");
            }
            "labeled" => {
                let body = &st["body"];
                if matches!(s(body, "k"), "while" | "for" | "do" | "switch") {
                    self.stmt(body, cx, out, ind);
                } else {
                    let idn = int(st, "id");
                    let _ = writeln!(out, "{pad}'b{idn}: {{");
                    self.body_of(body, cx, out, ind + 1);
                    let _ = writeln!(out, "{pad}}}");
                }
            }
            "switch" => self.switch(st, cx, out, ind),
            "break" => {
                let j = self.jump(cx, true, int(st, "target"), st);
                let _ = writeln!(out, "{pad}{j};");
            }
            "continue" => {
                let j = self.jump(cx, false, int(st, "target"), st);
                let _ = writeln!(out, "{pad}{j};");
            }
            "return" => {
                let v = match st.get("e") {
                    Some(e) => {
                        let c = self.expr(e, cx);
                        let ret = cx.ret.clone();
                        self.arg(c, s(e, "t"), &ret)
                    }
                    None => "()".into(),
                };
                if !cx.tries.is_empty() {
                    let _ = writeln!(out, "{pad}return jrt::Flow::Ret({v});");
                } else if v == "()" {
                    let _ = writeln!(out, "{pad}return;");
                } else {
                    let _ = writeln!(out, "{pad}return {v};");
                }
            }
            "throw" => {
                let e = &st["e"];
                if s(e, "k") == "ident" && s(e, "sym") == "local" {
                    let _ = writeln!(out, "{pad}jrt::rethrow({}.clone());", cx.local(s(e, "name")));
                } else if s(e, "k") == "new" {
                    let _ = writeln!(out, "{pad}jrt::throw_new({:?});", s(e, "class"));
                } else {
                    let t = self.todo(cx, "throw of an expression", &src, line);
                    let _ = writeln!(out, "{pad}{t};");
                }
            }
            "try" => self.try_stmt(st, cx, out, ind),
            "sync" => {
                let _ = writeln!(out, "{pad}{{ // synchronized: the port is single-threaded");
                self.body_of(&st["body"], cx, out, ind + 1);
                let _ = writeln!(out, "{pad}}}");
            }
            other => {
                let t = self.todo(cx, &format!("statement {other}"), &src, line);
                let _ = writeln!(out, "{pad}{t};");
            }
        }
    }

    fn switch(&mut self, st: &J, cx: &mut Ctx, out: &mut String, ind: usize) {
        let pad = "    ".repeat(ind);
        let idn = int(st, "id");
        let sel = &st["sel"];
        let selt = s(sel, "t").to_string();
        if !matches!(selt.as_str(), "int" | "byte" | "short" | "char") {
            let t = self.todo(cx, "switch on non-int", s(st, "src"), int(st, "line"));
            let _ = writeln!(out, "{pad}{t};");
            return;
        }
        let selc = self.expr(sel, cx);
        let selc = conv(selc, &selt, "int");
        let cases = arr(st, "cases");
        // does any non-last case fall through?
        let falls = cases.iter().enumerate().any(|(i, c)| i + 1 < cases.len() && !ends_abruptly(arr(c, "stmts")));
        let labels_of = |g: &mut Self, cx: &mut Ctx, c: &J| -> Vec<String> {
            arr(c, "labels").iter().map(|l| pat(&g.expr(l, cx))).collect()
        };
        let _ = writeln!(out, "{pad}'s{idn}: {{");
        if !falls {
            let _ = writeln!(out, "{pad}    match {selc} {{");
            let mut default_body: Option<&J> = None;
            for c in cases {
                if b(c, "default") {
                    default_body = Some(c);
                    continue;
                }
                let labels = labels_of(self, cx, c);
                let _ = writeln!(out, "{pad}        {} => {{", labels.join(" | "));
                let sts = strip_trailing_break(arr(c, "stmts"), idn);
                self.block(sts, cx, out, ind + 3);
                let _ = writeln!(out, "{pad}        }}");
            }
            let _ = writeln!(out, "{pad}        _ => {{");
            if let Some(c) = default_body {
                let sts = strip_trailing_break(arr(c, "stmts"), idn);
                self.block(sts, cx, out, ind + 3);
            }
            let _ = writeln!(out, "{pad}        }}");
            let _ = writeln!(out, "{pad}    }}");
        } else {
            // general form with fall-through
            let mut all_labels = Vec::new();
            let mut per_case = Vec::new();
            for c in cases {
                let l = labels_of(self, cx, c);
                all_labels.extend(l.clone());
                per_case.push(l);
            }
            let _ = writeln!(out, "{pad}    let __sel = {selc};");
            let any = if all_labels.is_empty() { "_".to_string() } else { all_labels.join(" | ") };
            let _ = writeln!(out, "{pad}    let __hit = matches!(__sel, {any});");
            let _ = writeln!(out, "{pad}    let mut __ft = false;");
            for (c, l) in cases.iter().zip(per_case) {
                let mut conds = vec!["__ft".to_string()];
                if !l.is_empty() {
                    conds.push(format!("matches!(__sel, {})", l.join(" | ")));
                }
                if b(c, "default") {
                    conds.push("!__hit".into());
                }
                let _ = writeln!(out, "{pad}    if {} {{", conds.join(" || "));
                let _ = writeln!(out, "{pad}        __ft = true;");
                self.block(arr(c, "stmts"), cx, out, ind + 2);
                let _ = writeln!(out, "{pad}    }}");
            }
            let _ = writeln!(out, "{pad}    let _ = __ft;");
        }
        let _ = writeln!(out, "{pad}}}");
    }

    fn try_stmt(&mut self, st: &J, cx: &mut Ctx, out: &mut String, ind: usize) {
        let pad = "    ".repeat(ind);
        let ret_rt = {
            let r = cx.ret.clone();
            self.rty(&r)
        };
        // targets declared inside the try (body and handlers)
        let mut declared = HashMap::new();
        self.collect_kinds(&st["body"], &mut declared);
        for c in arr(st, "catches") {
            self.collect_kinds(&c["body"], &mut declared);
        }
        cx.tries.push(TryFrame { declared: declared.keys().copied().collect(), escapes: BTreeSet::new() });
        let _ = writeln!(out, "{pad}let __flow: jrt::Flow<{ret_rt}> = match jrt::catch(|| -> jrt::Flow<{ret_rt}> {{");
        self.body_of(&st["body"], cx, out, ind + 1);
        let _ = writeln!(out, "{pad}    #[allow(unreachable_code)] jrt::Flow::Normal");
        let _ = writeln!(out, "{pad}}}) {{");
        let _ = writeln!(out, "{pad}    Ok(f) => f,");
        for c in arr(st, "catches") {
            let ct = s(c, "t").to_string();
            let name = cx.local(s(c, "name"));
            let _ = writeln!(out, "{pad}    Err(__e) if __e.is_a({ct:?}) => (|| -> jrt::Flow<{ret_rt}> {{");
            let _ = writeln!(out, "{pad}        #[allow(unused_variables)] let {name} = __e;");
            self.body_of(&c["body"], cx, out, ind + 2);
            let _ = writeln!(out, "{pad}        #[allow(unreachable_code)] jrt::Flow::Normal");
            let _ = writeln!(out, "{pad}    }})(),");
        }
        let _ = writeln!(out, "{pad}    Err(__e) => jrt::rethrow(__e),");
        let _ = writeln!(out, "{pad}}};");
        let frame = cx.tries.pop().expect("pushed above");
        if let Some(f) = st.get("finally") {
            let t = self.todo(cx, "finally", s(f, "src"), int(f, "line"));
            let _ = writeln!(out, "{pad}{t};");
        }
        let _ = writeln!(out, "{pad}match __flow {{");
        let _ = writeln!(out, "{pad}    jrt::Flow::Normal => {{}}");
        if cx.tries.is_empty() {
            let _ = writeln!(out, "{pad}    jrt::Flow::Ret(v) => return v,");
        } else {
            let _ = writeln!(out, "{pad}    jrt::Flow::Ret(v) => return jrt::Flow::Ret(v),");
        }
        for (kind, target) in &frame.escapes {
            let brk = *kind == 'b';
            let code = self.jump(cx, brk, *target, st);
            let _ = writeln!(out, "{pad}    jrt::Flow::{}({target}) => {code},", if brk { "Brk" } else { "Cont" });
        }
        let _ = writeln!(out, "{pad}    #[allow(unreachable_patterns)] _ => unreachable!(),");
        let _ = writeln!(out, "{pad}}}");
    }
}

/// Integer and floating arithmetic shared by binary and compound operators.
/// `None` for an operator that is not arithmetic.
fn arith(op: &str, l: &str, r: &str, integral: bool, long: bool, rt: &str) -> Option<String> {
    Some(match op {
        "PLUS" if integral => format!("{l}.wrapping_add({r})"),
        "MINUS" if integral => format!("{l}.wrapping_sub({r})"),
        "MUL" if integral => format!("{l}.wrapping_mul({r})"),
        "DIV" if integral => format!("jrt::{}div({l}, {r})", if long { "l" } else { "i" }),
        "MOD" if integral => format!("jrt::{}rem({l}, {r})", if long { "l" } else { "i" }),
        "PLUS" => format!("({l} + {r})"),
        "MINUS" => format!("({l} - {r})"),
        "MUL" => format!("({l} * {r})"),
        "DIV" => format!("({l} / {r})"),
        "MOD" => format!("({l} % {r})"),
        "SL" | "SR" | "USR" => {
            let r = conv(r.to_string(), rt, "int");
            let f = match (op, long) {
                ("SL", false) => "ishl",
                ("SR", false) => "ishr",
                ("USR", false) => "iushr",
                ("SL", true) => "lshl",
                ("SR", true) => "lshr",
                _ => "lushr",
            };
            format!("jrt::{f}({l}, {r})")
        }
        "BITAND" => format!("({l} & {r})"),
        "BITOR" => format!("({l} | {r})"),
        "BITXOR" => format!("({l} ^ {r})"),
        _ => return None,
    })
}

struct LValue {
    pre: Vec<String>,
    read: String,
    write: Box<dyn Fn(&str) -> String>,
    simple: bool,
}

/// Visits the direct child statements of `st`.
fn for_each_child_stmt(st: &J, f: &mut dyn FnMut(&J)) {
    for key in ["body", "then", "else", "finally"] {
        if let Some(x) = st.get(key) {
            if x.is_object() && x.get("k").is_some() {
                f(x);
            }
        }
    }
    for key in ["stmts", "init", "update"] {
        for x in arr(st, key) {
            f(x);
        }
    }
    for c in arr(st, "cases") {
        for x in arr(c, "stmts") {
            f(x);
        }
    }
    for c in arr(st, "catches") {
        f(&c["body"]);
    }
}

/// Java names of every local variable and catch parameter declared in `st`, in order.
fn collect_locals(st: &J, out: &mut Vec<String>) {
    if s(st, "k") == "local" {
        out.push(s(st, "name").to_string());
    }
    for c in arr(st, "catches") {
        out.push(s(c, "name").to_string());
    }
    for_each_child_stmt(st, &mut |x| collect_locals(x, out));
}

/// Rust names for the parameters and locals of one method.  `params` are
/// `(java name, name from the map or "")`.
fn local_names(params: &[(String, String)], body: Option<&J>) -> (Vec<String>, HashMap<String, String>) {
    let mut scope = Scope::with_reserved(&["this"]);
    let mut map = HashMap::new();
    let mut pnames = Vec::new();
    for (java, mapped) in params {
        let src = if mapped.is_empty() { java } else { mapped };
        let r = scope.claim(&names::value_base(src));
        map.insert(java.clone(), r.clone());
        pnames.push(r);
    }
    let mut locals = Vec::new();
    if let Some(b) = body {
        collect_locals(b, &mut locals);
    }
    for l in locals {
        if let std::collections::hash_map::Entry::Vacant(v) = map.entry(l) {
            let r = scope.claim(&names::value_base(v.key()));
            v.insert(r);
        }
    }
    (pnames, map)
}

fn pat(l: &str) -> String {
    // literal patterns: strip the cast form of char literals
    if let Some(x) = l.strip_prefix("('").and_then(|x| x.strip_suffix("' as u16)")) {
        return format!("{}", x.chars().next().map(|c| c as u32).unwrap_or(0));
    }
    let l = l.trim_end_matches("u16");
    let l = l.trim_start_matches('(').trim_end_matches(')');
    l.trim_end_matches("i8").trim_end_matches("i16").to_string()
}

fn jump_code(kind: TKind, brk: bool, target: i64) -> String {
    match (kind, brk) {
        (TKind::While, true) | (TKind::LoopWithCont, true) => format!("break 'l{target}"),
        (TKind::While, false) => format!("continue 'l{target}"),
        (TKind::LoopWithCont, false) => format!("break 'c{target}"),
        (TKind::Switch, _) => format!("break 's{target}"),
        (TKind::Block, _) => format!("break 'b{target}"),
    }
}

fn ends_abruptly(sts: &[J]) -> bool {
    match sts.last() {
        None => false,
        Some(l) => match s(l, "k") {
            "break" | "continue" | "return" | "throw" => true,
            "block" => ends_abruptly(arr(l, "stmts")),
            _ => false,
        },
    }
}

fn strip_trailing_break(sts: &[J], sw: i64) -> &[J] {
    match sts.last() {
        Some(l) if s(l, "k") == "break" && int(l, "target") == sw => &sts[..sts.len() - 1],
        _ => sts,
    }
}

/// `5` → `5i32` so that `5.wrapping_add(x)` has a known receiver type.
fn typed_lit(code: String, long: bool) -> String {
    let suf = if long { "i64" } else { "i32" };
    let (open, inner, close) = match code.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(x) => ("(", x, ")"),
        None => ("", code.as_str(), ""),
    };
    let digits = inner.strip_prefix('-').unwrap_or(inner);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        return format!("{open}{inner}{suf}{close}");
    }
    code
}

/// Emulator jars declare MIDP methods on their own subclasses; map them back.
fn platform_alias(owner: &str) -> &str {
    match owner {
        "org.recompile.mobile.PlatformGraphics" => "javax.microedition.lcdui.Graphics",
        "org.recompile.mobile.PlatformImage" => "javax.microedition.lcdui.Image",
        "org.recompile.mobile.PlatformFont" => "javax.microedition.lcdui.Font",
        o => o,
    }
}

/// `int` → `int`, `javax.microedition.lcdui.Graphics` → `graphics`, `byte[]` → `byte_arr`.
fn type_word(t: &str, classes: &HashMap<String, String>) -> String {
    if let Some(e) = t.strip_suffix("[]") {
        return format!("{}_arr", type_word(e, classes));
    }
    let simple = classes.get(t).map(String::as_str).unwrap_or_else(|| names::simple_of(t));
    names::snake_case(simple)
}

fn overload_suffix(ptypes: &[String], classes: &HashMap<String, String>) -> String {
    if ptypes.is_empty() {
        "noargs".to_string()
    } else {
        ptypes.iter().map(|p| type_word(p, classes)).collect::<Vec<_>>().join("_")
    }
}

/// Class table with every Rust name decided up front.
fn build_classes(classes_j: &[J], map: &NameMap) -> HashMap<String, ClassInfo> {
    let mut modules = Scope::default();
    // final simple Java name of every class (after the map), for overload suffixes
    let java_names: HashMap<String, String> = classes_j
        .iter()
        .map(|c| {
            let flat = s(c, "name");
            (flat.to_string(), map.class(flat).unwrap_or(names::simple_of(flat)).to_string())
        })
        .collect();
    let mut out = HashMap::new();
    for c in classes_j {
        let flat = s(c, "name").to_string();
        let simple = java_names[&flat].clone();
        let module = modules.claim(&names::module_base(&simple));
        let strukt = names::struct_base(&simple, &[PEER_STRUCT]);
        let mut inst = Scope::default();
        let mut statics = Scope::default();
        let mut fields = HashMap::new();
        let mut fns = Scope::with_reserved(MODULE_FNS);
        let mut methods = HashMap::new();
        let members = arr(c, "members");
        // constructors first: `new`/`construct` for the first, a typed suffix for overloads
        let mut first_ctor = true;
        for m in members.iter().filter(|m| s(m, "k") == "method" && s(m, "name") == "<init>") {
            let ptypes: Vec<String> = arr(m, "params").iter().map(|p| s(p, "t").to_string()).collect();
            let suffix = if first_ctor { String::new() } else { format!("_{}", overload_suffix(&ptypes, &java_names)) };
            first_ctor = false;
            let rname = fns.claim(&format!("new{suffix}"));
            let construct = fns.claim(&format!("construct{suffix}"));
            let param_names = map.method(&flat, "<init>", &ptypes).map(|n| n.params.clone()).unwrap_or_default();
            methods.insert(
                ("<init>".to_string(), s(m, "desc").to_string()),
                MethodInfo { rname, construct, is_static: false, param_names },
            );
        }
        let mut seen: HashSet<String> = HashSet::new();
        for m in members {
            match s(m, "k") {
                "field" => {
                    let name = s(m, "name");
                    let java = map.field(&flat, name).unwrap_or(name);
                    let r = if b(m, "static") {
                        statics.claim(&names::screaming_case(java))
                    } else {
                        inst.claim(&names::value_base(java))
                    };
                    fields.insert(name.to_string(), r);
                }
                "method" if s(m, "name") != "<init>" => {
                    let name = s(m, "name");
                    let ptypes: Vec<String> = arr(m, "params").iter().map(|p| s(p, "t").to_string()).collect();
                    let mapped = map.method(&flat, name, &ptypes);
                    let java = mapped.and_then(|n| n.name.clone()).unwrap_or_else(|| name.to_string());
                    let base = names::value_base(&java);
                    let base = if seen.insert(java.clone()) { base } else { format!("{base}_{}", overload_suffix(&ptypes, &java_names)) };
                    let rname = fns.claim(&base);
                    let param_names = mapped.map(|n| n.params.clone()).unwrap_or_default();
                    methods.insert(
                        (name.to_string(), s(m, "desc").to_string()),
                        MethodInfo { rname, construct: String::new(), is_static: b(m, "static"), param_names },
                    );
                }
                _ => {}
            }
        }
        out.insert(
            flat.clone(),
            ClassInfo {
                flat: flat.clone(),
                sup: s(c, "super").to_string(),
                ifaces: strings(c, "ifaces"),
                module,
                strukt,
                fields,
                methods,
            },
        );
    }
    out
}

/// The input classes in the shape [`NameMap::resolve`] needs.
pub fn input_classes(ast: &J) -> Vec<names::InputClass<'_>> {
    arr(ast, "classes")
        .iter()
        .map(|c| names::InputClass {
            flat: s(c, "name"),
            fields: arr(c, "members").iter().filter(|m| s(m, "k") == "field").map(|m| (s(m, "name"), s(m, "t"))).collect(),
            methods: arr(c, "members")
                .iter()
                .filter(|m| s(m, "k") == "method")
                .map(|m| (s(m, "name"), arr(m, "params").iter().map(|p| s(p, "t")).collect()))
                .collect(),
        })
        .collect()
}

/// What one generation produced.
pub struct Outcome {
    pub stats: Stats,
    pub residue: Vec<Residue>,
    /// Text of `report.txt`.
    pub report: String,
    pub classes: usize,
}

/// Writes the crate for `ast` into `out_dir` (`Cargo.toml`, `src/`, `src/bin/trace.rs`,
/// `residue.json`, `report.txt`).  `jrt_dependency` is the inline-table body of the
/// `jrt` dependency (`path = "…"` or `git = "…"`).
pub fn generate(ast: &J, api: &ApiTable, map: &NameMap, out_dir: &Path, crate_name: &str, jrt_dependency: &str) -> Result<Outcome, String> {
    let classes_j = arr(ast, "classes").to_vec();
    if classes_j.is_empty() {
        return Err("the AST has no classes".into());
    }
    let mut g = Gen { classes: build_classes(&classes_j, map), api, stats: Stats::default(), residue: vec![] };
    let io = |e: std::io::Error| format!("{}: {e}", out_dir.display());
    fs::create_dir_all(out_dir.join("src/bin")).map_err(io)?;

    let mut lib = String::new();
    lib.push_str("//! Translated from Java by `arlecchino port-rust` (mechanical translation; see report.txt).\n");
    lib.push_str("#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals, unused_mut, unused_variables, unused_labels, unused_parens, unused_braces, unreachable_code, dead_code, unused_assignments, unused_imports, unused_attributes, while_true, clippy::all)]\n\n");
    let mut init_order = Vec::new();
    let mut midlet = None;

    for c in &classes_j {
        let flat = s(c, "name").to_string();
        let (sup, ifaces, module, st) = {
            let ci = &g.classes[&flat];
            (ci.sup.clone(), ci.ifaces.clone(), ci.module.clone(), ci.strukt.clone())
        };
        let _ = writeln!(lib, "pub mod {module};");
        if sup == "javax.microedition.midlet.MIDlet" {
            midlet = Some(flat.clone());
        }
        let mut f = String::new();
        let _ = writeln!(f, "//! Java class `{flat}` (extends `{sup}`).");
        let _ = writeln!(f, "use jrt::SGet as _;\n");
        if g.classes.contains_key(&sup) {
            // inheritance between user classes is not modelled yet
            let mut cx = Ctx::new(&flat, "<class>", "void");
            let t = g.todo(&mut cx, "user-class inheritance", &format!("class {flat} extends {sup}"), 0);
            let _ = writeln!(f, "const _: () = {{ let _ = || {t}; }};");
        }
        // struct with the instance fields
        let _ = writeln!(f, "#[derive(Default)]\npub struct {st} {{");
        let mut statics = String::new();
        let mut static_inits = HashSet::new();
        let mut inst_inits = Vec::new();
        for m in arr(c, "members").iter().filter(|m| s(m, "k") == "field") {
            let t = s(m, "t").to_string();
            let rt = g.rty(&t);
            let name = s(m, "name");
            let rname = g.field(&flat, name);
            if b(m, "static") {
                let mut init = "Default::default()".to_string();
                if let Some(i) = m.get("init") {
                    if s(i, "k") == "lit" && s(i, "kind") != "string" {
                        let mut cx = Ctx::new(&flat, "<clinit>", "void");
                        let c0 = g.expr(i, &mut cx);
                        init = conv(c0, s(i, "t"), &t);
                    } else {
                        static_inits.insert(name.to_string());
                    }
                }
                let _ = writeln!(statics, "    /// Java `static {t} {name}`.");
                let _ = writeln!(statics, "    pub static {rname}: std::cell::RefCell<{rt}> = std::cell::RefCell::new({init});");
            } else {
                let _ = writeln!(f, "    pub {rname}: {rt},");
                if m.get("init").is_some() {
                    inst_inits.push(m.clone());
                }
            }
        }
        let _ = writeln!(f, "}}\n");
        if !statics.is_empty() {
            let _ = writeln!(f, "thread_local! {{\n{statics}}}\n");
        }
        // glue for platform callbacks
        let is_peer = PEER_TYPES.contains(&sup.as_str()) || ifaces.iter().any(|i| PEER_TYPES.contains(&i.as_str()));
        if is_peer {
            let _ = writeln!(f, "pub struct {PEER_STRUCT}(pub jrt::JRef<{st}>);");
            let _ = writeln!(f, "impl jrt::Callbacks for {PEER_STRUCT} {{");
            for (n, d, tpl) in CALLBACKS {
                if let Some(mi) = g.classes[&flat].methods.get(&(n.to_string(), d.to_string())) {
                    let _ = writeln!(f, "    {}", tpl.replace("{m}", &mi.rname));
                }
            }
            let _ = writeln!(f, "}}");
            let _ = writeln!(f, "pub fn peer(o: jrt::JRef<{st}>) -> jrt::Peer {{ jrt::Peer::of({PEER_STRUCT}(o)) }}\n");
        }
        // field initializers and instance initializer blocks
        let _ = writeln!(f, "fn init_fields(this: &jrt::JRef<{st}>) {{");
        for m in &inst_inits {
            let mut cx = Ctx::new(&flat, "<init fields>", "void");
            let i = &m["init"];
            let t = s(m, "t").to_string();
            let c0 = g.expr(i, &mut cx);
            let v = g.arg(c0, s(i, "t"), &t);
            let _ = writeln!(f, "    jrt::put!(this, {}, {v});", g.field(&flat, s(m, "name")));
        }
        for m in arr(c, "members").iter().filter(|m| s(m, "k") == "init" && !b(m, "static")) {
            let mut cx = Ctx::new(&flat, "<instance init>", "void");
            cx.locals = local_names(&[], Some(&m["body"])).1;
            let mut body = String::new();
            g.stmt(&m["body"], &mut cx, &mut body, 1);
            f.push_str(&body);
        }
        let _ = writeln!(f, "}}\n");
        // static initializer
        let _ = writeln!(f, "/// Java `<clinit>` of `{flat}`.");
        let _ = writeln!(f, "pub fn static_init() {{");
        for m in arr(c, "members") {
            if s(m, "k") == "field" && b(m, "static") && static_inits.contains(s(m, "name")) {
                let mut cx = Ctx::new(&flat, "<clinit>", "void");
                let i = &m["init"];
                let t = s(m, "t").to_string();
                let c0 = g.expr(i, &mut cx);
                let v = g.arg(c0, s(i, "t"), &t);
                let _ = writeln!(f, "    {}.set({v});", g.field(&flat, s(m, "name")));
            }
            if s(m, "k") == "init" && b(m, "static") {
                let mut cx = Ctx::new(&flat, "<clinit>", "void");
                cx.locals = local_names(&[], Some(&m["body"])).1;
                let mut body = String::new();
                g.stmt(&m["body"], &mut cx, &mut body, 1);
                f.push_str(&body);
            }
        }
        let _ = writeln!(f, "}}\n");
        init_order.push(module.clone());

        // methods
        for m in arr(c, "members") {
            match s(m, "k") {
                "method" => {}
                "nested" | "unknown" => {
                    let mut cx = Ctx::new(&flat, "<member>", "void");
                    let t = g.todo(&mut cx, "nested/unknown member", s(m, "src"), 0);
                    let _ = writeln!(f, "const _: () = {{ let _ = || {t}; }};");
                    continue;
                }
                _ => continue,
            }
            gen_method(&mut g, &flat, &st, m, &mut f);
        }
        fs::write(out_dir.join("src").join(format!("{}.rs", module.trim_start_matches("r#"))), f).map_err(io)?;
    }
    // entry points
    let _ = writeln!(lib, "\n/// Runs every static initializer (eagerly, in class order — an accepted deviation).");
    let _ = writeln!(lib, "pub fn static_init_all() {{");
    for m in &init_order {
        let _ = writeln!(lib, "    {m}::static_init();");
    }
    let _ = writeln!(lib, "}}");
    if let Some(m) = &midlet {
        if let Some(mi) = g.classes[m].methods.get(&("<init>".to_string(), "()void".to_string())) {
            let module = &g.classes[m].module;
            let _ = writeln!(lib, "\n/// Creates the MIDlet object (Java class `{m}`).");
            let _ = writeln!(lib, "pub fn new_midlet() -> jrt::Peer {{ {module}::peer({module}::{}()) }}", mi.rname);
        }
    }
    fs::write(out_dir.join("src/lib.rs"), lib).map_err(io)?;
    fs::write(
        out_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n\
             [dependencies]\njrt = {{ package = \"arlecchino-jrt\", {jrt_dependency} }}\n\n\
             [profile.dev]\noverflow-checks = true\n\n\
             # a crate of its own, even when written inside another workspace\n[workspace]\n"
        ),
    )
    .map_err(io)?;
    let lib_name = crate_name.replace('-', "_");
    let entry = if midlet.is_some() { format!("{lib_name}::new_midlet") } else { "|| jrt::Peer::default()".to_string() };
    fs::write(
        out_dir.join("src/bin/trace.rs"),
        format!(
            "//! Headless run that writes a midp-rt-style call trace (and, with\n\
             //! --frames-dir, the frames as PNG).\n\
             //! usage: trace <res-dir> <trace-out> <width> <height> <frames> [keys: frame:code:d|u,...]\n\
             //!        [--ticks] [--inputs ticks.json] [--frames-dir DIR]\n\
             fn main() {{\n\
             \x20   jrt::api::run_cli({lib_name}::static_init_all, {entry});\n\
             }}\n"
        ),
    )
    .map_err(io)?;

    // residue + report
    let rj = serde_json::to_string_pretty(&g.residue).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("residue.json"), rj).map_err(io)?;
    let report = report_text(&g.stats, map);
    fs::write(out_dir.join("report.txt"), &report).map_err(io)?;
    Ok(Outcome { stats: g.stats, residue: g.residue, report, classes: classes_j.len() })
}

fn gen_method(g: &mut Gen, flat: &str, st: &str, m: &J, f: &mut String) {
    let mname = s(m, "name").to_string();
    let desc = s(m, "desc").to_string();
    let mi = g.classes[flat].methods[&(mname.clone(), desc.clone())].clone();
    let ctor = mname == "<init>";
    let ret = s(m, "ret").to_string();
    let mut cx = Ctx::new(flat, &mname, if ctor { "void" } else { &ret });
    let params_j = arr(m, "params");
    let pairs: Vec<(String, String)> = params_j
        .iter()
        .enumerate()
        .map(|(i, p)| (s(p, "name").to_string(), mi.param_names.get(i).cloned().unwrap_or_default()))
        .collect();
    let (pnames, locals) = local_names(&pairs, m.get("body"));
    cx.locals = locals;
    let mut params = Vec::new();
    if !mi.is_static {
        params.push(format!("this: &jrt::JRef<{st}>"));
    }
    for (p, rn) in params_j.iter().zip(&pnames) {
        let prt = g.rty(s(p, "t"));
        params.push(format!("mut {rn}: {prt}"));
    }
    let Some(body) = m.get("body") else {
        let _ = writeln!(f, "// abstract or native: {mname}{desc}\n");
        return;
    };
    g.stats.methods += 1;
    g.collect_kinds(body, &mut cx.kinds);
    let mut bodys = String::new();
    let sts = arr(body, "stmts");
    if ctor {
        // this(...) delegation or super(); then field initializers; then the rest
        let delegation = sts
            .first()
            .filter(|x| s(x, "k") == "expr" && s(&x["e"], "k") == "call" && matches!(s(&x["e"], "implicit"), "super" | "this"));
        let mut rest = sts;
        if let Some(x) = delegation {
            g.stmt(x, &mut cx, &mut bodys, 1);
            rest = &sts[1..];
        }
        if delegation.map(|x| s(&x["e"], "implicit")) != Some("this") {
            let _ = writeln!(bodys, "    init_fields(this);");
        }
        g.block(rest, &mut cx, &mut bodys, 1);
        let _ = writeln!(f, "/// Java constructor `{flat}{}` (line {}).", desc.trim_end_matches("void"), int(m, "line"));
        let _ = writeln!(f, "pub fn {}({}) {{\n{bodys}}}\n", mi.construct, params.join(", "));
        let _ = writeln!(
            f,
            "/// `new {flat}{}`.\npub fn {}({}) -> jrt::JRef<{st}> {{\n    let this = jrt::JRef::new({st}::default());\n    {}(&this{}{});\n    this\n}}\n",
            desc.trim_end_matches("void"),
            mi.rname,
            params[1..].join(", "),
            mi.construct,
            if pnames.is_empty() { "" } else { ", " },
            pnames.join(", ")
        );
    } else {
        g.block(sts, &mut cx, &mut bodys, 1);
        let rrt = g.rty(&ret);
        let rets = if ret == "void" { String::new() } else { format!(" -> {rrt}") };
        if ret != "void" {
            bodys.push_str("    #[allow(unreachable_code)]\n    unreachable!(\"end of a non-void Java method\")\n");
        }
        let _ = writeln!(f, "/// Java `{mname}{desc}` (line {}).", int(m, "line"));
        let _ = writeln!(f, "pub fn {}({}){rets} {{\n{bodys}}}\n", mi.rname, params.join(", "));
    }
    if cx.todo_here == 0 {
        g.stats.methods_clean += 1;
    }
}

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 * 100.0 / b as f64
    }
}

fn report_text(st: &Stats, map: &NameMap) -> String {
    let mut rep = String::new();
    let _ = writeln!(rep, "methods with a body: {}", st.methods);
    let _ = writeln!(rep, "methods without todo_ai: {} ({:.1} %)", st.methods_clean, pct(st.methods_clean, st.methods));
    let _ = writeln!(rep, "statements: {}", st.stmts);
    let _ = writeln!(rep, "todo_ai sites: {}", st.todo_sites);
    let _ = writeln!(rep, "source lines with statements: {}", st.source_lines());
    let todo_in = st.todo_source_lines();
    let _ = writeln!(rep, "  of them touched by todo_ai: {} ({:.2} %)", todo_in, pct(todo_in, st.source_lines()));
    if !map.is_empty() || !map.warnings.is_empty() {
        let _ = writeln!(rep, "\nname map: {} entries applied, {} not matched", map.applied, map.warnings.len());
        for w in &map.warnings {
            let _ = writeln!(rep, "  {w}");
        }
    }
    let _ = writeln!(rep, "\ntodo_ai by reason:");
    let mut why: Vec<_> = st.why.iter().collect();
    why.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (w, n) in why {
        let _ = writeln!(rep, "  {n:5}  {w}");
    }
    let _ = writeln!(rep, "\nplatform API used (translated): {} distinct", st.api_used.len());
    for (k, n) in &st.api_used {
        let _ = writeln!(rep, "  {n:5}  {k}");
    }
    let _ = writeln!(rep, "\nplatform API missing: {} distinct", st.api_missing.len());
    for (k, n) in &st.api_missing {
        let _ = writeln!(rep, "  {n:5}  {k}");
    }
    rep
}
