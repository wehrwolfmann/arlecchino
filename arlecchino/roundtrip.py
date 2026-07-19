"""
Round-trip / bytecode-structure check (point 4).

After merging we have a source tree; round-trip means: compile it back to .class
and compare the result against the ORIGINAL jar's bytecode, to catch silent
mismatches (a class that compiles but is structurally wrong, or missing).

Pragmatic scope (as specified):
  * Coverage: does the merge produce a .class for EVERY class of the original jar?
    (per-class present / missing)
  * Coarse bytecode diff: for each class present in both, compare the number of
    methods and fields declared (read straight from the constant-method/field
    counts via `javap -p`).  A mismatch flags a likely decompiler artifact even
    when the source compiled.

This does NOT prove semantic equivalence (that needs full bytecode/behaviour
comparison) — it is a cheap structural smoke test, documented as such.
"""
from __future__ import annotations

import re
import subprocess
import zipfile
from pathlib import Path

_METHOD_RE = re.compile(r"^\s+.*\(.*\).*;\s*$")  # javap member lines end with ;


def _jar_classes(jar: Path) -> set[str]:
    out = set()
    with zipfile.ZipFile(jar) as z:
        for n in z.namelist():
            if n.endswith(".class"):
                out.add(n[:-len(".class")])      # e.g. "o", "Container/GameMIDlet"
    return out


def _javap_counts(classpath: Path, binary_name: str) -> tuple[int, int] | None:
    """Return (#members, #methods-ish) for a class via `javap -p`."""
    cls = binary_name.replace("/", ".")
    r = subprocess.run(["javap", "-p", "-cp", str(classpath), cls],
                       capture_output=True, text=True)
    if r.returncode != 0:
        return None
    methods = 0
    members = 0
    for line in r.stdout.splitlines():
        s = line.strip()
        if not s or s.startswith("Compiled") or s.endswith("{") or s == "}":
            continue
        members += 1
        if "(" in s and ")" in s:
            methods += 1
    return (members, methods)


def check(jar: Path, merged_classes_dir: Path) -> dict:
    """Compare original jar classes vs compiled merged classes.
    merged_classes_dir is the `_classes` output of the final compile."""
    orig = _jar_classes(jar)
    present, missing = [], []
    method_mismatch = []

    # extract original .class to a temp dir for javap
    import tempfile
    tmp = Path(tempfile.mkdtemp(prefix="arl_rt_"))
    with zipfile.ZipFile(jar) as z:
        z.extractall(tmp)

    for bn in sorted(orig):
        compiled = merged_classes_dir / (bn + ".class")
        if not compiled.exists():
            missing.append(bn)
            continue
        present.append(bn)
        oc = _javap_counts(tmp, bn)
        mc = _javap_counts(merged_classes_dir, bn)
        if oc and mc and oc != mc:
            method_mismatch.append({"class": bn, "orig": oc, "merged": mc})

    return {
        "total_original": len(orig),
        "present": len(present),
        "missing": missing,
        "coverage_pct": round(100.0 * len(present) / max(1, len(orig)), 1),
        "structural_mismatch": method_mismatch,
    }
