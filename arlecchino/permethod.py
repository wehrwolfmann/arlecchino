"""
Per-method merge (point 3) — EXPERIMENTAL.

When no single engine compiles a class cleanly, try to assemble the class body
from the methods of different engines: keep each method's text from whichever
engine yields the fewest errors for the whole reconstructed file.

This is research-grade and intentionally guarded behind --per-method:

  * It uses `javalang` to locate method boundaries (by position) so it can splice
    method bodies textually without a full Java pretty-printer.  We splice TEXT,
    not AST, to preserve each engine's exact formatting/casts.
  * Methods are matched across engines by signature key (name + arity + erased
    param type names).  Overloads with identical erased keys are skipped (left as
    base) to stay safe.
  * Decompilers differ in field/local naming, so a spliced method may reference a
    name that doesn't exist in the base class -> we ONLY keep a splice if it
    strictly reduces the file's javac error count (same safety rule as the rest of
    the toolkit).  In practice this means per-method help is modest and very
    dependent on the engines agreeing on member names; it shines mainly when
    engines share naming (CFR/Procyon/Vineflower on non-colliding members).

Honest limitation: for descriptor-collision classes (e.g. the Allods `o.java`
constant interface where two fields share the name `a`), per-method merge does
NOT help because the problem is field-level, not method-level, and the names are
ambiguous in Java source.  That case is documented in the README.
"""
from __future__ import annotations

import shutil
from pathlib import Path

try:
    import javalang  # type: ignore
    HAVE_JAVALANG = True
except Exception:  # noqa: BLE001
    HAVE_JAVALANG = False


def _method_spans(text: str):
    """Return [(key, start_line, end_line)] for top-level method declarations.
    Lines are 1-based; end is the line of the closing brace (approx via brace
    counting from the method's opening brace)."""
    if not HAVE_JAVALANG:
        return []
    try:
        tree = javalang.parse.parse(text)
    except Exception:  # noqa: BLE001
        return []
    lines = text.splitlines()
    spans = []
    # Include CONSTRUCTORS as well as methods: decompiler type-inference artifacts
    # (e.g. an array literal mis-typed as int[] instead of char[]) frequently land in
    # <init>, and javalang models those as ConstructorDeclaration, not
    # MethodDeclaration.  We key a constructor as "<init>(params)" so it matches the
    # same constructor across engines (its source name is the class name, which can
    # differ if a normaliser renamed the type, so we normalise to <init>).
    decls = []
    for _, node in tree.filter(javalang.tree.MethodDeclaration):
        decls.append(("m", node))
    for _, node in tree.filter(javalang.tree.ConstructorDeclaration):
        decls.append(("c", node))
    for kind, node in decls:
        if node.position is None:
            continue
        start = node.position.line
        params = ",".join(p.type.name if hasattr(p.type, "name") else "?"
                          for p in node.parameters)
        mname = "<init>" if kind == "c" else node.name
        key = f"{mname}({params})"
        # find opening brace at/after start, then brace-match
        i = start - 1
        depth = 0
        open_found = False
        end = None
        while i < len(lines):
            for ch in lines[i]:
                if ch == "{":
                    depth += 1
                    open_found = True
                elif ch == "}":
                    depth -= 1
                    if open_found and depth == 0:
                        end = i + 1
                        break
            if end is not None:
                break
            i += 1
        if end is None:            # abstract / interface method (no body)
            end = start
        spans.append((key, start, end))
    return spans


def _replace_lines(text: str, start: int, end: int, repl: str) -> str:
    lines = text.splitlines(keepends=True)
    new = lines[:start - 1] + [repl if repl.endswith("\n") else repl + "\n"] + lines[end:]
    return "".join(new)


def _extract(text: str, start: int, end: int) -> str:
    lines = text.splitlines(keepends=True)
    return "".join(lines[start - 1:end])


def per_method_merge(rel: str, base_text: str, donor_texts: dict[str, str],
                     compile_one, ) -> tuple[str, list[str]]:
    """Try to lower the error count of one file by swapping individual methods
    from donor engines.  `compile_one(text) -> error_count`.  Returns
    (best_text, log)."""
    if not HAVE_JAVALANG:
        return base_text, ["per-method: javalang unavailable, skipped"]
    log: list[str] = []
    cur = base_text
    cur_err = compile_one(cur)
    if cur_err == 0:
        return cur, log

    base_spans = {k: (s, e) for k, s, e in _method_spans(cur)}
    for key, (s, e) in list(base_spans.items()):
        improved = False
        for dn, dtext in donor_texts.items():
            dspans = {k: (ds, de) for k, ds, de in _method_spans(dtext)}
            if key not in dspans:
                continue
            ds, de = dspans[key]
            donor_method = _extract(dtext, ds, de)
            # recompute current span (it may have shifted after a prior splice)
            cur_spans = {k: (cs, ce) for k, cs, ce in _method_spans(cur)}
            if key not in cur_spans:
                break
            cs, ce = cur_spans[key]
            cand = _replace_lines(cur, cs, ce, donor_method.rstrip("\n"))
            t2 = compile_one(cand)
            if t2 < cur_err:
                cur, cur_err = cand, t2
                log.append(f"  {rel}::{key} <- {dn} (errors->{cur_err})")
                improved = True
                break
        if cur_err == 0:
            break
    return cur, log


def merge_per_method(engine_data: dict, merge_dir: Path, per_iso: dict[str, int],
                     api_classpath: str, compile_file_isolated, tmp: Path) -> list[str]:
    """Run per-method merge, in place, over files that still fail in ISOLATION.

    Scoring is per single file (isolated, unmasked) — fast and not corrupted by
    javac's batch error-masking.  For each failing file we try, method-by-method,
    swapping in each donor engine's version of that method, keeping a swap only if
    it strictly lowers the file's isolated error count.
    """
    if not HAVE_JAVALANG:
        return ["per-method merge: javalang not installed (pip install javalang), skipped"]
    tmp.mkdir(parents=True, exist_ok=True)
    failing = {rel for rel, e in per_iso.items() if e}
    log = [f"per-method start: {len(failing)} failing class(es), "
           f"isolated-sum={sum(per_iso.values())}"]

    def compile_one(text: str, rel: str) -> int:
        tf = tmp / rel
        tf.parent.mkdir(parents=True, exist_ok=True)
        tf.write_text(text, encoding="utf-8")
        return compile_file_isolated(tf, api_classpath, tmp / "_o")

    names = [n for n, d in engine_data.items() if d["type"] == "names"]
    improved = 0
    for rel in sorted(failing):
        target = merge_dir / rel
        if not target.exists():
            continue
        base_text = target.read_text(encoding="utf-8", errors="replace")
        donor_texts = {}
        for n in names:
            cand = engine_data[n]["files"].get(rel)
            if cand:
                donor_texts[n] = Path(cand).read_text(encoding="utf-8", errors="replace")
        best, sub = per_method_merge(rel, base_text, donor_texts,
                                     lambda t, r=rel: compile_one(t, r))
        if best != base_text and compile_one(best, rel) < per_iso[rel]:
            target.write_text(best, encoding="utf-8")
            per_iso[rel] = compile_one(best, rel)
            log += sub
            improved += 1
    log.append(f"per-method result: improved {improved} class(es), "
               f"isolated-sum={sum(per_iso.values())}")
    return log
