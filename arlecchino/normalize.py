"""
Class-name normalisation across engines (point 1) — the main quality upgrade.

Goal: bring every engine's output into ONE canonical name space so that files can
be swapped between engines during the merge.  The canonical names are the names of
the .class entries in the jar (a, b, ..., o, Container/GameMIDlet, ...).

CFR / Procyon / Vineflower already emit the canonical names and default package,
so they need no work.

jadx is the interesting case for this jar:
  * It KEEPS the original short class names (a.java, b.java, ..., o.java) — the
    old "renames everything to C00xx" behaviour does NOT happen here.
  * BUT it wraps every default-package class in a synthetic package `defpackage`
    (`package defpackage;` + files under sources/defpackage/), and it litters the
    code with `/* JADX INFO: ... */` comments.
  * It also renames *colliding* fields (two bytecode fields `a` with different
    descriptors -> `a` and `f259a`).  We leave those: the externally-referenced
    field keeps its original name, only the internal collider is renamed, so a
    swapped jadx class stays API-compatible with its callers.

normalize_jadx() therefore rewrites a jadx source tree in place to the canonical
layout: move sources/defpackage/X.java -> X.java, drop `package defpackage;`,
rewrite `defpackage.X` / `import defpackage.X;` references, and strip JADX INFO
comments.  After this, jadx files have the same relative paths as the other
engines and can take part in the best-of merge.
"""
from __future__ import annotations

import re
import shutil
from pathlib import Path

_PKG_DEFPACKAGE = re.compile(r"^\s*package\s+defpackage\s*;\s*$", re.MULTILINE)
_IMPORT_DEFPACKAGE = re.compile(r"^\s*import\s+defpackage\.[A-Za-z_$][\w$]*\s*;\s*\n",
                                re.MULTILINE)
_QUALIFIED = re.compile(r"\bdefpackage\.")
_JADX_INFO = re.compile(r"[ \t]*/\* JADX INFO:.*?\*/[ \t]*\n?", re.DOTALL)
_JADX_WARN = re.compile(r"[ \t]*/\* (?:JADX WARNING|Code decompiled incorrectly).*?\*/[ \t]*\n?",
                        re.DOTALL)


def _clean_text(text: str, strip_comments: bool) -> str:
    text = _PKG_DEFPACKAGE.sub("", text)
    text = _IMPORT_DEFPACKAGE.sub("", text)
    text = _QUALIFIED.sub("", text)
    if strip_comments:
        text = _JADX_INFO.sub("", text)
        text = _JADX_WARN.sub("", text)
    # collapse the leading blank line(s) left by a removed `package` statement
    text = re.sub(r"\A\s*\n", "", text)
    return text


def normalize_jadx(srcdir: Path, strip_comments: bool = True) -> int:
    """Rewrite a jadx source tree in `srcdir` to the canonical default-package
    layout, in place.  Returns the number of files moved out of `defpackage`."""
    defpkg = srcdir / "defpackage"
    moved = 0
    if defpkg.is_dir():
        for jf in list(defpkg.rglob("*.java")):
            rel = jf.relative_to(defpkg)
            dst = srcdir / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(jf), str(dst))
            moved += 1
        shutil.rmtree(defpkg, ignore_errors=True)
    # rewrite every java file (also those that referenced defpackage.* qualified)
    for jf in srcdir.rglob("*.java"):
        try:
            text = jf.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        new = _clean_text(text, strip_comments)
        if new != text:
            jf.write_text(new, encoding="utf-8")
    return moved


def apply(engine_normalize: str, srcdir: Path, strip_comments: bool = True) -> int:
    """Dispatch by the engine's `normalize` setting.  Returns files touched."""
    if engine_normalize == "jadx":
        return normalize_jadx(srcdir, strip_comments=strip_comments)
    return 0
