//! Class-name normalisation across engines — the main quality upgrade.
//!
//! Goal: bring every engine's output into ONE canonical name space so files can be
//! swapped between engines during the merge.  The canonical names are the names of
//! the .class entries in the jar (a, b, ..., o, Container/GameMIDlet, ...).
//!
//! CFR / Procyon / Vineflower already emit canonical names + the default package.
//! jadx keeps the original short class names but wraps every default-package class
//! in a synthetic package `defpackage` and litters `/* JADX INFO ... */` comments.
//! `normalize_jadx` rewrites a jadx tree in place to the canonical layout: move
//! `sources/defpackage/X.java -> X.java`, drop `package defpackage;`, rewrite
//! `defpackage.X` references, and strip JADX INFO comments.
//!
//! Sporeflower does the same with a package called `defaultpkg`, but only when the
//! jar also has named packages (a named package cannot import the default one).  It
//! also gives `field_N` / `method_N` names to members whose original names would be
//! captured in source (a field hiding an inherited one of another type, ...).  Those
//! names are consistent inside Sporeflower's own tree but match nothing in the jar
//! the other variants are compiled against, so `normalize_sporeflower` un-wraps the
//! package and puts the original member names back from Sporeflower's Tiny map.

use crate::util;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

static PKG_DEFPACKAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*package\s+defpackage\s*;\s*$").unwrap());
static IMPORT_DEFPACKAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*import\s+defpackage\.[A-Za-z_$][\w$]*\s*;\s*\n").unwrap());
static QUALIFIED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bdefpackage\.").unwrap());
static JADX_INFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)[ \t]*/\* JADX INFO:.*?\*/[ \t]*\n?").unwrap());
static JADX_WARN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)[ \t]*/\* (?:JADX WARNING|Code decompiled incorrectly).*?\*/[ \t]*\n?").unwrap()
});
static LEAD_BLANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A\s*\n").unwrap());

fn clean_text(text: &str, strip_comments: bool) -> String {
    let mut t = PKG_DEFPACKAGE.replace_all(text, "").into_owned();
    t = IMPORT_DEFPACKAGE.replace_all(&t, "").into_owned();
    t = QUALIFIED.replace_all(&t, "").into_owned();
    if strip_comments {
        t = JADX_INFO.replace_all(&t, "").into_owned();
        t = JADX_WARN.replace_all(&t, "").into_owned();
    }
    // collapse the leading blank line(s) left by a removed `package` statement
    t = LEAD_BLANK.replace(&t, "").into_owned();
    t
}

/// Rewrite a jadx source tree in `srcdir` to the canonical default-package layout,
/// in place.  Returns the number of files moved out of `defpackage`.
pub fn normalize_jadx(srcdir: &Path, strip_comments: bool) -> usize {
    let defpkg = srcdir.join("defpackage");
    let mut moved = 0usize;
    if defpkg.is_dir() {
        for jf in util::rglob_ext(&defpkg, "java") {
            let rel = match jf.strip_prefix(&defpkg) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let dst = srcdir.join(rel);
            if util::move_file(&jf, &dst).is_ok() {
                moved += 1;
            }
        }
        let _ = std::fs::remove_dir_all(&defpkg);
    }
    // rewrite every java file (also those that referenced defpackage.* qualified)
    for jf in util::rglob_ext(srcdir, "java") {
        let text = match util::read_text_lossy(&jf) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let new = clean_text(&text, strip_comments);
        if new != text {
            let _ = std::fs::write(&jf, new);
        }
    }
    moved
}

/// Un-wrap Sporeflower's synthetic `defaultpkg` package in `srcdir`, in place: move
/// `defaultpkg/X.java -> X.java`, drop `package defaultpkg;` and `import defaultpkg.X;`,
/// and rewrite `defaultpkg.X` references.  Returns the number of files moved.
///
/// `field_N` / `method_N` names listed in `<srcdir>/mapping.tiny` are reverted to the
/// jar's names (the counter is global, so each such name means one member).  The map
/// and Sporeflower's `.sporeflower-compile.json` are removed afterwards.
pub fn normalize_sporeflower(srcdir: &Path) -> usize {
    let tiny = srcdir.join("mapping.tiny");
    let back = util::read_text_lossy(&tiny).map(|t| sporeflower_synthetic_names(&t)).unwrap_or_default();
    let _ = std::fs::remove_file(&tiny);
    let _ = std::fs::remove_file(srcdir.join(".sporeflower-compile.json"));
    let moved = unwrap_package(srcdir, "defaultpkg");
    if !back.is_empty() {
        for jf in util::rglob_ext(srcdir, "java") {
            let text = match util::read_text_lossy(&jf) {
                Ok(t) => t,
                Err(_) => continue,
            };
            // match on the length-preserving masked view, so a `field_N` inside a
            // string literal or a comment is left alone
            let masked = crate::autofix::mask_literals(&text);
            let mut new = String::with_capacity(text.len());
            let (mut last, mut changed) = (0usize, false);
            for m in SYNTHETIC_MEMBER.find_iter(&masked) {
                if let Some(orig) = back.get(m.as_str()) {
                    new.push_str(&text[last..m.start()]);
                    new.push_str(orig);
                    last = m.end();
                    changed = true;
                }
            }
            if changed {
                new.push_str(&text[last..]);
                let _ = std::fs::write(&jf, new);
            }
        }
    }
    moved
}

static SYNTHETIC_MEMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:field|method)_[0-9]+\b").unwrap());

/// `field_N` / `method_N` -> original name, from a Tiny v2 map whose two namespaces
/// are `original` and `source` (`f|m <TAB> desc <TAB> original <TAB> source`).
pub fn sporeflower_synthetic_names(tiny: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for line in tiny.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 5 || !cols[0].is_empty() || !(cols[1] == "f" || cols[1] == "m") {
            continue;
        }
        let (orig, src) = (cols[3], cols[4]);
        if orig != src && !orig.contains('\\') && SYNTHETIC_MEMBER.find(src).map(|m| m.as_str() == src).unwrap_or(false) {
            out.insert(src.to_string(), orig.to_string());
        }
    }
    out
}

fn unwrap_package(srcdir: &Path, pkg: &str) -> usize {
    let wrap = srcdir.join(pkg);
    if !wrap.is_dir() {
        return 0;
    }
    let mut moved = 0usize;
    for jf in util::rglob_ext(&wrap, "java") {
        let rel = match jf.strip_prefix(&wrap) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if util::move_file(&jf, &srcdir.join(rel)).is_ok() {
            moved += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&wrap);
    let q = regex::escape(pkg);
    let pkg_re = Regex::new(&format!(r"(?m)^\s*package\s+{q}\s*;[ \t]*\n?")).unwrap();
    let imp_re = Regex::new(&format!(r"(?m)^\s*import\s+{q}\.[A-Za-z_$][\w$]*\s*;[ \t]*\n")).unwrap();
    let qual_re = Regex::new(&format!(r"\b{q}\.")).unwrap();
    for jf in util::rglob_ext(srcdir, "java") {
        let text = match util::read_text_lossy(&jf) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut t = pkg_re.replace_all(&text, "").into_owned();
        t = imp_re.replace_all(&t, "").into_owned();
        t = qual_re.replace_all(&t, "").into_owned();
        t = LEAD_BLANK.replace(&t, "").into_owned();
        if t != text {
            let _ = std::fs::write(&jf, t);
        }
    }
    moved
}

/// Extract `.java` members out of any `.jar` an engine emitted at the top level of
/// `srcdir`, writing them as loose files (preserving internal paths), then delete the
/// jar.  Fernflower packs its output into `<out>/<input>.jar` when the input is a jar;
/// this brings its sources back onto the shared canonical tree so they merge.
/// Returns the number of `.java` files extracted.
pub fn normalize_unjar(srcdir: &Path) -> usize {
    let jars: Vec<std::path::PathBuf> = match std::fs::read_dir(srcdir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().map(|x| x == "jar").unwrap_or(false))
            .collect(),
        Err(_) => return 0,
    };
    let mut extracted = 0usize;
    for jar in jars {
        let names = match util::zip_namelist(&jar) {
            Ok(n) => n,
            Err(_) => continue,
        };
        for name in names {
            if !name.ends_with(".java") {
                continue;
            }
            if let Ok(bytes) = util::zip_read_entry(&jar, &name) {
                let dst = srcdir.join(&name);
                if let Some(p) = dst.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                if std::fs::write(&dst, bytes).is_ok() {
                    extracted += 1;
                }
            }
        }
        let _ = std::fs::remove_file(&jar);
    }
    extracted
}

/// Dispatch by the engine's `normalize` setting.  Returns files touched.
pub fn apply(engine_normalize: &str, srcdir: &Path, strip_comments: bool) -> usize {
    match engine_normalize {
        "jadx" => normalize_jadx(srcdir, strip_comments),
        "unjar" => normalize_unjar(srcdir),
        "sporeflower" => normalize_sporeflower(srcdir),
        _ => 0,
    }
}
