"""
Arlecchino — CLI with subcommands (point: toolkit restructure).

  arlecchino setup     [--engines-root DIR] [--only a,b] [--freej2me-prebuilt P]
  arlecchino decompile <jar> [-o OUT] [-cp DEPS] [--config C] [-j N]
                             [--per-method] [--deobf] [--no-autofix] [--no-roundtrip]
  arlecchino emulate   <jar> [--freej2me P] [-W 176] [-H 220] [-s 2]

Backward compatibility: `arlecchino <something.jar>` (or running the old
python -m arlecchino on a jar) is treated as `decompile <jar>`.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

from . import __version__, core, deobf, permethod, rename, roundtrip
from .config import ENGINES_ROOT, load_engines
from .emulate import emulate, find_freej2me
from .setup_engines import setup_engines, setup_freej2me


# ─────────────────────────────── decompile ──────────────────────────────────
def cmd_decompile(args) -> int:
    jar = Path(args.jar).resolve()
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if not jar.exists():
        print(f"jar not found: {jar}"); return 2

    engines, cfg, root = load_engines(args.config)
    print(f"== Arlecchino v{__version__} :: {jar.name} ==")
    print(f"   config: {cfg}")
    print(f"   engines root: {root}")

    # Resolve the classpath used for *all* compilations.  J2ME jars depend on the
    # MIDP/CLDC API (javax.microedition.*) which is NOT in the JDK; without it,
    # `cannot find symbol` errors from missing MIDP classes are reported — and,
    # worse, a single early structural error can MASK them (javac aborts before
    # attributing later files).  So if the user gave no classpath we auto-use
    # FreeJ2ME's jar as a MIDP stub, which makes the error counts honest.  Disable
    # with --no-midp-stub.
    classpath = args.classpath
    if classpath is None and not args.no_midp_stub:
        fj = find_freej2me(getattr(args, "midp_stub", None), root)
        if fj:
            classpath = str(fj)
            print(f"   MIDP stub (auto): {fj}")
        else:
            print("   MIDP stub: none found (run `arlecchino setup`, or pass "
                  "-cp / --no-midp-stub). javax.microedition.* will count as errors.")

    # ── Bytecode-first canonical MEMBER RENAMER (point: descriptor-overloaded
    #    members).  ProGuard reuses a member name with different descriptors (legal
    #    bytecode, illegal Java source) → decompiler output won't compile.  We rewrite
    #    the jar with ASM so every field/method gets a unique, source-legal name BEFORE
    #    decompiling, preserving virtual dispatch and locking external (MIDP/JDK)
    #    overrides.  The RENAMED jar then becomes the API reference for the whole
    #    pipeline (so isolated scoring + round-trip stay consistent).
    rename_mode = getattr(args, "rename_mode", "auto")
    orig_jar = jar
    jar, rename_map = rename.maybe_rename(
        jar, out / "_rename", root=root, mode=rename_mode, framework_cp=classpath)
    if jar is not orig_jar:
        print(f"   member-rename: using renamed jar -> {jar} "
              f"(mapping: {rename_map})")

    data = core.decompile_all(engines, jar, out, root, classpath, args.jobs)
    if not data:
        print("No working engines (run `arlecchino setup`)."); return 1

    ranking = sorted(data.items(), key=lambda kv: (kv[1]["errors"], -kv[1]["nclasses"]))
    print("\n== RANKING (fewer errors = better) ==")
    for n, d in ranking:
        print(f"  {n:12} classes={d['nclasses']:3} errors={d['errors']}")

    # per-class report
    all_classes = sorted({c for d in data.values() for c in d["files"]})
    per_class = {}
    for c in all_classes:
        clean = [n for n, d in data.items() if c in d["files"] and d["per"].get(c, 0) == 0]
        per_class[c] = {"clean_engines": clean,
                        "errors": {n: data[n]["per"].get(c)
                                   for n in data if c in data[n]["files"]}}

    names_rank = [(n, d) for n, d in ranking if d["type"] == "names"]
    merged_dir = merged_total = None
    merge_log: list[str] = []
    per_iso: dict[str, int] = {}
    if names_rank:
        print("\n== BEST-OF MERGE (isolated per-class scoring) ==")
        merged_dir, merged_total, merge_log, per_iso = core.merge_isolated(
            data, out, jar, classpath, do_autofix=not args.no_autofix)
        for l in merge_log:
            print("  " + l)

        if args.per_method and merged_total and merged_total > 0:
            print("\n== PER-METHOD MERGE (experimental) ==")
            api_cp = core.build_api_classpath(jar, classpath, out)
            pm_log = permethod.merge_per_method(
                data, merged_dir, per_iso, api_cp,
                core.compile_file_isolated, out / "_pm")
            for l in pm_log:
                print("  " + l)
            merge_log += pm_log
            merged_total = sum(per_iso.values())

    # round-trip
    rt = None
    if merged_dir and not args.no_roundtrip:
        print("\n== ROUND-TRIP CHECK ==")
        # Files that still fail in isolation are the ones to exclude.  Seed their
        # original bytecode so dependents resolve, then compile the rest against
        # the original jar + MIDP stub (the canonical API), unmasked.
        broken = {rel for rel, e in per_iso.items() if e}
        ndrop, dropped = core.compile_best_effort(
            merged_dir, jar, classpath, drop=broken)
        if dropped:
            print(f"  ({ndrop} class(es) still broken, excluded from coverage: "
                  f"{', '.join(sorted(dropped))})")
        rt = roundtrip.check(jar, merged_dir / "_classes")
        rt["excluded_broken"] = sorted(dropped)
        print(f"  coverage: {rt['present']}/{rt['total_original']} "
              f"classes ({rt['coverage_pct']}%)")
        if rt["missing"]:
            print(f"  MISSING compiled classes: {', '.join(rt['missing'])}")
        if rt["structural_mismatch"]:
            print(f"  structural mismatches: {len(rt['structural_mismatch'])} "
                  f"(member/method counts differ from original bytecode)")
            for m in rt["structural_mismatch"][:10]:
                print(f"    {m['class']}: orig{m['orig']} merged{m['merged']}")

    # deobf
    deobf_info = None
    if args.deobf and merged_dir:
        print("\n== DEOBFUSCATION (readability) ==")
        n = deobf.annotate_in_place(merged_dir)
        deobf_dir = out / "deobf"
        jadx_bin = root / "jadx" / "bin" / "jadx"
        emitted = deobf.emit_jadx_deobf(jar, jadx_bin, deobf_dir)
        deobf_info = {"annotated": n, "jadx_deobf_tree": str(deobf_dir) if emitted else None}
        print(f"  annotated {n} classified file(s) in merged/")
        print(f"  jadx --deobf tree: {'-> ' + str(deobf_dir) if emitted else 'unavailable'}")

    # reports
    report = {
        "version": __version__, "jar": str(jar),
        "original_jar": str(orig_jar),
        "member_rename": {
            "applied": jar is not orig_jar,
            "mode": rename_mode,
            "renamed_jar": str(jar) if jar is not orig_jar else None,
            "mapping": str(rename_map) if rename_map else None,
        },
        "ranking": [{"engine": n, "classes": d["nclasses"], "errors": d["errors"]}
                    for n, d in ranking],
        "best_base": names_rank[0][0] if names_rank else None,
        "merged_errors": merged_total, "merge_log": merge_log,
        "per_class": per_class, "roundtrip": rt, "deobf": deobf_info,
    }
    (out / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2),
                                     encoding="utf-8")
    _write_md(out, jar, ranking, report, merged_total, merge_log, per_class, all_classes, rt)
    print(f"\nDone. merged -> {merged_dir}")
    print(f"Reports: {out/'report.md'}, {out/'report.json'}")
    return 0


def _write_md(out, jar, ranking, report, merged_total, merge_log, per_class, all_classes, rt):
    md = [f"# Arlecchino — report (v{report['version']})",
          f"jar: `{jar.name}`", "", "## Engine ranking", "",
          "| engine | classes | errors |", "|---|---|---|"]
    for n, d in ranking:
        md.append(f"| {n} | {d['nclasses']} | {d['errors']} |")
    md += ["", f"**best base:** {report['best_base']}",
           f"**merged errors:** {merged_total}", "", "## Merge log", "```"] \
        + merge_log + ["```", ""]
    if rt:
        md += ["## Round-trip", "",
               f"- coverage: {rt['present']}/{rt['total_original']} ({rt['coverage_pct']}%)",
               f"- missing: {', '.join(rt['missing']) or 'none'}",
               f"- structural mismatches: {len(rt['structural_mismatch'])}", ""]
    md += ["## Classes nobody compiles cleanly"]
    for c in all_classes:
        if not per_class[c]["clean_engines"]:
            md.append(f"- `{c}` — errors: {per_class[c]['errors']}")
    (out / "report.md").write_text("\n".join(md), encoding="utf-8")


# ─────────────────────────────── setup ──────────────────────────────────────
def cmd_setup(args) -> int:
    root = Path(args.engines_root).expanduser() if args.engines_root else ENGINES_ROOT
    only = [s.strip() for s in args.only.split(",")] if args.only else None
    print(f"== Arlecchino setup -> {root} ==")
    res = setup_engines(root, only)
    for k, v in res.items():
        print(f"  {k:12} {v}")
    if not args.no_freej2me:
        print("  freej2me     ...")
        fr = setup_freej2me(root, Path(args.freej2me_prebuilt) if args.freej2me_prebuilt else None,
                            Path(args.freej2me_src) if args.freej2me_src else None)
        print(f"  freej2me     {fr}")
    print("\nTip: copy arlecchino/engines.default.toml to ~/.arlecchino/engines.toml to customise.")
    return 0


# ─────────────────────────────── emulate ────────────────────────────────────
def cmd_emulate(args) -> int:
    jar = Path(args.jar).resolve()
    if not jar.exists():
        print(f"jar not found: {jar}"); return 2
    fj = find_freej2me(args.freej2me)
    if not fj:
        print("FreeJ2ME not found — run `arlecchino setup` or pass --freej2me PATH")
        return 1
    return emulate(jar, fj, args.width, args.height, args.scale)


# ─────────────────────────────── parser ─────────────────────────────────────
def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="arlecchino",
                                 description="Meta-decompiler / J2ME port toolkit (Apache-2.0)")
    ap.add_argument("--version", action="version", version=f"arlecchino {__version__}")
    sub = ap.add_subparsers(dest="cmd")

    d = sub.add_parser("decompile", help="multi-decompile + best-of merge")
    d.add_argument("jar")
    d.add_argument("-o", "--out", default="arlecchino_out")
    d.add_argument("-cp", "--classpath", default=None,
                   help="dependency classpath (e.g. a MIDP stub); overrides auto stub")
    d.add_argument("--midp-stub", default=None,
                   help="path to a MIDP/CLDC stub jar (default: auto-find FreeJ2ME)")
    d.add_argument("--no-midp-stub", action="store_true",
                   help="do not auto-add a MIDP stub classpath")
    d.add_argument("--config", default=None)
    d.add_argument("-j", "--jobs", type=int, default=4)
    d.add_argument("--rename-members", dest="rename_mode", action="store_const",
                   const="on", default="auto",
                   help="force the bytecode member-renamer ON (default: auto — run "
                        "only when source-illegal member collisions are detected and "
                        "ASM+helper are available)")
    d.add_argument("--no-rename-members", dest="rename_mode", action="store_const",
                   const="off", help="disable the bytecode member-renamer")
    d.add_argument("--per-method", action="store_true", help="experimental per-method merge")
    d.add_argument("--deobf", action="store_true", help="readability deobfuscation pass")
    d.add_argument("--no-autofix", action="store_true")
    d.add_argument("--no-roundtrip", action="store_true")
    d.set_defaults(func=cmd_decompile)

    s = sub.add_parser("setup", help="download engines + FreeJ2ME")
    s.add_argument("--engines-root", default=None)
    s.add_argument("--only", default=None, help="comma list: vineflower,cfr,procyon,jadx")
    s.add_argument("--no-freej2me", action="store_true")
    s.add_argument("--freej2me-prebuilt", default=None)
    s.add_argument("--freej2me-src", default=None)
    s.set_defaults(func=cmd_setup)

    e = sub.add_parser("emulate", help="run a J2ME jar in FreeJ2ME")
    e.add_argument("jar")
    e.add_argument("--freej2me", default=None)
    e.add_argument("-W", "--width", type=int, default=176)
    e.add_argument("-H", "--height", type=int, default=220)
    e.add_argument("-s", "--scale", type=int, default=2)
    e.set_defaults(func=cmd_emulate)
    return ap


def main(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    # backward-compat: `arlecchino foo.jar ...` == `arlecchino decompile foo.jar ...`
    if argv and not argv[0].startswith("-") and argv[0] not in ("decompile", "setup", "emulate"):
        argv = ["decompile"] + argv
    ap = build_parser()
    args = ap.parse_args(argv)
    if not getattr(args, "func", None):
        ap.print_help(); return 1
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
