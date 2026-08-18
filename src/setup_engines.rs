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

// ── GitHub release helpers ───────────────────────────────────────────────────
fn http_get_json(url: &str) -> Result<serde_json::Value, String> {
    let resp = ureq::get(url)
        .set("User-Agent", UA)
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(|e| e.to_string())?;
    resp.into_json::<serde_json::Value>().map_err(|e| e.to_string())
}

fn download(url: &str, dst: &Path) -> Result<(), String> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let resp = ureq::get(url)
        .set("User-Agent", UA)
        .timeout(Duration::from_secs(300))
        .call()
        .map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader();
    let mut f = std::fs::File::create(dst).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut f).map_err(|e| e.to_string())?;
    Ok(())
}

/// (asset_name, browser_download_url) of the first release asset matching `pred`.
fn latest_asset(repo: &str, pred: impl Fn(&str) -> bool) -> Result<Option<(String, String)>, String> {
    let data = http_get_json(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
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
            a.starts_with("procyon-decompiler") && a.ends_with(".jar") && !a.contains("sources")
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
        let status = (|| -> String {
            let asset = match latest_asset(repo, engine_pred(name)) {
                Ok(Some(a)) => a,
                Ok(None) => return "no matching asset".into(),
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
                "ok (jadx/bin/jadx)".into()
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
                "ok (jd-cli.jar)".into()
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
                format!("ok ({})", target.file_name().unwrap().to_string_lossy())
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
fn download_freej2me_release(dst: &Path) -> Option<String> {
    let rels = http_get_json(&format!("https://api.github.com/repos/{FREEJ2ME_REPO}/releases")).ok()?;
    let arr = rels.as_array()?;
    let mut url: Option<String> = None;
    'outer: for rel in arr {
        if let Some(assets) = rel.get("assets").and_then(|a| a.as_array()) {
            for a in assets {
                let n = a.get("name").and_then(|x| x.as_str()).unwrap_or("");
                if n.ends_with(".zip") && !n.contains("libretro") {
                    url = a
                        .get("browser_download_url")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                    break 'outer;
                }
            }
        }
    }
    let url = url?;
    let ztmp = dst.parent()?.join("_fj_release.zip");
    println!(
        "  [freej2me] downloading prebuilt {} ...",
        url.rsplit('/').next().unwrap_or("")
    );
    download(&url, &ztmp).ok()?;
    // find the freej2me.jar member and extract it to dst
    let names = util::zip_namelist(&ztmp).ok()?;
    let member = names.into_iter().find(|m| m.ends_with("freej2me.jar"))?;
    if let Some(p) = dst.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let bytes = util::zip_read_entry(&ztmp, &member).ok()?;
    std::fs::write(dst, bytes).ok()?;
    let _ = std::fs::remove_file(&ztmp);
    Some(format!("ok (prebuilt release -> {})", dst.display()))
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

    if src_dir.is_none() {
        if let Some(rel) = download_freej2me_release(&dst) {
            return rel;
        }
    }

    let mut work = root.join("freej2me-src");
    if let Some(sd) = src_dir {
        if sd.exists() {
            work = sd.to_path_buf();
        }
    } else if !work.exists() {
        println!("  [freej2me] cloning TASEmulators/freej2me-plus ...");
        let argv = vec![
            "git".to_string(),
            "clone".into(),
            "--depth".into(),
            "1".into(),
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
