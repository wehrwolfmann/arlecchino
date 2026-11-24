//! Asking a model for names: which symbols, how they are batched, what the prompt
//! says, and how a reply becomes [`NameEntry`]s.
//!
//! One JOB is one top-level class together with its nested classes (decompilers put
//! them in one `.java` file).  Jobs are packed into batches under a token budget; a
//! batch whose reply cannot be used is retried job by job, so one bad class never
//! sinks its neighbours.  A class too large for the budget is sent with its source
//! visibly cut (and reported), never silently.
//!
//! Only "obfuscated-looking" symbols are offered (see [`looks_obfuscated`]); a method
//! is offered only where it is first declared in the in-jar hierarchy, because its
//! overrides will follow the same name anyway.

use super::classfile::{ClassInfo, Member};
use super::mapfile::Label;
use super::names::{looks_obfuscated, simple_name, Kind, NameEntry, Plan, World};
use super::provider::{estimate_tokens, extract_json, CACHE_MISS_OFFLINE};
use crate::ai_model::{ChatMessage, ChatModel, ChatRequest};
use std::collections::BTreeMap;
use std::path::Path;

pub const PROMPT_VERSION: &str = "ai-rename/2";

#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub id: String,
    pub kind: Kind,
    pub owner: String,
    pub name: String,
    pub desc: String,
    /// `Local` only: the label, the value's descriptor, whether it is a parameter.
    pub local: String,
    pub ltype: String,
    pub param: bool,
}

impl Symbol {
    fn new(id: String, kind: Kind, owner: &str, name: &str, desc: &str) -> Self {
        Symbol { id, kind, owner: owner.into(), name: name.into(), desc: desc.into(), local: String::new(), ltype: String::new(), param: false }
    }
    fn key(&self) -> (Kind, String, String, String) {
        let desc = if self.local.is_empty() { self.desc.clone() } else { format!("{}#{}", self.desc, self.local) };
        (self.kind, self.owner.clone(), self.name.clone(), desc)
    }
}

/// What a second round knows: the names applied so far (the source shown is then a
/// decompile of the RENAMED jar), the local labels the applier wrote, and the symbols
/// a map already names (never offered again).
#[derive(Default)]
pub struct Context<'a> {
    pub plan: Option<&'a Plan>,
    pub labels: &'a [Label],
    pub named: std::collections::BTreeSet<(Kind, String, String, String)>,
}

#[derive(Debug, Clone)]
pub struct Job {
    /// Top-level class internal name; also the reply key.
    pub key: String,
    pub classes: Vec<String>,
    pub source: Option<String>,
    pub symbols: Vec<Symbol>,
    /// Classes this job's code references (for the glossary of known names).
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Approximate input-token budget per request.
    pub batch_tokens: usize,
    pub max_jobs_per_batch: usize,
    pub reply_tokens: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { batch_tokens: 24_000, max_jobs_per_batch: 8, reply_tokens: 8_000 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Named(usize),
    Failed(String),
    NotCached,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub outcomes: BTreeMap<String, Outcome>,
    pub truncated: Vec<String>,
    pub requests: usize,
}

fn top_level(c: &str) -> &str {
    let simple_start = c.rfind('/').map(|i| i + 1).unwrap_or(0);
    match c[simple_start..].find('$') {
        Some(i) => &c[..simple_start + i],
        None => c,
    }
}

fn first_declared(world: &World, owner: &str, mm: &Member) -> bool {
    !world.ancestors(owner).iter().any(|a| {
        world
            .classes
            .get(a)
            .map(|ai| ai.methods.iter().any(|x| x.name == mm.name && x.desc == mm.desc && !x.is_private()))
            .unwrap_or(false)
    })
}

/// Build one job per top-level class that has something worth naming.
/// `src_dir` is a decompiled tree (`<pkg>/<Name>.java`); a class without a source
/// file is described by its signatures alone.
pub fn collect_jobs(world: &World, src_dir: Option<&Path>) -> Vec<Job> {
    collect_jobs_with(world, src_dir, &Context::default())
}

/// The current (possibly already renamed) internal name of a class.
pub fn now_class(plan: Option<&Plan>, c: &str) -> String {
    plan.and_then(|p| p.classes.get(c).cloned()).unwrap_or_else(|| c.to_string())
}

/// Does `src` contain a local label token (`v12`, `v3b`)?
fn has_labels(src: &str) -> bool {
    src.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$')).any(super::mapfile::is_label)
}

pub fn collect_jobs_with(world: &World, src_dir: Option<&Path>, ctx: &Context) -> Vec<Job> {
    let mut by_top: BTreeMap<String, Vec<&ClassInfo>> = BTreeMap::new();
    for ci in world.classes.values() {
        by_top.entry(top_level(&ci.name).to_string()).or_default().push(ci);
    }
    let mut jobs = Vec::new();
    for (top, members) in by_top {
        let mut symbols = Vec::new();
        let mut refs = Vec::new();
        let mut n = 0;
        let mut next = || {
            n += 1;
            format!("s{n}")
        };
        for ci in &members {
            refs.extend(ci.class_refs.iter().cloned());
            let simple = simple_name(&ci.name);
            let anon = simple.chars().all(|c| c.is_ascii_digit());
            let mut push = |s: Symbol| {
                if !ctx.named.contains(&s.key()) {
                    symbols.push(s);
                }
            };
            if !anon && looks_obfuscated(simple) && !world.locked_classes.contains_key(&ci.name) {
                push(Symbol::new(next(), Kind::Class, &ci.name, "", ""));
            }
            for f in ci.fields.iter().filter(|f| !f.is_synthetic() && looks_obfuscated(&f.name)) {
                push(Symbol::new(next(), Kind::Field, &ci.name, &f.name, &f.desc));
            }
            for mm in ci.methods.iter().filter(|m| {
                !m.is_synthetic() && m.name != "<init>" && m.name != "<clinit>" && looks_obfuscated(&m.name)
            }) {
                if first_declared(world, &ci.name, mm) {
                    push(Symbol::new(next(), Kind::Method, &ci.name, &mm.name, &mm.desc));
                }
            }
        }
        let file = now_class(ctx.plan, &top);
        let source = src_dir.and_then(|d| std::fs::read_to_string(d.join(format!("{file}.java"))).ok());
        // locals only where the source shows their labels (a labelled decompile)
        if source.as_deref().map(has_labels).unwrap_or(false) {
            for l in ctx.labels.iter().filter(|l| top_level(&l.owner) == top) {
                let mut s = Symbol::new(next(), Kind::Local, &l.owner, &l.method, &l.desc);
                s.local = l.label.clone();
                s.ltype = l.ty.clone();
                s.param = l.param;
                if !ctx.named.contains(&s.key()) {
                    symbols.push(s);
                }
            }
        }
        if symbols.is_empty() {
            continue;
        }
        refs.sort();
        refs.dedup();
        jobs.push(Job { key: top.clone(), classes: members.iter().map(|c| c.name.clone()).collect(), source, symbols, refs });
    }
    jobs
}

fn java_type(desc: &str) -> (String, usize) {
    let c = desc.as_bytes().first().copied().unwrap_or(b'?') as char;
    match c {
        'B' => ("byte".into(), 1),
        'C' => ("char".into(), 1),
        'D' => ("double".into(), 1),
        'F' => ("float".into(), 1),
        'I' => ("int".into(), 1),
        'J' => ("long".into(), 1),
        'S' => ("short".into(), 1),
        'Z' => ("boolean".into(), 1),
        'V' => ("void".into(), 1),
        '[' => {
            let (t, n) = java_type(&desc[1..]);
            (format!("{t}[]"), n + 1)
        }
        'L' => {
            let end = desc.find(';').unwrap_or(desc.len() - 1);
            (desc[1..end].replace(['/', '$'], "."), end + 1)
        }
        _ => ("?".into(), 1),
    }
}

pub fn describe(sym: &Symbol, world: &World) -> String {
    describe_now(sym, world, None)
}

/// Rewrite the class names inside a descriptor to their current names.
fn desc_now(desc: &str, plan: Option<&Plan>) -> String {
    let Some(p) = plan else { return desc.to_string() };
    let mut out = String::new();
    let mut rest = desc;
    while let Some(i) = rest.find('L') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find(';') else {
            break;
        };
        let name = &rest[i + 1..i + j];
        out.push('L');
        out.push_str(&now_class(Some(p), name));
        out.push(';');
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

fn method_sig(name: &str, desc: &str) -> String {
    let close = desc.find(')').unwrap_or(0);
    let inner = &desc[1..close.max(1)];
    let mut args = Vec::new();
    let mut i = 0;
    while i < inner.len() {
        let (t, k) = java_type(&inner[i..]);
        args.push(t);
        i += k;
    }
    format!("{name}({}) : {}", args.join(", "), java_type(&desc[close + 1..]).0)
}

/// `describe`, with owners and members under the names the shown source uses.
pub fn describe_now(sym: &Symbol, world: &World, plan: Option<&Plan>) -> String {
    let dotted = now_class(plan, &sym.owner).replace('/', ".");
    let member_now = |name: &str, desc: &str| -> String {
        plan.and_then(|p| p.methods.get(&(sym.owner.clone(), name.to_string(), desc.to_string())).cloned()).unwrap_or_else(|| name.to_string())
    };
    match sym.kind {
        Kind::Local => {
            let what = if sym.param { "parameter" } else { "local" };
            let m = if sym.name == "<init>" { "constructor".to_string() } else { format!("method {}", member_now(&sym.name, &sym.desc)) };
            format!("{what} {} : {} in {m}{} of {dotted}", sym.local, java_type(&desc_now(&sym.ltype, plan)).0, method_sig("", &desc_now(&sym.desc, plan)))
        }
        Kind::Field if plan.is_some() => {
            let now = plan.and_then(|p| p.fields.get(&(sym.owner.clone(), sym.name.clone())).cloned()).unwrap_or_else(|| sym.name.clone());
            format!("field {dotted}.{now} : {}", java_type(&desc_now(&sym.desc, plan)).0)
        }
        Kind::Method if plan.is_some() => format!("method {dotted}.{}", method_sig(&member_now(&sym.name, &sym.desc), &desc_now(&sym.desc, plan))),
        _ => describe_plain(sym, world, &dotted),
    }
}

fn describe_plain(sym: &Symbol, world: &World, dotted: &str) -> String {
    let dotted = dotted.to_string();
    match sym.kind {
        Kind::Class => {
            let ci = world.classes.get(&sym.owner);
            let mut s = format!("class {dotted}");
            if let Some(ci) = ci {
                if let Some(sup) = ci.super_name.as_deref().filter(|s| *s != "java/lang/Object") {
                    s.push_str(&format!(" extends {}", sup.replace('/', ".")));
                }
                if !ci.interfaces.is_empty() {
                    let v: Vec<String> = ci.interfaces.iter().map(|i| i.replace('/', ".")).collect();
                    s.push_str(&format!(" implements {}", v.join(", ")));
                }
            }
            s
        }
        Kind::Field => format!("field {dotted}.{} : {}", sym.name, java_type(&sym.desc).0),
        Kind::Method => {
            let close = sym.desc.find(')').unwrap_or(0);
            let inner = &sym.desc[1..close.max(1)];
            let mut args = Vec::new();
            let mut i = 0;
            while i < inner.len() {
                let (t, k) = java_type(&inner[i..]);
                args.push(t);
                i += k;
            }
            format!("method {dotted}.{}({}) : {}", sym.name, args.join(", "), java_type(&sym.desc[close + 1..]).0)
        }
        Kind::Local => String::new(),
    }
}

pub const SYSTEM_PROMPT: &str = "You restore meaningful identifiers in decompiled, \
obfuscated Java ME (J2ME/MIDP) code. For each listed symbol, propose a short, \
descriptive Java name that reflects what the code actually does. Rules: classes \
UpperCamelCase; methods and fields lowerCamelCase (static final constants may be \
UPPER_SNAKE_CASE); ASCII letters, digits and underscores only; never a Java keyword; \
never a name of a java.lang or javax.microedition type. Parameters and local \
variables appear in the source under labels such as v4 or v4b; give them lowerCamelCase \
names that differ from every field name. Name only symbols you can \
infer with reasonable confidence from the code — omit the rest rather than guessing. \
Reply with ONE JSON object and nothing else, of the form \
{\"<file key>\": {\"<symbol id>\": \"<new name>\", ...}, ...}. \
Use the file keys and symbol ids exactly as given.";

fn job_block(job: &Job, world: &World, glossary: &BTreeMap<String, String>, budget: usize, plan: Option<&Plan>) -> (String, bool) {
    let mut s = format!("=== FILE {} ===\nSymbols to name:\n", job.key);
    for sym in &job.symbols {
        s.push_str(&format!("{}: {}\n", sym.id, describe_now(sym, world, plan)));
    }
    let known: Vec<String> = job
        .refs
        .iter()
        .filter_map(|r| glossary.get(r).map(|n| format!("{} = {n}", r.replace('/', "."))))
        .collect();
    if !known.is_empty() {
        s.push_str(&format!("Names already chosen for referenced classes: {}\n", known.join("; ")));
    }
    let mut truncated = false;
    match &job.source {
        Some(src) => {
            let room = budget.saturating_sub(estimate_tokens(&s) + 200);
            s.push_str("--- decompiled source ---\n");
            if estimate_tokens(src) <= room {
                s.push_str(src);
            } else {
                truncated = true;
                let keep: String = src.chars().take(room * 4).collect();
                let cut = keep.rfind('\n').unwrap_or(keep.len());
                let omitted = src[cut..].lines().count();
                s.push_str(&keep[..cut]);
                s.push_str(&format!("\n/* … source cut here: {omitted} more lines not shown (request size limit) */\n"));
            }
        }
        None => s.push_str("(no decompiled source available — judge from the signatures)\n"),
    }
    s.push('\n');
    (s, truncated)
}

/// Pack jobs into batches (indices into `jobs`), preserving order.
pub fn batches(jobs: &[Job], world: &World, opt: &Options) -> Vec<Vec<usize>> {
    let empty = BTreeMap::new();
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut used = 0usize;
    for (i, j) in jobs.iter().enumerate() {
        let t = estimate_tokens(&job_block(j, world, &empty, opt.batch_tokens, None).0);
        if !cur.is_empty() && (used + t > opt.batch_tokens || cur.len() >= opt.max_jobs_per_batch) {
            out.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(i);
        used += t;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn build_request(batch: &[&Job], world: &World, glossary: &BTreeMap<String, String>, opt: &Options) -> (ChatRequest, Vec<String>) {
    build_request_with(batch, world, glossary, opt, None)
}

pub fn build_request_with(batch: &[&Job], world: &World, glossary: &BTreeMap<String, String>, opt: &Options, plan: Option<&Plan>) -> (ChatRequest, Vec<String>) {
    let mut user = String::new();
    let mut truncated = Vec::new();
    for j in batch {
        let (b, t) = job_block(j, world, glossary, opt.batch_tokens, plan);
        if t {
            truncated.push(j.key.clone());
        }
        user.push_str(&b);
    }
    (
        ChatRequest {
            system: format!("{SYSTEM_PROMPT}\n[{PROMPT_VERSION}]"),
            messages: vec![ChatMessage::user(user)],
            max_tokens: opt.reply_tokens,
        },
        truncated,
    )
}

/// Turn a reply into entries for the jobs of one batch (pure; unit-tested).
pub fn parse_reply(reply: &str, batch: &[&Job]) -> Result<BTreeMap<String, Vec<NameEntry>>, String> {
    let v = extract_json(reply)?;
    let obj = v.as_object().ok_or("reply is not a JSON object")?;
    let mut out = BTreeMap::new();
    for j in batch {
        let mut entries = Vec::new();
        let per = obj.get(&j.key).or_else(|| obj.get(&j.key.replace('/', ".")));
        if let Some(per) = per.and_then(|p| p.as_object()) {
            for sym in &j.symbols {
                let Some(name) = per.get(&sym.id).and_then(|n| n.as_str()) else { continue };
                let name = name.trim();
                let name = name.rsplit('.').next().unwrap_or(name).trim();
                if name.is_empty() || name.eq_ignore_ascii_case("null") {
                    continue;
                }
                entries.push(NameEntry {
                    kind: sym.kind,
                    owner: sym.owner.clone(),
                    name: sym.name.clone(),
                    desc: sym.desc.clone(),
                    local: sym.local.clone(),
                    new: name.to_string(),
                    origin: "model".into(),
                    status: String::new(),
                });
            }
        }
        out.insert(j.key.clone(), entries);
    }
    if batch.len() > 1 && out.values().all(|e| e.is_empty()) && !obj.is_empty() {
        return Err("reply used none of the file keys".into());
    }
    Ok(out)
}

/// Ask the model about every job.  `glossary` seeds known class names (e.g. from an
/// existing map) and grows as classes are named, so later batches see them.
pub fn run(model: &dyn ChatModel, world: &World, jobs: &[Job], opt: &Options, glossary: &mut BTreeMap<String, String>) -> (Vec<NameEntry>, Report) {
    run_with(model, world, jobs, opt, glossary, None)
}

pub fn run_with(
    model: &dyn ChatModel,
    world: &World,
    jobs: &[Job],
    opt: &Options,
    glossary: &mut BTreeMap<String, String>,
    plan: Option<&Plan>,
) -> (Vec<NameEntry>, Report) {
    let mut report = Report::default();
    let mut all = Vec::new();
    let order = batches(jobs, world, opt);
    let total = order.len();
    for (bi, b) in order.into_iter().enumerate() {
        let batch: Vec<&Job> = b.iter().map(|&i| &jobs[i]).collect();
        eprintln!("   ai-rename: batch {}/{} ({} classes)", bi + 1, total, batch.len());
        let mut pending: Vec<Vec<&Job>> = vec![batch];
        while let Some(group) = pending.pop() {
            let (req, truncated) = build_request_with(&group, world, glossary, opt, plan);
            report.requests += 1;
            let res = model.chat(&req).and_then(|r| parse_reply(&r, &group));
            match res {
                Ok(map) => {
                    for t in truncated {
                        if !report.truncated.contains(&t) {
                            report.truncated.push(t);
                        }
                    }
                    for (k, entries) in map {
                        for e in entries.iter().filter(|e| e.kind == Kind::Class) {
                            glossary.insert(e.owner.clone(), e.new.clone());
                        }
                        report.outcomes.insert(k, Outcome::Named(entries.len()));
                        all.extend(entries);
                    }
                }
                Err(e) if e == CACHE_MISS_OFFLINE => {
                    for j in &group {
                        report.outcomes.insert(j.key.clone(), Outcome::NotCached);
                    }
                }
                Err(e) if group.len() > 1 => {
                    eprintln!("   ai-rename: batch failed ({e}); retrying its classes one by one");
                    for j in group.into_iter().rev() {
                        pending.push(vec![j]);
                    }
                }
                Err(e) => {
                    report.outcomes.insert(group[0].key.clone(), Outcome::Failed(e));
                }
            }
        }
    }
    (all, report)
}
