//! `arlecchino run`: where the MIDP runtime jar is looked for, and the readable errors.
//!
//! Ported from the reference suite's `tests/test_midprun.py`.  The argv construction
//! itself is already pinned by the golden parity fixture (`behaviour_parity.rs`); what
//! is added here is the RESOLUTION ORDER and the CLI's exit codes on the error paths.
//!
//! Nothing is ever launched: every CLI case here returns before the emulator would be
//! spawned.
//!
//! `find_midp_rt` reads `$ARLECCHINO_MIDP_RT`, so the resolution test mutates the
//! process environment.  It is the only test in this binary that does, and the CLI cases
//! all pass `--midp-rt` explicitly, which short-circuits the environment lookup.

mod common;

use arlecchino::{cli, midprun};
use std::path::PathBuf;

const ENV: &str = "ARLECCHINO_MIDP_RT";

/// Restores `$ARLECCHINO_MIDP_RT` however the test ends.
struct EnvGuard(Option<String>);

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var(ENV, v),
            None => std::env::remove_var(ENV),
        }
    }
}

#[test]
fn find_midp_rt_follows_the_documented_priority_order() {
    let w = common::workdir("rt-order");
    let _guard = EnvGuard(std::env::var(ENV).ok());

    let explicit = w.join("explicit.jar");
    std::fs::write(&explicit, b"PK").unwrap();
    let engines = w.join("engines");
    std::fs::create_dir_all(&engines).unwrap();
    let in_root = engines.join("midp-rt.jar");
    std::fs::write(&in_root, b"PK").unwrap();
    let missing = w.join("missing.jar");

    std::env::remove_var(ENV);

    // 3. the engines root, when nothing more specific is given
    assert_eq!(midprun::find_midp_rt(None, Some(&engines)), Some(in_root.clone()));
    // 1. an explicit path beats it
    assert_eq!(midprun::find_midp_rt(Some(&explicit.to_string_lossy()), Some(&engines)), Some(explicit.clone()));
    // …and an explicit path that does NOT exist is an explicit "no": naming a jar and
    // silently getting a different one would be worse than failing.
    assert_eq!(midprun::find_midp_rt(Some(&missing.to_string_lossy()), Some(&engines)), None);

    // 2. the environment beats the engines root…
    std::env::set_var(ENV, &explicit);
    assert_eq!(midprun::find_midp_rt(None, Some(&engines)), Some(explicit.clone()));
    // …and the explicit argument still beats the environment
    assert_eq!(midprun::find_midp_rt(Some(&in_root.to_string_lossy()), Some(&engines)), Some(in_root.clone()));
    std::env::remove_var(ENV);

    // 4. with an EMPTY engines root the search falls through to the later candidates
    //    (next to the binary, then a fork checkout) — never to the empty root itself.
    let empty = w.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let fallback = midprun::find_midp_rt(None, Some(&empty));
    assert_ne!(fallback, Some(empty.join("midp-rt.jar")));
    if let Some(p) = &fallback {
        assert!(p.exists(), "a resolved candidate must exist: {}", p.display());
    }
    let _ = std::fs::remove_dir_all(&w);
}

/// A jar with just a manifest — enough for `autostart_target`, never launched.
fn game_jar(dir: &PathBuf, name: &str, midlet1: Option<&str>) -> PathBuf {
    use std::io::Write;
    let jar = dir.join(name);
    let mut manifest = String::from("Manifest-Version: 1.0");
    if let Some(m) = midlet1 {
        manifest.push_str(&format!("\r\nMIDlet-1: {m}"));
    }
    manifest.push_str("\r\n\r\n");
    let f = std::fs::File::create(&jar).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
    z.start_file("META-INF/MANIFEST.MF", o).unwrap();
    z.write_all(manifest.as_bytes()).unwrap();
    z.finish().unwrap();
    jar
}

fn run_cli(args: &[&str]) -> i32 {
    cli::main(Some(args.iter().map(|s| s.to_string()).collect()))
}

#[test]
fn cli_run_reports_a_missing_game_jar() {
    let w = common::workdir("rt-cli-missing");
    let rc = run_cli(&["run", &w.join("absent.jar").to_string_lossy()]);
    assert_eq!(rc, 2, "a missing jar must be a usage error, not a crash");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn cli_run_reports_a_missing_runtime_jar() {
    let w = common::workdir("rt-cli-nort");
    let game = game_jar(&w, "game.jar", Some("Allods,/i.png,GameMIDlet"));
    let rc = run_cli(&[
        "run",
        &game.to_string_lossy(),
        "--midp-rt",
        &w.join("no-rt.jar").to_string_lossy(),
    ]);
    assert_eq!(rc, 1, "a missing runtime must fail with the build hint, not launch");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn cli_run_rejects_a_malformed_size() {
    let w = common::workdir("rt-cli-size");
    let game = game_jar(&w, "game.jar", Some("Allods,/i.png,GameMIDlet"));
    // a jar-shaped file is enough to get PAST runtime resolution; --size is rejected
    // before anything is spawned.
    let fake_rt = w.join("midp-rt.jar");
    std::fs::write(&fake_rt, b"PK").unwrap();
    let rc = run_cli(&[
        "run",
        &game.to_string_lossy(),
        "--midp-rt",
        &fake_rt.to_string_lossy(),
        "--size",
        "huge",
    ]);
    assert_eq!(rc, 2, "a malformed --size must be a usage error");
    let _ = std::fs::remove_dir_all(&w);
}
