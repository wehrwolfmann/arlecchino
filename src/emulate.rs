//! `arlecchino emulate <jar>`: run a J2ME/MIDP jar in FreeJ2ME.
//!
//! FreeJ2ME's AWT front-end takes the midlet jar as its first argument:
//!     java -jar freej2me.jar <midlet.jar> [width] [height] [scale]
//! This is a thin wrapper — FreeJ2ME does the emulation.

use crate::config::engines_root_default;
use crate::util;
use std::path::{Path, PathBuf};

pub fn find_freej2me(explicit: Option<&str>, root: Option<&Path>) -> Option<PathBuf> {
    if let Some(e) = explicit {
        let p = PathBuf::from(e);
        if p.exists() {
            return Some(p);
        }
    }
    let root = root
        .map(|r| r.to_path_buf())
        .unwrap_or_else(engines_root_default);
    [
        root.join("freej2me.jar"),
        root.join("freej2me-src").join("build").join("freej2me.jar"),
        root.join("fj2me").join("build").join("freej2me.jar"),
    ]
    .into_iter()
    .find(|cand| cand.exists())
}

/// The automatic API classpath `javac` compiles decompiled sources against.
///
/// FreeJ2ME is the base: it carries the widest set of vendor APIs (Nokia, Siemens,
/// Samsung, ...).  But it is an emulator, not a reference ABI, and some of its
/// standard types drop checked exceptions the specification declares — e.g. its
/// `javax.microedition.media.Player.stop()` has no `throws MediaException`, so a
/// faithful `try { p.stop(); } catch (MediaException e)` fails with "exception
/// MediaException is never thrown" (Silent Storm, every engine).  When midp-rt (our
/// MicroEmulator fork, spec-faithful signatures) is installed, it goes FIRST, so the
/// standard `javax.microedition.*` types resolve from it and everything it lacks still
/// comes from FreeJ2ME.  An explicit `--midp-stub` is used alone, as given.
pub fn auto_api_classpath(explicit: Option<&str>, root: &Path) -> Option<String> {
    let fj = find_freej2me(explicit, Some(root));
    if explicit.is_some() && fj.is_some() {
        return fj.map(|p| p.to_string_lossy().into_owned());
    }
    let rt = crate::midprun::find_midp_rt(None, Some(root));
    let parts: Vec<String> = [rt, fj]
        .into_iter()
        .flatten()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(":"))
    }
}

/// Find the jar of the FreeJ2ME-plus LIBRETRO frontend (`freej2me-lr.jar`) — the very
/// stdin/stdout-channel frontend `diffrun` uses to capture frames headless
/// (block A). Installed next to freej2me.jar by `arlecchino setup`.
pub fn find_freej2me_lr(explicit: Option<&str>, root: Option<&Path>) -> Option<PathBuf> {
    if let Some(e) = explicit {
        let p = PathBuf::from(e);
        if p.exists() {
            return Some(p);
        }
    }
    let root = root
        .map(|r| r.to_path_buf())
        .unwrap_or_else(engines_root_default);
    [
        root.join("freej2me-lr.jar"),
        root.join("freej2me-src").join("build").join("freej2me-lr.jar"),
    ]
    .into_iter()
    .find(|cand| cand.exists())
}

pub fn emulate(
    jar: &Path,
    freej2me: &Path,
    width: i64,
    height: i64,
    scale: i64,
    extra: Option<&[String]>,
) -> i32 {
    let java = util::tool("ARLECCHINO_JAVA", "java");
    let mut argv = vec![
        java,
        "-jar".into(),
        freej2me.to_string_lossy().into_owned(),
        jar.to_string_lossy().into_owned(),
        width.to_string(),
        height.to_string(),
        scale.to_string(),
    ];
    if let Some(ex) = extra {
        argv.extend(ex.iter().cloned());
    }
    println!("  launching: {}", argv.join(" "));
    util::run_inherit(&argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_api_classpath_puts_midp_rt_before_freej2me() {
        if std::env::var("ARLECCHINO_MIDP_RT").map(|v| !v.is_empty()).unwrap_or(false) {
            return; // an explicit override would win over the engines root
        }
        let root = crate::util::make_temp_dir("arl_apicp_").unwrap();
        let fj = root.join("freej2me.jar");
        let rt = root.join("midp-rt.jar");
        std::fs::write(&fj, b"x").unwrap();
        std::fs::write(&rt, b"x").unwrap();
        let cp = auto_api_classpath(None, &root).unwrap();
        assert_eq!(cp, format!("{}:{}", rt.display(), fj.display()));
        // an explicit --midp-stub is used alone
        let only = auto_api_classpath(Some(fj.to_str().unwrap()), &root).unwrap();
        assert_eq!(only, fj.display().to_string());
        let _ = std::fs::remove_dir_all(&root);
    }
}
