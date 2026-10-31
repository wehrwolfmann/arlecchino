//! Runs the ASM `NameApplier` helper: plan -> renamed jar.
//!
//! The helper source is embedded and compiled on demand against the same ASM jars
//! `arlecchino setup` installs for the member renamer, into its own class directory
//! (recompiled whenever the embedded source changes).

use super::names::Plan;
use crate::rename;
use crate::util::{self, PATHSEP};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const APPLIER_CLASS: &str = "NameApplier";
pub const APPLIER_SOURCE: &str = include_str!("../../assets/NameApplier.java");

fn classes_dir(root: Option<&Path>) -> PathBuf {
    rename::asm_dir(root).join("_ai_classes")
}

/// Compile the helper if it is missing or stale.  `Err` carries a human reason.
pub fn ensure_helper(root: Option<&Path>) -> Result<PathBuf, String> {
    let jars = rename::asm_jars(root);
    if jars.is_empty() {
        return Err("ASM jars unavailable — run `arlecchino setup`".into());
    }
    let dir = classes_dir(root);
    let src = dir.join(format!("{APPLIER_CLASS}.java"));
    let class = dir.join(format!("{APPLIER_CLASS}.class"));
    let fresh = class.exists() && std::fs::read_to_string(&src).map(|s| s == APPLIER_SOURCE).unwrap_or(false);
    if fresh {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(&src, APPLIER_SOURCE).map_err(|e| format!("{}: {e}", src.display()))?;
    let cp = jars.iter().map(|j| j.to_string_lossy().into_owned()).collect::<Vec<_>>().join(PATHSEP);
    let argv = vec![
        util::tool("ARLECCHINO_JAVAC", "javac"),
        "-cp".into(),
        cp,
        "-d".into(),
        dir.to_string_lossy().into_owned(),
        src.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture_in(&argv, Some(Duration::from_secs(300)), Some(&dir));
    if r.status != Some(0) || !class.exists() {
        let _ = std::fs::remove_file(&src); // force a retry next time
        return Err(format!("javac of {APPLIER_CLASS} failed: {}", r.stderr.trim()));
    }
    Ok(dir)
}

/// Write the table next to `out_jar` and rewrite `jar` -> `out_jar`.
pub fn apply(plan: &Plan, jar: &Path, out_jar: &Path, root: Option<&Path>) -> Result<String, String> {
    let dir = ensure_helper(root)?;
    let table = out_jar.with_extension("table.tsv");
    std::fs::write(&table, plan.applier_table()).map_err(|e| format!("{}: {e}", table.display()))?;
    let mut cp: Vec<String> = rename::asm_jars(root).iter().map(|j| j.to_string_lossy().into_owned()).collect();
    cp.push(dir.to_string_lossy().into_owned());
    let argv = vec![
        util::tool("ARLECCHINO_JAVA", "java"),
        "-cp".into(),
        cp.join(PATHSEP),
        APPLIER_CLASS.into(),
        jar.to_string_lossy().into_owned(),
        out_jar.to_string_lossy().into_owned(),
        table.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture(&argv, Some(Duration::from_secs(600)));
    if let Some(e) = r.spawn_err {
        return Err(format!("cannot start java: {e}"));
    }
    if r.status != Some(0) || !out_jar.exists() {
        return Err(format!("{APPLIER_CLASS} failed: {}", r.stderr.trim()));
    }
    Ok(r.stdout.trim().to_string())
}
