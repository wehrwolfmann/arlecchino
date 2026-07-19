//! Uninstall — remove Arlecchino's user-installed files, behind an Executor trait so
//! the destructive step is mockable and never runs during tests.
//!
//! What is removed (all under `$HOME`, no root):
//!   1. installed binaries in `~/.local/bin` (arlecchino, arlecchino-gui);
//!   2. the launcher `.desktop` + its icon;
//!   3. OPTIONAL (purge): the engines cache `~/.arlecchino` and GUI config
//!      `~/.config/arlecchino`.
//!
//! Layer split (so everything is testable without risk):
//!   - [`Layout`]   — where things live (built from `$HOME`);
//!   - [`plan`]     — PURELY computes the target list from the purge flag;
//!   - [`Executor`] — trait "do it": remove file / remove dir. Real + mock;
//!   - [`execute`]  — runs a plan through an executor.
//!
//! The UI must confirm ("Точно удалить?") before calling [`execute`] with the real
//! executor.  The module itself does nothing until then.

use crate::launcher;
use std::path::{Path, PathBuf};

/// Installed binaries under `~/.local/bin`.
pub const BINARIES: &[&str] = &["arlecchino", "arlecchino-gui"];

#[derive(Debug, Clone)]
pub struct Layout {
    /// `~/.local/bin`
    pub bin_dir: PathBuf,
    /// `~/.local/share/applications`
    pub app_dir: PathBuf,
    /// `~/.local/share/icons/hicolor/scalable/apps/arlecchino.svg`
    pub icon_path: PathBuf,
    /// `~/.arlecchino` (downloaded engines cache)
    pub engines_dir: PathBuf,
    /// `~/.config/arlecchino` (GUI config)
    pub gui_config_dir: PathBuf,
}

impl Layout {
    pub fn from_home(home: &Path) -> Self {
        Layout {
            bin_dir: home.join(".local/bin"),
            app_dir: home.join(".local/share/applications"),
            icon_path: home.join(".local/share/icons/hicolor/scalable/apps/arlecchino.svg"),
            engines_dir: home.join(".arlecchino"),
            gui_config_dir: home.join(".config/arlecchino"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallPlan {
    pub files: Vec<PathBuf>,
    pub dirs: Vec<PathBuf>,
}

/// Compute the uninstall plan. Pure — removes nothing.
///
/// * `purge` — also remove the engines cache and GUI config directories.
pub fn plan(layout: &Layout, purge: bool) -> UninstallPlan {
    let mut files = Vec::new();
    for b in BINARIES {
        files.push(layout.bin_dir.join(b));
    }
    files.push(layout.app_dir.join(launcher::DESKTOP_BASENAME));
    files.push(layout.icon_path.clone());

    let mut dirs = Vec::new();
    if purge {
        dirs.push(layout.engines_dir.clone());
        dirs.push(layout.gui_config_dir.clone());
    }
    UninstallPlan { files, dirs }
}

/// Executor of destructive steps. Real touches the filesystem; mock does not.
pub trait Executor {
    fn remove_file(&self, path: &Path) -> Result<(), String>;
    fn remove_dir_all(&self, path: &Path) -> Result<(), String>;
}

/// Run a plan through an executor. Returns the number of items removed.
pub fn execute(plan: &UninstallPlan, exec: &dyn Executor) -> Result<usize, String> {
    let mut removed = 0usize;
    for f in &plan.files {
        exec.remove_file(f)?;
        removed += 1;
    }
    for d in &plan.dirs {
        exec.remove_dir_all(d)?;
        removed += 1;
    }
    Ok(removed)
}

/// Real executor: actually deletes files/dirs (missing target is not an error).
///
/// Invoked ONLY after explicit user confirmation in the UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealExecutor;

impl Executor for RealExecutor {
    fn remove_file(&self, path: &Path) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
        }
    }
    fn remove_dir_all(&self, path: &Path) -> Result<(), String> {
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove dir {}: {e}", path.display())),
        }
    }
}

pub fn home_dir() -> PathBuf {
    crate::util::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn plan_targets_binaries_desktop_icon() {
        let home = Path::new("/home/u");
        let layout = Layout::from_home(home);
        let p = plan(&layout, false);
        assert!(p.files.contains(&home.join(".local/bin/arlecchino")));
        assert!(p.files.contains(&home.join(".local/bin/arlecchino-gui")));
        assert!(p.files.contains(&home.join(".local/share/applications/arlecchino-gui.desktop")));
        assert!(p.files.contains(
            &home.join(".local/share/icons/hicolor/scalable/apps/arlecchino.svg")
        ));
        assert!(p.dirs.is_empty());
    }

    #[test]
    fn plan_purge_adds_cache_dirs() {
        let layout = Layout::from_home(Path::new("/home/u"));
        let p = plan(&layout, true);
        assert!(p.dirs.contains(&Path::new("/home/u/.arlecchino").to_path_buf()));
        assert!(p.dirs.contains(&Path::new("/home/u/.config/arlecchino").to_path_buf()));
    }

    #[test]
    fn all_targets_stay_under_home() {
        let home = Path::new("/home/u");
        let layout = Layout::from_home(home);
        let p = plan(&layout, true);
        for f in p.files.iter().chain(p.dirs.iter()) {
            assert!(f.starts_with(home), "target {} escapes HOME", f.display());
        }
    }

    #[derive(Default)]
    struct MockExec {
        calls: RefCell<Vec<String>>,
    }
    impl Executor for MockExec {
        fn remove_file(&self, path: &Path) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("rm {}", path.display()));
            Ok(())
        }
        fn remove_dir_all(&self, path: &Path) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("rmdir {}", path.display()));
            Ok(())
        }
    }

    #[test]
    fn execute_removes_targets_via_mock() {
        let layout = Layout::from_home(Path::new("/home/u"));
        let p = plan(&layout, true);
        let m = MockExec::default();
        let removed = execute(&p, &m).unwrap();
        let calls = m.calls.borrow();
        assert!(calls.contains(&"rm /home/u/.local/bin/arlecchino-gui".to_string()));
        assert!(calls.contains(&"rmdir /home/u/.arlecchino".to_string()));
        assert_eq!(removed, p.files.len() + p.dirs.len());
    }

    #[test]
    fn execute_against_temp_removes_only_temp_files() {
        let tmp = std::env::temp_dir().join(format!("arl-uninstall-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let layout = Layout::from_home(&tmp);
        std::fs::create_dir_all(&layout.bin_dir).unwrap();
        std::fs::create_dir_all(&layout.app_dir).unwrap();
        std::fs::create_dir_all(layout.icon_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&layout.engines_dir).unwrap();
        std::fs::write(layout.bin_dir.join("arlecchino-gui"), b"x").unwrap();
        std::fs::write(layout.app_dir.join("arlecchino-gui.desktop"), b"x").unwrap();
        std::fs::write(&layout.icon_path, b"x").unwrap();

        let p = plan(&layout, true);
        execute(&p, &RealExecutor).unwrap();
        assert!(!layout.bin_dir.join("arlecchino-gui").exists());
        assert!(!layout.app_dir.join("arlecchino-gui.desktop").exists());
        assert!(!layout.icon_path.exists());
        assert!(!layout.engines_dir.exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
