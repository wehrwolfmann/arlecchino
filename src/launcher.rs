//! Application-menu launcher — freedesktop `.desktop` + icon, written to the user's
//! XDG dirs (NO root):
//!   - `~/.local/share/applications/arlecchino-gui.desktop`
//!   - `~/.local/share/icons/hicolor/scalable/apps/arlecchino.svg`
//!
//! `Exec` is the absolute path to the GUI binary; `Name` is Russian (project rule).
//! `StartupWMClass` matches the eframe `app_id` so the window maps to this launcher.
//!
//! Content generation (`desktop_entry`) is separated from disk I/O (`install`) so the
//! former is testable without a filesystem.

use std::path::{Path, PathBuf};

/// `.desktop` file basename.
pub const DESKTOP_BASENAME: &str = "arlecchino-gui.desktop";
/// Icon basename (file under hicolor/scalable/apps).
pub const ICON_BASENAME: &str = "arlecchino.svg";
/// Icon name used in the `.desktop` `Icon=` field (basename without extension).
pub const ICON_NAME: &str = "arlecchino";
/// Window class = `StartupWMClass` = eframe `app_id`.
pub const WM_CLASS: &str = "arlecchino-gui";

/// PLACEHOLDER application icon (embedded). Replace `assets/icon.svg` with the real
/// artwork and rebuild, or swap the installed file in place.
pub const ICON_SVG: &str = include_str!("../assets/icon.svg");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherPaths {
    pub desktop: PathBuf,
    pub icon: PathBuf,
}

pub fn applications_dir(home: &Path) -> PathBuf {
    home.join(".local/share/applications")
}

pub fn icons_dir(home: &Path) -> PathBuf {
    home.join(".local/share/icons/hicolor/scalable/apps")
}

pub fn target_paths(home: &Path) -> LauncherPaths {
    LauncherPaths {
        desktop: applications_dir(home).join(DESKTOP_BASENAME),
        icon: icons_dir(home).join(ICON_BASENAME),
    }
}

/// Generate the `.desktop` content. `exec` is the absolute path to the GUI binary.
pub fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=Arlecchino — мета-декомпилятор\n\
         GenericName=Набор для декомпиляции и порта J2ME\n\
         Comment=Мета-декомпиляция Java/J2ME: слияние лучших результатов декомпиляторов, авто-фикс, round-trip, эмуляция\n\
         Exec={exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Utility;Development;\n\
         Keywords=decompiler;java;j2me;midp;jar;декомпилятор;\n\
         StartupNotify=true\n\
         StartupWMClass={wm}\n",
        exec = exec,
        icon = ICON_NAME,
        wm = WM_CLASS,
    )
}

/// Install the launcher: write the `.desktop` + icon into the user's XDG dirs.
pub fn install(home: &Path, exec: &Path) -> std::io::Result<LauncherPaths> {
    let paths = target_paths(home);
    let exec_str = exec.to_string_lossy();

    if let Some(dir) = paths.icon.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&paths.icon, ICON_SVG)?;

    if let Some(dir) = paths.desktop.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&paths.desktop, desktop_entry(&exec_str))?;
    Ok(paths)
}

/// Absolute path to the current executable (for `Exec=`), or the WM class as fallback.
pub fn current_exe_string() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| WM_CLASS.to_string())
}

pub fn home_dir() -> PathBuf {
    crate::util::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_has_exec_icon_wmclass() {
        let d = desktop_entry("/home/u/.local/bin/arlecchino-gui");
        assert!(d.contains("[Desktop Entry]"));
        assert!(d.contains("Type=Application"));
        assert!(d.contains("Exec=/home/u/.local/bin/arlecchino-gui"));
        assert!(d.contains("Icon=arlecchino"));
        assert!(d.contains("StartupWMClass=arlecchino-gui"));
        assert!(d.contains("Name=Arlecchino — мета-декомпилятор"));
        assert!(d.contains("Categories=Utility;Development;"));
        assert!(d.contains("Terminal=false"));
    }

    #[test]
    fn target_paths_under_xdg_user_dirs() {
        let home = Path::new("/home/u");
        let p = target_paths(home);
        assert_eq!(
            p.desktop,
            Path::new("/home/u/.local/share/applications/arlecchino-gui.desktop")
        );
        assert_eq!(
            p.icon,
            Path::new("/home/u/.local/share/icons/hicolor/scalable/apps/arlecchino.svg")
        );
    }

    #[test]
    fn install_writes_both_files_to_temp() {
        let tmp = std::env::temp_dir().join(format!("arl-launcher-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let exec = Path::new("/opt/arlecchino/arlecchino-gui");
        let paths = install(&tmp, exec).expect("install ok");
        assert!(paths.desktop.exists());
        assert!(paths.icon.exists());
        let d = std::fs::read_to_string(&paths.desktop).unwrap();
        assert!(d.contains("Exec=/opt/arlecchino/arlecchino-gui"));
        let svg = std::fs::read_to_string(&paths.icon).unwrap();
        assert!(svg.contains("<svg"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
