"""
Core meta-decompilation: run engines (parallel), compile, rank, best-of merge,
auto-fix, round-trip.  This is the v0 engine + points 1-4 and the parallelism of
point 6.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from . import autofix, normalize
from .config import Engine

_ERR_RE = re.compile(r"^(.*?\.java):\d+: error:", re.MULTILINE)
_TOP_TYPE_RE = re.compile(r"\b(class|interface|enum)\s+[A-Za-z_$][\w$]*", re.MULTILINE)


def _is_interface(text: str) -> bool:
    """True if the FIRST top-level type declaration in `text` is an interface.
    Used as a whole-tree-consistency tiebreak: a constant interface that other
    classes implement must stay an interface, even if turning it into a class would
    compile equally well in isolation."""
    m = _TOP_TYPE_RE.search(text)
    return bool(m) and m.group(1) == "interface"


# ───────────────────────── decompile / compile ──────────────────────────────
def run_decompiler(eng: Engine, jar: Path, outdir: Path, root: Path) -> Path:
    outdir.mkdir(parents=True, exist_ok=True)
    try:
        subprocess.run(eng.argv(jar, outdir, root), capture_output=True, timeout=900)
    except Exception as e:  # noqa: BLE001
        print(f"  [{eng.name}] launch error: {e}")
    src = eng.sources_dir(outdir)
    if eng.normalize != "none":
        moved = normalize.apply(eng.normalize, src)
        if moved:
            print(f"    [{eng.name}] normalised {moved} file(s) out of defpackage")
    return src


def collect_java(srcdir: Path) -> dict[str, Path]:
    out = {}
    for p in srcdir.rglob("*.java"):
        if p.name in ("_sources.txt",):
            continue
        out[str(p.relative_to(srcdir))] = p
    return out


def compile_dir(srcdir: Path, classpath: str | None = None) -> tuple[int, dict[str, int], str]:
    """Compile all .java under srcdir. -> (total_errors, {relpath: count}, log)."""
    files = [p for p in srcdir.rglob("*.java")]
    if not files:
        return (10 ** 9, {}, "no sources")
    listf = srcdir / "_sources.txt"
    listf.write_text("\n".join(str(f) for f in files), encoding="utf-8")
    cmd = [os.environ.get("ARLECCHINO_JAVAC", "javac"),
           "-d", str(srcdir / "_classes"), "--release", "8",
           "-encoding", "utf-8", "-nowarn", "-proc:none"]
    if classpath:
        cmd += ["-cp", classpath]
    cmd += [f"@{listf}"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    log = r.stderr
    per: dict[str, int] = {}
    for m in _ERR_RE.finditer(log):
        raw = m.group(1)
        try:
            rel = str(Path(raw).resolve().relative_to(srcdir.resolve()))
        except Exception:  # noqa: BLE001
            rel = os.path.basename(raw)
        per[rel] = per.get(rel, 0) + 1
    return (sum(per.values()), per, log)


# ───────────────────────── decompile all (parallel) ─────────────────────────
def decompile_all(engines: list[Engine], jar: Path, out: Path, root: Path,
                  classpath: str | None, jobs: int) -> dict[str, dict]:
    usable = []
    for eng in engines:
        if not eng.enabled:
            continue
        binp = eng.resolve_path(root)
        if not binp.exists():
            print(f"  [{eng.name}] skip (missing {binp})")
            continue
        usable.append(eng)

    def work(eng: Engine):
        print(f"  decompiling: {eng.name} ...")
        src = run_decompiler(eng, jar, out / eng.name, root)
        files = collect_java(src)
        total, per, _ = compile_dir(src, classpath)
        print(f"    [{eng.name}] classes={len(files)}  compile errors={total}")
        return eng.name, {"type": eng.type, "src": src, "files": files,
                          "errors": total, "per": per, "nclasses": len(files)}

    data: dict[str, dict] = {}
    with ThreadPoolExecutor(max_workers=max(1, jobs)) as ex:
        for name, d in ex.map(work, usable):
            data[name] = d
    return data


# ───────────────────────── best-of merge + autofix ──────────────────────────
def merge_best(engine_data: dict[str, dict], base_name: str, outdir: Path,
               classpath: str | None, do_autofix: bool = True) -> tuple[Path, int, list[str]]:
    base = engine_data[base_name]
    merge = outdir / "merged"
    if merge.exists():
        shutil.rmtree(merge)
    shutil.copytree(base["src"], merge,
                    ignore=shutil.ignore_patterns("_classes", "_sources.txt"))

    total, per, _ = compile_dir(merge, classpath)
    log = [f"base={base_name} errors={total}"]
    donors = [n for n, d in engine_data.items()
              if d["type"] == "names" and n != base_name]
    # order donors by their overall quality (fewest errors first)
    donors.sort(key=lambda n: engine_data[n]["errors"])

    # 1) per-class file swap
    for rel in sorted(per):
        target = merge / rel
        best_total = total
        best_donor = None
        backup = target.read_text(encoding="utf-8", errors="replace") if target.exists() else None
        for dn in donors:
            cand = engine_data[dn]["files"].get(rel)
            if not cand:
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(cand, target)
            t2, _, _ = compile_dir(merge, classpath)
            if t2 < best_total:
                best_total, best_donor = t2, dn
            if backup is not None:
                target.write_text(backup, encoding="utf-8")
        if best_donor:
            shutil.copy(engine_data[best_donor]["files"][rel], target)
            total = best_total
            log.append(f"  {rel}: {base_name}->{best_donor} (errors->{total})")

    # 2) auto-fix pass on whatever still fails.  For each failing file we try the
    #    autofix on the CURRENT variant AND on every engine's variant of that file
    #    (a fix may only pay off on a particular engine's rendering), keeping the
    #    globally best result.  Never accept a worse total.
    if do_autofix and total > 0:
        all_names = [n for n, d in engine_data.items() if d["type"] == "names"]
        _, per_now, _ = compile_dir(merge, classpath)
        for rel in sorted(per_now):
            target = merge / rel
            if not target.exists():
                continue
            current = target.read_text(encoding="utf-8", errors="replace")
            best_text, best_total, best_tag = current, total, None
            # candidate source texts: current + each engine's variant
            cands = [("current", current)]
            for n in all_names:
                f = engine_data[n]["files"].get(rel)
                if f:
                    cands.append((n, Path(f).read_text(encoding="utf-8", errors="replace")))
            for tag, src_text in cands:
                fixed, applied = autofix.apply_all(src_text)
                if not applied or fixed == src_text:
                    continue
                target.write_text(fixed, encoding="utf-8")
                t2, _, _ = compile_dir(merge, classpath)
                if t2 < best_total:
                    best_total, best_text, best_tag = t2, fixed, f"{tag}+{','.join(applied)}"
            target.write_text(best_text, encoding="utf-8")  # keep best (>= reverts to current)
            if best_tag and best_total < total:
                total = best_total
                log.append(f"  {rel}: autofix {best_tag} (errors->{total})")

    final_total, final_per, _ = compile_dir(merge, classpath)
    log.append(f"RESULT merged: errors={final_total} "
               f"(classes with errors: {len(final_per)})")
    return merge, final_total, log


def compile_file_isolated(java_file: Path, api_classpath: str,
                          tmp_out: Path) -> int:
    """Compile a SINGLE .java against `api_classpath` (which should provide the
    canonical bytecode of every sibling class + the MIDP stub), counting only that
    file's javac errors.  This avoids javac's error-MASKING: in a batch compile an
    early structural error (e.g. an illegal interface initializer) aborts
    attribution of later files, hiding their real errors and corrupting any
    per-class comparison.  Isolated compilation gives each variant an honest,
    independent score."""
    tmp_out.mkdir(parents=True, exist_ok=True)
    cmd = [os.environ.get("ARLECCHINO_JAVAC", "javac"),
           "-d", str(tmp_out), "--release", "8", "-encoding", "utf-8",
           "-nowarn", "-proc:none", "-cp", api_classpath, str(java_file)]
    r = subprocess.run(cmd, capture_output=True, text=True)
    return len(_ERR_RE.findall(r.stderr))


def build_api_classpath(orig_jar: Path, extra: str | None, workdir: Path) -> str:
    """Classpath that exposes the ORIGINAL jar's bytecode (every canonical class)
    plus any extra (MIDP stub) — used as the stable 'API' for isolated compiles."""
    parts = [str(orig_jar)]
    if extra:
        parts.append(extra)
    return os.pathsep.join(parts)


def merge_isolated(engine_data: dict[str, dict], outdir: Path, orig_jar: Path,
                   classpath: str | None, do_autofix: bool = True
                   ) -> tuple[Path, int, list[str], dict[str, int]]:
    """Per-class best-of merge using ISOLATED, unmasked per-file scoring.

    For each canonical class we score every engine's variant by compiling it alone
    against the original jar (+ MIDP stub) as the API, and keep the lowest-scoring
    variant.  Then we run the autofix pass (also isolated-scored).  Finally we do a
    whole-tree compile for the headline number.  Returns
    (merge_dir, tree_errors, log, per_class_isolated_errors)."""
    merge = outdir / "merged"
    if merge.exists():
        shutil.rmtree(merge)
    merge.mkdir(parents=True)

    api_cp = build_api_classpath(orig_jar, classpath, outdir)
    tmp = outdir / "_iso"
    if tmp.exists():
        shutil.rmtree(tmp)

    names = [n for n, d in engine_data.items() if d["type"] == "names"]
    all_rels = sorted({r for n in names for r in engine_data[n]["files"]})
    log: list[str] = []
    per_iso: dict[str, int] = {}

    for rel in all_rels:
        best_text = best_score = best_eng = None
        for n in names:
            f = engine_data[n]["files"].get(rel)
            if not f:
                continue
            score = compile_file_isolated(Path(f), api_cp, tmp / "score")
            if best_score is None or score < best_score:
                best_score, best_eng = score, n
                best_text = Path(f).read_text(encoding="utf-8", errors="replace")
        if best_text is None:
            continue
        # autofix attempt (isolated-scored)
        if do_autofix and best_score and best_score > 0:
            # donor texts for cross-engine repairs (e.g. resolving Vineflower's
            # <unrepresentable> casts from a sibling decompiler that named the type).
            donor_texts = [Path(engine_data[n]["files"][rel]).read_text(
                               encoding="utf-8", errors="replace")
                           for n in names if engine_data[n]["files"].get(rel)]
            for src_eng in names:
                f = engine_data[src_eng]["files"].get(rel)
                if not f:
                    continue
                cand_src = Path(f).read_text(encoding="utf-8", errors="replace")
                fixed, applied = autofix.apply_all(cand_src)
                # cross-engine cast resolution (uses the other engines as donors)
                resolved = autofix.resolve_unrepresentable_casts(fixed, donor_texts)
                if resolved != fixed:
                    fixed = resolved
                    applied = applied + ["resolve_unrepresentable_casts"]
                if not applied or fixed == cand_src:
                    continue
                tf = tmp / "fix" / rel
                tf.parent.mkdir(parents=True, exist_ok=True)
                tf.write_text(fixed, encoding="utf-8")
                sc = compile_file_isolated(tf, api_cp, tmp / "score")
                # Accept on a strictly lower score, OR on a TIE when the candidate is
                # more tree-compatible.  A constant interface that other classes
                # `implements` MUST stay an interface: the `const_interface_to_class`
                # fallback yields a clean ISOLATED compile but breaks every
                # `implements X` in the tree, whereas `fold_interface_static_init`
                # keeps it an interface.  So on equal isolated score we prefer a
                # candidate that preserved the interface keyword over one that turned
                # it into a class.  (Isolated score is unchanged; only the tie is
                # broken toward whole-tree consistency.)
                better = sc < best_score
                tie_pref = (sc == best_score
                            and _is_interface(fixed) and not _is_interface(best_text))
                if better or tie_pref:
                    best_score, best_text, best_eng = sc, fixed, f"{src_eng}+autofix({','.join(applied)})"
        dst = merge / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text(best_text, encoding="utf-8")
        per_iso[rel] = best_score or 0
        if best_score:
            log.append(f"  {rel}: <- {best_eng} (isolated errors={best_score})")
        else:
            log.append(f"  {rel}: <- {best_eng} (clean)")

    if tmp.exists():
        shutil.rmtree(tmp, ignore_errors=True)
    iso_total = sum(per_iso.values())
    tree_total, _, _ = compile_dir(merge, classpath)
    log.insert(0, f"isolated per-class scoring; sum of per-class errors={iso_total}")
    log.append(f"RESULT merged: isolated-sum={iso_total}, whole-tree errors={tree_total} "
               f"(classes still failing: {sum(1 for v in per_iso.values() if v)})")
    return merge, iso_total, log, per_iso


def compile_best_effort(srcdir: Path, orig_jar: Path | None = None,
                        classpath: str | None = None,
                        drop: set[str] | None = None) -> tuple[int, set[str]]:
    """Compile srcdir, seeding broken classes' bytecode from `orig_jar` so their
    dependents resolve, and recompiling the rest against orig_jar + classpath as the
    API.  Returns (n_dropped, set_of_dropped_relpaths).

    `drop` seeds the initial broken set (e.g. the isolated-failing classes).  Because
    a class that compiles in ISOLATION can still fail in a BATCH (a sibling source it
    binds to is itself an artifact — e.g. a constant interface the autofix turned into
    a class, breaking `implements X`), we then ITERATE: compile the kept set, drop any
    newly-failing classes (seed them from the jar too), and repeat until the kept set
    compiles cleanly or no progress is made.  This makes coverage reflect the classes
    that genuinely compile together against the (renamed) jar API, instead of letting a
    single batch error mask — and zero out — everyone else via javac's all-or-nothing
    attribution."""
    import zipfile
    classes_dir = srcdir / "_classes"
    if classes_dir.exists():
        shutil.rmtree(classes_dir)
    classes_dir.mkdir(parents=True, exist_ok=True)

    all_rels = {str(p.relative_to(srcdir)) for p in srcdir.rglob("*.java")
                if not str(p.relative_to(srcdir)).startswith("_")}
    if drop is None:
        total, per, _ = compile_dir(srcdir, classpath)
        if total == 0:
            return 0, set()
        dropped = set(per)
    else:
        dropped = set(drop)

    jar_names: set[str] = set()
    if orig_jar and orig_jar.exists():
        with zipfile.ZipFile(orig_jar) as z:
            jar_names = set(z.namelist())

    def _seed(rels: set[str]):
        if not (orig_jar and orig_jar.exists()):
            return
        with zipfile.ZipFile(orig_jar) as z:
            for rel in rels:
                stem = rel[:-len(".java")] if rel.endswith(".java") else rel
                cn = stem + ".class"
                if cn in jar_names:
                    (classes_dir / cn).parent.mkdir(parents=True, exist_ok=True)
                    (classes_dir / cn).write_bytes(z.read(cn))

    def _compile(keep_rels: set[str]) -> dict[str, int]:
        listf = srcdir / "_sources_clean.txt"
        keep = [srcdir / r for r in sorted(keep_rels)]
        listf.write_text("\n".join(str(f) for f in keep), encoding="utf-8")
        cp = str(classes_dir)
        if classpath:
            cp = classpath + os.pathsep + cp
        cmd = [os.environ.get("ARLECCHINO_JAVAC", "javac"),
               "-d", str(classes_dir), "--release", "8",
               "-encoding", "utf-8", "-nowarn", "-proc:none", "-cp", cp,
               f"@{listf}"]
        r = subprocess.run(cmd, capture_output=True, text=True)
        per: dict[str, int] = {}
        for m in _ERR_RE.finditer(r.stderr):
            try:
                rel = str(Path(m.group(1)).resolve().relative_to(srcdir.resolve()))
            except Exception:  # noqa: BLE001
                rel = os.path.basename(m.group(1))
            per[rel] = per.get(rel, 0) + 1
        return per

    # iterate: seed dropped, compile the rest, drop newcomers, repeat
    for _ in range(len(all_rels) + 1):
        _seed(dropped)
        keep = all_rels - dropped
        per = _compile(keep)
        new_bad = set(per) - dropped
        if not new_bad:
            break
        dropped |= new_bad
    return len(dropped), dropped
