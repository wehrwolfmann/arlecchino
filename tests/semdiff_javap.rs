//! Semantic round-trip check (`--semdiff` / `--reject-semantic-drift`).
//!
//! Ported from the reference suite's `tests/test_semdiff.py`.  javac/javap-dependent but
//! engine-INDEPENDENT, so it runs on a bare JDK.
//!
//! Three properties:
//!   * a semantics-CHANGING autofix is FLAGGED — both real cases: an interface turned
//!     into a class flips a call site from `invokeinterface` to `invokevirtual`, and a
//!     widening to `Object` that changes a primitive array's element type changes the
//!     `newarray` operand;
//!   * a semantics-PRESERVING rename, reconciled through `mapping.txt`, is NOT flagged;
//!   * `variant_drifts` — the core of `--reject-semantic-drift` — flags the drifting
//!     variant and ignores a cosmetic declaration split.
//!
//! The golden parity fixture already pins `parse_javap` / `canon_ref` / `normsig` on
//! captured text; what is pinned HERE is the end-to-end behaviour against a live javap.

mod common;

use arlecchino::semdiff;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn prerequisites() -> Result<(), String> {
    if !common::have(&common::javac()) {
        return Err("javac not on PATH".into());
    }
    if !semdiff::javap_available() {
        return Err("javap not on PATH".into());
    }
    Ok(())
}

/// Compile the given `(filename, source)` pairs into `out`; panics on a broken fixture
/// (a fixture that does not compile is a bug in the test, not a skip).
fn javac(w: &Path, tag: &str, files: &[(&str, &str)], out: &Path) {
    let src = w.join(tag);
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(out).unwrap();
    let mut argv = vec![
        common::javac(),
        "-d".into(),
        out.to_string_lossy().into_owned(),
        "--release".into(),
        "8".into(),
        "-encoding".into(),
        "utf-8".into(),
        "-nowarn".into(),
        "-proc:none".into(),
    ];
    for (name, text) in files {
        let p = src.join(name);
        std::fs::write(&p, text).unwrap();
        argv.push(p.to_string_lossy().into_owned());
    }
    let r = arlecchino::util::run_capture(&argv, Some(Duration::from_secs(300)));
    assert!(r.success(), "fixture did not compile:\n{}", r.stderr);
}

fn cp(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ────────────────── a semantics-changing fix must be flagged ────────────────

#[test]
fn interface_turned_into_a_class_is_flagged() {
    let w = common::workdir("sd-iface");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let orig = w.join("orig");
    let drift = w.join("drift");
    javac(
        &w,
        "s_o",
        &[
            ("K.java", "interface K { int m(); }"),
            ("U.java", "class U { int call(K k) { return k.m(); } }"),
        ],
        &orig,
    );
    javac(
        &w,
        "s_d",
        &[
            ("K.java", "class K { int m(){ return 1; } }"),
            ("U.java", "class U { int call(K k) { return k.m(); } }"),
        ],
        &drift,
    );
    // interface -> class flips invokeinterface to invokevirtual at the call site
    let res = semdiff::compare_class(&cp(&orig), &cp(&drift), "U", &BTreeMap::new())
        .expect("both sides must disassemble");
    assert!(
        res.flagged.iter().any(|f| f.method.contains("call")),
        "the call site was not flagged: {:?}",
        res.flagged
    );
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn an_array_element_type_change_is_flagged() {
    let w = common::workdir("sd-array");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let orig = w.join("orig");
    let drift = w.join("drift");
    javac(
        &w,
        "w_o",
        &[("W.java", "class W { int f(){ int[] a = new int[]{1,2,3}; return a.length; } }")],
        &orig,
    );
    javac(
        &w,
        "w_d",
        &[(
            "W.java",
            "class W { int f(){ Object a = new char[]{'x'}; return ((char[])a).length; } }",
        )],
        &drift,
    );
    let res = semdiff::compare_class(&cp(&orig), &cp(&drift), "W", &BTreeMap::new())
        .expect("both sides must disassemble");
    assert!(!res.flagged.is_empty(), "expected newarray int->char to be flagged");
    let blob = format!("{:?}", res.flagged[0]);
    assert!(blob.contains("NewArray"), "{blob}");
    assert!(blob.contains("int") && blob.contains("char"), "{blob}");
    let _ = std::fs::remove_dir_all(&w);
}

// ───────────── a semantics-preserving rename must NOT be flagged ────────────

#[test]
fn a_rename_reconciled_through_the_mapping_is_not_flagged() {
    let w = common::workdir("sd-rename");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let orig = w.join("orig");
    let ren = w.join("ren");
    javac(
        &w,
        "r_o",
        &[("R.java", "class R { int oldName(int x){ return x+1; } int call(){ return oldName(5); } }")],
        &orig,
    );
    javac(
        &w,
        "r_r",
        &[("R.java", "class R { int a_123(int x){ return x+1; } int call(){ return a_123(5); } }")],
        &ren,
    );
    let mapping = w.join("map.txt");
    std::fs::write(&mapping, "R.oldName (I)I -> a_123\n").unwrap();

    // WITHOUT the mapping the rename shows up — proof the fixture really differs
    let raw = semdiff::compare_class(&cp(&orig), &cp(&ren), "R", &BTreeMap::new()).unwrap();
    assert!(!raw.flagged.is_empty(), "the fixture must differ before reconciliation");

    // WITH the mapping it is reconciled away
    let rev = semdiff::load_reverse_map(Some(&mapping));
    assert_eq!(rev.get("a_123").map(String::as_str), Some("oldName"));
    let rec = semdiff::compare_class(&cp(&orig), &cp(&ren), "R", &rev).unwrap();
    assert!(rec.flagged.is_empty(), "a pure rename must not be flagged: {:?}", rec.flagged);
    let _ = std::fs::remove_dir_all(&w);
}

// ──────────────── variant_drifts: the drift-rejection primitive ─────────────

#[test]
fn variant_drifts_flags_the_drift_and_ignores_the_cosmetic_change() {
    use std::io::Write;
    let w = common::workdir("sd-variant");
    if let Err(e) = prerequisites() {
        return common::skip(&e);
    }
    let cls = w.join("cls");
    javac(
        &w,
        "src",
        &[(
            "W.java",
            "public class W { public int f(){ int[] a = new int[]{1,2,3}; return a.length; } }",
        )],
        &cls,
    );
    let jar: PathBuf = w.join("orig.jar");
    {
        let f = std::fs::File::create(&jar).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        z.start_file("W.class", o).unwrap();
        z.write_all(&std::fs::read(cls.join("W.class")).unwrap()).unwrap();
        z.finish().unwrap();
    }
    let api = cp(&jar);
    let before = "public class W { public int f(){ int[] a = new int[]{1,2,3}; return a.length; } }";
    let drifting =
        "public class W { public int f(){ Object a = new char[]{'x'}; return ((char[])a).length; } }";
    let cosmetic = "public class W { public int f(){ int[] a; a = new int[]{1,2,3}; return a.length; } }";

    assert!(
        semdiff::variant_drifts(&jar, before, drifting, "W.java", &api, &w.join("w1"), None),
        "a variant that changes the array element type must be rejected"
    );
    assert!(
        !semdiff::variant_drifts(&jar, before, cosmetic, "W.java", &api, &w.join("w2"), None),
        "a cosmetic declaration split must NOT be rejected"
    );
    let _ = std::fs::remove_dir_all(&w);
}
