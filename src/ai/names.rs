//! The name map and the rename planner.
//!
//! A [`NameMap`] is the human-editable record of proposed names (`ai-names.json`):
//! one entry per class, field or method, keyed by its CURRENT (obfuscated) name, with
//! the proposed `new` name.  The model fills it; a person may edit it and apply it
//! again.  Nothing in the map is trusted: [`plan`] re-validates every entry against
//! the jar's real class hierarchy and produces a [`Plan`] that is safe to apply:
//!
//! * identifiers must be legal, non-reserved Java identifiers;
//! * a method is renamed as a whole OVERRIDE GROUP — every declaration that overrides,
//!   implements or is reachable under the same name+descriptor from some in-jar class
//!   gets the same name, so virtual dispatch is unchanged.  This includes the
//!   "inherited implementation" case (class `C extends D implements I`, where `D.m`
//!   implements `I.m` without either knowing the other);
//! * a group that overrides or implements anything declared OUTSIDE the jar (JDK,
//!   MIDP, vendor APIs) is locked, as are classes named by the manifest or by string
//!   constants (reflective use), constructors and static initialisers;
//! * a new name must not collide with anything it could be confused with in Java
//!   source: same-parameter methods across the class's whole hierarchy (including
//!   external supertypes — a new name must never ACCIDENTALLY override a framework
//!   method), fields across the hierarchy, sibling class names (case-insensitively,
//!   for case-insensitive file systems), `java.lang` and referenced external simple
//!   names, and package names a field could shadow.  A colliding name is retried with
//!   a numeric suffix (`Sprite2`), otherwise rejected with a reason.
//!
//! The plan is emitted as an owner-expanded remapping table for the ASM
//! `NameApplier` helper (every in-jar class through which a renamed member is
//! referenced gets its own line, because bytecode references name the class they
//! were resolved through, not the declaring one), and as a ProGuard `mapping.txt`
//! that jadx, Recaf and ProGuard retrace understand.

use super::classfile::{ClassInfo, Member};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const MAP_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Class,
    Field,
    Method,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameEntry {
    pub kind: Kind,
    /// Internal name of the class (for `class`: the class itself; for members: the
    /// DECLARING class).
    pub owner: String,
    /// Current member name (empty for `class`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// JVM descriptor (empty for `class`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub desc: String,
    /// Proposed simple name.  Edit this by hand; delete the entry to drop it.
    pub new: String,
    /// `model` or `manual` (anything a person typed should say `manual`).
    #[serde(default = "default_origin")]
    pub origin: String,
    /// Filled in by the planner on every run; ignored on input.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
}

fn default_origin() -> String {
    "manual".into()
}

impl NameEntry {
    pub fn key(&self) -> (Kind, String, String, String) {
        (self.kind, self.owner.clone(), self.name.clone(), self.desc.clone())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NameMap {
    pub version: u32,
    /// SHA-256 of the jar the map was made for (a warning, not an error, on mismatch).
    #[serde(default)]
    pub jar_sha256: String,
    pub entries: Vec<NameEntry>,
}

impl NameMap {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let m: NameMap = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if m.version > MAP_VERSION {
            return Err(format!("{}: map version {} is newer than this tool", path.display(), m.version));
        }
        Ok(m)
    }
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
    }
    /// Add entries that are not in the map yet; existing entries (possibly edited by
    /// hand) always win.  Returns how many were added.
    pub fn merge_new(&mut self, incoming: Vec<NameEntry>) -> usize {
        let have: BTreeSet<_> = self.entries.iter().map(|e| e.key()).collect();
        let mut n = 0;
        for e in incoming {
            if !have.contains(&e.key()) {
                self.entries.push(e);
                n += 1;
            }
        }
        n
    }
}

// ── identifier rules ─────────────────────────────────────────────────────────

const JAVA_RESERVED: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const",
    "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally", "float",
    "for", "goto", "if", "implements", "import", "instanceof", "int", "interface", "long", "native",
    "new", "package", "private", "protected", "public", "return", "short", "static", "strictfp",
    "super", "switch", "synchronized", "this", "throw", "throws", "transient", "try", "void",
    "volatile", "while", "true", "false", "null", "var", "yield", "record", "sealed", "permits",
    "non-sealed", "_",
];

/// `java.lang` simple names: a class named like one of these silently shadows the
/// implicit import in its whole package.
const JAVA_LANG: &[&str] = &[
    "Object", "String", "Class", "System", "Math", "Thread", "Runnable", "Integer", "Long", "Short",
    "Byte", "Character", "Boolean", "Float", "Double", "Number", "Void", "Exception", "Error",
    "Throwable", "RuntimeException", "StringBuffer", "StringBuilder", "Runtime", "Comparable",
    "Iterable", "CharSequence", "Cloneable", "AutoCloseable", "Override", "Deprecated", "Enum",
    "Record", "Process", "Package", "ClassLoader", "SecurityManager", "Module", "Compiler",
    "InterruptedException", "ArithmeticException", "ClassCastException", "NullPointerException",
    "IllegalArgumentException", "IllegalStateException", "IndexOutOfBoundsException",
    "ArrayIndexOutOfBoundsException", "OutOfMemoryError", "SuppressWarnings", "FunctionalInterface",
    "SafeVarargs", "StackOverflowError", "VirtualMachineError", "ThreadGroup", "ThreadLocal",
];

pub fn is_valid_identifier(s: &str) -> bool {
    let mut it = s.chars();
    let Some(first) = it.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    s.len() <= 64
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !JAVA_RESERVED.contains(&s)
}

/// Does a name look machine-generated?  ProGuard's default dictionary yields `a`,
/// `b`, … `aa`, `ab`, …; our own collision renamer yields `<name>_<8 hex>`.
pub fn looks_obfuscated(simple: &str) -> bool {
    if simple.is_empty() {
        return false;
    }
    let short = simple.len() <= 3 && simple.chars().all(|c| c.is_ascii_alphanumeric()) && simple.chars().next().unwrap().is_ascii_alphabetic();
    let tagged = match simple.rsplit_once('_') {
        Some((head, tag)) => tag.len() == 8 && tag.chars().all(|c| c.is_ascii_hexdigit()) && looks_obfuscated(head),
        None => false,
    };
    short || tagged
}

pub fn simple_name(internal: &str) -> &str {
    let s = internal.rsplit('/').next().unwrap_or(internal);
    s.rsplit('$').next().unwrap_or(s)
}

pub fn package_of(internal: &str) -> &str {
    internal.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

fn params_of(desc: &str) -> &str {
    match desc.find(')') {
        Some(i) => &desc[..=i],
        None => desc,
    }
}

// ── external (out-of-jar) type model ─────────────────────────────────────────

/// What is known about types outside the jar.  `known` holds parsed class models
/// (from the generated MIDP stub, from `javap` on the JDK, or the built-ins below);
/// an external supertype that is NOT in `known` makes its subclasses "opaque".
#[derive(Debug, Clone, Default)]
pub struct ExternalModel {
    pub known: BTreeMap<String, ClassInfo>,
}

fn m(name: &str, desc: &str) -> Member {
    Member { access: 0x0001, name: name.into(), desc: desc.into() }
}

impl ExternalModel {
    /// `java.lang.Object` and `java.lang.Runnable` are always known exactly.
    pub fn builtin() -> Self {
        let mut known = BTreeMap::new();
        known.insert(
            "java/lang/Object".to_string(),
            ClassInfo {
                access: 0x0001,
                name: "java/lang/Object".into(),
                super_name: None,
                interfaces: vec![],
                fields: vec![],
                methods: vec![
                    m("equals", "(Ljava/lang/Object;)Z"),
                    m("hashCode", "()I"),
                    m("toString", "()Ljava/lang/String;"),
                    m("getClass", "()Ljava/lang/Class;"),
                    m("notify", "()V"),
                    m("notifyAll", "()V"),
                    m("wait", "()V"),
                    m("wait", "(J)V"),
                    m("wait", "(JI)V"),
                    m("clone", "()Ljava/lang/Object;"),
                    m("finalize", "()V"),
                ],
                strings: vec![],
                class_refs: vec![],
            },
        );
        known.insert(
            "java/lang/Runnable".to_string(),
            ClassInfo {
                access: 0x0601,
                name: "java/lang/Runnable".into(),
                super_name: Some("java/lang/Object".into()),
                interfaces: vec![],
                fields: vec![],
                methods: vec![m("run", "()V")],
                strings: vec![],
                class_refs: vec![],
            },
        );
        ExternalModel { known }
    }

    pub fn add(&mut self, ci: ClassInfo) {
        self.known.entry(ci.name.clone()).or_insert(ci);
    }
}

// ── the world: in-jar classes + hierarchy queries ────────────────────────────

pub struct World<'a> {
    pub classes: &'a BTreeMap<String, ClassInfo>,
    pub ext: &'a ExternalModel,
    /// Classes that must keep their names (manifest, reflective strings, …), with why.
    pub locked_classes: BTreeMap<String, String>,
    descendants: HashMap<String, BTreeSet<String>>,
}

impl<'a> World<'a> {
    pub fn new(classes: &'a BTreeMap<String, ClassInfo>, ext: &'a ExternalModel, manifest: &[String]) -> Self {
        let mut locked_classes = BTreeMap::new();
        for c in manifest {
            if classes.contains_key(c) {
                locked_classes.insert(c.clone(), "named by the jar manifest".to_string());
            }
        }
        // A class name inside a string matters only where it can reach the
        // reflection API — i.e. in classes that reference java/lang/Class
        // (Class.forName, newInstance).  Everywhere else a one-letter literal such as
        // "a" is just text and must not freeze class `a`.
        for ci in classes.values().filter(|ci| ci.class_refs.iter().any(|r| r == "java/lang/Class")) {
            for s in &ci.strings {
                let slashed = s.replace('.', "/");
                if classes.contains_key(&slashed) && slashed.contains(|c: char| c.is_ascii_alphabetic()) {
                    locked_classes
                        .entry(slashed)
                        .or_insert_with(|| format!("its name appears as a string constant in {} (reflection)", ci.name));
                }
            }
        }
        let mut descendants: HashMap<String, BTreeSet<String>> = HashMap::new();
        for c in classes.keys() {
            for a in Self::ancestors_of(classes, ext, c) {
                descendants.entry(a).or_default().insert(c.clone());
            }
        }
        World { classes, ext, locked_classes, descendants }
    }

    fn info(&self, n: &str) -> Option<&ClassInfo> {
        self.classes.get(n).or_else(|| self.ext.known.get(n))
    }

    fn ancestors_of(classes: &BTreeMap<String, ClassInfo>, ext: &ExternalModel, c: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut work: Vec<String> = Vec::new();
        if let Some(ci) = classes.get(c).or_else(|| ext.known.get(c)) {
            work.extend(ci.supertypes().cloned());
        }
        while let Some(s) = work.pop() {
            if !seen.insert(s.clone()) {
                continue;
            }
            out.push(s.clone());
            if let Some(si) = classes.get(&s).or_else(|| ext.known.get(&s)) {
                work.extend(si.supertypes().cloned());
            }
        }
        out
    }

    /// Strict ancestors (in-jar and external), deterministic order.
    pub fn ancestors(&self, c: &str) -> Vec<String> {
        Self::ancestors_of(self.classes, self.ext, c)
    }

    pub fn in_jar(&self, c: &str) -> bool {
        self.classes.contains_key(c)
    }

    /// External ancestors we know nothing about (vendor APIs, or JDK types when
    /// `javap` was unavailable).
    pub fn opaque_ancestors(&self, c: &str) -> Vec<String> {
        self.ancestors(c)
            .into_iter()
            .filter(|a| !self.in_jar(a) && !self.ext.known.contains_key(a))
            .collect()
    }

    /// self ∪ in-jar ancestors ∪ in-jar descendants: every class whose members can
    /// meet this one's under one name in source.
    pub fn family(&self, c: &str) -> BTreeSet<String> {
        let mut f: BTreeSet<String> = self.ancestors(c).into_iter().collect();
        f.insert(c.to_string());
        if let Some(d) = self.descendants.get(c) {
            f.extend(d.iter().cloned());
        }
        f
    }

    /// Resolve a field reference `C.name` the way the JVM does (JVMS §5.4.3.2):
    /// the class itself, then its superinterfaces, then its superclass.
    pub fn resolve_field(&self, c: &str, name: &str) -> Option<String> {
        let ci = self.info(c)?;
        if ci.fields.iter().any(|f| f.name == name) {
            return Some(c.to_string());
        }
        for i in &ci.interfaces {
            if let Some(o) = self.resolve_field(i, name) {
                return Some(o);
            }
        }
        ci.super_name.as_deref().and_then(|s| self.resolve_field(s, name))
    }
}

// ── method override groups ───────────────────────────────────────────────────

type MKey = (String, String, String); // owner, name, desc

struct Groups {
    parent: HashMap<MKey, MKey>,
}

impl Groups {
    fn find(&mut self, k: &MKey) -> MKey {
        let p = self.parent.get(k).cloned().unwrap_or_else(|| k.clone());
        if &p == k {
            return p;
        }
        let r = self.find(&p);
        self.parent.insert(k.clone(), r.clone());
        r
    }
    fn union(&mut self, a: &MKey, b: &MKey) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // deterministic root: the smaller key
            if ra < rb {
                self.parent.insert(rb, ra);
            } else {
                self.parent.insert(ra, rb);
            }
        }
    }
}

fn inheritable(mm: &Member) -> bool {
    !mm.is_private() && mm.name != "<init>" && mm.name != "<clinit>"
}

// ── the plan ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// old internal name -> new internal name (includes nested classes that follow a
    /// renamed outer class).
    pub classes: BTreeMap<String, String>,
    /// (class through which it is referenced, name, desc) -> new name.
    pub methods: BTreeMap<(String, String, String), String>,
    /// (class through which it is referenced, name) -> new name.
    pub fields: BTreeMap<(String, String), String>,
    /// The input map with `status` filled in.
    pub map: NameMap,
    pub applied: usize,
    pub rejected: usize,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty() && self.methods.is_empty() && self.fields.is_empty()
    }

    /// Tab-separated table for `NameApplier`:
    /// `class OLD NEW` / `method OWNER NAME DESC NEW` / `field OWNER NAME NEW`.
    pub fn applier_table(&self) -> String {
        let mut s = String::new();
        for (o, n) in &self.classes {
            s.push_str(&format!("class\t{o}\t{n}\n"));
        }
        for ((o, nm, d), n) in &self.methods {
            s.push_str(&format!("method\t{o}\t{nm}\t{d}\t{n}\n"));
        }
        for ((o, nm), n) in &self.fields {
            s.push_str(&format!("field\t{o}\t{nm}\t{n}\n"));
        }
        s
    }
}

/// Everything locked for a reason, discovered before any name is considered.
struct Locks {
    groups: BTreeMap<MKey, String>, // group root -> reason
    fields: BTreeMap<(String, String), String>,
}

pub fn plan(world: &World, input: &NameMap) -> Plan {
    let classes = world.classes;

    // 1) override groups + visibility --------------------------------------------
    let mut g = Groups { parent: HashMap::new() };
    // visible[C][(name,desc)] = declarations reachable from C
    let mut visible: BTreeMap<String, BTreeMap<(String, String), Vec<MKey>>> = BTreeMap::new();
    let mut lock_reasons: Vec<(MKey, String)> = Vec::new();
    for (c, ci) in classes {
        let mut vis: BTreeMap<(String, String), Vec<MKey>> = BTreeMap::new();
        for mm in &ci.methods {
            vis.entry((mm.name.clone(), mm.desc.clone())).or_default().push((c.clone(), mm.name.clone(), mm.desc.clone()));
        }
        let opaque = world.opaque_ancestors(c);
        for a in world.ancestors(c).iter().filter(|a| world.in_jar(a)) {
            let Some(ai) = world.info(a) else { continue };
            for mm in ai.methods.iter().filter(|mm| inheritable(mm)) {
                vis.entry((mm.name.clone(), mm.desc.clone())).or_default().push((a.clone(), mm.name.clone(), mm.desc.clone()));
            }
        }
        // a signature visible from an external supertype locks every in-jar decl that
        // shares it from this class's point of view
        for a in world.ancestors(c).iter().filter(|a| !world.in_jar(a)) {
            if let Some(ai) = world.info(a) {
                for mm in ai.methods.iter().filter(|mm| inheritable(mm)) {
                    if let Some(decls) = vis.get(&(mm.name.clone(), mm.desc.clone())) {
                        for d in decls {
                            lock_reasons.push((d.clone(), format!("overrides {}.{}{}", a, mm.name, mm.desc)));
                        }
                    }
                }
            }
        }
        for decls in vis.values() {
            let inh: Vec<&MKey> = decls
                .iter()
                .filter(|k| {
                    world
                        .info(&k.0)
                        .and_then(|i| i.methods.iter().find(|mm| mm.name == k.1 && mm.desc == k.2))
                        .map(inheritable)
                        .unwrap_or(false)
                })
                .collect();
            for w in inh.windows(2) {
                g.union(w[0], w[1]);
            }
            if !opaque.is_empty() {
                for k in &inh {
                    let mm = world.info(&k.0).and_then(|i| i.methods.iter().find(|mm| mm.name == k.1 && mm.desc == k.2));
                    if mm.map(|mm| !mm.is_static()).unwrap_or(false) {
                        lock_reasons.push(((*k).clone(), format!("class {c} has an unknown external supertype {}", opaque[0])));
                    }
                }
            }
        }
        visible.insert(c.clone(), vis);
    }
    let mut locks = Locks { groups: BTreeMap::new(), fields: BTreeMap::new() };
    for (k, why) in lock_reasons {
        let r = g.find(&k);
        locks.groups.entry(r).or_insert(why);
    }
    for (c, ci) in classes {
        for mm in &ci.methods {
            if mm.name == "<init>" || mm.name == "<clinit>" {
                let r = g.find(&(c.clone(), mm.name.clone(), mm.desc.clone()));
                locks.groups.insert(r, "constructor / static initialiser".into());
            }
            if mm.name == "main" && mm.desc == "([Ljava/lang/String;)V" && mm.is_static() {
                let r = g.find(&(c.clone(), mm.name.clone(), mm.desc.clone()));
                locks.groups.insert(r, "program entry point".into());
            }
        }
        let opaque = world.opaque_ancestors(c);
        if !opaque.is_empty() {
            for f in ci.fields.iter().filter(|f| !f.is_private()) {
                locks.fields.insert((c.clone(), f.name.clone()), format!("class {c} has an unknown external supertype {}", opaque[0]));
            }
        }
    }
    // group members
    let mut members: BTreeMap<MKey, Vec<MKey>> = BTreeMap::new();
    for (c, ci) in classes {
        for mm in &ci.methods {
            let k = (c.clone(), mm.name.clone(), mm.desc.clone());
            let r = g.find(&k);
            members.entry(r).or_default().push(k);
        }
    }

    // 2) the state the checks run against ----------------------------------------
    let mut class_new: BTreeMap<String, String> = BTreeMap::new(); // simple-name decisions
    let mut group_new: BTreeMap<MKey, String> = BTreeMap::new();
    let mut field_new: BTreeMap<(String, String), String> = BTreeMap::new();

    // external simple names the jar can see (types it references or extends)
    let mut ext_simple: BTreeSet<String> = JAVA_LANG.iter().map(|s| s.to_string()).collect();
    let mut packages_top: BTreeSet<String> = BTreeSet::new();
    for (c, ci) in classes {
        for r in ci.class_refs.iter().chain(ci.supertypes()) {
            if !world.in_jar(r) {
                ext_simple.insert(simple_name(r).to_string());
            }
        }
        for d in ci.fields.iter().map(|f| &f.desc).chain(ci.methods.iter().map(|m| &m.desc)) {
            for t in desc_class_names(d) {
                if !world.in_jar(&t) {
                    ext_simple.insert(simple_name(&t).to_string());
                }
            }
        }
        if let Some(top) = c.split('/').next().filter(|_| c.contains('/')) {
            packages_top.insert(top.to_string());
        }
        for t in world.ancestors(c).iter().filter(|a| !world.in_jar(a)) {
            if let Some(top) = t.split('/').next() {
                packages_top.insert(top.to_string());
            }
        }
    }

    let mut out_map = input.clone();
    // deterministic processing order: classes, then methods, then fields; inside a
    // kind, shallower owners first (a supertype's name wins over a subtype's)
    let depth = |c: &str| world.ancestors(c).len();
    let mut order: Vec<usize> = (0..out_map.entries.len()).collect();
    order.sort_by(|&a, &b| {
        let (ea, eb) = (&out_map.entries[a], &out_map.entries[b]);
        ea.kind
            .cmp(&eb.kind)
            .then(depth(&ea.owner).cmp(&depth(&eb.owner)))
            .then(ea.key().cmp(&eb.key()))
    });

    let mut applied = 0;
    let mut rejected = 0;
    let mut seen_keys: BTreeSet<(Kind, String, String, String)> = BTreeSet::new();
    for idx in order {
        let e = out_map.entries[idx].clone();
        let status = if !seen_keys.insert(e.key()) {
            Err("duplicate entry (the first one is used)".to_string())
        } else {
            match e.kind {
                Kind::Class => decide_class(world, &e, &mut class_new, &ext_simple),
                Kind::Method => decide_method(world, &e, &mut g, &members, &locks, &mut group_new),
                Kind::Field => decide_field(world, &e, &locks, &class_new, &mut field_new, &ext_simple, &packages_top),
            }
        };
        out_map.entries[idx].status = match status {
            Ok(s) => {
                applied += 1;
                s
            }
            Err(s) => {
                rejected += 1;
                format!("rejected: {s}")
            }
        };
    }

    // 3) expand into the owner-qualified remapping table -------------------------
    let mut plan = Plan { map: out_map, applied, rejected, ..Default::default() };
    for c in classes.keys() {
        let f = final_class_name(world, c, &class_new);
        if &f != c {
            plan.classes.insert(c.clone(), f);
        }
    }
    for (c, vis) in &visible {
        for ((name, desc), decls) in vis {
            for d in decls {
                let r = g.find(d);
                if let Some(n) = group_new.get(&r) {
                    plan.methods.insert((c.clone(), name.clone(), desc.clone()), n.clone());
                    break;
                }
            }
        }
    }
    for (c, ci) in classes {
        // every field name that can be referenced through c
        let mut names: BTreeSet<String> = ci.fields.iter().map(|f| f.name.clone()).collect();
        for a in world.ancestors(c) {
            if let Some(ai) = world.classes.get(&a) {
                names.extend(ai.fields.iter().map(|f| f.name.clone()));
            }
        }
        for n in names {
            if let Some(owner) = world.resolve_field(c, &n) {
                if let Some(nn) = field_new.get(&(owner, n.clone())) {
                    plan.fields.insert((c.clone(), n), nn.clone());
                }
            }
        }
    }
    plan
}

/// The final internal name of `c`: package + new simple name, or — for a nested
/// class `Outer$Inner` — the outer class's final name + `$` + (new) inner name.
pub fn final_class_name(world: &World, c: &str, class_new: &BTreeMap<String, String>) -> String {
    if let Some((outer, inner)) = c.rsplit_once('$') {
        if world.in_jar(outer) {
            let o = final_class_name(world, outer, class_new);
            let i = class_new.get(c).map(|s| s.as_str()).unwrap_or(inner);
            return format!("{o}${i}");
        }
    }
    match class_new.get(c) {
        Some(n) => {
            let p = package_of(c);
            if p.is_empty() {
                n.clone()
            } else {
                format!("{p}/{n}")
            }
        }
        None => c.to_string(),
    }
}

/// Try `want`, then `want2` … `want9`, accepting the first candidate `ok` allows.
fn with_suffix(want: &str, mut ok: impl FnMut(&str) -> Result<(), String>) -> Result<String, String> {
    let first = ok(want);
    if first.is_ok() {
        return Ok(want.to_string());
    }
    for i in 2..=9 {
        let cand = format!("{want}{i}");
        if ok(&cand).is_ok() {
            return Ok(cand);
        }
    }
    Err(first.unwrap_err())
}

fn applied_status(want: &str, got: &str) -> String {
    if want == got {
        "applied".into()
    } else {
        format!("applied as {got} (name {want} was taken)")
    }
}

fn decide_class(
    world: &World,
    e: &NameEntry,
    class_new: &mut BTreeMap<String, String>,
    ext_simple: &BTreeSet<String>,
) -> Result<String, String> {
    if !world.in_jar(&e.owner) {
        return Err(format!("class {} is not in the jar", e.owner));
    }
    if let Some(why) = world.locked_classes.get(&e.owner) {
        return Err(format!("locked — {why}"));
    }
    if !is_valid_identifier(&e.new) || e.new.contains('$') {
        return Err(format!("`{}` is not a valid Java class name", e.new));
    }
    if e.new == simple_name(&e.owner) {
        return Err("same as the current name".into());
    }
    let nested = e.owner.rsplit_once('$').map(|(o, _)| world.in_jar(o)).unwrap_or(false);
    if nested && e.owner.rsplit('$').next().map(|s| s.chars().all(|c| c.is_ascii_digit())).unwrap_or(false) {
        return Err("anonymous class — its name is a number by definition".into());
    }
    let got = with_suffix(&e.new, |cand| {
        if ext_simple.contains(cand) {
            return Err(format!("`{cand}` would shadow a java.lang / platform type of that name"));
        }
        // siblings: classes whose final name lives in the same scope
        let mut trial = class_new.clone();
        trial.insert(e.owner.clone(), cand.to_string());
        let mine = final_class_name(world, &e.owner, &trial);
        let scope = |n: &str| -> String {
            match n.rsplit_once('$') {
                Some((o, _)) if nested => o.to_string(),
                _ => package_of(n).to_string(),
            }
        };
        for other in world.classes.keys().filter(|o| *o != &e.owner) {
            let f = final_class_name(world, other, &trial);
            if scope(&f) == scope(&mine) && f.eq_ignore_ascii_case(&mine) {
                return Err(format!("`{cand}` collides with class {other}"));
            }
        }
        // a top-level class must not take a name equal to a package segment it sits next to
        if !nested && world.classes.keys().any(|o| package_of(o).eq_ignore_ascii_case(&mine)) {
            return Err(format!("`{cand}` collides with a package of the same name"));
        }
        Ok(())
    })?;
    class_new.insert(e.owner.clone(), got.clone());
    Ok(applied_status(&e.new, &got))
}

fn decide_method(
    world: &World,
    e: &NameEntry,
    g: &mut Groups,
    members: &BTreeMap<MKey, Vec<MKey>>,
    locks: &Locks,
    group_new: &mut BTreeMap<MKey, String>,
) -> Result<String, String> {
    let Some(ci) = world.classes.get(&e.owner) else {
        return Err(format!("class {} is not in the jar", e.owner));
    };
    if !ci.methods.iter().any(|mm| mm.name == e.name && mm.desc == e.desc) {
        return Err(format!("{}.{}{} is not declared there", e.owner, e.name, e.desc));
    }
    if !is_valid_identifier(&e.new) {
        return Err(format!("`{}` is not a valid Java method name", e.new));
    }
    if e.new == e.name {
        return Err("same as the current name".into());
    }
    let root = g.find(&(e.owner.clone(), e.name.clone(), e.desc.clone()));
    if let Some(why) = locks.groups.get(&root) {
        return Err(format!("locked — {why}"));
    }
    if let Some(prev) = group_new.get(&root) {
        return Err(format!("its override group was already named `{prev}`"));
    }
    let group = members.get(&root).cloned().unwrap_or_default();
    let params = params_of(&e.desc).to_string();
    // current name of any method declaration, honouring decisions made so far
    let cur_name = |g: &mut Groups, group_new: &BTreeMap<MKey, String>, k: &MKey| -> String {
        let r = g.find(k);
        group_new.get(&r).cloned().unwrap_or_else(|| k.1.clone())
    };
    let mut family: BTreeSet<String> = BTreeSet::new();
    for k in &group {
        family.extend(world.family(&k.0));
    }
    let mut clash_check = |cand: &str| -> Result<(), String> {
        for c in &family {
            let Some(fi) = world.info(c) else { continue };
            for mm in &fi.methods {
                if params_of(&mm.desc) != params {
                    continue;
                }
                let k = (c.clone(), mm.name.clone(), mm.desc.clone());
                if world.in_jar(c) && g.find(&k) == root {
                    continue;
                }
                let n = if world.in_jar(c) { cur_name(g, group_new, &k) } else { mm.name.clone() };
                if n == cand {
                    let what = if world.in_jar(c) { "collide with" } else { "accidentally override" };
                    return Err(format!("`{cand}{params}` would {what} {c}.{}{}", mm.name, mm.desc));
                }
            }
        }
        Ok(())
    };
    let got = with_suffix(&e.new, &mut clash_check)?;
    group_new.insert(root, got.clone());
    let mut st = applied_status(&e.new, &got);
    let others: Vec<String> = group.iter().filter(|k| k.0 != e.owner).map(|k| k.0.clone()).collect();
    if !others.is_empty() {
        st.push_str(&format!("; also in {}", others.join(", ")));
    }
    Ok(st)
}

fn decide_field(
    world: &World,
    e: &NameEntry,
    locks: &Locks,
    class_new: &BTreeMap<String, String>,
    field_new: &mut BTreeMap<(String, String), String>,
    ext_simple: &BTreeSet<String>,
    packages_top: &BTreeSet<String>,
) -> Result<String, String> {
    let Some(ci) = world.classes.get(&e.owner) else {
        return Err(format!("class {} is not in the jar", e.owner));
    };
    if !ci.fields.iter().any(|f| f.name == e.name) {
        return Err(format!("{}.{} is not declared there", e.owner, e.name));
    }
    if !is_valid_identifier(&e.new) {
        return Err(format!("`{}` is not a valid Java field name", e.new));
    }
    if e.new == e.name {
        return Err("same as the current name".into());
    }
    if let Some(why) = locks.fields.get(&(e.owner.clone(), e.name.clone())) {
        return Err(format!("locked — {why}"));
    }
    let class_simple: BTreeSet<String> =
        world.classes.keys().map(|c| simple_name(&final_class_name(world, c, class_new)).to_string()).collect();
    let family = world.family(&e.owner);
    let got = with_suffix(&e.new, |cand| {
        if class_simple.contains(cand) || ext_simple.contains(cand) {
            return Err(format!("`{cand}` would shadow a type of the same name"));
        }
        if packages_top.contains(cand) {
            return Err(format!("`{cand}` would shadow the package `{cand}`"));
        }
        for c in &family {
            let Some(fi) = world.info(c) else { continue };
            for f in &fi.fields {
                if c == &e.owner && f.name == e.name {
                    continue;
                }
                let cur = field_new.get(&(c.clone(), f.name.clone())).cloned().unwrap_or_else(|| f.name.clone());
                if cur == cand {
                    return Err(format!("`{cand}` collides with field {c}.{}", f.name));
                }
            }
        }
        Ok(())
    })?;
    field_new.insert((e.owner.clone(), e.name.clone()), got.clone());
    Ok(applied_status(&e.new, &got))
}

/// Class names (internal form) mentioned by a descriptor.
pub fn desc_class_names(desc: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = desc.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == 'L' {
            if let Some(end) = desc[i..].find(';') {
                out.push(desc[i + 1..i + end].to_string());
                for _ in 0..end {
                    it.next();
                }
            }
        }
    }
    out
}

/// One descriptor type as Java source text, with in-jar classes mapped.
fn java_type(desc: &str, map: &dyn Fn(&str) -> String) -> (String, usize) {
    let c = desc.as_bytes()[0] as char;
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
            let (t, n) = java_type(&desc[1..], map);
            (format!("{t}[]"), n + 1)
        }
        'L' => {
            let end = desc.find(';').unwrap_or(desc.len() - 1);
            (map(&desc[1..end]).replace(['/', '$'], "."), end + 1)
        }
        _ => ("?".into(), 1),
    }
}

/// ProGuard `mapping.txt` (deobfuscated -> obfuscated), readable by jadx
/// (`--mappings-path`), Recaf and ProGuard retrace.
pub fn proguard_mapping(world: &World, plan: &Plan) -> String {
    let fin = |c: &str| plan.classes.get(c).cloned().unwrap_or_else(|| c.to_string());
    let mut s = String::from("# generated by arlecchino ai-rename: new name -> name in the input jar\n");
    for (c, ci) in world.classes {
        let mut lines = Vec::new();
        for f in &ci.fields {
            if let Some(n) = plan.fields.get(&(c.clone(), f.name.clone())) {
                if world.resolve_field(c, &f.name).as_deref() == Some(c.as_str()) {
                    let (t, _) = java_type(&f.desc, &fin);
                    lines.push(format!("    {t} {n} -> {}", f.name));
                }
            }
        }
        for mm in &ci.methods {
            if let Some(n) = plan.methods.get(&(c.clone(), mm.name.clone(), mm.desc.clone())) {
                let inner = &mm.desc[1..mm.desc.find(')').unwrap_or(1)];
                let mut args = Vec::new();
                let mut i = 0;
                while i < inner.len() {
                    let (t, k) = java_type(&inner[i..], &fin);
                    args.push(t);
                    i += k;
                }
                let (ret, _) = java_type(&mm.desc[mm.desc.find(')').map(|x| x + 1).unwrap_or(0)..], &fin);
                lines.push(format!("    {ret} {n}({}) -> {}", args.join(","), mm.name));
            }
        }
        if plan.classes.contains_key(c) || !lines.is_empty() {
            s.push_str(&format!("{} -> {}:\n", fin(c).replace('/', "."), c.replace('/', ".")));
            for l in lines {
                s.push_str(&l);
                s.push('\n');
            }
        }
    }
    s
}
