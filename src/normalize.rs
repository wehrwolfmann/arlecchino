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

/// Dispatch by the engine's `normalize` setting.  Returns files touched.
pub fn apply(engine_normalize: &str, srcdir: &Path, strip_comments: bool) -> usize {
    if engine_normalize == "jadx" {
        normalize_jadx(srcdir, strip_comments)
    } else {
        0
    }
}
