"""
`arlecchino setup` (point 5): download decompiler engines + a J2ME emulator into
~/.arlecchino/engines/.

Engines are fetched from their official GitHub releases:
  * Vineflower   Vineflower/vineflower
  * CFR          leibnitz27/cfr
  * Procyon      mstrobel/procyon  (decompiler-shell fat jar)
  * jadx         skylot/jadx       (release zip with bin/jadx)

FreeJ2ME (for `arlecchino emulate`): we prefer a pre-built jar.  If a local
checkout/build is available we copy it; otherwise we clone TASEmulators/freej2me-plus
and build it with `javac` (no Ant required — we replicate build.xml's javac steps).
A pre-existing build can be supplied with --freej2me-prebuilt PATH.

Network access may be unavailable in some environments; every step degrades
gracefully and reports what succeeded.  Versions are pinned but overridable.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import urllib.request
import zipfile
from pathlib import Path

from . import rename as _rename
from .config import ENGINES_ROOT

UA = {"User-Agent": "arlecchino-setup"}

# ASM (org.ow2.asm, BSD-3-Clause) — used by the bytecode member-renamer.  Invoked,
# not vendored.  Pinned, overridable via ARLECCHINO_ASM_VERSION.
ASM_VERSION = os.environ.get("ARLECCHINO_ASM_VERSION", "9.7")
ASM_MODULES = ("asm", "asm-tree", "asm-commons", "asm-analysis")
ASM_MAVEN = "https://repo1.maven.org/maven2/org/ow2/asm"

# pinned, known-good asset patterns (resolved against the latest release API)
GH = {
    "vineflower": ("Vineflower/vineflower", lambda a: a.endswith(".jar") and "slim" not in a and "sources" not in a and "javadoc" not in a),
    "cfr":        ("leibnitz27/cfr",        lambda a: a.startswith("cfr-") and a.endswith(".jar")
                                              and "javadoc" not in a and "sources" not in a),
    "procyon":    ("mstrobel/procyon",      lambda a: "decompiler-shell" in a and a.endswith(".jar")),
    "jadx":       ("skylot/jadx",           lambda a: a.startswith("jadx-") and a.endswith(".zip") and "gui" not in a),
}


def _http_json(url: str):
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def _download(url: str, dst: Path):
    dst.parent.mkdir(parents=True, exist_ok=True)
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=300) as r, open(dst, "wb") as f:
        shutil.copyfileobj(r, f)


def _latest_asset(repo: str, pred) -> tuple[str, str] | None:
    data = _http_json(f"https://api.github.com/repos/{repo}/releases/latest")
    for a in data.get("assets", []):
        if pred(a["name"]):
            return a["name"], a["browser_download_url"]
    return None


def setup_engines(root: Path | None = None, only: list[str] | None = None) -> dict:
    root = root or ENGINES_ROOT
    root.mkdir(parents=True, exist_ok=True)
    result = {}
    for name, (repo, pred) in GH.items():
        if only and name not in only:
            continue
        try:
            asset = _latest_asset(repo, pred)
            if not asset:
                result[name] = "no matching asset"
                continue
            fname, url = asset
            if name == "jadx":
                zpath = root / fname
                print(f"  [{name}] downloading {fname} ...")
                _download(url, zpath)
                jadx_dir = root / "jadx"
                if jadx_dir.exists():
                    shutil.rmtree(jadx_dir)
                with zipfile.ZipFile(zpath) as z:
                    z.extractall(jadx_dir)
                os.chmod(jadx_dir / "bin" / "jadx", 0o755)
                zpath.unlink(missing_ok=True)
                result[name] = "ok (jadx/bin/jadx)"
            else:
                target = root / {"vineflower": "vineflower.jar",
                                 "cfr": "cfr.jar",
                                 "procyon": "procyon.jar"}[name]
                print(f"  [{name}] downloading {fname} ...")
                _download(url, target)
                result[name] = f"ok ({target.name})"
        except Exception as e:  # noqa: BLE001
            result[name] = f"FAILED: {e}"

    # ── ASM + compile the member-renamer helper ──────────────────────────────
    if not only or "asm" in only:
        result["asm"] = setup_asm(root)
    return result


def setup_asm(root: Path) -> str:
    """Download ASM jars from Maven Central into <root>/asm and javac-compile the
    MemberRenamer helper against them.  Degrades gracefully offline (if the jars are
    already present we still try to compile; if neither is possible we report why)."""
    asmdir = root / "asm"
    asmdir.mkdir(parents=True, exist_ok=True)
    got, failed = [], []
    for mod in ASM_MODULES:
        dst = asmdir / f"{mod}-{ASM_VERSION}.jar"
        if dst.exists() and dst.stat().st_size > 0:
            got.append(mod); continue
        url = f"{ASM_MAVEN}/{mod}/{ASM_VERSION}/{mod}-{ASM_VERSION}.jar"
        try:
            print(f"  [asm] downloading {mod}-{ASM_VERSION}.jar ...")
            _download(url, dst)
            got.append(mod)
        except Exception as e:  # noqa: BLE001
            failed.append(f"{mod}: {e}")
    if not _rename.asm_jars(root):
        return f"FAILED (no ASM jars; offline?). {('; '.join(failed))[:300]}"
    # compile the helper against whatever ASM jars we have
    comp = _rename.compile_helper(root)
    tail = f" [missing: {', '.join(failed)}]" if failed else ""
    return f"jars={len(got)}/{len(ASM_MODULES)}; helper {comp}{tail}"


# ─────────────────────────────── FreeJ2ME ───────────────────────────────────
FREEJ2ME_REPO = "TASEmulators/freej2me-plus"


def _download_freej2me_release(dst: Path) -> str | None:
    """Download the latest FreeJ2ME release zip and extract its `freej2me.jar`
    (the AWT frontend).  Returns a status string on success, None on failure."""
    try:
        rels = _http_json(f"https://api.github.com/repos/{FREEJ2ME_REPO}/releases")
        url = None
        for rel in rels:
            for a in rel.get("assets", []):
                n = a["name"]
                if n.endswith(".zip") and "libretro" not in n:
                    url = a["browser_download_url"]
                    break
            if url:
                break
        if not url:
            return None
        ztmp = dst.parent / "_fj_release.zip"
        print(f"  [freej2me] downloading prebuilt {url.split('/')[-1]} ...")
        _download(url, ztmp)
        with zipfile.ZipFile(ztmp) as z:
            member = next((m for m in z.namelist()
                           if m.endswith("freej2me.jar")), None)
            if not member:
                return None
            dst.parent.mkdir(parents=True, exist_ok=True)
            with z.open(member) as src, open(dst, "wb") as out:
                shutil.copyfileobj(src, out)
        ztmp.unlink(missing_ok=True)
        return f"ok (prebuilt release -> {dst})"
    except Exception:  # noqa: BLE001
        return None


def setup_freej2me(root: Path | None = None, prebuilt: Path | None = None,
                   src_dir: Path | None = None) -> str:
    root = root or ENGINES_ROOT
    dst = root / "freej2me.jar"
    if prebuilt and Path(prebuilt).exists():
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(prebuilt, dst)
        return f"ok (copied prebuilt -> {dst})"

    # Prefer the official prebuilt release jar (FreeJ2ME's AWT frontend often won't
    # compile cleanly on modern JDKs), and only fall back to a from-source build.
    if not src_dir:
        rel = _download_freej2me_release(dst)
        if rel:
            return rel

    work = root / "freej2me-src"
    if src_dir and Path(src_dir).exists():
        work = Path(src_dir)
    elif not work.exists():
        try:
            print("  [freej2me] cloning TASEmulators/freej2me-plus ...")
            subprocess.run(["git", "clone", "--depth", "1",
                            "https://github.com/TASEmulators/freej2me-plus.git",
                            str(work)], capture_output=True, timeout=600, check=True)
        except Exception as e:  # noqa: BLE001
            return f"FAILED clone: {e}"
    return build_freej2me(work, dst)


def build_freej2me(src: Path, dst: Path) -> str:
    """Replicate build.xml's javac steps without Ant.
    1) compile org/objectweb/asm, 2) compile the rest (minus libretro/objectweb),
    3) jar with Main-Class org.recompile.freej2me.FreeJ2ME + resources/META-INF."""
    srcroot = src / "src"
    if not srcroot.is_dir():
        return f"FAILED: no src/ in {src}"
    classes = src / "build" / "classes"
    if classes.exists():
        shutil.rmtree(classes)
    classes.mkdir(parents=True)
    javac = os.environ.get("ARLECCHINO_JAVAC", "javac")

    def collect(pred):
        return [str(p) for p in srcroot.rglob("*.java")
                if pred(str(p.relative_to(srcroot)))]

    # step 1: asm
    asm = collect(lambda r: r.startswith("org/objectweb/asm/"))
    if asm:
        lf = src / "_asm.txt"; lf.write_text("\n".join(asm))
        subprocess.run([javac, "-d", str(classes), "-nowarn", "-proc:none",
                        f"@{lf}"], capture_output=True, text=True)
    # step 2: everything except libretro + objectweb
    rest = collect(lambda r: not r.startswith("libretro/")
                   and not r.startswith("org/objectweb/"))
    lf = src / "_rest.txt"; lf.write_text("\n".join(rest))
    r = subprocess.run([javac, "-d", str(classes), "-cp", str(classes),
                        "-nowarn", "-proc:none", "-encoding", "utf-8", f"@{lf}"],
                       capture_output=True, text=True)
    # AWT target may emit some errors on newer JDKs; jar whatever compiled
    n_cls = sum(1 for _ in classes.rglob("*.class"))
    if n_cls == 0:
        return f"FAILED javac (no classes). tail:\n{r.stderr[-800:]}"

    # copy resources
    for extra in ("resources",):
        ed = src / extra
        if ed.is_dir():
            for p in ed.rglob("*"):
                if p.is_file():
                    rel = p.relative_to(ed)
                    out = classes / rel
                    out.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy(p, out)

    manifest = src / "_MANIFEST.MF"
    manifest.write_text("Main-Class: org.recompile.freej2me.FreeJ2ME\n"
                        "Implementation-Title: FreeJ2ME\n")
    jar = os.environ.get("ARLECCHINO_JAR", "jar")
    dst.parent.mkdir(parents=True, exist_ok=True)
    rj = subprocess.run([jar, "cfm", str(dst), str(manifest), "-C", str(classes), "."],
                        capture_output=True, text=True)
    if rj.returncode != 0 or not dst.exists():
        return f"FAILED jar: {rj.stderr[-400:]}"
    # sanity: the AWT main class must be present, or `emulate` won't launch.  On
    # modern JDKs FreeJ2ME-plus's AWT frontend can fail to compile (it targets an
    # old bootclasspath/rt.jar), leaving only the core classes -> warn loudly.
    main_ok = (classes / "org" / "recompile" / "freej2me" / "FreeJ2ME.class").exists()
    if not main_ok:
        return (f"PARTIAL ({n_cls} classes, but org.recompile.freej2me.FreeJ2ME is "
                f"missing — AWT frontend did not compile on this JDK). Use "
                f"--freej2me-prebuilt with a released jar, or build with the JDK "
                f"FreeJ2ME targets. Jar at {dst} (core only).")
    return f"ok (built {n_cls} classes -> {dst})"
