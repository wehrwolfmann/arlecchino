//! `--flatten-packages`: the cross-package fix.
//!
//! Ported from the reference suite's `tests/test_flatten.py`.
//!
//! The fixture mirrors the irreducible Allods situation — a NAMED-package class
//! (`Container.App`) referencing a DEFAULT-package type (`Lib`), which the Java language
//! has no syntax for (JLS §6.5/§7.4.3), so no decompiler can emit compilable source for
//! it.  The three assertions are: without the flag the error stands, with the flag every
//! type moves to the default package (jar path AND the manifest `MIDlet-N` entry
//! rewritten) and the source compiles with 0 errors, and the round-trip set is intact.
//!
//! javac + ASM only — no decompiler engines, no network.

mod common;

use arlecchino::rename;

#[test]
fn fixture_is_genuinely_cross_package() {
    let w = common::workdir("flat-fixture");
    let jar = match common::build_flatten_jar(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let ents = common::jar_entries(&jar);
    assert!(ents.contains("Container/App.class"), "{ents:?}");
    assert!(ents.contains("Lib.class"), "{ents:?}");
    let mf = common::jar_manifest(&jar);
    assert!(mf.contains("MIDlet-1: Demo,/icon.png,Container.App"), "{mf}");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn without_flatten_a_named_class_cannot_name_a_default_package_type() {
    let w = common::workdir("flat-off");
    let jar = match common::build_flatten_jar(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let (jar2, mapping) = rename::maybe_rename(&jar, &w.join("_rn_off"), None, "on", None, false);
    assert_ne!(jar2, jar, "the renamer produced no new jar");
    let ents = common::jar_entries(&jar2);
    assert!(ents.contains("Container/App.class"), "without the flag App must stay named-package");
    assert!(ents.contains("Lib.class"));
    let mtext = mapping.map(|m| std::fs::read_to_string(m).unwrap()).unwrap_or_default();
    assert!(!mtext.contains("(flatten-package)"), "the flatten note must be absent:\n{mtext}");

    // A `Container.App` source naming the default-package `Lib` cannot compile: there
    // is no syntax for it.  This is the error --flatten-packages exists to remove.
    let bad = "package Container;\n\
               public class App {\n\
               \x20   Lib lib = new Lib();\n\
               \x20   int run() { return this.lib.get() + 1; }\n\
               }\n";
    let errs = common::compile_source_isolated(bad, "Container/App.java", &jar2, &w.join("off"));
    assert!(errs > 0, "a named-package class naming a default-package type must NOT compile");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn with_flatten_the_cross_package_error_is_gone() {
    let w = common::workdir("flat-on");
    let jar = match common::build_flatten_jar(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let (jar2, mapping) = rename::maybe_rename(&jar, &w.join("_rn_on"), None, "on", None, true);
    assert_ne!(jar2, jar);
    let ents = common::jar_entries(&jar2);
    // the jar path follows the new internal name; the named package is gone
    assert!(ents.contains("App.class"), "{ents:?}");
    assert!(!ents.contains("Container/App.class"), "App must be flattened to the default package");
    assert!(ents.contains("Lib.class"));
    // round-trip set intact: both classes still present, nothing lost or duplicated
    assert_eq!(ents.iter().filter(|e| e.ends_with(".class")).count(), 2, "{ents:?}");

    let mtext = std::fs::read_to_string(mapping.expect("mapping")).unwrap();
    assert!(mtext.contains("Container/App -> App  (flatten-package)"), "{mtext}");

    // the manifest entry class is rewritten from the dotted FQN to the simple name
    let mf = common::jar_manifest(&jar2);
    assert!(!mf.contains("Container.App"), "manifest entry class not flattened:\n{mf}");
    assert!(mf.contains("MIDlet-1: Demo,/icon.png,App"), "{mf}");

    // and now a default-package App naming its sibling Lib compiles with 0 errors
    let good = "public class App {\n\
                \x20   Lib lib = new Lib();\n\
                \x20   int run() { return this.lib.get() + 1; }\n\
                }\n";
    let errs = common::compile_source_isolated(good, "App.java", &jar2, &w.join("on"));
    assert_eq!(errs, 0, "a default-package class naming a sibling default-package type must compile");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn flatten_naming_is_deterministic_and_collision_safe() {
    let w = common::workdir("flat-det");
    let jar = match common::build_flatten_jar(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    let (_j1, m1) = rename::maybe_rename(&jar, &w.join("a"), None, "on", None, true);
    let (_j2, m2) = rename::maybe_rename(&jar, &w.join("b"), None, "on", None, true);
    let notes = |m: Option<std::path::PathBuf>| -> Vec<String> {
        std::fs::read_to_string(m.expect("mapping"))
            .unwrap()
            .lines()
            .filter(|l| l.contains("(flatten-package)"))
            .map(|l| l.to_string())
            .collect()
    };
    let t1 = notes(m1);
    let t2 = notes(m2);
    assert_eq!(t1, t2, "flatten mapping must be deterministic");
    // this fixture has no simple-name clash, so no disambiguation suffix may appear
    assert_eq!(t1, vec!["Container/App -> App  (flatten-package)".to_string()], "{t1:?}");
    let _ = std::fs::remove_dir_all(&w);
}
