//! Application-menu launcher — freedesktop `.desktop` + icon, written to the user's
//! XDG dirs (NO root):
//!   - `~/.local/share/applications/arlecchino-gui.desktop`
//!   - `~/.local/share/icons/hicolor/scalable/apps/arlecchino.svg`
//!   - `~/.local/share/icons/hicolor/{16x16,24x24}/apps/arlecchino.svg` — the
//!     small-size build of the same artwork (see [`ICON_SVG_SMALL`]).
//!
//! `Exec` is the absolute path to the GUI binary. Name/Comment ship in English plus a
//! Russian `[ru]` translation (project rule), worded for someone who has never read the
//! documentation.
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

/// Icon-theme sizes that get the simplified artwork instead of the master.
pub const SMALL_ICON_SIZES: &[u32] = &[16, 24];

/// Application icon (embedded): a gold venetian half-mask with the harlequin lozenge.
/// To change the artwork, replace `assets/icon.svg` and rebuild, or swap the installed
/// file in place.
pub const ICON_SVG: &str = include_str!("../assets/icon.svg");

/// The same icon with the engraving stripped, for [`SMALL_ICON_SIZES`]: below ~32 px a
/// whole ornament falls under one pixel and only muddies the silhouette.
pub const ICON_SVG_SMALL: &str = include_str!("../assets/icon-small.svg");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherPaths {
    pub desktop: PathBuf,
    pub icon: PathBuf,
    /// Small-size copies, one per [`SMALL_ICON_SIZES`] entry.
    pub small_icons: Vec<PathBuf>,
}

pub fn applications_dir(home: &Path) -> PathBuf {
    home.join(".local/share/applications")
}

pub fn icons_dir(home: &Path) -> PathBuf {
    home.join(".local/share/icons/hicolor/scalable/apps")
}

/// `~/.local/share/icons/hicolor/<px>x<px>/apps`
pub fn sized_icons_dir(home: &Path, px: u32) -> PathBuf {
    home.join(format!(".local/share/icons/hicolor/{px}x{px}/apps"))
}

/// Every icon file the launcher installs, in install order.
pub fn icon_paths(home: &Path) -> Vec<PathBuf> {
    let mut v = vec![icons_dir(home).join(ICON_BASENAME)];
    v.extend(SMALL_ICON_SIZES.iter().map(|px| sized_icons_dir(home, *px).join(ICON_BASENAME)));
    v
}

pub fn target_paths(home: &Path) -> LauncherPaths {
    LauncherPaths {
        desktop: applications_dir(home).join(DESKTOP_BASENAME),
        icon: icons_dir(home).join(ICON_BASENAME),
        small_icons: SMALL_ICON_SIZES
            .iter()
            .map(|px| sized_icons_dir(home, *px).join(ICON_BASENAME))
            .collect(),
    }
}

/// Generate the `.desktop` content. `exec` is the absolute path to the GUI binary.
pub fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=Arlecchino — Java game recovery toolkit\n\
         Name[ru]=Arlecchino — восстановление старых Java-игр\n\
         GenericName=Java/J2ME decompiler and porting toolkit\n\
         GenericName[ru]=Декомпилятор и набор для переноса Java-игр\n\
         Comment=Recovers readable Java source from old mobile (J2ME) games, repairs it and helps port them to the PC\n\
         Comment[ru]=Разбирает старые мобильные игры (Java/J2ME) до исходного кода, чинит его и помогает перенести игру на компьютер\n\
         Exec={exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Development;Java;\n\
         Keywords=decompiler;deobfuscator;java;j2me;midp;midlet;jar;emulator;game;port;reverse-engineering;\n\
         Keywords[ru]=декомпилятор;деобфускатор;ява;джава;джар;мидлет;мобильные игры;старые игры;перенос;порт;эмулятор;исходный код;\n\
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

    for small in &paths.small_icons {
        if let Some(dir) = small.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(small, ICON_SVG_SMALL)?;
    }

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
        assert!(d.contains("Name=Arlecchino — Java game recovery toolkit"));
        assert!(d.contains("Name[ru]=Arlecchino — восстановление старых Java-игр"));
        // Exactly ONE main category (plus the "Java" sub-category), otherwise the entry
        // is listed twice in the application menu.
        assert!(d.contains("Categories=Development;Java;"));
        assert!(d.contains("Terminal=false"));
    }

    /// Every user-visible field must exist in both English and Russian, and the search
    /// keywords in both alphabets — otherwise the entry is unfindable in one of them.
    #[test]
    fn desktop_entry_is_bilingual() {
        let d = desktop_entry("/home/u/.local/bin/arlecchino-gui");
        for key in ["Name", "GenericName", "Comment", "Keywords"] {
            assert!(d.contains(&format!("\n{key}=")), "missing {key}=");
            assert!(d.contains(&format!("\n{key}[ru]=")), "missing {key}[ru]=");
        }
        assert!(d.contains("декомпилятор"), "no Cyrillic keywords");
        assert!(d.contains("decompiler"), "no Latin keywords");
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
        assert_eq!(
            p.small_icons,
            vec![
                PathBuf::from("/home/u/.local/share/icons/hicolor/16x16/apps/arlecchino.svg"),
                PathBuf::from("/home/u/.local/share/icons/hicolor/24x24/apps/arlecchino.svg"),
            ]
        );
        assert_eq!(icon_paths(home).len(), 1 + SMALL_ICON_SIZES.len());
    }

    #[test]
    fn small_build_is_simpler_than_the_master() {
        // Both are self-contained SVGs of the same artwork; the small one must not carry
        // the engraving (that is the whole point of shipping it separately).
        assert!(ICON_SVG.contains("<svg") && ICON_SVG_SMALL.contains("<svg"));
        assert!(ICON_SVG_SMALL.len() < ICON_SVG.len());
        for svg in [ICON_SVG, ICON_SVG_SMALL] {
            assert!(!svg.contains("base64"), "icon must not embed raster data");
            assert!(!svg.contains("font-family"), "icon must not depend on fonts");
            assert!(!svg.contains("xlink:href"), "icon must not have external references");
        }
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
        for small in &paths.small_icons {
            assert!(small.exists(), "missing {}", small.display());
            assert!(std::fs::read_to_string(small).unwrap().contains("<svg"));
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
