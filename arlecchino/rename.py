"""
Bytecode-first canonical member renamer (orchestration side).

ProGuard-style obfuscation reuses the SAME member name with DIFFERENT descriptors
(legal in JVM bytecode, ILLEGAL in Java source).  This makes decompiler output
uncompilable: `cannot find symbol`, `reference … is ambiguous`, `… cannot be
dereferenced`, `variable … is already defined`.

This module runs the ASM-based `MemberRenamer` Java helper (see
arlecchino/java/MemberRenamer.java) BEFORE decompilation.  The helper rewrites the
jar so every field/method has a Java-source-legal, unique name — splitting shared
NameAndType constant entries and rewriting ALL references, while preserving virtual
dispatch and never touching members bound by an external (not-in-jar) contract
(MIDP callbacks, java/* overrides).  The renamed jar is then fed to the normal
multi-decompiler → merge → autofix → round-trip pipeline, and the output compiles.

We do NOT do source-level textual renaming: it cannot resolve cross-class ambiguity
correctly (a use site doesn't know which overloaded declaration it binds to).

Everything degrades gracefully: if ASM jars or the compiled helper are unavailable
(e.g. offline, `setup` not run), `maybe_rename` logs and returns None so the caller
falls back to the original jar.
"""
from __future__ import annotations

import os
import subprocess
import zipfile
from collections import defaultdict
from pathlib import Path

from .config import ENGINES_ROOT

ASM_SUBDIR = "asm"
HELPER_CLASS = "MemberRenamer"
# the compiled helper lives next to the ASM jars, under _classes/
HELPER_CLASSES_SUBDIR = "_classes"


def helper_source() -> Path:
    """Path to the MemberRenamer.java shipped with the package."""
    return Path(__file__).with_name("java") / "MemberRenamer.java"


def _candidate_roots(root: Path | None) -> list[Path]:
    """ASM is a renamer helper, not a decompiler; it may live under the config's
    engines_root OR under the default ~/.arlecchino/engines (where `setup` installs
    it).  Search the given root first, then the default — de-duplicated."""
    roots: list[Path] = []
    for r in (root, ENGINES_ROOT):
        if r is not None and r not in roots:
            roots.append(r)
    return roots


def asm_dir(root: Path | None = None) -> Path:
    """The asm/ dir under the FIRST candidate root that actually has ASM jars, else
    under the primary root (for writing during setup)."""
    for r in _candidate_roots(root):
        d = r / ASM_SUBDIR
        if d.is_dir() and any(d.glob("asm*.jar")):
            return d
    return (root or ENGINES_ROOT) / ASM_SUBDIR


def asm_jars(root: Path | None = None) -> list[Path]:
    for r in _candidate_roots(root):
        d = r / ASM_SUBDIR
        if d.is_dir():
            jars = sorted(d.glob("asm*.jar"))
            if jars:
                return jars
    return []


def helper_classes_dir(root: Path | None = None) -> Path:
    return asm_dir(root) / HELPER_CLASSES_SUBDIR


def helper_compiled(root: Path | None = None) -> bool:
    return (helper_classes_dir(root) / f"{HELPER_CLASS}.class").exists()


def available(root: Path | None = None) -> bool:
    """True iff ASM jars + the compiled helper are both present."""
    return bool(asm_jars(root)) and helper_compiled(root)


def compile_helper(root: Path | None = None) -> str:
    """javac-compile MemberRenamer.java against the ASM jars (idempotent).
    Returns a status string.  Used by `setup`."""
    jars = asm_jars(root)
    if not jars:
        return "skip (no ASM jars; run setup with network access)"
    src = helper_source()
    if not src.exists():
        return f"FAILED (helper source missing: {src})"
    outdir = helper_classes_dir(root)
    outdir.mkdir(parents=True, exist_ok=True)
    cp = os.pathsep.join(str(j) for j in jars)
    javac = os.environ.get("ARLECCHINO_JAVAC", "javac")
    r = subprocess.run([javac, "-cp", cp, "-d", str(outdir), str(src)],
                       capture_output=True, text=True)
    if r.returncode != 0 or not helper_compiled(root):
        return f"FAILED javac: {r.stderr[-500:]}"
    return f"ok (compiled -> {outdir})"


def has_member_collisions(jar: Path) -> bool:
    """Cheap pure-Python pre-check: does the jar contain Java-source-ILLEGAL member
    collisions (duplicate field names in a class, or return-type-only method
    overloads)?  Parses the constant pool / member tables straight from the .class
    bytes — no javap, no ASM — so it works even when the helper is unavailable, and
    lets the AUTO mode decide whether the rename pass is worth running.

    Returns True on the first collision found."""
    try:
        with zipfile.ZipFile(jar) as z:
            for name in z.namelist():
                if not name.endswith(".class"):
                    continue
                if _class_has_collision(z.read(name)):
                    return True
    except Exception:  # noqa: BLE001
        return False
    return False


def _class_has_collision(data: bytes) -> bool:
    """Parse one .class file's field/method tables; report a Java-illegal collision.
    Minimal classfile reader (constant pool + field_info/method_info name+desc)."""
    try:
        cp, off = _parse_constant_pool(data)
        # after constant pool: access_flags(2) this_class(2) super_class(2)
        off += 6
        ifc = int.from_bytes(data[off:off + 2], "big"); off += 2
        off += ifc * 2
        # fields
        fnames: dict[str, set[str]] = defaultdict(set)
        nf = int.from_bytes(data[off:off + 2], "big"); off += 2
        for _ in range(nf):
            off, name, desc = _read_member(data, off, cp)
            if name in fnames:           # duplicate FIELD name -> illegal in Java
                return True
            fnames[name] = {desc}
        # methods: collision = same name+params, different return type
        seen: dict[tuple[str, str], set[str]] = defaultdict(set)
        nm = int.from_bytes(data[off:off + 2], "big"); off += 2
        for _ in range(nm):
            off, name, desc = _read_member(data, off, cp)
            if name in ("<init>", "<clinit>"):
                continue
            params = desc[:desc.index(")") + 1]
            ret = desc[desc.index(")") + 1:]
            key = (name, params)
            if key in seen and ret not in seen[key]:
                return True
            seen[key].add(ret)
    except Exception:  # noqa: BLE001
        return False
    return False


def _parse_constant_pool(data: bytes):
    """Return ({index: utf8_string_or_None}, offset_after_pool)."""
    # magic(4) minor(2) major(2) cp_count(2)
    count = int.from_bytes(data[8:10], "big")
    cp: dict[int, str] = {}
    off = 10
    i = 1
    while i < count:
        tag = data[off]; off += 1
        if tag == 1:                     # Utf8
            ln = int.from_bytes(data[off:off + 2], "big"); off += 2
            cp[i] = data[off:off + ln].decode("utf-8", "replace"); off += ln
        elif tag in (5, 6):              # Long / Double take two slots
            off += 8; i += 1
        elif tag in (7, 8, 16, 19, 20):  # Class/String/MethodType/Module/Package: u2
            off += 2
        elif tag == 15:                  # MethodHandle: u1 u2
            off += 3
        elif tag in (3, 4, 9, 10, 11, 12, 17, 18):  # Int/Float/refs/NameAndType/Dyn: u4
            off += 4
        else:
            raise ValueError(f"bad cp tag {tag}")
        i += 1
    return cp, off


def _read_member(data: bytes, off: int, cp: dict[int, str]):
    """Read one field_info/method_info: access(2) name_idx(2) desc_idx(2) attrs.
    Returns (new_off, name, descriptor)."""
    off += 2
    name = cp.get(int.from_bytes(data[off:off + 2], "big"), ""); off += 2
    desc = cp.get(int.from_bytes(data[off:off + 2], "big"), ""); off += 2
    natt = int.from_bytes(data[off:off + 2], "big"); off += 2
    for _ in range(natt):
        off += 2                          # attribute_name_index
        ln = int.from_bytes(data[off:off + 4], "big"); off += 4
        off += ln
    return off, name, desc


def run_renamer(jar: Path, out_jar: Path, mapping: Path,
                root: Path | None = None,
                framework_cp: str | None = None) -> Path | None:
    """Invoke the ASM helper: rewrite `jar` -> `out_jar`, writing `mapping`.
    `framework_cp` (e.g. a MIDP/CLDC stub jar) lets the helper RESOLVE external
    supertypes so it locks genuine MIDP overrides precisely.  Returns out_jar on
    success, None on any failure (logged)."""
    jars = asm_jars(root)
    if not jars or not helper_compiled(root):
        print("   member-rename: ASM/helper unavailable — skipping "
              "(run `arlecchino setup` with network access). Using original jar.")
        return None
    cp = os.pathsep.join([*(str(j) for j in jars), str(helper_classes_dir(root))])
    java = os.environ.get("ARLECCHINO_JAVA", "java")
    out_jar.parent.mkdir(parents=True, exist_ok=True)
    argv = [java, "-cp", cp, HELPER_CLASS,
            str(jar), str(out_jar), str(mapping), framework_cp or ""]
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=600)
    except Exception as e:  # noqa: BLE001
        print(f"   member-rename: launch error ({e}); using original jar.")
        return None
    if r.returncode != 0 or not out_jar.exists():
        print(f"   member-rename: FAILED ({r.stderr.strip()[-300:]}); using original jar.")
        return None
    print(f"   member-rename: {r.stdout.strip()}")
    return out_jar


def maybe_rename(jar: Path, workdir: Path, root: Path | None = None,
                 mode: str = "auto",
                 framework_cp: str | None = None) -> tuple[Path, Path | None]:
    """Decide-and-run the rename pass.

    mode:
      "auto"  — run only if collisions are detected AND the helper is available.
      "on"    — run if available (warn if not).
      "off"   — never run.

    Returns (jar_to_use, mapping_path_or_None).  jar_to_use is the RENAMED jar on
    success, else the original `jar` (so the pipeline always has a valid input)."""
    if mode == "off":
        print("   member-rename: disabled (--no-rename-members).")
        return jar, None

    collisions = has_member_collisions(jar)
    avail = available(root)

    if mode == "auto":
        if not collisions:
            print("   member-rename: no source-illegal member collisions detected — "
                  "skipping (not needed).")
            return jar, None
        if not avail:
            print("   member-rename: collisions detected BUT ASM/helper unavailable — "
                  "skipping (run `arlecchino setup`). Errors from overloaded members "
                  "will remain; using original jar.")
            return jar, None
        print("   member-rename: source-illegal member collisions detected; "
              "running ASM bytecode renamer first.")
    elif mode == "on":
        if not avail:
            print("   member-rename: requested but ASM/helper unavailable — "
                  "run `arlecchino setup`. Using original jar.")
            return jar, None
        if not collisions:
            print("   member-rename: no collisions detected, but forced ON.")

    out_jar = workdir / (jar.stem + ".renamed.jar")
    mapping = workdir / (jar.stem + ".rename-map.txt")
    res = run_renamer(jar, out_jar, mapping, root, framework_cp)
    if res is None:
        return jar, None
    return res, mapping
