//! `ai-rename` / `ai-annotate` on a scripted model — no network, fixed answers.
//!
//! * pure planner tests on hand-built class models (no JDK needed): override groups,
//!   the inherited-implementation case, external locks, every kind of conflict;
//! * the batch fallback (a garbage reply makes the batch retry class by class);
//! * end to end on a synthetic "obfuscated" jar (`tests/fixtures/airename`): names
//!   from the scripted model are planned, applied to the bytecode with ASM, and the
//!   renamed program still prints exactly what the original printed — which it
//!   cannot if an override group was split;
//! * `ai-annotate` output still compiles; `--dry-run` sends nothing.

mod common;

use arlecchino::ai::classfile::{self, ClassInfo, Member};
use arlecchino::ai::names::{self, ExternalModel, Kind, NameEntry, NameMap, World};
use arlecchino::ai::{annotate, apply, external, suggest};
use arlecchino::ai_model::ScriptedModel;
use common::{skip, workdir};
use std::collections::BTreeMap;
use std::path::Path;

fn mem(access: u16, name: &str, desc: &str) -> Member {
    Member { access, name: name.into(), desc: desc.into() }
}

fn class(name: &str, sup: &str, ifaces: &[&str], fields: Vec<Member>, methods: Vec<Member>) -> ClassInfo {
    ClassInfo {
        access: 0x0001,
        name: name.into(),
        super_name: Some(sup.into()),
        interfaces: ifaces.iter().map(|s| s.to_string()).collect(),
        fields,
        methods,
        strings: vec![],
        class_refs: vec![],
    }
}

fn iface(name: &str, methods: Vec<Member>) -> ClassInfo {
    let mut c = class(name, "java/lang/Object", &[], vec![], methods);
    c.access = 0x0601;
    c
}

fn entry(kind: Kind, owner: &str, name: &str, desc: &str, new: &str) -> NameEntry {
    NameEntry { kind, owner: owner.into(), name: name.into(), desc: desc.into(), new: new.into(), origin: "model".into(), status: String::new() }
}

fn status<'a>(p: &'a names::Plan, kind: Kind, owner: &str, name: &str) -> &'a str {
    &p.map.entries.iter().find(|e| e.kind == kind && e.owner == owner && e.name == name).expect("entry").status
}

/// A small world: canvas `b` (extends the MIDP Canvas), hierarchy `p` <- `q`,
/// interface `r` implemented through an inherited method (`s` extends `t` implements `r`).
fn sample() -> (BTreeMap<String, ClassInfo>, ExternalModel) {
    let mut cs = BTreeMap::new();
    let pub_ = 0x0001;
    cs.insert("p".into(), class("p", "java/lang/Object", &[], vec![mem(pub_, "a", "I"), mem(pub_, "b", "I")], vec![mem(pub_, "c", "()I"), mem(pub_, "d", "(I)V"), mem(pub_, "<init>", "()V")]));
    cs.insert("q".into(), class("q", "p", &[], vec![mem(pub_, "e", "I")], vec![mem(pub_, "c", "()I"), mem(pub_, "f", "(I)V")]));
    cs.insert("r".into(), iface("r", vec![mem(0x0401, "g", "()V")]));
    cs.insert("t".into(), class("t", "java/lang/Object", &[], vec![], vec![mem(pub_, "g", "()V")]));
    cs.insert("s".into(), class("s", "t", &["r"], vec![], vec![]));
    cs.insert(
        "b".into(),
        class("b", "javax/microedition/lcdui/Canvas", &[], vec![], vec![mem(pub_, "paint", "(Ljavax/microedition/lcdui/Graphics;)V"), mem(pub_, "h", "(I)V"), mem(pub_, "i", "()V")]),
    );
    let mut ext = ExternalModel::builtin();
    let mut canvas = class("javax/microedition/lcdui/Canvas", "java/lang/Object", &[], vec![mem(0x0019, "UP", "I")], vec![
        mem(pub_, "paint", "(Ljavax/microedition/lcdui/Graphics;)V"),
        mem(pub_, "keyPressed", "(I)V"),
        mem(pub_, "repaint", "()V"),
    ]);
    canvas.access = 0x0401;
    ext.add(canvas);
    (cs, ext)
}

fn plan_of(entries: Vec<NameEntry>) -> names::Plan {
    let (cs, ext) = sample();
    let w = World::new(&cs, &ext, &[]);
    names::plan(&w, &NameMap { version: 1, jar_sha256: String::new(), entries })
}

#[test]
fn override_group_follows_the_first_name_and_rejects_a_second() {
    let p = plan_of(vec![entry(Kind::Method, "p", "c", "()I", "area"), entry(Kind::Method, "q", "c", "()I", "volume")]);
    assert_eq!(status(&p, Kind::Method, "p", "c"), "applied; also in q");
    assert!(status(&p, Kind::Method, "q", "c").contains("already named `area`"));
    assert_eq!(p.methods.get(&("q".into(), "c".into(), "()I".into())).map(String::as_str), Some("area"));
    assert_eq!(p.methods.get(&("p".into(), "c".into(), "()I".into())).map(String::as_str), Some("area"));
}

#[test]
fn inherited_implementation_links_interface_and_superclass() {
    // r.g is implemented by t.g only through s; both must get the same name,
    // and references through s must be remapped too.
    let p = plan_of(vec![entry(Kind::Method, "r", "g", "()V", "tick")]);
    assert!(status(&p, Kind::Method, "r", "g").starts_with("applied"));
    for owner in ["r", "t", "s"] {
        assert_eq!(p.methods.get(&(owner.into(), "g".into(), "()V".into())).map(String::as_str), Some("tick"), "owner {owner}");
    }
}

#[test]
fn framework_overrides_are_locked_and_never_shadowed() {
    let p = plan_of(vec![
        entry(Kind::Method, "b", "paint", "(Ljavax/microedition/lcdui/Graphics;)V", "draw"),
        // would accidentally override Canvas.keyPressed(int)
        entry(Kind::Method, "b", "h", "(I)V", "keyPressed"),
        entry(Kind::Method, "b", "i", "()V", "repaint"),
    ]);
    assert!(status(&p, Kind::Method, "b", "paint").contains("locked — overrides javax/microedition/lcdui/Canvas.paint"));
    assert_eq!(status(&p, Kind::Method, "b", "h"), "applied as keyPressed2 (name keyPressed was taken)");
    assert_eq!(status(&p, Kind::Method, "b", "i"), "applied as repaint2 (name repaint was taken)");
}

#[test]
fn conflicts_reserved_words_and_shadowing() {
    let p = plan_of(vec![
        entry(Kind::Class, "p", "", "", "Shape"),
        entry(Kind::Class, "q", "", "", "shape"), // case-insensitive sibling clash
        entry(Kind::Class, "t", "", "", "String"), // java.lang shadow
        entry(Kind::Class, "s", "", "", "class"), // keyword
        entry(Kind::Field, "p", "a", "I", "size"),
        entry(Kind::Field, "q", "e", "I", "size"), // inherited field already named size
        entry(Kind::Field, "p", "b", "I", "Shape"), // would shadow the renamed class
        entry(Kind::Method, "q", "f", "(I)V", "d"), // same params as p.d(I)V in the family
    ]);
    assert_eq!(status(&p, Kind::Class, "p", ""), "applied");
    assert_eq!(status(&p, Kind::Class, "q", ""), "applied as shape2 (name shape was taken)");
    assert_eq!(status(&p, Kind::Class, "t", ""), "applied as String2 (name String was taken)");
    assert!(status(&p, Kind::Class, "s", "").starts_with("rejected: `class` is not a valid"));
    assert_eq!(status(&p, Kind::Field, "p", "a"), "applied");
    assert_eq!(status(&p, Kind::Field, "q", "e"), "applied as size2 (name size was taken)");
    assert!(status(&p, Kind::Field, "p", "b").starts_with("applied as Shape2"));
    assert!(status(&p, Kind::Method, "q", "f").starts_with("applied as d2"));
    assert_eq!(p.classes.get("p").map(String::as_str), Some("Shape"));
    // a field renamed in p is remapped when referenced through the subclass q too
    assert_eq!(p.fields.get(&("q".into(), "a".into())).map(String::as_str), Some("size"));
}

#[test]
fn constructors_and_unknown_supertypes_stay_locked() {
    let (mut cs, ext) = sample();
    cs.insert("v".into(), class("v", "com/vendor/FullCanvas", &[], vec![mem(1, "a", "I")], vec![mem(1, "b", "()V"), mem(0x000A, "c", "()V")]));
    let w = World::new(&cs, &ext, &["s".to_string()]);
    let p = names::plan(&w, &NameMap {
        version: 1,
        jar_sha256: String::new(),
        entries: vec![
            entry(Kind::Method, "p", "<init>", "()V", "make"),
            entry(Kind::Method, "v", "b", "()V", "step"),
            entry(Kind::Method, "v", "c", "()V", "helper"), // private static: cannot override anything
            entry(Kind::Field, "v", "a", "I", "count"),
            entry(Kind::Class, "s", "", "", "Ticker"),
        ],
    });
    assert!(status(&p, Kind::Method, "p", "<init>").starts_with("rejected: `<init>`") || status(&p, Kind::Method, "p", "<init>").contains("constructor"));
    assert!(status(&p, Kind::Method, "v", "b").contains("unknown external supertype com/vendor/FullCanvas"));
    assert_eq!(status(&p, Kind::Method, "v", "c"), "applied");
    assert!(status(&p, Kind::Field, "v", "a").contains("unknown external supertype"));
    assert!(status(&p, Kind::Class, "s", "").contains("named by the jar manifest"));
}

#[test]
fn hand_edits_win_over_new_suggestions() {
    let mut m = NameMap { version: 1, jar_sha256: String::new(), entries: vec![entry(Kind::Class, "p", "", "", "Mine")] };
    m.entries[0].origin = "manual".into();
    let added = m.merge_new(vec![entry(Kind::Class, "p", "", "", "Theirs"), entry(Kind::Class, "q", "", "", "Other")]);
    assert_eq!(added, 1);
    assert_eq!(m.entries[0].new, "Mine");
}

#[test]
fn proguard_mapping_uses_new_names_in_signatures() {
    let (cs, ext) = sample();
    let w = World::new(&cs, &ext, &[]);
    let p = names::plan(&w, &NameMap {
        version: 1,
        jar_sha256: String::new(),
        entries: vec![entry(Kind::Class, "p", "", "", "Shape"), entry(Kind::Method, "p", "d", "(I)V", "grow"), entry(Kind::Field, "p", "a", "I", "size")],
    });
    let txt = names::proguard_mapping(&w, &p);
    assert!(txt.contains("Shape -> p:\n    int size -> a\n    void grow(int) -> d\n"), "{txt}");
}

// ── suggest: batching and fallback ───────────────────────────────────────────

fn reply_for(jobs: &[suggest::Job], picks: &[(Kind, &str, &str, &str)]) -> String {
    let mut root = serde_json::Map::new();
    for j in jobs {
        let mut per = serde_json::Map::new();
        for (k, owner, name, new) in picks {
            if let Some(sym) = j.symbols.iter().find(|s| s.kind == *k && s.owner == *owner && s.name == *name) {
                per.insert(sym.id.clone(), serde_json::Value::String(new.to_string()));
            }
        }
        if !per.is_empty() {
            root.insert(j.key.clone(), serde_json::Value::Object(per));
        }
    }
    serde_json::Value::Object(root).to_string()
}

#[test]
fn garbage_batch_reply_falls_back_to_one_class_per_request() {
    let (cs, ext) = sample();
    let w = World::new(&cs, &ext, &[]);
    let jobs = suggest::collect_jobs(&w, None);
    // b, p, q, r, s, t — `s` has only its class name; `q.c` is not offered (p.c is first)
    assert!(jobs.iter().all(|j| !j.symbols.iter().any(|s| s.owner == "q" && s.name == "c")));
    let pj: Vec<suggest::Job> = jobs.iter().filter(|j| j.key == "p" || j.key == "q").cloned().collect();
    let r1 = reply_for(&pj[..1], &[(Kind::Class, "p", "", "Shape")]);
    let r2 = reply_for(&pj[1..], &[(Kind::Class, "q", "", "Square")]);
    let model = ScriptedModel::new(["this is not JSON".to_string(), r1, r2]);
    let mut glossary = BTreeMap::new();
    let (entries, rep) = suggest::run(&model, &w, &pj, &suggest::Options::default(), &mut glossary);
    assert_eq!(model.requests().len(), 3);
    assert_eq!(rep.requests, 3);
    assert_eq!(entries.len(), 2);
    assert_eq!(glossary.get("p").map(String::as_str), Some("Shape"));
}

#[test]
fn small_budget_splits_batches() {
    let (cs, ext) = sample();
    let w = World::new(&cs, &ext, &[]);
    let jobs = suggest::collect_jobs(&w, None);
    let one = suggest::batches(&jobs, &w, &suggest::Options { batch_tokens: 1, ..Default::default() });
    assert_eq!(one.len(), jobs.len());
    let all = suggest::batches(&jobs, &w, &suggest::Options::default());
    assert_eq!(all.len(), 1);
}

// ── end to end on a real jar ─────────────────────────────────────────────────

fn build_fixture(work: &Path) -> Result<std::path::PathBuf, String> {
    if !common::have(&common::javac()) || !common::have(&common::java()) {
        return Err("javac/java not on PATH".into());
    }
    let classes = work.join("classes");
    common::javac_dir(&common::fixtures_dir().join("airename"), &classes, None, "airename fixture")?;
    let jar = work.join("obf.jar");
    common::pack_jar(&classes, &jar, Some("Manifest-Version: 1.0\nMain-Class: Main\n"));
    Ok(jar)
}

fn run_main(jar: &Path) -> Result<String, String> {
    common::run(&[common::java(), "-cp".into(), jar.to_string_lossy().into_owned(), "Main".into()], "java Main")
}

#[test]
fn renamed_jar_keeps_behaviour() {
    let work = workdir("ai-rename-e2e");
    let jar = match build_fixture(&work) {
        Ok(j) => j,
        Err(e) => return skip(&e),
    };
    if let Err(e) = common::need_java_and_asm() {
        return skip(&e);
    }
    if let Err(e) = apply::ensure_helper(None) {
        return skip(&e);
    }
    let before = run_main(&jar).expect("original runs");
    assert_eq!(before, "14\n7\n2\na7\ne5\n");

    let classes = classfile::read_jar(&jar).unwrap();
    let ext = external::load(&classes, false, true);
    let w = World::new(&classes, &ext, &classfile::manifest_classes(&jar));
    let src = common::fixtures_dir().join("airename");
    let jobs = suggest::collect_jobs(&w, Some(&src));
    assert!(jobs.iter().any(|j| j.source.as_deref().map(|s| s.contains("abstract class a")).unwrap_or(false)));
    let reply = reply_for(&jobs, &[
        (Kind::Class, "a", "", "Shape"),
        (Kind::Class, "k", "", "Shape"),        // clash -> Shape2
        (Kind::Class, "f", "", "Valued"),
        (Kind::Class, "h", "", "Base"),
        (Kind::Class, "i", "", "Impl"),
        (Kind::Class, "j", "", "Counter"),
        (Kind::Field, "a", "b", "size"),
        (Kind::Field, "k", "x", "size"),        // inherited name -> size2
        (Kind::Field, "j", "a", "count"),
        (Kind::Method, "a", "c", "area"),       // k.c follows
        (Kind::Method, "a", "d", "doubled"),
        (Kind::Method, "f", "g", "value"),      // h.g follows through i
        (Kind::Method, "h", "g", "compute"),    // same group -> rejected
        (Kind::Method, "j", "b", "run"),        // would override Runnable.run -> run2
        (Kind::Method, "e", "a", "five"),
    ]);
    let model = ScriptedModel::new([reply]);
    let mut glossary = BTreeMap::new();
    let (entries, rep) = suggest::run(&model, &w, &jobs, &suggest::Options::default(), &mut glossary);
    assert_eq!(rep.requests, 1, "all classes fit one batch");
    assert!(!jobs.iter().any(|j| j.symbols.iter().any(|s| s.kind == Kind::Class && s.owner == "e")), "locked class offered");
    let mut entries = entries;
    entries.push(entry(Kind::Class, "e", "", "", "Loader")); // a hand edit for a locked class
    let map = NameMap { version: 1, jar_sha256: String::new(), entries };
    let plan = names::plan(&w, &map);
    let st = |k, o: &str, n: &str| status(&plan, k, o, n).to_string();
    assert!(st(Kind::Class, "e", "").contains("string constant"), "{}", st(Kind::Class, "e", ""));
    assert!(st(Kind::Method, "h", "g").contains("already named `value`"));
    assert!(st(Kind::Method, "j", "b").starts_with("applied as run2"));
    assert!(st(Kind::Field, "k", "x").starts_with("applied as size2"));

    let out = work.join("renamed.jar");
    let msg = apply::apply(&plan, &jar, &out, None).expect("apply");
    assert!(msg.contains("rewrote"), "{msg}");
    let after = run_main(&out).expect("renamed jar runs");
    // toString() prints the literal "a", which is data, not a name
    assert_eq!(after, before, "dispatch must be unchanged");

    let renamed = classfile::read_jar(&out).unwrap();
    for c in ["Shape", "Shape2", "Valued", "Base", "Impl", "Counter", "e", "Main"] {
        assert!(renamed.contains_key(c), "missing class {c}: {:?}", renamed.keys().collect::<Vec<_>>());
    }
    let has = |c: &str, m: &str| renamed[c].methods.iter().any(|x| x.name == m);
    assert!(has("Shape", "area") && has("Shape2", "area") && has("Shape", "doubled"));
    assert!(has("Base", "value") && has("Valued", "value"));
    assert!(has("Counter", "run") && has("Counter", "run2"));
    assert!(renamed["Shape2"].fields.iter().any(|f| f.name == "size2"));
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn annotated_source_still_compiles() {
    if !common::have(&common::javac()) {
        return skip("javac not on PATH");
    }
    let work = workdir("ai-annotate");
    let src = common::fixtures_dir().join("airename").join("a.java");
    let text = std::fs::read_to_string(&src).unwrap();
    let decl = |needle: &str| text.lines().position(|l| l.contains(needle)).unwrap() + 1;
    let reply = serde_json::json!({"docs": [
        {"line": decl("public abstract class a"), "text": "Base shape. Ends */ early? \\u000a no."},
        {"line": decl("public int d()"), "text": "Twice the area."},
        {"line": decl("protected int b;"), "text": "not a declaration of a method"},
    ]})
    .to_string();
    let model = ScriptedModel::new([reply]);
    let (out, res) = annotate::annotate_file(&model, "a.java", &text, 10_000, 1000);
    assert_eq!(res.inserted, 2, "{:?}", res.rejected);
    assert_eq!(res.rejected.len(), 1);
    let dir = work.join("src");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.java"), &out).unwrap();
    common::javac_dir(&dir, &work.join("classes"), None, "annotated").expect("annotated source compiles");
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn dry_run_sends_nothing_and_saves_prompts() {
    let work = workdir("ai-dry");
    let jar = match build_fixture(&work) {
        Ok(j) => j,
        Err(e) => return skip(&e),
    };
    let out = work.join("out");
    let argv: Vec<String> = [
        "ai-rename",
        jar.to_str().unwrap(),
        "--src",
        common::fixtures_dir().join("airename").to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--dry-run",
        "--cache-dir",
        work.join("cache").to_str().unwrap(),
        "--api-key-env",
        "ARLECCHINO_TEST_NO_SUCH_KEY",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(arlecchino::cli::main(Some(argv)), 0);
    assert!(out.join("ai-prompts").join("batch-001.txt").exists());
    let prompt = std::fs::read_to_string(out.join("ai-prompts").join("batch-001.txt")).unwrap();
    assert!(prompt.contains("=== FILE a ===") && prompt.contains("abstract class a"));
    assert!(out.join("ai-names.json").exists());
    assert!(!out.join("obf.ai-renamed.jar").exists());
    let _ = std::fs::remove_dir_all(&work);
}
