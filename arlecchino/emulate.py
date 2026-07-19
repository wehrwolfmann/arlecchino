"""
`arlecchino emulate <jar>` (part of point 5): run a J2ME/MIDP jar in FreeJ2ME.

FreeJ2ME's AWT front-end takes the midlet jar as its first argument:
    java -jar freej2me.jar <midlet.jar> [width] [height] [scale]

We locate freej2me.jar in ~/.arlecchino/engines (or a path the user passes), and
launch it.  This is a thin wrapper — FreeJ2ME does the emulation.
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

from .config import ENGINES_ROOT


def find_freej2me(explicit: str | None = None, root: Path | None = None) -> Path | None:
    if explicit and Path(explicit).exists():
        return Path(explicit)
    root = root or ENGINES_ROOT
    for cand in (root / "freej2me.jar",
                 root / "freej2me-src" / "build" / "freej2me.jar",
                 root / "fj2me" / "build" / "freej2me.jar"):
        if cand.exists():
            return cand
    return None


def emulate(jar: Path, freej2me: Path, width: int = 176, height: int = 220,
            scale: int = 2, extra: list[str] | None = None) -> int:
    java = os.environ.get("ARLECCHINO_JAVA", "java")
    argv = [java, "-jar", str(freej2me), str(jar),
            str(width), str(height), str(scale)]
    if extra:
        argv += extra
    print("  launching: " + " ".join(argv))
    return subprocess.run(argv).returncode
