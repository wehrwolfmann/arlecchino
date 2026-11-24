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

/// How locals are handled on top of the plan.
#[derive(Debug, Clone, Default)]
pub struct LocalOptions {
    /// give every unnamed parameter/local its label (`v4`) in the bytecode
    pub label_all: bool,
    /// where the applier writes the label table (see [`super::mapfile::read_labels`])
    pub labels_out: Option<PathBuf>,
}

/// Write the table next to `out_jar` and rewrite `jar` -> `out_jar`.
pub fn apply(plan: &Plan, jar: &Path, out_jar: &Path, root: Option<&Path>) -> Result<String, String> {
    apply_with(plan, jar, out_jar, root, &LocalOptions::default())
}

/// [`apply`] with local-variable options.  Messages about labels that a map names
/// but the bytecode does not have come back after the summary line.
pub fn apply_with(plan: &Plan, jar: &Path, out_jar: &Path, root: Option<&Path>, lo: &LocalOptions) -> Result<String, String> {
    let dir = ensure_helper(root)?;
    let table = out_jar.with_extension("table.tsv");
    let mut text = plan.applier_table();
    if lo.label_all || lo.labels_out.is_some() || !plan.locals.is_empty() {
        for p in rename::resolver_classpath(None).split(PATHSEP).filter(|p| !p.is_empty()) {
            text.push_str(&format!("classpath\t{p}\n"));
        }
    }
    if lo.label_all {
        text.push_str("labels\ton\n");
    }
    if let Some(p) = &lo.labels_out {
        text.push_str(&format!("labels-out\t{}\n", p.display()));
    }
    std::fs::write(&table, text).map_err(|e| format!("{}: {e}", table.display()))?;
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
    let warnings: Vec<&str> = r.stderr.lines().filter(|l| l.starts_with("warning:")).collect();
    let mut msg = r.stdout.trim().to_string();
    for w in warnings {
        msg.push('\n');
        msg.push_str(w);
    }
    Ok(msg)
}
