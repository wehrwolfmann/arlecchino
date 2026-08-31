//! `arlecchino setup`: download decompiler engines + a J2ME emulator into
//! ~/.arlecchino/engines/.
//!
//! Engines are fetched from their official GitHub releases (Vineflower, CFR, Procyon,
//! jadx, JD-Core via jd-cli) plus a few pinned direct downloads (Jode from SourceForge,
//! the standalone Fernflower and Soot/Dava jars from Maven Central).  ASM (org.ow2.asm,
//! BSD-3-Clause) is fetched from Maven Central for the bytecode member-renamer helper.
//! FreeJ2ME is fetched as a prebuilt release jar or built from source with `javac`
//! (no Ant).  Every step degrades gracefully offline.
//!
//! Every GitHub-hosted engine is PINNED to an exact release tag, overridable via
//! ARLECCHINO_<ENGINE>_VERSION.  We ask for that tag's assets (`releases/tags/<tag>`)
//! and never `releases/latest`, so an upstream release cannot silently shift the
//! headline metric (Allods 712 -> 2; see README "Reproducibility").  The direct
//! downloads carry their version in the URL and are pinned by construction.
//!
//! When GITHUB_TOKEN / GH_TOKEN is set it is sent as an `Authorization: Bearer`
//! header, lifting the anonymous 60 req/h GitHub API rate limit.
//!
//! Together these give the eight decompilers of Harrand et al.'s decompiler-diversity
//! study: CFR, Procyon, Fernflower (both the maintained Vineflower fork and the original
//! standalone engine), JADX, JD-Core, Jode, and Dava.  Krakatau is not fetched: its v1
//! decompiler is Python 2 only and the maintained v2 Rust rewrite has no decompiler yet,
//! so the standalone Fernflower stands in for it.

use crate::config::engines_root_default;
use crate::rename;
use crate::util;
use std::path::Path;
use std::time::Duration;

const UA: &str = "arlecchino-setup";
const ASM_MODULES: &[&str] = &["asm", "asm-tree", "asm-commons", "asm-analysis"];
const ASM_MAVEN: &str = "https://repo1.maven.org/maven2/org/ow2/asm";
const FREEJ2ME_REPO: &str = "TASEmulators/freej2me-plus";

fn asm_version() -> String {
    util::tool("ARLECCHINO_ASM_VERSION", "9.7")
}

// Pinned engine release tags, overridable via ARLECCHINO_<ENGINE>_VERSION.  These
// exact versions reproduce the Allods headline metric 712 -> 2.  jd-cli's newest
// release is a beta; the pin freezes the one this port was measured against instead
// of following whatever `releases/latest` happens to point at.
fn engine_tag(name: &str) -> String {
    match name {
        "vineflower" => util::tool("ARLECCHINO_VINEFLOWER_VERSION", "1.12.0"),
        "cfr" => util::tool("ARLECCHINO_CFR_VERSION", "0.152"),
        "procyon" => util::tool("ARLECCHINO_PROCYON_VERSION", "v0.6.0"),
        "jadx" => util::tool("ARLECCHINO_JADX_VERSION", "v1.5.5"),
        "jd-core" => util::tool("ARLECCHINO_JD_CLI_VERSION", "jd-cli-1.3.0-beta-1"),
        _ => String::new(),
    }
}

/// Pinned FreeJ2ME-plus release tag (reproducibility of the diff-testing harness).
/// Tag 1.52 ships `freej2me-v1.52.zip` with freej2me.jar (AWT) AND freej2me-lr.jar
/// (libretro pipe frontend, which diffrun drives for headless capture).
fn freej2me_version() -> String {
    util::tool("ARLECCHINO_FREEJ2ME_PLUS_VERSION", "1.52")
}

/// The FreeJ2ME release asset we want: the jar bundle, NOT the native libretro core.
fn freej2me_asset_pred(a: &str) -> bool {
    a.ends_with(".zip") && !a.contains("libretro")
}

/// Jars extracted from the FreeJ2ME bundle into the engines root.
const FREEJ2ME_JARS: &[&str] = &["freej2me.jar", "freej2me-lr.jar"];

/// GitHub API URL for a SPECIFIC release tag (never `releases/latest`).
fn release_api_url(repo: &str, tag: &str) -> String {
    format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
}

/// Attach our base headers, plus `Authorization: Bearer` when GITHUB_TOKEN /
/// GH_TOKEN is set (lifts the 60 req/h anonymous rate limit).
fn gh_request(url: &str, timeout: Duration) -> ureq::Request {
    let mut req = ureq::get(url).set("User-Agent", UA).timeout(timeout);
    if let Some(t) = gh_token() {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    req
}

fn gh_token() -> Option<String> {
    for k in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

// ── GitHub release helpers ───────────────────────────────────────────────────
fn http_get_json(url: &str) -> Result<serde_json::Value, String> {
    let resp = gh_request(url, Duration::from_secs(60)).call().map_err(|e| e.to_string())?;
    resp.into_json::<serde_json::Value>().map_err(|e| e.to_string())
}

fn download(url: &str, dst: &Path) -> Result<(), String> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let resp = gh_request(url, Duration::from_secs(300)).call().map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader();
    let mut f = std::fs::File::create(dst).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut f).map_err(|e| e.to_string())?;
    Ok(())
}

/// (asset_name, browser_download_url) of the first asset matching `pred` in the
/// release tagged `tag`.  Pinned by construction — no `releases/latest`.
fn tagged_asset(
    repo: &str,
    tag: &str,
    pred: impl Fn(&str) -> bool,
) -> Result<Option<(String, String)>, String> {
    let data = http_get_json(&release_api_url(repo, tag))?;
    if let Some(assets) = data.get("assets").and_then(|a| a.as_array()) {
        for a in assets {
            let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if pred(name) {
                let url = a
                    .get("browser_download_url")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();
                return Ok(Some((name.to_string(), url)));
            }
        }
    }
    Ok(None)
}

fn engine_pred(name: &str) -> Box<dyn Fn(&str) -> bool> {
    match name {
        "vineflower" => Box::new(|a: &str| {
            a.ends_with(".jar") && !a.contains("slim") && !a.contains("sources") && !a.contains("javadoc")
        }),
        "cfr" => Box::new(|a: &str| {
            a.starts_with("cfr-") && a.ends_with(".jar") && !a.contains("javadoc") && !a.contains("sources")
        }),
        "procyon" => Box::new(|a: &str| {
            a.starts_with("procyon-decompiler")
                && a.ends_with(".jar")
                && !a.contains("sources")
                && !a.contains("javadoc")
        }),
        "jadx" => Box::new(|a: &str| a.starts_with("jadx-") && a.ends_with(".zip") && !a.contains("gui")),
        "jd-core" => Box::new(|a: &str| a.starts_with("jd-cli") && a.ends_with("-dist.zip")),
        _ => Box::new(|_: &str| false),
    }
}

fn engine_repo(name: &str) -> &'static str {
    match name {
        "vineflower" => "Vineflower/vineflower",
        "cfr" => "leibnitz27/cfr",
        "procyon" => "mstrobel/procyon",
        "jadx" => "skylot/jadx",
        "jd-core" => "intoolswetrust/jd-cli",
        _ => "",
    }
}

const GH_ORDER: &[&str] = &["vineflower", "cfr", "procyon", "jadx", "jd-core"];

/// Engines fetched as a single pinned jar (name, url, target filename).  Jode is the
/// classic 2001 decompiler; Fernflower is the standalone JetBrains engine (repackaged
/// by the JBoss Windup project); Soot carries the Dava decompiler back-end.
const DIRECT_JARS: &[(&str, &str, &str)] = &[
    (
        "jode",
        "https://master.dl.sourceforge.net/project/jode/jode/1.1.2-pre1/jode-1.1.2-pre1.jar?viasf=1",
        "jode.jar",
    ),
    (
        "fernflower",
        "https://repo1.maven.org/maven2/org/jboss/windup/decompiler/fernflower/fernflower/2.5.0.Final/fernflower-2.5.0.Final.jar",
        "fernflower.jar",
    ),
    (
        "dava",
        "https://repo1.maven.org/maven2/org/soot-oss/soot/4.3.0/soot-4.3.0-jar-with-dependencies.jar",
        "soot.jar",
    ),
];

/// Download engines. Returns ordered (name, status) pairs.
pub fn setup_engines(root: &Path, only: Option<&[String]>) -> Vec<(String, String)> {
    let _ = std::fs::create_dir_all(root);
    let mut result: Vec<(String, String)> = Vec::new();
    let in_only = |n: &str| only.map(|o| o.iter().any(|x| x == n)).unwrap_or(false);

    for name in GH_ORDER {
        if only.is_some() && !in_only(name) {
            continue;
        }
        let repo = engine_repo(name);
        let tag = engine_tag(name);
        let status = (|| -> String {
            let asset = match tagged_asset(repo, &tag, engine_pred(name)) {
                Ok(Some(a)) => a,
                Ok(None) => return format!("no matching asset for tag {tag}"),
                Err(e) => return format!("FAILED: {e}"),
            };
            let (fname, url) = asset;
            if *name == "jadx" {
                let zpath = root.join(&fname);
                println!("  [{name}] downloading {fname} ...");
                if let Err(e) = download(&url, &zpath) {
                    return format!("FAILED: {e}");
                }
                let jadx_dir = root.join("jadx");
                if jadx_dir.exists() {
                    let _ = std::fs::remove_dir_all(&jadx_dir);
                }
                if let Err(e) = util::zip_extract_all(&zpath, &jadx_dir) {
                    return format!("FAILED: {e}");
                }
                let jadx_bin = jadx_dir.join("bin").join("jadx");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&jadx_bin, std::fs::Permissions::from_mode(0o755));
                }
                let _ = std::fs::remove_file(&zpath);
                format!("ok (jadx/bin/jadx) [{tag}]")
            } else if *name == "jd-core" {
                // the jd-cli release is a zip; pull the single fat jar out of it.
                let zpath = root.join(&fname);
                println!("  [{name}] downloading {fname} ...");
                if let Err(e) = download(&url, &zpath) {
                    return format!("FAILED: {e}");
                }
                let member = match util::zip_namelist(&zpath) {
                    Ok(names) => names.into_iter().find(|m| m.ends_with("jd-cli.jar")),
                    Err(e) => return format!("FAILED: {e}"),
                };
                let member = match member {
                    Some(m) => m,
                    None => return "FAILED: jd-cli.jar not in release zip".into(),
                };
                let bytes = match util::zip_read_entry(&zpath, &member) {
                    Ok(b) => b,
                    Err(e) => return format!("FAILED: {e}"),
                };
                let target = root.join("jd-cli.jar");
                if let Err(e) = std::fs::write(&target, bytes) {
                    return format!("FAILED: {e}");
                }
                let _ = std::fs::remove_file(&zpath);
                format!("ok (jd-cli.jar) [{tag}]")
            } else {
                let target = root.join(match *name {
                    "vineflower" => "vineflower.jar",
                    "cfr" => "cfr.jar",
                    "procyon" => "procyon.jar",
                    _ => "engine.jar",
                });
                println!("  [{name}] downloading {fname} ...");
                if let Err(e) = download(&url, &target) {
                    return format!("FAILED: {e}");
                }
                format!("ok ({}) [{tag}]", target.file_name().unwrap().to_string_lossy())
            }
        })();
        result.push((name.to_string(), status));
    }

    for (name, url, target) in DIRECT_JARS {
        if only.is_some() && !in_only(name) {
            continue;
        }
        let dst = root.join(target);
        let status = if dst.exists() && dst.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            format!("ok (cached {target})")
        } else {
            println!("  [{name}] downloading {target} ...");
            match download(url, &dst) {
                Ok(()) => format!("ok ({target})"),
                Err(e) => format!("FAILED: {e}"),
            }
        };
        result.push((name.to_string(), status));
    }

    if only.is_none() || in_only("asm") {
        result.push(("asm".to_string(), setup_asm(root)));
    }
    // Рантайм MIDP (LGPL-форк MicroEmulator) — ОТДЕЛЬНЫЙ репозиторий/артефакт,
    // сюда он никогда не качается и не вкладывается. Этот шаг только НАХОДИТ
    // midp-rt.jar (настраиваемо: $ARLECCHINO_MIDP_RT / корень движков / чекаут
    // форка) и сообщает, лежат ли рядом его jar-ы ASM.
    if only.is_none() || in_only("midp-rt") {
        result.push(("midp-rt".to_string(), crate::midprun::setup_midp_rt(Some(root))));
    }
    result
}

/// Download ASM jars from Maven Central into <root>/asm and compile the helper.
pub fn setup_asm(root: &Path) -> String {
    let asmdir = root.join("asm");
    let _ = std::fs::create_dir_all(&asmdir);
    let ver = asm_version();
    let mut got = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for m in ASM_MODULES {
        let dst = asmdir.join(format!("{m}-{ver}.jar"));
        if dst.exists() && dst.metadata().map(|md| md.len() > 0).unwrap_or(false) {
            got += 1;
            continue;
        }
        let url = format!("{ASM_MAVEN}/{m}/{ver}/{m}-{ver}.jar");
        println!("  [asm] downloading {m}-{ver}.jar ...");
        match download(&url, &dst) {
            Ok(()) => got += 1,
            Err(e) => failed.push(format!("{m}: {e}")),
        }
    }
    if rename::asm_jars(Some(root)).is_empty() {
        let joined = failed.join("; ");
        let trimmed: String = joined.chars().take(300).collect();
        return format!("FAILED (no ASM jars; offline?). {trimmed}");
    }
    // компилируем ОБА ASM-помощника против тех jar-ов ASM, что есть: переименователь
    // членов (конвейер decompile) и A7-трансформер детерминированных часов
    // (difftest --fixed-clock по запросу). Механика и каталог _classes/ общие.
    let comp = rename::compile_helper(Some(root));
    let comp_a7 = crate::fixedclock::compile_helper(Some(root));
    let tail = if failed.is_empty() {
        String::new()
    } else {
        format!(" [missing: {}]", failed.join(", "))
    };
    format!(
        "jars={got}/{}; renamer {comp}; fixed-clock {comp_a7}{tail}",
        ASM_MODULES.len()
    )
}

// ── FreeJ2ME ─────────────────────────────────────────────────────────────────
/// Download the PINNED FreeJ2ME-plus release zip into `dst_dir` and extract BOTH of
/// its jars: freej2me.jar (AWT frontend) and freej2me-lr.jar (libretro pipe frontend,
/// which diffrun drives for headless frame capture).  None on failure.
fn download_freej2me_release(dst_dir: &Path) -> Option<String> {
    let tag = freej2me_version();
    let (fname, url) = tagged_asset(FREEJ2ME_REPO, &tag, freej2me_asset_pred).ok()??;
    let ztmp = dst_dir.join("_fj_release.zip");
    println!("  [freej2me] downloading prebuilt {fname} [{tag}] ...");
    download(&url, &ztmp).ok()?;
    let names = util::zip_namelist(&ztmp).ok()?;
    let mut got: Vec<&str> = Vec::new();
    for jar in FREEJ2ME_JARS {
        let member = match names.iter().find(|m| m.ends_with(jar)) {
            Some(m) => m.clone(),
            None => continue,
        };
        let dst = dst_dir.join(jar);
        if let Some(p) = dst.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let bytes = match util::zip_read_entry(&ztmp, &member) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if std::fs::write(&dst, bytes).is_ok() {
            got.push(jar);
        }
    }
    let _ = std::fs::remove_file(&ztmp);
    if !got.contains(&"freej2me.jar") {
        return None;
    }
    Some(format!(
        "ok (prebuilt release {} -> {}) [{tag}]",
        got.join(", "),
        dst_dir.display()
    ))
}

pub fn setup_freej2me(root: Option<&Path>, prebuilt: Option<&Path>, src_dir: Option<&Path>) -> String {
    let root = root.map(|p| p.to_path_buf()).unwrap_or_else(engines_root_default);
    let dst = root.join("freej2me.jar");
    if let Some(pb) = prebuilt {
        if pb.exists() {
            if let Some(p) = dst.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            if std::fs::copy(pb, &dst).is_ok() {
                return format!("ok (copied prebuilt -> {})", dst.display());
            }
        }
    }

    // Prefer the official prebuilt release jars (FreeJ2ME's AWT frontend often will
    // not compile cleanly on modern JDKs); the release is PINNED, no `latest` drift.
    if src_dir.is_none() {
        if let Some(rel) = download_freej2me_release(&root) {
            return rel;
        }
    }

    let mut work = root.join("freej2me-src");
    if let Some(sd) = src_dir {
        if sd.exists() {
            work = sd.to_path_buf();
        }
    } else if !work.exists() {
        let tag = freej2me_version();
        println!("  [freej2me] cloning {FREEJ2ME_REPO} @ {tag} ...");
        let argv = vec![
            "git".to_string(),
            "clone".into(),
            "--depth".into(),
            "1".into(),
            "--branch".into(),
            tag,
            "https://github.com/TASEmulators/freej2me-plus.git".into(),
            work.to_string_lossy().into_owned(),
        ];
        let r = util::run_capture(&argv, Some(Duration::from_secs(600)));
        if r.spawn_err.is_some() || r.status != Some(0) {
            return format!(
                "FAILED clone: {}",
                r.spawn_err.unwrap_or_else(|| r.stderr.trim().to_string())
            );
        }
    }
    build_freej2me(&work, &dst)
}

/// Replicate build.xml's javac steps without Ant.
pub fn build_freej2me(src: &Path, dst: &Path) -> String {
    let srcroot = src.join("src");
    if !srcroot.is_dir() {
        return format!("FAILED: no src/ in {}", src.display());
    }
    let classes = src.join("build").join("classes");
    if classes.exists() {
        let _ = std::fs::remove_dir_all(&classes);
    }
    let _ = std::fs::create_dir_all(&classes);
    let javac = util::tool("ARLECCHINO_JAVAC", "javac");

    let collect = |pred: &dyn Fn(&str) -> bool| -> Vec<String> {
        util::rglob_ext(&srcroot, "java")
            .into_iter()
            .filter(|p| {
                p.strip_prefix(&srcroot)
                    .map(|r| pred(&r.to_string_lossy()))
                    .unwrap_or(false)
            })
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };

    // step 1: asm
    let asm = collect(&|r: &str| r.starts_with("org/objectweb/asm/"));
    if !asm.is_empty() {
        let lf = src.join("_asm.txt");
        let _ = std::fs::write(&lf, asm.join("\n"));
        let argv = vec![
            javac.clone(),
            "-d".into(),
            classes.to_string_lossy().into_owned(),
            "-nowarn".into(),
            "-proc:none".into(),
            format!("@{}", lf.to_string_lossy()),
        ];
        let _ = util::run_capture(&argv, None);
    }
    // step 2: everything except libretro + objectweb
    let rest = collect(&|r: &str| !r.starts_with("libretro/") && !r.starts_with("org/objectweb/"));
    let lf = src.join("_rest.txt");
    let _ = std::fs::write(&lf, rest.join("\n"));
    let argv = vec![
        javac.clone(),
        "-d".into(),
        classes.to_string_lossy().into_owned(),
        "-cp".into(),
        classes.to_string_lossy().into_owned(),
        "-nowarn".into(),
        "-proc:none".into(),
        "-encoding".into(),
        "utf-8".into(),
        format!("@{}", lf.to_string_lossy()),
    ];
    let r = util::run_capture(&argv, None);

    let n_cls = util::rglob_ext(&classes, "class").len();
    if n_cls == 0 {
        let tail: String = r.stderr.chars().rev().take(800).collect::<String>().chars().rev().collect();
        return format!("FAILED javac (no classes). tail:\n{tail}");
    }

    // copy resources
    let ed = src.join("resources");
    if ed.is_dir() {
        for p in util::walk_files(&ed) {
            if let Ok(rel) = p.strip_prefix(&ed) {
                let out = classes.join(rel);
                if let Some(parent) = out.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::copy(&p, &out);
            }
        }
    }

    let manifest = src.join("_MANIFEST.MF");
    let _ = std::fs::write(
        &manifest,
        "Main-Class: org.recompile.freej2me.FreeJ2ME\nImplementation-Title: FreeJ2ME\n",
    );
    let jar = util::tool("ARLECCHINO_JAR", "jar");
    if let Some(p) = dst.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let argv = vec![
        jar,
        "cfm".into(),
        dst.to_string_lossy().into_owned(),
        manifest.to_string_lossy().into_owned(),
        "-C".into(),
        classes.to_string_lossy().into_owned(),
        ".".into(),
    ];
    let rj = util::run_capture(&argv, None);
    if rj.status != Some(0) || !dst.exists() {
        let tail: String = rj.stderr.chars().rev().take(400).collect::<String>().chars().rev().collect();
        return format!("FAILED jar: {tail}");
    }
    let main_ok = classes
        .join("org")
        .join("recompile")
        .join("freej2me")
        .join("FreeJ2ME.class")
        .exists();
    if !main_ok {
        return format!(
            "PARTIAL ({n_cls} classes, but org.recompile.freej2me.FreeJ2ME is missing — AWT \
             frontend did not compile on this JDK). Use --freej2me-prebuilt with a released \
             jar, or build with the JDK FreeJ2ME targets. Jar at {} (core only).",
            dst.display()
        );
    }
    format!("ok (built {n_cls} classes -> {})", dst.display())
}

// ── stage E2: engine version pinning, ENV overrides, GITHUB_TOKEN header ─────
// Port of tests/test_engine_pins.py.  No network: we assert the CONSTRUCTED API
// URL (tag, never `releases/latest`), the asset predicates and the request
// HEADERS.  The version accessors read the environment on every call, so the
// ENV-override checks just set the variable — but the environment is process-wide,
// so those tests take a lock and put it back.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// pinned-by-default versions (these reproduce Allods 712 -> 2)
    const PINNED: &[(&str, &str, &str)] = &[
        ("vineflower", "Vineflower/vineflower", "1.12.0"),
        ("cfr", "leibnitz27/cfr", "0.152"),
        ("procyon", "mstrobel/procyon", "v0.6.0"),
        ("jadx", "skylot/jadx", "v1.5.5"),
    ];

    const ENV_KEYS: &[&str] = &[
        "ARLECCHINO_VINEFLOWER_VERSION",
        "ARLECCHINO_CFR_VERSION",
        "ARLECCHINO_PROCYON_VERSION",
        "ARLECCHINO_JADX_VERSION",
        "ARLECCHINO_JD_CLI_VERSION",
        "ARLECCHINO_FREEJ2ME_PLUS_VERSION",
        "GITHUB_TOKEN",
        "GH_TOKEN",
    ];

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Take the env lock and clear every variable these tests touch, so a test
    /// always starts from the pinned defaults and cannot leak an override into a
    /// test running after it.
    fn clean_env() -> MutexGuard<'static, ()> {
        let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for k in ENV_KEYS {
            std::env::remove_var(k);
        }
        g
    }

    // ── (a) pinned by default ───────────────────────────────────────────────
    #[test]
    fn each_engine_resolves_to_pinned_version_by_default() {
        let _g = clean_env();
        assert_eq!(engine_tag("vineflower"), "1.12.0");
        assert_eq!(engine_tag("cfr"), "0.152");
        assert_eq!(engine_tag("procyon"), "v0.6.0");
        assert_eq!(engine_tag("jadx"), "v1.5.5");
        for (name, repo, tag) in PINNED {
            assert_eq!(engine_repo(name), *repo);
            assert_eq!(engine_tag(name), *tag);
        }
    }

    #[test]
    fn release_url_uses_tag_not_latest() {
        let _g = clean_env();
        for (_name, repo, tag) in PINNED {
            let url = release_api_url(repo, tag);
            assert_eq!(url, format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"));
            assert!(!url.contains("releases/latest"), "{url}");
            assert!(url.contains(&format!("/tags/{tag}")), "{url}");
        }
    }

    #[test]
    fn asset_predicates_match_pinned_asset_names() {
        // the exact asset filenames published under each pinned tag
        let assets = [
            ("vineflower", "vineflower-1.12.0.jar"),
            ("cfr", "cfr-0.152.jar"),
            ("procyon", "procyon-decompiler-0.6.0.jar"),
            ("jadx", "jadx-1.5.5.zip"),
        ];
        for (name, fname) in assets {
            assert!(engine_pred(name)(fname), "{name}: {fname}");
        }
        // negative: sources/javadoc/slim/gui variants must NOT match
        assert!(!engine_pred("vineflower")("vineflower-1.12.0-slim.jar"));
        assert!(!engine_pred("cfr")("cfr-0.152-sources.jar"));
        assert!(!engine_pred("procyon")("procyon-decompiler-0.6.0-sources.jar"));
        assert!(!engine_pred("procyon")("procyon-decompiler-0.6.0-javadoc.jar"));
        assert!(!engine_pred("jadx")("jadx-gui-1.5.5.zip"));
    }

    /// jd-cli has no counterpart in the Python original (this port adds four more
    /// engines).  Its newest release is a beta, which is exactly why the tag is
    /// pinned rather than resolved through `releases/latest`.
    #[test]
    fn jd_core_is_pinned_too() {
        let _g = clean_env();
        let tag = engine_tag("jd-core");
        assert_eq!(tag, "jd-cli-1.3.0-beta-1");
        assert_eq!(engine_repo("jd-core"), "intoolswetrust/jd-cli");
        assert!(release_api_url(engine_repo("jd-core"), &tag).ends_with(
            "/repos/intoolswetrust/jd-cli/releases/tags/jd-cli-1.3.0-beta-1"
        ));
        assert!(engine_pred("jd-core")("jd-cli-1.3.0-beta-1-dist.zip"));
        assert!(!engine_pred("jd-core")("jd-cli-1.3.0-beta-1-dist.tar.gz"));
    }

    // ── (b) ENV override honored ────────────────────────────────────────────
    #[test]
    fn env_override_is_honored() {
        let _g = clean_env();
        let cases = [
            ("ARLECCHINO_VINEFLOWER_VERSION", "vineflower", "1.10.1"),
            ("ARLECCHINO_CFR_VERSION", "cfr", "0.151"),
            ("ARLECCHINO_PROCYON_VERSION", "procyon", "v0.5.36"),
            ("ARLECCHINO_JADX_VERSION", "jadx", "v1.4.7"),
        ];
        for (var, name, override_) in cases {
            std::env::set_var(var, override_);
            assert_eq!(engine_tag(name), override_);
            let url = release_api_url(engine_repo(name), &engine_tag(name));
            assert!(url.ends_with(&format!("/releases/tags/{override_}")), "{url}");
            std::env::remove_var(var);
        }
    }

    // ── (b2) FreeJ2ME-plus pin ──────────────────────────────────────────────
    #[test]
    fn freej2me_plus_pinned_by_default() {
        let _g = clean_env();
        assert_eq!(freej2me_version(), "1.52");
        assert_eq!(FREEJ2ME_REPO, "TASEmulators/freej2me-plus");
        let url = release_api_url(FREEJ2ME_REPO, &freej2me_version());
        assert_eq!(
            url,
            "https://api.github.com/repos/TASEmulators/freej2me-plus/releases/tags/1.52"
        );
        assert!(!url.contains("releases/latest"));
    }

    #[test]
    fn freej2me_plus_env_override_is_honored() {
        let _g = clean_env();
        std::env::set_var("ARLECCHINO_FREEJ2ME_PLUS_VERSION", "1.51");
        assert_eq!(freej2me_version(), "1.51");
        let url = release_api_url(FREEJ2ME_REPO, &freej2me_version());
        std::env::remove_var("ARLECCHINO_FREEJ2ME_PLUS_VERSION");
        assert!(url.ends_with("/releases/tags/1.51"), "{url}");
    }

    #[test]
    fn freej2me_plus_asset_predicate() {
        // the jar bundle of the pinned tag must match; the native libretro core must not
        assert!(freej2me_asset_pred("freej2me-v1.52.zip"));
        assert!(!freej2me_asset_pred("freej2me_libretro-v1.52.zip"));
        assert!(!freej2me_asset_pred("freej2me_libretro.info"));
        // both frontends are extracted from the bundle
        assert_eq!(FREEJ2ME_JARS, &["freej2me.jar", "freej2me-lr.jar"]);
    }

    // ── (c) GITHUB_TOKEN -> Authorization header ────────────────────────────
    #[test]
    fn no_token_means_anonymous() {
        let _g = clean_env();
        let req = gh_request("https://api.github.com/x", Duration::from_secs(1));
        assert!(req.header("Authorization").is_none());
        assert_eq!(req.header("User-Agent"), Some("arlecchino-setup"));
    }

    #[test]
    fn github_token_adds_bearer_header() {
        let _g = clean_env();
        std::env::set_var("GITHUB_TOKEN", "ghp_secret123");
        let req = gh_request("https://api.github.com/x", Duration::from_secs(1));
        std::env::remove_var("GITHUB_TOKEN");
        assert_eq!(req.header("Authorization"), Some("Bearer ghp_secret123"));
    }

    #[test]
    fn gh_token_alias_also_works() {
        let _g = clean_env();
        std::env::set_var("GH_TOKEN", "gho_alias456");
        let req = gh_request("https://api.github.com/x", Duration::from_secs(1));
        std::env::remove_var("GH_TOKEN");
        assert_eq!(req.header("Authorization"), Some("Bearer gho_alias456"));
    }

    /// The header is actually attached to the request that goes out: `gh_request`
    /// is the single construction point behind both http_get_json and download, so
    /// this covers the real request without any network I/O.
    #[test]
    fn token_header_reaches_http_request() {
        let _g = clean_env();
        std::env::set_var("GITHUB_TOKEN", "ghp_zzz");
        let url = release_api_url("leibnitz27/cfr", &engine_tag("cfr"));
        let req = gh_request(&url, Duration::from_secs(1));
        std::env::remove_var("GITHUB_TOKEN");
        assert_eq!(req.header("Authorization"), Some("Bearer ghp_zzz"));
        assert!(req.url().ends_with("/releases/tags/0.152"), "{}", req.url());
    }
}
