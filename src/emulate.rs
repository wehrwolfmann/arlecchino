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

/// Найти jar LIBRETRO-фронтенда FreeJ2ME-plus (`freej2me-lr.jar`) — тот самый
/// фронтенд на канале stdin/stdout, которым `diffrun` снимает кадры безголово
/// (блок A). Ставится рядом с freej2me.jar командой `arlecchino setup`.
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
