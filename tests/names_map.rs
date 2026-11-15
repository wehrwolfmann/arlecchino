//! Name maps (`.map`, the Sporeflower format) without any model:
//!
//! * a hand-written map (`tests/fixtures/names/synth.map`) applied to the synthetic
//!   test MIDlet renames a class and a field and names parameters and locals through
//!   a synthesized LocalVariableTable; a parameter that would shadow a field is
//!   rejected; the file itself is not touched;
//! * `--complete` turns the map into a worksheet (every unnamed local listed by its
//!   label) and keeps every line a person wrote byte for byte;
//! * the renamed MIDlet behaves exactly like the original under midp-rt;
//! * the patcher fills unnamed tokens only, never a name a person chose.

mod common;

use arlecchino::ai::classfile::{self, ClassInfo, Member};
use arlecchino::ai::mapfile;
use arlecchino::ai::names::{self, ExternalModel, World};
use arlecchino::ai::{apply, suggest};
use arlecchino::{difftest, diffrun, midprun};
use common::{skip, workdir};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn argv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn javap_locals(jar: &Path, class: &str) -> Result<String, String> {
    common::run(&["javap".into(), "-l".into(), "-p".into(), "-cp".into(), jar.to_string_lossy().into_owned(), class.into()], "javap")
}

fn synth(work: &Path) -> Result<PathBuf, String> {
    common::need_java_and_asm()?;
    apply::ensure_helper(None)?;
    if !common::have("javap") {
        return Err("javap not on PATH".into());
    }
    common::build_synth_midlet(work, 0x00CC66)
}

#[test]
fn hand_written_map_names_classes_members_and_locals() {
    let work = workdir("names-synth");
    let jar = match synth(&work) {
        Ok(j) => j,
        Err(e) => return skip(&e),
    };
    let map = work.join("synth.map");
    let original = std::fs::read_to_string(common::fixtures_dir().join("names").join("synth.map")).unwrap();
    std::fs::write(&map, &original).unwrap();
    let out = work.join("out");
    let code = arlecchino::cli::main(Some(argv(&["apply-names", jar.to_str().unwrap(), "--names", map.to_str().unwrap(), "-o", out.to_str().unwrap()])));
    assert_eq!(code, 0);
    let renamed = out.join(format!("{}.ai-renamed.jar", jar.file_stem().unwrap().to_string_lossy()));
    assert!(renamed.exists());

    let classes = classfile::read_jar(&renamed).unwrap();
    assert!(classes.contains_key("CheckerCanvas") && !classes.contains_key("SynthCanvas"), "{:?}", classes.keys());
    assert!(classes["CheckerCanvas"].fields.iter().any(|f| f.name == "lastKeyCode"));
    assert!(classes["SynthMidlet"].methods.iter().any(|m| m.name == "startApp"), "framework callbacks keep their names");

    let lv = javap_locals(&renamed, "CheckerCanvas").unwrap();
    for n in ["graphics", "width", "height", "row", "column"] {
        assert!(lv.contains(&format!(" {n}   ")) || lv.contains(&format!(" {n} ")), "local {n} missing:\n{lv}");
    }
    // the shadowing parameter was refused and keeps its label
    assert!(!lv.contains(" lastKeyCode   I"), "{lv}");
    let report = std::fs::read_to_string(out.join("ai-rename-report.md")).unwrap();
    assert!(report.contains("would shadow a field"), "{report}");
    let lm = javap_locals(&renamed, "SynthMidlet").unwrap();
    assert!(lm.contains("unconditional   Z"), "a boolean parameter keeps its declared type:\n{lm}");
    assert!(lm.contains(" store ") && lm.contains(" bytes ") && lm.contains(" unconditional "), "{lm}");

    // the label table lists what the applier labelled
    let labels = mapfile::read_labels(&out.join("labels.tsv"));
    assert!(labels.iter().any(|l| l.owner == "SynthCanvas" && l.method == "paint" && l.label == "v4" && l.ty == "I"), "{labels:?}");
    assert!(labels.iter().any(|l| l.owner == "SynthCanvas" && l.method == "keyPressed" && l.label == "v1" && l.param));

    // without --complete the person's file is left exactly as it was
    assert_eq!(std::fs::read_to_string(&map).unwrap(), original);

    // applying twice is deterministic
    let out2 = work.join("out2");
    assert_eq!(arlecchino::cli::main(Some(argv(&["apply-names", jar.to_str().unwrap(), "--names", map.to_str().unwrap(), "-o", out2.to_str().unwrap()]))), 0);
    let again = out2.join(renamed.file_name().unwrap());
    assert_eq!(std::fs::read(&renamed).unwrap(), std::fs::read(&again).unwrap());

    // --complete: a worksheet; the hand-written lines survive byte for byte
    assert_eq!(arlecchino::cli::main(Some(argv(&["apply-names", jar.to_str().unwrap(), "--names", map.to_str().unwrap(), "-o", out2.to_str().unwrap(), "--complete"]))), 0);
    let completed = std::fs::read_to_string(&map).unwrap();
    for line in original.lines().filter(|l| !l.trim().is_empty()) {
        if line.contains("startApp") {
            continue; // gains a `/* locals: … */` for its unnamed catch variable
        }
        assert!(completed.contains(line), "line lost: {line}\n---\n{completed}");
    }
    assert!(completed.contains("v1b"), "the catch variable is listed by its label:\n{completed}");
    let reparsed = mapfile::parse(&map, &completed);
    assert!(reparsed.errors.is_empty(), "{:?}", reparsed.errors);

    // the renamed MIDlet behaves like the original
    if midprun::find_midp_rt(None, None).is_some() {
        let o = diffrun::RunOpts { max_frames: 8, frame_period_ms: 100, boot_wait_s: 2.0, ..Default::default() };
        match difftest::difftest(&jar, &renamed, &work.join("dt"), None, difftest::DEFAULT_SHIFT_TOL, difftest::DEFAULT_JACCARD_MARGIN, false, "midp-rt", false, &o) {
            Ok(v) => assert!(v.passed, "renamed MIDlet behaves differently: {:?}", v.reasons),
            Err(e) => eprintln!("SKIP (behaviour): headless run unavailable: {e}"),
        }
    } else {
        eprintln!("SKIP (behaviour): midp-rt.jar not installed");
    }

    // and decompiles to compilable source that uses the names
    if let Some(vf) = engine_jar("vineflower.jar") {
        let dec = work.join("vf");
        let _ = std::fs::create_dir_all(&dec);
        if common::run(&[common::java(), "-jar".into(), vf.to_string_lossy().into_owned(), renamed.to_string_lossy().into_owned(), dec.to_string_lossy().into_owned()], "vineflower").is_ok() {
            let src = std::fs::read_to_string(dec.join("CheckerCanvas.java")).unwrap_or_default();
            assert!(src.contains("int row") && src.contains("Graphics graphics"), "{src}");
            let stub = arlecchino::midp_stub::stub_jar().expect("stub built by the fixture");
            common::javac_dir(&dec, &work.join("vf-classes"), Some(stub.to_str().unwrap()), "decompiled renamed MIDlet").expect("compiles");
        }
    }
    let _ = std::fs::remove_dir_all(&work);
}

fn engine_jar(name: &str) -> Option<PathBuf> {
    let p = arlecchino::midp_stub::arlecchino_home().join("engines").join(name);
    p.exists().then_some(p)
}

// ── the patcher, on a hand-built world (no JDK) ──────────────────────────────

fn mem(access: u16, name: &str, desc: &str) -> Member {
    Member { access, name: name.into(), desc: desc.into() }
}

fn world_classes() -> BTreeMap<String, ClassInfo> {
    let mut cs = BTreeMap::new();
    cs.insert(
        "a".to_string(),
        ClassInfo {
            access: 0x0001,
            name: "a".into(),
            super_name: Some("java/lang/Object".into()),
            interfaces: vec![],
            fields: vec![mem(1, "b", "I"), mem(1, "c", "I")],
            methods: vec![mem(1, "d", "(II)V"), mem(1, "e", "()V"), mem(1, "<init>", "()V")],
            strings: vec![],
            class_refs: vec![],
        },
    );
    cs
}

#[test]
fn patch_fills_unnamed_tokens_and_keeps_chosen_names() {
    let cs = world_classes();
    let ext = ExternalModel::builtin();
    let w = World::new(&cs, &ext, &[]);
    let text = "class Player /* was a */ {\n    int lives /* was b */;\n    int c /* was c */;\n    void d(int v1, int speed) /* was d */; /* locals: v3, v4=total */\n}\n";
    let path = Path::new("m.map");
    let files = vec![mapfile::parse(path, text)];
    let (entries, errs) = mapfile::entries(&w, &files);
    assert!(errs.is_empty(), "{errs:?}");
    let mut map = names::NameMap { version: 1, jar_sha256: String::new(), entries };
    // what a model would add: names for everything, including the chosen ones
    let model = |kind, name: &str, desc: &str, local: &str, new: &str| names::NameEntry {
        kind,
        owner: "a".into(),
        name: name.into(),
        desc: desc.into(),
        local: local.into(),
        new: new.into(),
        origin: "model".into(),
        status: String::new(),
    };
    use names::Kind::*;
    map.merge_new(vec![
        model(Class, "", "", "", "Hero"),
        model(Field, "b", "I", "", "health"),
        model(Field, "c", "I", "", "coins"),
        model(Method, "d", "(II)V", "", "move"),
        model(Local, "d", "(II)V", "v1", "dx"),
        model(Local, "d", "(II)V", "v2", "velocity"),
        model(Local, "d", "(II)V", "v3", "step"),
        model(Local, "d", "(II)V", "v4", "sum"),
        model(Method, "e", "()V", "", "reset"),
    ]);
    let plan = names::plan(&w, &map);
    let want = mapfile::wanted(&w, &plan, &[]);
    let auto: BTreeSet<String> = ["m a e()V".to_string()].into_iter().collect();
    let out = mapfile::patch(&w, &plan, &files, &want, path, &auto);
    assert_eq!(out.len(), 1);
    let t = &out[0].1;
    assert!(t.contains("class Player /* was a */"), "chosen class name kept:\n{t}");
    assert!(t.contains("int lives /* was b */;"), "chosen field name kept:\n{t}");
    assert!(t.contains("int coins /* was c */;"), "unnamed field filled:\n{t}");
    assert!(t.contains("void move(int dx, int speed) /* was d */; /* locals: v3=step, v4=total */"), "{t}");
    assert!(t.contains("void reset() /* was e */; // auto"), "new declaration inserted and marked:\n{t}");
    let re = mapfile::parse(path, t);
    assert!(re.errors.is_empty(), "{:?}", re.errors);
    // the result reads back to the same names
    let (again, errs) = mapfile::entries(&w, &[re]);
    assert!(errs.is_empty(), "{errs:?}");
    let names: BTreeSet<String> = again.iter().map(|e| e.new.clone()).collect();
    for n in ["Player", "lives", "coins", "move", "dx", "speed", "step", "total", "reset"] {
        assert!(names.contains(n), "{n} missing from {names:?}");
    }
}

#[test]
fn locals_are_offered_only_from_a_labelled_source() {
    let cs = world_classes();
    let ext = ExternalModel::builtin();
    let w = World::new(&cs, &ext, &[]);
    let work = workdir("names-offer");
    std::fs::create_dir_all(&work).unwrap();
    let labels = vec![mapfile::Label { owner: "a".into(), method: "d".into(), desc: "(II)V".into(), label: "v3".into(), ty: "I".into(), param: false }];
    std::fs::write(work.join("a.java"), "class a { void d(int var1, int var2) { int var3 = var1; } }").unwrap();
    let ctx = suggest::Context { plan: None, labels: &labels, named: BTreeSet::new() };
    let jobs = suggest::collect_jobs_with(&w, Some(&work), &ctx);
    assert!(!jobs[0].symbols.iter().any(|s| s.kind == names::Kind::Local), "no labels in that source");
    std::fs::write(work.join("a.java"), "class a { void d(int v1, int v2) { int v3 = v1; } }").unwrap();
    let jobs = suggest::collect_jobs_with(&w, Some(&work), &ctx);
    let loc: Vec<_> = jobs[0].symbols.iter().filter(|s| s.kind == names::Kind::Local).collect();
    assert_eq!(loc.len(), 1);
    assert!(suggest::describe_now(loc[0], &w, None).starts_with("local v3 : int in method d"), "{}", suggest::describe_now(loc[0], &w, None));
    let reply = serde_json::json!({"a": {loc[0].id.clone(): "total"}}).to_string();
    let job_refs: Vec<&suggest::Job> = jobs.iter().collect();
    let parsed = suggest::parse_reply(&reply, &job_refs).unwrap();
    let e = parsed["a"].iter().find(|e| e.kind == names::Kind::Local).unwrap();
    assert_eq!((e.local.as_str(), e.new.as_str(), e.name.as_str()), ("v3", "total", "d"));
    let _ = std::fs::remove_dir_all(&work);
}
