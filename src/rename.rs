//! Bytecode-first canonical member renamer (orchestration side).
//!
//! ProGuard-style obfuscation reuses the SAME member name with DIFFERENT descriptors
//! (legal in JVM bytecode, ILLEGAL in Java source), making decompiler output
//! uncompilable.  This module runs the ASM-based `MemberRenamer` Java helper (shipped
//! embedded, materialised at setup time) BEFORE decompilation.  The renamed jar is
//! then fed to the normal pipeline and the output compiles.
//!
//! Everything degrades gracefully: if ASM jars or the compiled helper are unavailable,
//! `maybe_rename` logs and returns the original jar so the caller can fall back.

use crate::config::engines_root_default;
use crate::midp_stub;
use crate::util::{self, PATHSEP};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const ASM_SUBDIR: &str = "asm";
pub const HELPER_CLASS: &str = "MemberRenamer";
pub const HELPER_CLASSES_SUBDIR: &str = "_classes";

/// The ASM member-renamer helper source (original work), embedded in the binary.
pub const HELPER_SOURCE: &str = include_str!("../assets/MemberRenamer.java");

fn candidate_roots(root: Option<&Path>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let default = engines_root_default();
    for p in [root.map(|p| p.to_path_buf()), Some(default)].into_iter().flatten() {
        if !roots.contains(&p) {
            roots.push(p);
        }
    }
    roots
}

fn glob_asm_jars(dir: &Path) -> Vec<PathBuf> {
    let mut jars: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("asm") && name.ends_with(".jar") {
                jars.push(e.path());
            }
        }
    }
    jars.sort();
    jars
}

pub fn asm_dir(root: Option<&Path>) -> PathBuf {
    for r in candidate_roots(root) {
        let d = r.join(ASM_SUBDIR);
        if d.is_dir() && !glob_asm_jars(&d).is_empty() {
            return d;
        }
    }
    root.map(|p| p.to_path_buf())
        .unwrap_or_else(engines_root_default)
        .join(ASM_SUBDIR)
}

pub fn asm_jars(root: Option<&Path>) -> Vec<PathBuf> {
    for r in candidate_roots(root) {
        let d = r.join(ASM_SUBDIR);
        if d.is_dir() {
            let jars = glob_asm_jars(&d);
            if !jars.is_empty() {
                return jars;
            }
        }
    }
    Vec::new()
}

pub fn helper_classes_dir(root: Option<&Path>) -> PathBuf {
    asm_dir(root).join(HELPER_CLASSES_SUBDIR)
}

pub fn helper_compiled(root: Option<&Path>) -> bool {
    helper_classes_dir(root)
        .join(format!("{HELPER_CLASS}.class"))
        .exists()
}

pub fn available(root: Option<&Path>) -> bool {
    !asm_jars(root).is_empty() && helper_compiled(root)
}

/// javac-compile the embedded MemberRenamer source against the ASM jars (idempotent).
pub fn compile_helper(root: Option<&Path>) -> String {
    let jars = asm_jars(root);
    if jars.is_empty() {
        return "skip (no ASM jars; run setup with network access)".into();
    }
    let dir = asm_dir(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return format!("FAILED (cannot create {})", dir.display());
    }
    let src = dir.join(format!("{HELPER_CLASS}.java"));
    if std::fs::write(&src, HELPER_SOURCE).is_err() {
        return format!("FAILED (cannot write helper source: {})", src.display());
    }
    let outdir = helper_classes_dir(root);
    let _ = std::fs::create_dir_all(&outdir);
    let cp = jars
        .iter()
        .map(|j| j.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(PATHSEP);
    let javac = util::tool("ARLECCHINO_JAVAC", "javac");
    let argv = vec![
        javac,
        "-cp".into(),
        cp,
        "-d".into(),
        outdir.to_string_lossy().into_owned(),
        src.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture_in(&argv, None, Some(&outdir));
    if r.status != Some(0) || !helper_compiled(root) {
        let tail: String = r.stderr.chars().rev().take(500).collect::<String>().chars().rev().collect();
        return format!("FAILED javac: {tail}");
    }
    format!("ok (compiled -> {})", outdir.display())
}

// ── pure classfile collision pre-check ───────────────────────────────────────
fn u16(data: &[u8], off: usize) -> Option<usize> {
    if off + 2 > data.len() {
        return None;
    }
    Some(((data[off] as usize) << 8) | data[off + 1] as usize)
}
fn u32(data: &[u8], off: usize) -> Option<usize> {
    if off + 4 > data.len() {
        return None;
    }
    Some(((data[off] as usize) << 24)
        | ((data[off + 1] as usize) << 16)
        | ((data[off + 2] as usize) << 8)
        | data[off + 3] as usize)
}

/// Return ({index: utf8}, offset_after_pool).
fn parse_constant_pool(data: &[u8]) -> Option<(HashMap<usize, String>, usize)> {
    let count = u16(data, 8)?;
    let mut cp: HashMap<usize, String> = HashMap::new();
    let mut off = 10usize;
    let mut i = 1usize;
    while i < count {
        let tag = *data.get(off)?;
        off += 1;
        match tag {
            1 => {
                let ln = u16(data, off)?;
                off += 2;
                if off + ln > data.len() {
                    return None;
                }
                cp.insert(i, String::from_utf8_lossy(&data[off..off + ln]).into_owned());
                off += ln;
            }
            5 | 6 => {
                off += 8;
                i += 1;
            }
            7 | 8 | 16 | 19 | 20 => off += 2,
            15 => off += 3,
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => off += 4,
            _ => return None,
        }
        i += 1;
    }
    Some((cp, off))
}

/// Read one field_info/method_info. Returns (new_off, name, descriptor).
fn read_member(data: &[u8], mut off: usize, cp: &HashMap<usize, String>) -> Option<(usize, String, String)> {
    off += 2; // access_flags
    let name = cp.get(&u16(data, off)?).cloned().unwrap_or_default();
    off += 2;
    let desc = cp.get(&u16(data, off)?).cloned().unwrap_or_default();
    off += 2;
    let natt = u16(data, off)?;
    off += 2;
    for _ in 0..natt {
        off += 2; // attribute_name_index
        let ln = u32(data, off)?;
        off += 4;
        off += ln;
    }
    Some((off, name, desc))
}

fn class_has_collision(data: &[u8]) -> bool {
    let inner = || -> Option<bool> {
        let (cp, mut off) = parse_constant_pool(data)?;
        // access_flags(2) this_class(2) super_class(2)
        off += 6;
        let ifc = u16(data, off)?;
        off += 2;
        off += ifc * 2;
        // fields
        let mut fnames: HashSet<String> = HashSet::new();
        let nf = u16(data, off)?;
        off += 2;
        for _ in 0..nf {
            let (no, name, _desc) = read_member(data, off, &cp)?;
            off = no;
            if fnames.contains(&name) {
                return Some(true); // duplicate FIELD name -> illegal in Java
            }
            fnames.insert(name);
        }
        // methods: collision = same name+params, different return type
        let mut seen: HashMap<(String, String), HashSet<String>> = HashMap::new();
        let nm = u16(data, off)?;
        off += 2;
        for _ in 0..nm {
            let (no, name, desc) = read_member(data, off, &cp)?;
            off = no;
            if name == "<init>" || name == "<clinit>" {
                continue;
            }
            let close = desc.find(')')?;
            let params = desc[..close + 1].to_string();
            let ret = desc[close + 1..].to_string();
            let key = (name, params);
            if let Some(rets) = seen.get(&key) {
                if !rets.contains(&ret) {
                    return Some(true);
                }
            }
            seen.entry(key).or_default().insert(ret);
        }
        Some(false)
    };
    inner().unwrap_or(false)
}

/// Cheap pure check: does the jar contain Java-source-ILLEGAL member collisions?
pub fn has_member_collisions(jar: &Path) -> bool {
    let names = match util::zip_namelist(jar) {
        Ok(n) => n,
        Err(_) => return false,
    };
    for name in names {
        if !name.ends_with(".class") {
            continue;
        }
        match util::zip_read_entry(jar, &name) {
            Ok(bytes) => {
                if class_has_collision(&bytes) {
                    return true;
                }
            }
            Err(_) => return false,
        }
    }
    false
}

/// The classpath the renamer's reflection RESOLVER sees (4th argv to the helper).
///
/// Always prepends the generated clean-room MIDP/CLDC stub (Stage Э8) when present, so
/// `jdkResolvable` / `jdkDeclaresMethod` resolve `javax.microedition.*` contracts
/// PRECISELY and lock genuine framework callbacks.  The caller's `framework_cp` (e.g.
/// an auto FreeJ2ME jar that is also used for compilation) is appended too,
/// de-duplicated — but is no longer REQUIRED for correct locking.  If neither the stub
/// nor `framework_cp` is available, the helper falls back to its `MIDP_CALLBACKS`
/// denylist (offline-safe).
///
/// The stub is generated on demand: `setup` normally has it ready, but a first run on a
/// fresh machine must get the precise path too.
pub fn resolver_classpath(framework_cp: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(stub) = midp_stub::ensure_stub() {
        parts.push(stub.to_string_lossy().into_owned());
    }
    for p in framework_cp.unwrap_or("").split(PATHSEP) {
        if !p.is_empty() && !parts.iter().any(|q| q == p) {
            parts.push(p.to_string());
        }
    }
    parts.join(PATHSEP)
}

/// Invoke the ASM helper: rewrite `jar` -> `out_jar`, writing `mapping`.
///
/// The resolver classpath (the generated MIDP/CLDC stub + any `framework_cp`) lets the
/// helper RESOLVE external supertypes so it locks genuine MIDP overrides precisely.
///
/// `flatten` (OPT-IN): pass the helper's 5th `flatten` argv so it ALSO moves every
/// in-jar TYPE into the default (unnamed) package — letting a named-package class name
/// what used to be a default-package type.  Off by default; without it the produced jar
/// is byte-identical to the previous behaviour.
pub fn run_renamer(
    jar: &Path,
    out_jar: &Path,
    mapping: &Path,
    root: Option<&Path>,
    framework_cp: Option<&str>,
    flatten: bool,
) -> Option<PathBuf> {
    let jars = asm_jars(root);
    if jars.is_empty() || !helper_compiled(root) {
        println!(
            "   member-rename: ASM/helper unavailable — skipping (run `arlecchino setup` \
             with network access). Using original jar."
        );
        return None;
    }
    let mut cp_parts: Vec<String> = jars.iter().map(|j| j.to_string_lossy().into_owned()).collect();
    cp_parts.push(helper_classes_dir(root).to_string_lossy().into_owned());
    let cp = cp_parts.join(PATHSEP);
    let java = util::tool("ARLECCHINO_JAVA", "java");
    if let Some(p) = out_jar.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let mut argv = vec![
        java,
        "-cp".into(),
        cp,
        HELPER_CLASS.into(),
        jar.to_string_lossy().into_owned(),
        out_jar.to_string_lossy().into_owned(),
        mapping.to_string_lossy().into_owned(),
        resolver_classpath(framework_cp),
    ];
    if flatten {
        argv.push("flatten".into());
    }
    let r = util::run_capture(&argv, Some(Duration::from_secs(600)));
    if let Some(e) = r.spawn_err {
        println!("   member-rename: launch error ({e}); using original jar.");
        return None;
    }
    if r.status != Some(0) || !out_jar.exists() {
        let tail: String = r.stderr.trim().chars().rev().take(300).collect::<String>().chars().rev().collect();
        println!("   member-rename: FAILED ({tail}); using original jar.");
        return None;
    }
    println!("   member-rename: {}", r.stdout.trim());
    Some(out_jar.to_path_buf())
}

/// Decide-and-run the rename pass.  Returns (jar_to_use, mapping_path_or_None).
///
/// `flatten` (OPT-IN, `--flatten-packages`): also move every in-jar type into the
/// default package so named-package classes can name default-package types.  It rides
/// on the SAME ASM pass, so when `mode` decides not to run the renamer at all (off, or
/// auto with no collisions and no flatten) it has no effect.
pub fn maybe_rename(
    jar: &Path,
    workdir: &Path,
    root: Option<&Path>,
    mode: &str,
    framework_cp: Option<&str>,
    flatten: bool,
) -> (PathBuf, Option<PathBuf>) {
    if mode == "off" {
        println!("   member-rename: disabled (--no-rename-members).");
        return (jar.to_path_buf(), None);
    }

    let collisions = has_member_collisions(jar);
    let avail = available(root);

    if mode == "auto" {
        // --flatten-packages needs the ASM pass to run even on a collision-free jar
        // (the cross-package fix is independent of member collisions).
        if !collisions && !flatten {
            println!(
                "   member-rename: no source-illegal member collisions detected — \
                 skipping (not needed)."
            );
            return (jar.to_path_buf(), None);
        }
        if !avail {
            println!(
                "   member-rename: collisions detected BUT ASM/helper unavailable — \
                 skipping (run `arlecchino setup`). Errors from overloaded members will \
                 remain; using original jar."
            );
            return (jar.to_path_buf(), None);
        }
        if collisions {
            println!(
                "   member-rename: source-illegal member collisions detected; running ASM \
                 bytecode renamer first."
            );
        } else {
            println!(
                "   member-rename: no member collisions, but running ASM pass for \
                 --flatten-packages."
            );
        }
    } else if mode == "on" {
        if !avail {
            println!(
                "   member-rename: requested but ASM/helper unavailable — run \
                 `arlecchino setup`. Using original jar."
            );
            return (jar.to_path_buf(), None);
        }
        if !collisions {
            println!("   member-rename: no collisions detected, but forced ON.");
        }
    }

    if flatten {
        println!(
            "   member-rename: --flatten-packages ON — all types moved to the default \
             package (cross-package references made nameable)."
        );
    }

    let stem = jar.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let out_jar = workdir.join(format!("{stem}.renamed.jar"));
    let mapping = workdir.join(format!("{stem}.rename-map.txt"));
    match run_renamer(jar, &out_jar, &mapping, root, framework_cp, flatten) {
        None => (jar.to_path_buf(), None),
        Some(res) => (res, Some(mapping)),
    }
}
