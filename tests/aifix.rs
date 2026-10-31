//! `ai-fix` repair loop, driven by a scripted model — never the network.
//!
//! The fixture is written here, by hand: a two-class program (`demo/Counter` uses
//! `demo/Item`) compiled to a jar, plus three "decompiler variants" of `Counter`
//! that each break in a different, typical way (an undeclared temporary, an
//! illegal cast, a missing return).  No real program and no real decompiler output
//! is involved.  Needs javac (and javap for the semantic-gate cases).

mod common;

use arlecchino::ai_model::{ScriptedModel, Role};
use arlecchino::aifix::{self, Options, Workspace};
use arlecchino::util;
use std::path::{Path, PathBuf};

const ITEM: &str = "package demo;

public class Item {
    private final int w;

    public Item(int w) {
        this.w = w;
    }

    public int weight() {
        return w;
    }
}
";

const COUNTER: &str = "package demo;

public class Counter {
    private int total;

    public int add(Item[] items) {
        for (int i = 0; i < items.length; i++) {
            total += items[i].weight();
        }
        return total;
    }

    public String label() {
        return \"n=\" + total;
    }
}
";

/// What best-of + autofix left in `merged/`: an undeclared temporary.
const MERGED_BROKEN: &str = "package demo;

public class Counter {
    private int total;

    public int add(Item[] var1) {
        for (int var2 = 0; var2 < var1.length; ++var2) {
            this.total += var1[var3].weight();
        }
        return this.total;
    }

    public String label() {
        return \"n=\" + this.total;
    }
}
";

/// Another engine: an illegal cast instead.
const CFR_BROKEN: &str = "package demo;

public class Counter {
    private int total;

    public int add(Item[] items) {
        for (int i = 0; i < items.length; ++i) {
            this.total += (String) items[i].weight();
        }
        return this.total;
    }

    public String label() {
        return \"n=\" + this.total;
    }
}
";

/// A reply that compiles but drops the loop: must be caught by the semantic gate.
const DRIFTING: &str = "package demo;

public class Counter {
    private int total;

    public int add(Item[] items) {
        return 0;
    }

    public String label() {
        return \"n=\" + this.total;
    }
}
";

struct Fixture {
    root: PathBuf,
    ws: Workspace,
}

fn javac8(src: &[PathBuf], dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let mut argv = vec![
        common::javac(),
        "--release".into(),
        "8".into(),
        "-d".into(),
        dst.to_string_lossy().into_owned(),
    ];
    argv.extend(src.iter().map(|p| p.to_string_lossy().into_owned()));
    common::run(&argv, "javac (fixture)").map(|_| ())
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn fixture(tag: &str) -> Result<Fixture, String> {
    if !common::have(&common::javac()) && !common::have("javac") {
        return Err("javac not found".into());
    }
    let root = common::workdir(tag);
    // original program -> jar
    let orig = root.join("orig_src");
    write(&orig.join("demo/Item.java"), ITEM);
    write(&orig.join("demo/Counter.java"), COUNTER);
    javac8(
        &[orig.join("demo/Item.java"), orig.join("demo/Counter.java")],
        &root.join("orig_classes"),
    )?;
    let jar = root.join("demo.jar");
    common::pack_jar(&root.join("orig_classes"), &jar, None);

    // decompile output layout: one tree per engine + merged/
    let out = root.join("out");
    write(&out.join("vineflower/demo/Counter.java"), MERGED_BROKEN);
    write(&out.join("vineflower/demo/Item.java"), ITEM);
    write(&out.join("cfr/demo/Counter.java"), CFR_BROKEN);
    write(&out.join("cfr/demo/Item.java"), ITEM);
    write(&out.join("merged/demo/Counter.java"), MERGED_BROKEN);
    write(&out.join("merged/demo/Item.java"), ITEM);

    let ws = Workspace {
        merged: out.join("merged"),
        jar,
        classpath: None,
        mapping: None,
        engines: vec![
            ("vineflower".into(), out.join("vineflower")),
            ("cfr".into(), out.join("cfr")),
        ],
        work: out.join("ai_fix"),
    };
    Ok(Fixture { root, ws })
}

fn fenced(code: &str) -> String {
    format!("Here is the corrected class.\n\n```java\n{code}```\n")
}

fn merged_text(f: &Fixture) -> String {
    util::read_text_lossy(&f.ws.merged.join("demo/Counter.java")).unwrap()
}

#[test]
fn fixing_reply_is_accepted_and_substituted() {
    let f = match fixture("aifix-accept") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let model = ScriptedModel::new([fenced(COUNTER)]);
    let s = aifix::run(&model, &f.ws, None, &Options::default());

    assert_eq!(s.failing_before, 1);
    assert_eq!(s.failing_after, 0);
    assert_eq!(s.accepted, 1);
    assert_eq!(s.attempts_total, 1);
    assert!(s.isolated_errors_before > 0);
    assert_eq!(s.isolated_errors_after, 0);
    assert!(s.tree_errors_before > 0);
    assert_eq!(s.tree_errors_after, 0);
    assert_eq!(merged_text(&f), COUNTER);
    // the pre-fix text is kept for review / manual revert
    let kept = util::read_text_lossy(&f.ws.work.join("before/demo/Counter.java")).unwrap();
    assert_eq!(kept, MERGED_BROKEN);
    // the gate ran when javap exists
    let gate = s.classes[0].attempts[0].gate.clone().unwrap();
    if common::have("javap") {
        assert!(matches!(gate, aifix::Gate::Passed { .. }), "{gate:?}");
    }

    // the prompt carried the error, the other engine's divergent region and the
    // referenced class's signatures
    let req = &model.requests()[0];
    assert_eq!(req.messages.len(), 1);
    let p = &req.messages[0].content;
    assert!(p.contains("var3"), "javac error missing from the prompt");
    assert!(p.contains("### cfr"), "other engine's variant missing");
    assert!(p.contains("(String) items[i].weight()"), "divergent hunk missing");
    if common::have("javap") {
        assert!(p.contains("public int weight()"), "referenced signature missing");
        assert!(p.contains("Original bytecode of the methods"), "bytecode of failing method missing");
    }

    // journal + report
    assert!(f.ws.work.join("summary.json").exists());
    assert!(f.ws.work.join("journal.jsonl").exists());
    let md = std::fs::read_to_string(f.ws.work.join("report.md")).unwrap();
    assert!(md.contains("failing classes: 1 -> 0"), "{md}");
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn breaking_then_fixing_reply_feeds_the_new_error_back() {
    let f = match fixture("aifix-retry") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let still_broken = MERGED_BROKEN.replace("var3", "var4");
    let model = ScriptedModel::new([fenced(&still_broken), fenced(COUNTER)]);
    let s = aifix::run(&model, &f.ws, None, &Options::default());

    assert_eq!(s.accepted, 1);
    let c = &s.classes[0];
    assert_eq!(c.attempts.len(), 2);
    assert_eq!(c.attempts[0].outcome, "compile_errors");
    assert_eq!(c.attempts[1].outcome, "accepted");
    // second request = first prompt + the model's bad answer + the NEW error
    let reqs = model.requests();
    assert_eq!(reqs.len(), 2);
    let m = &reqs[1].messages;
    assert_eq!(m.len(), 3);
    assert_eq!(m[1].role, Role::Assistant);
    assert_eq!(m[2].role, Role::User);
    assert!(m[2].content.contains("var4"), "new error not fed back: {}", m[2].content);
    assert_eq!(merged_text(&f), COUNTER);
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn endlessly_wrong_model_hits_the_attempt_limit_and_changes_nothing() {
    let f = match fixture("aifix-limit") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    // compiles never; one reply has no code at all; one renames the class
    let model = ScriptedModel::new([
        fenced(&MERGED_BROKEN.replace("var3", "var9")),
        "I am not sure what you mean.".to_string(),
        fenced(&COUNTER.replace("class Counter", "class Tally")),
        fenced(&MERGED_BROKEN.replace("var3", "var8")),
    ]);
    let opts = Options { max_attempts: 4, ..Options::default() };
    let s = aifix::run(&model, &f.ws, None, &opts);

    assert_eq!(s.accepted, 0);
    assert_eq!(s.rejected, 1);
    assert_eq!(s.failing_after, 1);
    assert_eq!(s.attempts_total, 4);
    let outcomes: Vec<&str> = s.classes[0].attempts.iter().map(|a| a.outcome.as_str()).collect();
    assert_eq!(outcomes, vec!["compile_errors", "no_code", "not_same_unit", "compile_errors"]);
    assert_eq!(s.isolated_errors_after, s.isolated_errors_before);
    assert_eq!(s.tree_errors_after, s.tree_errors_before);
    assert_eq!(merged_text(&f), MERGED_BROKEN, "a rejected class must stay untouched");
    // every candidate is kept for inspection
    assert!(f.ws.work.join("attempts/demo__Counter.java.attempt1.java").exists());
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn compiling_but_drifting_reply_is_rejected_by_the_semantic_gate() {
    if !common::have("javap") {
        return common::skip("javap not found");
    }
    let f = match fixture("aifix-drift") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let model = ScriptedModel::new([fenced(DRIFTING), fenced(COUNTER)]);
    let s = aifix::run(&model, &f.ws, None, &Options::default());

    let c = &s.classes[0];
    assert_eq!(c.attempts[0].outcome, "drift");
    assert!(matches!(c.attempts[0].gate, Some(aifix::Gate::Drift { .. })));
    assert_eq!(c.attempts[1].outcome, "accepted");
    let fb = &model.requests()[1].messages[2].content;
    assert!(fb.contains("add("), "drifting method not named: {fb}");
    assert!(fb.contains("weight"), "lost call not named: {fb}");
    assert_eq!(merged_text(&f), COUNTER);

    // with the gate switched off the drifting reply would have been taken
    let g = match fixture("aifix-nogate") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let model = ScriptedModel::new([fenced(DRIFTING)]);
    let opts = Options { semantic_gate: false, ..Options::default() };
    let s = aifix::run(&model, &g.ws, None, &opts);
    assert_eq!(s.accepted, 1);
    assert_eq!(merged_text(&g), DRIFTING);
    let _ = std::fs::remove_dir_all(&f.root);
    let _ = std::fs::remove_dir_all(&g.root);
}

#[test]
fn dry_run_writes_prompts_and_calls_nobody() {
    let f = match fixture("aifix-dry") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let model = ScriptedModel::new(Vec::<String>::new());
    let opts = Options { dry_run: true, ..Options::default() };
    let s = aifix::run(&model, &f.ws, None, &opts);
    assert!(model.requests().is_empty());
    assert_eq!(s.classes[0].status, "dry_run");
    assert_eq!(merged_text(&f), MERGED_BROKEN);
    let p = f.ws.work.join("prompts/demo__Counter.java.prompt.md");
    assert!(std::fs::read_to_string(p).unwrap().contains("var3"));
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn model_error_stops_the_class_without_retrying() {
    let f = match fixture("aifix-err") {
        Ok(f) => f,
        Err(e) => return common::skip(&e),
    };
    let model = ScriptedModel::with_results([Err("401 invalid x-api-key".to_string())]);
    let s = aifix::run(&model, &f.ws, None, &Options::default());
    assert_eq!(s.attempts_total, 1);
    assert_eq!(s.classes[0].attempts[0].outcome, "model_error");
    assert_eq!(merged_text(&f), MERGED_BROKEN);
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn failing_set_comes_from_the_merge_log() {
    let report = serde_json::json!({
        "merge_log": [
            "isolated per-class scoring; sum of per-class errors=3",
            "  a/B.java: <- cfr (isolated errors=3)",
            "  a/C.java: <- vineflower (clean)",
            "  d.java: <- jadx+autofix(duplicate_imports) (isolated errors=1)",
        ]
    });
    assert_eq!(
        aifix::failing_from_report(&report).unwrap(),
        vec!["a/B.java".to_string(), "d.java".to_string()]
    );
}
