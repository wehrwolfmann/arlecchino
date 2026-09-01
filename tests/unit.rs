//! Pure unit tests — no Java, no engines, no network.
//!
//! Ported from the reference suite's `tests/test_unit.py`, plus the pieces of
//! `test_difftest.py`, `test_diffrun.py`, `test_semdiff.py` and `test_midprun.py` that
//! the golden parity fixture does not already pin.
//!
//! The classes fed to the collision pre-check are HAND-ASSEMBLED here (constant pool,
//! field and method tables written by hand), so the detector is exercised on the exact
//! source-illegal shapes without needing javac.

mod common;

use arlecchino::{config, difftest, diffrun, midprun, rename, semdiff};
use std::path::Path;

// ───────────────────────────── config loading ───────────────────────────────

#[test]
fn packaged_default_config_parses() {
    // Load the PACKAGED default EXPLICITLY.  Which config `find_config` resolves to
    // depends on the machine (a user ~/.arlecchino/engines.toml wins), so the packaged
    // one is written out and named directly.
    let w = common::workdir("cfg");
    let path = w.join("engines.default.toml");
    std::fs::write(&path, config::DEFAULT_CONFIG_TOML).unwrap();
    let (engines, src, _root) = config::load_engines(Some(&path.to_string_lossy())).unwrap();
    let names: Vec<&str> = engines.iter().map(|e| e.name.as_str()).collect();
    for want in ["vineflower", "cfr", "procyon", "jadx"] {
        assert!(names.contains(&want), "{want} missing from the packaged default: {names:?}");
    }
    assert!(matches!(src, config::ConfigSource::File(_)));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn engine_argv_substitutes_jar_and_out() {
    let e = config::Engine {
        name: "x".into(),
        type_: "names".into(),
        kind: "jar".into(),
        path: "x.jar".into(),
        args: vec!["{jar}".into(), "{out}".into()],
        sources_subdir: String::new(),
        normalize: "none".into(),
        enabled: true,
    };
    let root = Path::new("/root");
    let argv = e.argv(Path::new("/root/in.jar"), Path::new("/root/o"), root).unwrap();
    assert_eq!(argv[1], "-jar");
    assert!(argv.contains(&"/root/in.jar".to_string()), "{argv:?}");
    assert!(argv.contains(&"/root/o".to_string()), "{argv:?}");
}

// ─────────────── collision pre-check on synthetic .class bytes ──────────────

/// Hand-assemble a minimal `.class` carrying the given `(name, descriptor)` members.
fn make_class(this_name: &str, fields: &[(&str, &str)], methods: &[(&str, &str)]) -> Vec<u8> {
    let mut cp: Vec<Vec<u8>> = Vec::new(); // 1-indexed entries, appended in order

    fn utf8(cp: &mut Vec<Vec<u8>>, s: &str) -> u16 {
        let b = s.as_bytes();
        let mut e = vec![0x01u8];
        e.extend_from_slice(&(b.len() as u16).to_be_bytes());
        e.extend_from_slice(b);
        cp.push(e);
        cp.len() as u16
    }
    fn klass(cp: &mut Vec<Vec<u8>>, name: &str) -> u16 {
        let ni = utf8(cp, name);
        let mut e = vec![0x07u8];
        e.extend_from_slice(&ni.to_be_bytes());
        cp.push(e);
        cp.len() as u16
    }

    let this_idx = klass(&mut cp, this_name);
    let super_idx = klass(&mut cp, "java/lang/Object");

    let member_table = |cp: &mut Vec<Vec<u8>>, members: &[(&str, &str)]| -> Vec<u8> {
        let mut out = (members.len() as u16).to_be_bytes().to_vec();
        for (name, desc) in members {
            let ni = utf8(cp, name);
            let di = utf8(cp, desc);
            // access_flags = 0, name, descriptor, 0 attributes
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&ni.to_be_bytes());
            out.extend_from_slice(&di.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
        }
        out
    };
    let f_table = member_table(&mut cp, fields);
    let m_table = member_table(&mut cp, methods);

    let mut out: Vec<u8> = vec![0xca, 0xfe, 0xba, 0xbe];
    out.extend_from_slice(&0u16.to_be_bytes()); // minor
    out.extend_from_slice(&52u16.to_be_bytes()); // major — Java 8
    out.extend_from_slice(&((cp.len() + 1) as u16).to_be_bytes());
    for e in &cp {
        out.extend_from_slice(e);
    }
    out.extend_from_slice(&0u16.to_be_bytes()); // access_flags
    out.extend_from_slice(&this_idx.to_be_bytes());
    out.extend_from_slice(&super_idx.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // interfaces_count
    out.extend_from_slice(&f_table);
    out.extend_from_slice(&m_table);
    out.extend_from_slice(&0u16.to_be_bytes()); // class attributes_count
    out
}

/// One hand-assembled class packed into a jar, run past the pre-check.
fn jar_has_collision(dir: &Path, tag: &str, data: &[u8], entry: &str) -> bool {
    use std::io::Write;
    let jar = dir.join(format!("{tag}.jar"));
    let f = std::fs::File::create(&jar).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
    z.start_file(entry, o).unwrap();
    z.write_all(data).unwrap();
    z.finish().unwrap();
    rename::has_member_collisions(&jar)
}

#[test]
fn collision_detector_sees_duplicate_field_name() {
    let w = common::workdir("coll-field");
    // two fields named `a` with different descriptors — illegal in Java source
    let data = make_class("C", &[("a", "I"), ("a", "Ljava/lang/String;")], &[]);
    assert!(jar_has_collision(&w, "dup", &data, "C.class"));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn collision_detector_sees_return_type_only_overload() {
    let w = common::workdir("coll-ret");
    // same name + params, different return type — illegal in Java source
    let data = make_class("C", &[], &[("f", "(I)Z"), ("f", "(I)I")]);
    assert!(jar_has_collision(&w, "ret", &data, "C.class"));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn collision_detector_passes_a_clean_class() {
    let w = common::workdir("coll-clean");
    let data = make_class("C", &[("a", "I"), ("b", "I")], &[("f", "(I)Z"), ("g", "(I)I")]);
    assert!(!jar_has_collision(&w, "clean", &data, "C.class"));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn has_member_collisions_scans_a_whole_jar() {
    let w = common::workdir("coll-jar");
    let dirty = make_class("Dirty", &[("a", "I"), ("a", "J")], &[]);
    assert!(jar_has_collision(&w, "dirty", &dirty, "Dirty.class"));
    let clean = make_class("Clean", &[("a", "I")], &[("f", "()V")]);
    assert!(!jar_has_collision(&w, "cln", &clean, "Clean.class"));
    let _ = std::fs::remove_dir_all(&w);
}

// ─────────────────────── rebuilt-jar packing (difftest) ─────────────────────

#[test]
fn pack_rebuilt_jar_replaces_classes_and_keeps_resources() {
    use std::io::Write;
    let w = common::workdir("pack");
    let src = w.join("src.jar");
    {
        let f = std::fs::File::create(&src).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        for (name, body) in [
            ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n".to_vec()),
            ("res/level.dat", b"DATA".to_vec()),
            ("Old.class", b"OLDBYTES".to_vec()),
        ] {
            z.start_file(name, o).unwrap();
            z.write_all(&body).unwrap();
        }
        z.finish().unwrap();
    }
    let classes = w.join("classes");
    std::fs::create_dir_all(classes.join("pkg")).unwrap();
    std::fs::write(classes.join("Old.class"), b"NEWBYTES").unwrap();
    std::fs::write(classes.join("pkg").join("Extra.class"), b"EXTRA").unwrap();

    let out = difftest::pack_rebuilt_jar(&src, &classes, &w.join("rebuilt.jar")).unwrap();
    let read = |n: &str| arlecchino::util::zip_read_entry(&out, n).unwrap();
    assert_eq!(read("res/level.dat"), b"DATA", "resources must survive verbatim");
    assert_eq!(read("Old.class"), b"NEWBYTES", "stale class must be replaced");
    assert_eq!(read("pkg/Extra.class"), b"EXTRA", "new classes must be added");
    assert!(String::from_utf8_lossy(&read("META-INF/MANIFEST.MF")).contains("Manifest-Version"));
    let _ = std::fs::remove_dir_all(&w);
}

// ───────────────────────────── diffrun protocol ─────────────────────────────

#[test]
fn encode_input_rejects_an_unknown_key() {
    // A typo in an input script must be LOUD: silently dropping the event would make
    // a replay look like the game simply ignored the press.
    let ev = diffrun::InputEvent {
        t_ms: 0,
        key: serde_json::json!("NUM11"),
        action: "press".into(),
    };
    let err = diffrun::encode_input(&ev).unwrap_err();
    assert!(err.to_lowercase().contains("key"), "unhelpful error: {err}");
    // …while a known key still encodes.
    let ok = diffrun::InputEvent {
        t_ms: 0,
        key: serde_json::json!("NUM5"),
        action: "tap".into(),
    };
    assert!(diffrun::encode_input(&ok).is_ok());
}

// ────────────────────────────── semdiff mapping ─────────────────────────────

#[test]
fn load_reverse_map_strips_the_trailing_tag() {
    let w = common::workdir("revmap");
    let m = w.join("m.txt");
    std::fs::write(&m, "ab.c I -> c_af64044c\nab.i [I -> i_9613707b  (type-shadow)\n").unwrap();
    let rev = semdiff::load_reverse_map(Some(&m));
    assert_eq!(rev.get("c_af64044c").map(String::as_str), Some("c"));
    // the `(type-shadow)` note must not end up glued to the new name
    assert_eq!(rev.get("i_9613707b").map(String::as_str), Some("i"));
    let _ = std::fs::remove_dir_all(&w);
}

// ─────────────────────────── midp-rt: autostart target ──────────────────────

#[test]
fn autostart_target_is_graceful_on_garbage() {
    use std::io::Write;
    let w = common::workdir("autostart");
    let none: (Option<String>, Option<std::path::PathBuf>, Option<std::path::PathBuf>) =
        (None, None, None);

    // not a zip at all
    let bad = w.join("bad.jar");
    std::fs::write(&bad, b"\x00\x01garbage").unwrap();
    assert_eq!(midprun::autostart_target(&bad), none);

    // a .jad pointing at a jar that is not there
    let lost = w.join("lost.jad");
    std::fs::write(&lost, "MIDlet-1: X,/i.png,K\nMIDlet-Jar-URL: nowhere.jar\n").unwrap();
    assert_eq!(midprun::autostart_target(&lost), none);

    // a remote jar URL is not our business
    let remote = w.join("remote.jad");
    std::fs::write(&remote, "MIDlet-1: X,/i.png,K\nMIDlet-Jar-URL: http://x/y.jar\n").unwrap();
    assert_eq!(midprun::autostart_target(&remote), none);

    // a manifest whose MIDlet-1 has no class field
    let half = w.join("half.jar");
    {
        let f = std::fs::File::create(&half).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        z.start_file("META-INF/MANIFEST.MF", o).unwrap();
        z.write_all(b"Manifest-Version: 1.0\r\nMIDlet-1: OnlyName,/icon.png\r\n\r\n").unwrap();
        z.finish().unwrap();
    }
    assert_eq!(midprun::autostart_target(&half), none);
    let _ = std::fs::remove_dir_all(&w);
}

// ─────────────────────── midp-rt: runtime classpath layouts ─────────────────

#[test]
fn runtime_classpath_finds_the_forks_asm_in_every_layout() {
    let w = common::workdir("rtcp");
    // checkout layout: build/midp-rt.jar next to ../lib/asm-*.jar
    let repo = w.join("rt-repo");
    std::fs::create_dir_all(repo.join("build")).unwrap();
    std::fs::create_dir_all(repo.join("lib")).unwrap();
    let jar = repo.join("build").join("midp-rt.jar");
    std::fs::write(&jar, b"PK").unwrap();
    std::fs::write(repo.join("lib").join("asm-9.8.jar"), b"PK").unwrap();

    let cp = midprun::runtime_classpath(&jar);
    assert_eq!(cp[0], jar);
    assert_eq!(cp[1].file_name().unwrap(), "asm-9.8.jar");

    // any asm*.jar version is picked up, not a hard-coded one
    std::fs::remove_file(repo.join("lib").join("asm-9.8.jar")).unwrap();
    std::fs::write(repo.join("lib").join("asm-9.9.jar"), b"PK").unwrap();
    let names: Vec<String> =
        midprun::runtime_classpath(&jar).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["midp-rt.jar", "asm-9.9.jar"]);

    // a bare jar with no repo layout around it: just the jar, no crash
    let bare_dir = w.join("bare");
    std::fs::create_dir_all(&bare_dir).unwrap();
    let bare = bare_dir.join("midp-rt.jar");
    std::fs::write(&bare, b"PK").unwrap();
    assert_eq!(midprun::runtime_classpath(&bare), vec![bare.clone()]);
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn missing_hint_says_how_to_fix_it() {
    let w = common::workdir("hint");
    let hint = midprun::missing_hint(&[w.join("nowhere.jar")], Some(&w.join("engines")));
    assert!(hint.contains("midp-rt.jar not found"), "{hint}");
    // it must name a concrete next step: either build the fork or point at a jar
    assert!(hint.contains("build.sh") || hint.contains("ARLECCHINO_MIDP_RT_SRC"), "{hint}");
    assert!(hint.contains("--midp-rt"), "{hint}");
    // and it must list what it actually looked at
    assert!(hint.contains("nowhere.jar"), "{hint}");
    let _ = std::fs::remove_dir_all(&w);
}
