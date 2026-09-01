//! The generated clean-room MIDP/CLDC stub makes callback locking PRECISE.
//!
//! Ported from the reference suite's `tests/test_midp_stub.py`.  (Its first test — the
//! stub builds and carries the probed callback descriptors — is already a unit test of
//! `midp_stub`; what is added here is the renamer-facing property.)
//!
//! A synthetic in-jar class overrides a callback that is NOT in the helper's
//! `MIDP_CALLBACKS` denylist (`ItemStateListener.itemStateChanged`) and carries a
//! colliding return-type-only twin.  With the generated stub on the resolver classpath
//! the reflection path recognises the genuine override and LOCKS it, renaming only the
//! twin — the well-known `keyPressed(I)V` override in the same jar stays safe too.
//!
//! javac + java + ASM + the compiled renamer helper; skips cleanly without them.
//!
//! NOTE ON THE OFFLINE HALF.  The reference test had a second half that pointed
//! `ARLECCHINO_HOME` at an empty directory to prove the denylist still covers the
//! well-known minimum with no stub anywhere.  That half does not carry over: here
//! `rename::resolver_classpath` goes through `midp_stub::ensure_stub`, which GENERATES
//! the stub on demand, so a machine with javac always gets the precise path and the
//! denylist fallback is only reachable with no javac at all — where this whole file
//! skips anyway.

mod common;

use arlecchino::{rename, util};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Left-hand sides of the `owner.name desc -> new` lines: the members that were renamed.
fn renamed_keys(mapping: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(mapping)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once("->").map(|(a, _)| a.trim().to_string()))
        .collect()
}

/// Compile `Listener` + `Widget` against a throwaway compile-time MIDP ABI, then inject
/// the colliding twins with ASM.
fn build_twin_jar(w: &Path) -> Result<PathBuf, String> {
    let asm = common::need_java_and_asm()?;
    let asm_cp = asm.iter().map(|j| s(j)).collect::<Vec<_>>().join(util::PATHSEP);
    let fix = common::fixtures_dir().join("midpstub");

    // 1) a minimal compile-time ABI so the fixture sources compile.  This is only to
    //    keep javac happy — it is NOT the stub under test.
    let cstub_cls = w.join("cstub_cls");
    common::javac_dir(&fix.join("cstub"), &cstub_cls, None, "javac (compile ABI)")?;

    // 2) the in-jar sources against that ABI
    let app_cls = w.join("app_cls");
    common::javac_dir(&fix.join("src"), &app_cls, Some(&s(&cstub_cls)), "javac (fixture app)")?;

    // 3) compile + run the ASM twin injector
    let obf_dir = w.join("obf");
    std::fs::create_dir_all(&obf_dir).map_err(|e| e.to_string())?;
    common::run(
        &[
            common::javac(),
            "-cp".into(),
            asm_cp.clone(),
            "-d".into(),
            s(&obf_dir),
            s(&fix.join("TwinObf.java")),
        ],
        "javac (TwinObf)",
    )?;
    let out_jar = w.join("twin.jar");
    common::run(
        &[
            common::java(),
            "-cp".into(),
            format!("{asm_cp}{}{}", util::PATHSEP, s(&obf_dir)),
            "TwinObf".into(),
            s(&app_cls),
            s(&out_jar),
        ],
        "TwinObf",
    )?;
    if !out_jar.exists() {
        return Err("TwinObf produced no jar".into());
    }
    Ok(out_jar)
}

#[test]
fn a_non_denylisted_callback_is_locked_when_the_stub_resolves_it() {
    let w = common::workdir("stub-lock");
    if !rename::available(None) {
        return common::skip("ASM helper not compiled (run `arlecchino setup`)");
    }
    let jar = match build_twin_jar(&w) {
        Ok(j) => j,
        Err(e) => return common::skip(&e),
    };
    assert!(rename::has_member_collisions(&jar), "the fixture must carry the injected twins");

    // Build the stub into an isolated destination: a test never writes to the user's
    // home.  It is handed to the renamer as the framework classpath.
    let stub_jar = w.join("home").join(arlecchino::midp_stub::STUB_JAR_NAME);
    let status = arlecchino::midp_stub::build_stub(Some(&stub_jar), Some(&w.join("stubwork")));
    if status.starts_with("skip") || !stub_jar.exists() {
        return common::skip(&format!("MIDP stub unavailable: {status}"));
    }

    let (jar2, mapping) =
        rename::maybe_rename(&jar, &w.join("_with"), None, "on", Some(&s(&stub_jar)), false);
    let keys = renamed_keys(&mapping.expect("mapping"));

    // The genuine, NON-denylisted override is locked; only its twin is renamed.
    assert!(
        !keys.contains("Listener.itemStateChanged (Ljavax/microedition/lcdui/Item;)V"),
        "a non-denylist callback must be locked when the stub resolves it:\n{keys:?}"
    );
    assert!(
        keys.contains("Listener.itemStateChanged (Ljavax/microedition/lcdui/Item;)I"),
        "the colliding twin must be renamed:\n{keys:?}"
    );
    // The well-known callback is locked too, by the same precise path.
    assert!(!keys.contains("Widget.keyPressed (I)V"), "{keys:?}");
    assert!(keys.contains("Widget.keyPressed (I)I"), "{keys:?}");
    // And the jar is source-legal afterwards.
    assert!(!rename::has_member_collisions(&jar2), "collisions survived the rename");
    let _ = std::fs::remove_dir_all(&w);
}
