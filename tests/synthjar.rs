//! Synthetic-obfuscated-jar regression net for the bytecode member renamer.
//!
//! Ported from the reference suite's `tests/test_synthjar.py` + `test_renamer_perf.py`.
//!
//! The fixture generator (`tests/common/mod.rs`) compiles five small classes with javac
//! and then injects, with ASM, the exact source-illegal collisions ProGuard-style
//! obfuscation produces.  Nothing is vendored: the jar is built on every run.
//!
//! | case | injected | expected renamer behaviour |
//! |---|---|---|
//! | (a) duplicate field, different descriptor | `Engine.a:I` next to `Engine.a:String` | the `int a` is renamed, the `String a` kept |
//! | (b) return-type-only overload | `Engine.flag(I)I` next to `flag(I)Z` | the pair is disambiguated |
//! | (c) type-shadow | field `Engine.State:I` shadowing in-jar type `State` | renamed, tagged `(type-shadow)` |
//! | (d) const-interface shadow | `Consts.b` shadowed by `Holder.b` | the constant is renamed, tagged `(const-iface-shadow)` |
//! | (e) MIDP callback override + twin | `Screen` re-parented to `Canvas`, `keyPressed(I)I` next to `keyPressed(I)V` | `keyPressed(I)V` stays LOCKED; the twin is renamed |
//!
//! Needs javac + the ASM jars only — no decompiler engines, no network — except for
//! `full_pipeline_has_zero_isolated_errors`, which additionally needs the engines.

mod common;

use arlecchino::{config, core, rename};
use std::collections::BTreeMap;

/// `owner.oldName desc -> newName  (tag)` -> {"owner.old desc": "newName"}.
fn parse_mapping(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once("->") else { continue };
        let new = right.trim().split_whitespace().next().unwrap_or("").to_string();
        out.insert(left.trim().to_string(), new);
    }
    out
}

#[test]
fn synth_collisions_are_detected() {
    let w = common::workdir("synth-detect");
    let synth = match common::build_synth_jar(&w) {
        Ok(s) => s,
        Err(e) => return common::skip(&e),
    };
    // the cheap pure pre-check must see the injected source-illegal collisions
    assert!(rename::has_member_collisions(&synth.jar), "pre-check missed the injected collisions");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn renamer_resolves_every_injected_collision() {
    let w = common::workdir("synth-rename");
    if !rename::available(None) {
        return common::skip("ASM helper not compiled (run `arlecchino setup`)");
    }
    let synth = match common::build_synth_jar(&w) {
        Ok(s) => s,
        Err(e) => return common::skip(&e),
    };
    let cp = synth.midp_stub.to_string_lossy().into_owned();
    let (jar2, mapping) =
        rename::maybe_rename(&synth.jar, &w.join("_rename"), None, "on", Some(&cp), false);
    let mapping = mapping.expect("renamer produced no mapping");
    assert_ne!(jar2, synth.jar, "no renamed jar was produced");

    let text = std::fs::read_to_string(&mapping).unwrap();
    let m = parse_mapping(&text);

    // (a) duplicate field name, different descriptor: the int `a` is renamed, the
    //     original String `a` keeps its name (so it is absent from the map).
    assert!(m.contains_key("Engine.a I"), "dup-field rename missing:\n{text}");
    assert!(!m.contains_key("Engine.a Ljava/lang/String;"), "String a must keep its name:\n{text}");

    // (b) return-type-only overload: the pair must be disambiguated.
    assert!(
        m.keys().any(|k| k.starts_with("Engine.flag (")),
        "return-type overload not disambiguated:\n{text}"
    );

    // (c) type-shadow: the field whose simple name is an in-jar type, tagged.
    assert!(m.contains_key("Engine.State I"), "type-shadow rename missing:\n{text}");
    assert!(text.contains("(type-shadow)"), "type-shadow tag missing:\n{text}");

    // (d) const-interface shadow: the colliding constant is renamed and tagged.
    assert!(m.contains_key("Consts.b Z"), "const-iface-shadow rename missing:\n{text}");
    assert!(text.contains("(const-iface-shadow)"), "const-iface-shadow tag missing:\n{text}");

    // (e) MIDP callback: the genuine keyPressed(I)V is LOCKED, its twin renamed.
    assert!(
        !m.contains_key("Screen.keyPressed (I)V"),
        "MIDP callback keyPressed(I)V must stay locked:\n{text}"
    );
    assert!(
        m.contains_key("Screen.keyPressed (I)I"),
        "colliding keyPressed twin must be renamed:\n{text}"
    );

    // after renaming the jar must hold no source-illegal collisions at all
    assert!(!rename::has_member_collisions(&jar2), "collisions survived the rename");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn mapping_matches_the_committed_golden_byte_for_byte() {
    let w = common::workdir("synth-golden");
    if !rename::available(None) {
        return common::skip("ASM helper not compiled (run `arlecchino setup`)");
    }
    let synth = match common::build_synth_jar(&w) {
        Ok(s) => s,
        Err(e) => return common::skip(&e),
    };
    let mapping = w.join("synth.map.txt");
    let cp = synth.midp_stub.to_string_lossy().into_owned();
    let res =
        rename::run_renamer(&synth.jar, &w.join("synth.renamed.jar"), &mapping, None, Some(&cp), false);
    assert!(res.is_some() && mapping.exists(), "renamer did not produce a mapping");

    let produced = std::fs::read(&mapping).unwrap();
    let golden_path = common::golden_dir().join("synth_mapping.txt");
    let golden = std::fs::read(&golden_path).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&produced),
        String::from_utf8_lossy(&golden),
        "synthjar mapping.txt diverged from the golden ({})",
        golden_path.display()
    );
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn mapping_is_deterministic_across_runs() {
    let w = common::workdir("synth-determinism");
    if !rename::available(None) {
        return common::skip("ASM helper not compiled (run `arlecchino setup`)");
    }
    let synth = match common::build_synth_jar(&w) {
        Ok(s) => s,
        Err(e) => return common::skip(&e),
    };
    let cp = synth.midp_stub.to_string_lossy().into_owned();
    let m1 = w.join("m1.txt");
    let m2 = w.join("m2.txt");
    rename::run_renamer(&synth.jar, &w.join("r1.jar"), &m1, None, Some(&cp), false);
    rename::run_renamer(&synth.jar, &w.join("r2.jar"), &m2, None, Some(&cp), false);
    assert_eq!(
        std::fs::read(&m1).unwrap(),
        std::fs::read(&m2).unwrap(),
        "renamer mapping is not deterministic"
    );
    let _ = std::fs::remove_dir_all(&w);
}

/// rename -> decompile (every configured engine) -> merge_isolated must yield 0
/// isolated errors on the synthetic jar.  Skips where the engines are not installed.
#[test]
fn full_pipeline_has_zero_isolated_errors() {
    let w = common::workdir("synth-pipeline");
    if !rename::available(None) {
        return common::skip("ASM helper not compiled (run `arlecchino setup`)");
    }
    let (engines, _src, root) = match config::load_engines(None) {
        Ok(t) => t,
        Err(e) => return common::skip(&format!("no engine config: {e}")),
    };
    let usable: Vec<_> =
        engines.iter().filter(|e| e.enabled && e.resolve_path(&root).exists()).cloned().collect();
    if usable.is_empty() {
        return common::skip("no decompiler engines available on this machine");
    }
    let synth = match common::build_synth_jar(&w) {
        Ok(s) => s,
        Err(e) => return common::skip(&e),
    };

    let out = w.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let cp = synth.midp_stub.to_string_lossy().into_owned();
    let (jar2, _mapping) =
        rename::maybe_rename(&synth.jar, &out.join("_rename"), Some(&root), "on", Some(&cp), false);
    let data = core::decompile_all(&usable, &jar2, &out, &root, Some(&cp), 4);
    if data.is_empty() {
        let _ = std::fs::remove_dir_all(&w);
        return common::skip("no engine produced output");
    }
    let (_merge, total, _log, per) = core::merge_isolated(&data, &out, &jar2, Some(&cp), true);
    assert_eq!(total, 0, "expected 0 isolated errors, got {total}; per-class={per:?}");
    assert!(per.values().all(|v| *v == 0), "per-class errors remain: {per:?}");
    let _ = std::fs::remove_dir_all(&w);
}

/// Sanity: the golden the byte-for-byte test compares against is the real thing, not
/// an empty file that would make that assertion vacuous.
#[test]
fn golden_mapping_covers_all_five_cases() {
    let text = std::fs::read_to_string(common::golden_dir().join("synth_mapping.txt")).unwrap();
    for want in [
        "Consts.b Z -> ",
        "(const-iface-shadow)",
        "Engine.State I -> ",
        "(type-shadow)",
        "Engine.a I -> ",
        "Engine.flag (I)Z -> ",
        "Screen.keyPressed (I)I -> ",
    ] {
        assert!(text.contains(want), "golden is missing {want:?}:\n{text}");
    }
    assert!(!text.contains("Screen.keyPressed (I)V"), "golden must not rename the locked callback");
    assert_eq!(text.lines().filter(|l| l.contains("->")).count(), 5);
}
