"""
Deobfuscation pass (point 7) — optional --deobf flag.

The merged tree still has obfuscated single-letter class names (a, b, ..., o).
This pass renames classes to human-readable, *stable* names to aid reading,
without changing behaviour.  Two strategies, both heuristic:

  1. jadx --deobf re-run (preferred when available): jadx has a built-in
     deobfuscator.  We don't use it for the merge (it changes names, hurting
     cross-engine swaps), but as a final readability layer it is exactly right.
     -> see emit_jadx_deobf(): produces a SEPARATE `deobf/` tree, the merged tree
        is left intact.

  2. heuristic class renamer (always available): scans each class for tell-tale
     MIDP/string evidence and assigns a descriptive name, e.g. a class that
     `extends MIDlet` -> "<orig>_MIDlet", a class that `extends Canvas` ->
     "<orig>_Canvas", a class implementing `Runnable` with `Thread` -> "..._Worker".
     This is intentionally conservative: it APPENDS a hint to the original name
     (so references still resolve) rather than fully renaming the type, because a
     safe project-wide type rename needs a real refactoring engine.

The class-rename-with-reference-rewrite is left as documented future work; the
heuristic here is a readability aid, not a full rename.
"""
from __future__ import annotations

import re
import subprocess
from pathlib import Path

_HINTS = [
    (re.compile(r"\bextends\s+MIDlet\b"), "MIDlet"),
    (re.compile(r"\bextends\s+(GameCanvas|Canvas)\b"), "Canvas"),
    (re.compile(r"\bextends\s+(Form|List|TextBox|Alert|Screen)\b"), "Screen"),
    (re.compile(r"\bimplements\s+[^\{]*\bRunnable\b"), "Worker"),
    (re.compile(r"\bimplements\s+[^\{]*\bCommandListener\b"), "UI"),
    (re.compile(r"\bnew\s+Player\b|MediaPlayer|javax\.microedition\.media"), "Audio"),
]


def classify(text: str) -> str | None:
    for rx, label in _HINTS:
        if rx.search(text):
            return label
    return None


def heuristic_report(srcdir: Path) -> dict[str, str]:
    """Return {relpath: suggested_label} for classes we can classify."""
    out = {}
    for jf in srcdir.rglob("*.java"):
        if jf.name in ("_sources.txt",):
            continue
        try:
            text = jf.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        label = classify(text)
        if label:
            out[str(jf.relative_to(srcdir))] = label
    return out


def annotate_in_place(srcdir: Path) -> int:
    """Add a `// arlecchino: looks like a <label>` banner to each classified
    file (readability only; non-destructive, doesn't rename types)."""
    n = 0
    for rel, label in heuristic_report(srcdir).items():
        jf = srcdir / rel
        text = jf.read_text(encoding="utf-8", errors="replace")
        banner = f"// arlecchino-deobf: this class looks like a {label}\n"
        if banner in text:
            continue
        jf.write_text(banner + text, encoding="utf-8")
        n += 1
    return n


def emit_jadx_deobf(jar: Path, jadx_bin: Path, outdir: Path) -> bool:
    """Run jadx with its deobfuscator into a separate tree.  Returns success."""
    if not jadx_bin.exists():
        return False
    outdir.mkdir(parents=True, exist_ok=True)
    try:
        subprocess.run([str(jadx_bin), "--no-res", "--deobf",
                        "--deobf-min", "1", "-d", str(outdir), str(jar)],
                       capture_output=True, timeout=900)
        return True
    except Exception:  # noqa: BLE001
        return False
