//! Shared fixtures for the javac/ASM-dependent integration tests.
//!
//! Everything here is GENERATED on the fly — no jar, no class file and no decompiler
//! output is ever committed.  The Java sources under `tests/fixtures/` are compiled
//! with the same `javac` the toolkit itself uses, and post-processed with the same ASM
//! jars `arlecchino setup` installs for the member renamer.
//!
//! Every builder returns `Err(reason)` instead of panicking when a prerequisite is
//! missing (no javac, no ASM jars, no compiled helper), so a test on a bare machine
//! reports SKIP rather than a false failure.  See [`skip`].

#![allow(dead_code)]

use arlecchino::{diffrun, rename, util};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Report a missing prerequisite.  Rust has no `pytest.skip`, so a test that cannot
/// run prints this and returns green — the same graceful-degradation contract the
/// reference suite had.
pub fn skip(reason: &str) {
    eprintln!("SKIP: {reason}");
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden")
}

/// A fresh, uniquely named scratch directory for one test.
pub fn workdir(tag: &str) -> PathBuf {
    util::make_temp_dir(&format!("arl-test-{tag}")).expect("temp dir")
}

pub fn javac() -> String {
    util::tool("ARLECCHINO_JAVAC", "javac")
}

pub fn java() -> String {
    util::tool("ARLECCHINO_JAVA", "java")
}

pub fn have(bin: &str) -> bool {
    diffrun::which(bin).is_some()
}

/// javac + java on PATH, ASM jars installed.  `Err` carries the reason to skip on.
pub fn need_java_and_asm() -> Result<Vec<PathBuf>, String> {
    if !have(&javac()) || !have(&java()) {
        return Err("javac/java not on PATH".into());
    }
    let jars = rename::asm_jars(None);
    if jars.is_empty() {
        return Err("ASM jars unavailable (run `arlecchino setup`)".into());
    }
    Ok(jars)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Run a helper process; `Err` on failure so the caller can skip with the reason.
pub fn run(argv: &[String], what: &str) -> Result<String, String> {
    let r = util::run_capture(argv, Some(Duration::from_secs(600)));
    if let Some(e) = &r.spawn_err {
        return Err(format!("{what} could not start: {e}"));
    }
    if !r.success() {
        let tail: String = r.stderr.chars().rev().take(800).collect::<String>().chars().rev().collect();
        return Err(format!("{what} failed (rc={:?}):\n{tail}", r.status));
    }
    Ok(r.stdout)
}

/// Count `File.java:12: error: …` lines exactly the way `core` scores a compile.
pub fn javac_error_count(stderr: &str) -> usize {
    let re = regex::Regex::new(r"(?m)^.*?\.java:\d+: error:").unwrap();
    re.find_iter(stderr).count()
}

/// Compile ONE source (given as text) against `api_cp`; return the javac error count.
pub fn compile_source_isolated(src_text: &str, rel: &str, api_cp: &Path, tmp: &Path) -> usize {
    let f = tmp.join(rel);
    if let Some(p) = f.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(&f, src_text).unwrap();
    let out = tmp.join("_out");
    std::fs::create_dir_all(&out).unwrap();
    let argv = vec![
        javac(),
        "-d".into(),
        s(&out),
        "--release".into(),
        "8".into(),
        "-encoding".into(),
        "utf-8".into(),
        "-nowarn".into(),
        "-proc:none".into(),
        "-cp".into(),
        s(api_cp),
        s(&f),
    ];
    let r = util::run_capture(&argv, Some(Duration::from_secs(300)));
    javac_error_count(&r.stderr)
}

// ── jar helpers ──────────────────────────────────────────────────────────────

pub fn jar_entries(jar: &Path) -> BTreeSet<String> {
    util::zip_namelist(jar).unwrap_or_default().into_iter().collect()
}

pub fn jar_manifest(jar: &Path) -> String {
    util::zip_read_entry(jar, "META-INF/MANIFEST.MF")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// Class-file major version of one jar entry (52 = Java 8, 49 = Java 5).
pub fn class_major(jar: &Path, entry: &str) -> u16 {
    let b = util::zip_read_entry(jar, entry).expect("class entry");
    ((b[6] as u16) << 8) | b[7] as u16
}

/// Pack every `*.class` under `dir` into `out`, optionally with a manifest.
pub fn pack_jar(dir: &Path, out: &Path, manifest: Option<&str>) {
    use std::io::Write;
    let f = std::fs::File::create(out).expect("create jar");
    let mut z = zip::ZipWriter::new(f);
    let o: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    if let Some(m) = manifest {
        z.start_file("META-INF/MANIFEST.MF", o).unwrap();
        z.write_all(m.as_bytes()).unwrap();
    }
    let mut files = util::walk_files(dir);
    files.sort();
    for p in files {
        if p.extension().and_then(|e| e.to_str()) != Some("class") {
            continue;
        }
        let rel = p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
        z.start_file(rel, o).unwrap();
        z.write_all(&std::fs::read(&p).unwrap()).unwrap();
    }
    z.finish().unwrap();
}

/// Compile every `*.java` under `src` into `dst`, optionally against `cp`.
pub fn javac_dir(src: &Path, dst: &Path, cp: Option<&str>, what: &str) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let mut argv = vec![javac(), "-d".into(), s(dst), "-encoding".into(), "utf-8".into()];
    if let Some(c) = cp {
        argv.push("-cp".into());
        argv.push(c.to_string());
    }
    let mut srcs = util::rglob_ext(src, "java");
    srcs.sort();
    if srcs.is_empty() {
        return Err(format!("{what}: no sources under {}", src.display()));
    }
    argv.extend(srcs.iter().map(|p| s(p)));
    run(&argv, what).map(|_| ())
}

/// Compile ONE helper source against the ASM jars and return its classes dir.
fn build_asm_helper(src: &Path, dst: &Path, asm_cp: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let argv = vec![
        javac(),
        "-cp".into(),
        asm_cp.to_string(),
        "-d".into(),
        s(dst),
        s(src),
    ];
    run(&argv, &format!("javac ({})", src.file_name().unwrap().to_string_lossy()))?;
    Ok(dst.to_path_buf())
}

// ── the synthetic obfuscated jar (renamer safety net) ────────────────────────

/// The synthetic obfuscated jar plus the clean-room MIDP ABI stub its `Screen` class
/// needs on the compile classpath after ASM re-parents it to `Canvas`.
pub struct SynthJar {
    /// The synthetic obfuscated jar — five injected source-illegal collisions.
    pub jar: PathBuf,
    /// The pre-obfuscation classes, for reference.
    pub classes_dir: PathBuf,
    /// A minimal generated `javax.microedition.lcdui.Canvas` ABI jar.
    pub midp_stub: PathBuf,
}

/// The tiny compile-time MIDP ABI the synth fixture touches.  This is NOT the full
/// clean-room stub of `midp_stub.rs` — it exists only so the re-parented `Screen`
/// resolves.
const MIDP_ABI: &[(&str, &str)] = &[
    (
        "javax/microedition/lcdui/Displayable.java",
        "package javax.microedition.lcdui;\n\
         public abstract class Displayable {\n\
         \x20   protected Displayable() {}\n\
         }\n",
    ),
    (
        "javax/microedition/lcdui/Canvas.java",
        "package javax.microedition.lcdui;\n\
         public abstract class Canvas extends Displayable {\n\
         \x20   protected Canvas() {}\n\
         \x20   protected void keyPressed(int keyCode) {}\n\
         }\n",
    ),
];

/// Build the minimal MIDP ABI stub jar under `work`.
pub fn build_midp_abi_stub(work: &Path) -> Result<PathBuf, String> {
    let src = work.join("midp_abi_src");
    let cls = work.join("midp_abi_classes");
    for (rel, body) in MIDP_ABI {
        let p = src.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&p, body).map_err(|e| e.to_string())?;
    }
    javac_dir(&src, &cls, None, "javac (MIDP ABI stub)")?;
    let jar = work.join("midp_abi.jar");
    pack_jar(&cls, &jar, None);
    Ok(jar)
}

/// Compile the fixture sources, then inject the five source-illegal collisions with
/// ASM (`SynthObfuscator`), producing the synthetic obfuscated jar.
pub fn build_synth_jar(work: &Path) -> Result<SynthJar, String> {
    let asm = need_java_and_asm()?;
    let asm_cp = asm.iter().map(|j| s(j)).collect::<Vec<_>>().join(util::PATHSEP);

    let fix = fixtures_dir().join("synthjar");
    let classes = work.join("classes");
    javac_dir(&fix.join("src"), &classes, None, "javac (synth sources)")?;

    let helper = build_asm_helper(&fix.join("SynthObfuscator.java"), &work.join("helper"), &asm_cp)?;

    let out_jar = work.join("synth_obf.jar");
    let run_cp = format!("{asm_cp}{}{}", util::PATHSEP, s(&helper));
    run(
        &[java(), "-cp".into(), run_cp, "SynthObfuscator".into(), s(&classes), s(&out_jar)],
        "SynthObfuscator",
    )?;
    if !out_jar.exists() {
        return Err("SynthObfuscator produced no jar".into());
    }

    let midp_stub = build_midp_abi_stub(work)?;
    Ok(SynthJar { jar: out_jar, classes_dir: classes, midp_stub })
}

// ── the cross-package (flatten) fixture ──────────────────────────────────────

/// Build the `Container.App` -> default-package `Lib` jar: the irreducible situation
/// Java source cannot express, which `--flatten-packages` exists to resolve.
pub fn build_flatten_jar(work: &Path) -> Result<PathBuf, String> {
    let asm = need_java_and_asm()?;
    if !rename::available(None) {
        return Err("ASM member-renamer helper not compiled (run `arlecchino setup`)".into());
    }
    let asm_cp = asm.iter().map(|j| s(j)).collect::<Vec<_>>().join(util::PATHSEP);

    let fix = fixtures_dir().join("flatten");
    let classes = work.join("fclasses");
    javac_dir(&fix.join("src"), &classes, None, "javac (flatten fixture src)")?;

    let helper = build_asm_helper(&fix.join("FlattenFixture.java"), &work.join("fhelper"), &asm_cp)?;
    let out_jar = work.join("xpkg.jar");
    let run_cp = format!("{asm_cp}{}{}", util::PATHSEP, s(&helper));
    run(
        &[java(), "-cp".into(), run_cp, "FlattenFixture".into(), s(&classes), s(&out_jar)],
        "FlattenFixture",
    )?;
    if !out_jar.exists() {
        return Err("FlattenFixture produced no jar".into());
    }
    Ok(out_jar)
}

// ── the deterministic runnable MIDlet ────────────────────────────────────────

const MIDLET_MANIFEST: &str = "Manifest-Version: 1.0\n\
                               MIDlet-Name: ArlecchinoSynth\n\
                               MIDlet-Vendor: Arlecchino\n\
                               MIDlet-Version: 1.0\n\
                               MIDlet-1: Synth, , SynthMidlet\n\
                               MicroEdition-Configuration: CLDC-1.1\n\
                               MicroEdition-Profile: MIDP-2.0\n";

/// The full clean-room MIDP/CLDC stub: reuse the installed one when present, else
/// build one INTO `work` (a test never writes to the user's home).
fn full_midp_stub(work: &Path) -> Result<PathBuf, String> {
    if let Some(p) = arlecchino::midp_stub::stub_jar() {
        return Ok(p);
    }
    let dst = work.join("midp-stub.jar");
    let status = arlecchino::midp_stub::build_stub(Some(&dst), Some(&work.join("midp-stub-src")));
    if !dst.exists() || std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err(format!("MIDP stub jar could not be built: {status}"));
    }
    Ok(dst)
}

/// Build the deterministic test MIDlet (checkerboard in `color`, last-key colour bar,
/// one RecordStore write).  A different `color` is an intentionally different jar —
/// the difftest FAIL fixture.
pub fn build_synth_midlet(work: &Path, color: u32) -> Result<PathBuf, String> {
    if !have(&javac()) {
        return Err("javac not on PATH".into());
    }
    let stub = full_midp_stub(work)?;
    let tag = format!("{:06x}", color & 0xFF_FFFF);
    let src_dir = work.join(format!("midlet_src_{tag}"));
    let cls_dir = work.join(format!("midlet_classes_{tag}"));
    std::fs::create_dir_all(&src_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&cls_dir).map_err(|e| e.to_string())?;

    let template = std::fs::read_to_string(fixtures_dir().join("synthjar").join("midlet").join("SynthMidlet.java"))
        .map_err(|e| e.to_string())?;
    let src = src_dir.join("SynthMidlet.java");
    std::fs::write(&src, template.replace("@COLOR@", &format!("{:06X}", color & 0xFF_FFFF)))
        .map_err(|e| e.to_string())?;

    run(
        &[
            javac(),
            "--release".into(),
            "8".into(),
            "-encoding".into(),
            "utf-8".into(),
            "-nowarn".into(),
            "-proc:none".into(),
            "-cp".into(),
            s(&stub),
            "-d".into(),
            s(&cls_dir),
            s(&src),
        ],
        "javac (SynthMidlet)",
    )?;

    let jar = work.join(format!("synth_midlet_{tag}.jar"));
    pack_jar(&cls_dir, &jar, Some(MIDLET_MANIFEST));
    Ok(jar)
}
