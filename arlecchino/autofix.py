"""
Auto-fix common decompiler artifacts (point 2), as a post-merge pass.

These are universal, semantics-preserving rewrites that turn legal-bytecode /
illegal-Java constructs back into something javac accepts.  Each fix is applied
file-by-file and only KEPT if it strictly reduces that file's javac error count
(verified by the caller), so a fix can never make a file worse.

Implemented fixes:

  * const_interface_to_class
      A "constant interface": `interface X` whose fields are assigned in a
      `static {}` block or declared without an initialiser.  Java forbids
      `static {}` and uninitialised fields in interfaces, so the decompiler
      faithfully reproduces illegal Java.  Converting `interface X` -> `class X`
      makes it legal again WHEN the field names are unique.  (When two bytecode
      fields share a name with different descriptors — the `o.java` case — this
      fix alone is not enough; see README "Known limits".)

  * illegal_static_local
      A stray `static` modifier on a deeply-nested (local) declaration.
      Drop the `static`.

  * duplicate_imports
      The same `import a.b.C;` line appearing more than once — keep the first.

The dispatcher returns the rewritten text; the caller decides whether to keep it.
"""
from __future__ import annotations

import re

_INTERFACE_DECL = re.compile(
    r"(?P<pre>\bpublic\s+|\bfinal\s+|\babstract\s+|\bstrictfp\s+)*\binterface\s+"
    r"(?P<name>[A-Za-z_$][\w$]*)\b(?P<tail>[^\{]*)\{")
_STATIC_INIT = re.compile(r"\bstatic\s*\{")
_IMPORT_LINE = re.compile(r"^[ \t]*import\s+[\w.$*]+\s*;[ \t]*$")


def _is_constant_interface(text: str, m: "re.Match") -> bool:
    """Heuristic: an interface whose body contains a `static {}` block (only legal
    in a class) — the signature of a decompiled constant interface."""
    body = text[m.end():]
    return bool(_STATIC_INIT.search(body))


_IFACE_FIELD = re.compile(
    r"public\s+static\s+final\s+[\w$.]+(?:\s*\[\s*\])*\s+(?P<name>[A-Za-z_$][\w$]*)\s*;")


def _brace_block(text: str, start_kw: str):
    """Return (kw_start, body_start, body_end) for the brace block following the
    first occurrence of `start_kw`, or None."""
    si = text.find(start_kw)
    if si < 0:
        return None
    i = text.find("{", si)
    if i < 0:
        return None
    depth, j = 0, i
    while j < len(text):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return si, i, j
        j += 1
    return None


def fold_interface_static_init(text: str) -> str:
    """Keep a constant interface AS an interface but make it legal by folding its
    `static {}` block's `FIELD = value;` assignments into inline field initialisers
    (`public static final T FIELD = value;`).  This is the *correct* fix for a
    constant interface that is `implements`-ed by other classes (where converting
    to a class would break `implements`).  Only fires when EVERY declared
    uninitialised interface field is assigned exactly once in the static block (so
    we don't silently drop data); statements that assign locals (not fields) are
    dropped, matching the bytecode (their results were unused)."""
    m = _INTERFACE_DECL.search(text)
    if not m or not _is_constant_interface(text, m):
        return text
    blk = _brace_block(text, "static {")
    if not blk:
        return text
    _, body_start, body_end = blk
    body = text[body_start + 1:body_end]
    # collect declared, uninitialised interface fields (name -> full decl match)
    decls = {}
    for fm in _IFACE_FIELD.finditer(text[:blk[0]]):
        decls[fm.group("name")] = fm
    if not decls:
        return text
    inits = {}
    for line in body.splitlines():
        am = re.match(r"\s*([A-Za-z_$][\w$]*)\s*=\s*(.+);\s*$", line)
        if am and am.group(1) in decls:
            inits[am.group(1)] = am.group(2)
    # require every declared field to be initialised (no data loss)
    if set(inits) != set(decls):
        return text
    # rewrite each declaration to include its initialiser
    new = text
    # work back-to-front so earlier offsets stay valid
    for name, fm in sorted(decls.items(), key=lambda kv: kv[1].start(), reverse=True):
        decl = fm.group(0)                       # "... NAME ;"
        new_decl = decl[:-1].rstrip().rstrip(";").rstrip() + f" = {inits[name]};"
        new = new[:fm.start()] + new_decl + new[fm.end():]
    # remove the now-empty static block (recompute its span in `new`)
    blk2 = _brace_block(new, "static {")
    if blk2:
        ks, _, be = blk2
        # also swallow trailing newline/whitespace
        end = be + 1
        new = new[:ks] + new[end:]
    return new


def const_interface_to_class(text: str) -> str:
    """Convert the FIRST top-level constant interface to a final class.
    Conservative: only fires when a `static {}` block is present and the interface
    has no `extends` clause (an interface's superinterfaces can't become a class
    superclass)."""
    m = _INTERFACE_DECL.search(text)
    if not m or not _is_constant_interface(text, m):
        return text
    pre = m.group("pre") or ""
    name = m.group("name")
    tail = m.group("tail") or ""
    if re.search(r"\bextends\b", tail):
        return text
    repl = f"{pre}final class {name}{tail}{{"
    return text[:m.start()] + repl + text[m.end():]


def illegal_static_local(text: str) -> str:
    """Drop a `static` modifier that appears on a deeply-nested declaration
    (8+ spaces of indentation => almost certainly a local, where `static` is
    illegal).  Class members sit at shallower indentation and are untouched."""
    return re.sub(r"(?m)^(?P<ind>[ \t]{8,})static\s+(?=[A-Za-z_$])",
                  lambda mm: mm.group("ind"), text)


_SCRATCH_DEAD_DECL = re.compile(
    r"(?P<ind>[ \t]*)(?P<dtype>[A-Za-z_$][\w$.]*)\s*\[\s*\]\s+"
    r"(?P<var>var\d+)\s*=\s*new\s+(?P<itype>[A-Za-z_$][\w$.]*)\s*\[\s*\]\s*\{"
)


def scratch_temp_dead_array_type(text: str) -> str:
    """Repair a synthetic decompiler scratch temp whose array DECLARATION type
    conflicts with its initialiser's array type, when that initialiser is a DEAD
    store (the very next statement reassigns the same temp).

    Decompilers sometimes coalesce several short-lived array temporaries into one
    synthetic local `varNNNNN`, emitting e.g.

        int[] var10000 = new char[]{' ', ...};   // declared int[], assigned char[]
        var10000 = new int[]{0, 2, 3, 4};        // reassigned (the decl value is dead)
        var10000 = new int[]{0, 1, 3, 4};

    which is `incompatible types: char[] cannot be converted to int[]`.  Because the
    declaration's value is never read (the next statement overwrites it, and array-
    literal construction has no side effects), widening the *declaration* to `Object`
    is type-safe and lets every later `varNNNNN = new T[]{...}` reassignment stand.

    Strictly guarded (universal, not corpus-specific):
      * the variable name matches the synthetic temp pattern ``var<digits>``;
      * the declared array element type differs from the initialiser's;
      * the IMMEDIATELY following non-blank statement reassigns the same temp
        (`varNNNNN = ...;`), proving the declaration's value is dead.
    The caller still keeps the rewrite only if it lowers the error count, so a
    mis-fire can never make a file worse."""
    # collect rewrites first, apply back-to-front so earlier offsets stay valid even
    # though `Object` and `T[]` differ in length.
    edits = []
    for m in _SCRATCH_DEAD_DECL.finditer(text):
        if m.group("dtype") == m.group("itype"):
            continue  # declared and initialiser element types agree -> legal, skip
        var = m.group("var")
        # the next non-blank line after this declaration must reassign the same temp
        # (dead-store proof: the declaration's value is never read).
        nl = text.find("\n", m.start())
        rest = text[nl + 1:] if nl >= 0 else ""
        nxt = next((ln.strip() for ln in rest.splitlines() if ln.strip()), None)
        if not nxt or not re.match(rf"{re.escape(var)}\s*=", nxt):
            continue
        # widen just the declaration's type to Object (keep indentation + var name)
        seg = text[m.start():m.end()]
        seg_new = seg.replace(f'{m.group("dtype")}[] {var} =', f'Object {var} =', 1)
        if seg_new != seg:
            edits.append((m.start(), m.end(), seg_new))
    out = text
    for start, end, seg_new in reversed(edits):
        out = out[:start] + seg_new + out[end:]
    return out


_UNREP = "(<unrepresentable>)"
_CAST_BEFORE = re.compile(r"\(([A-Za-z_$][\w$.]*)\)\s*$")


def resolve_unrepresentable_casts(text: str, donor_texts: list[str]) -> str:
    """Replace Vineflower's ``(<unrepresentable>)`` placeholder casts with the real
    cast type recovered from another engine's rendering of the *same* expression.

    Vineflower emits the literal token ``<unrepresentable>`` as a cast type when it
    cannot name the target type (e.g. casting ``Enumeration.nextElement()`` to a
    package-private class), producing uncompilable source.  Other engines (CFR,
    Procyon, jadx) usually DO name it.  For each ``(<unrepresentable>)EXPR`` we take
    the text immediately AFTER the cast on that line (the casted expression + the
    member access that follows, e.g. ``var2.nextElement()).a_af64174c``), find a
    donor line that contains the same trailing fragment preceded by a normal cast
    ``(Type)``, and reuse ``Type``.  Purely textual and cross-checked by the caller's
    compile gate, so a wrong guess is discarded.  Generic: no type names are
    hard-coded; the replacement type always comes from a sibling decompiler."""
    if _UNREP not in text:
        return text
    out_lines = []
    for line in text.splitlines(keepends=True):
        if _UNREP not in line:
            out_lines.append(line)
            continue
        idx = line.find(_UNREP)
        # the casted-expression fragment that follows the placeholder on this line.
        tail = line[idx + len(_UNREP):].strip()
        tail = (tail.split("=")[0] if "=" in tail else tail).rstrip(";").strip()
        # Build a LOCAL-NAME-INDEPENDENT anchor: decompilers name the casted local
        # differently (var2 vs enumeration vs elements), but the member access that
        # FOLLOWS the cast — the first `.method()`/`.field` chain — is stable. Anchor
        # on the substring from the first '.' (e.g. ".nextElement()).a_af64174c").
        dot = tail.find(".")
        anchor = tail[dot:] if dot >= 0 else tail
        repl_type = None
        if len(anchor) >= 6:
            for dt in donor_texts:
                pos = dt.find(anchor)
                while pos != -1 and repl_type is None:
                    # the donor's casted variable sits just before `anchor`; its cast
                    # type sits just before THAT identifier.  Walk back over the local
                    # name, then read the `(Type)` cast in front of it.
                    pre = dt[:pos].rstrip()
                    vm = re.search(r"([A-Za-z_$][\w$]*)$", pre)
                    if vm:
                        before_var = pre[:vm.start()].rstrip()
                        cm = _CAST_BEFORE.search(before_var)
                        if cm and cm.group(1) != "<unrepresentable>":
                            repl_type = cm.group(1)
                    pos = dt.find(anchor, pos + 1)
                if repl_type:
                    break
        if repl_type:
            line = line.replace(_UNREP, f"({repl_type})")
        out_lines.append(line)
    return "".join(out_lines)


def duplicate_imports(text: str) -> str:
    seen: set[str] = set()
    out_lines = []
    for line in text.splitlines(keepends=True):
        if _IMPORT_LINE.fullmatch(line.rstrip("\n")):
            key = line.strip()
            if key in seen:
                continue          # drop the duplicate import line entirely
            seen.add(key)
        out_lines.append(line)
    return "".join(out_lines)


# ordered list of (name, function); each is tried independently
FIXES = [
    # Prefer folding the static block into inline initialisers (keeps the type an
    # interface, so `implements X` in other classes stays valid).  Only if that
    # cannot fire (e.g. not all fields initialised) do we fall back to converting
    # the interface to a class.  fold removes the static block, so the fallback is
    # naturally a no-op once fold succeeded.
    ("fold_interface_static_init", fold_interface_static_init),
    ("const_interface_to_class", const_interface_to_class),
    ("scratch_temp_dead_array_type", scratch_temp_dead_array_type),
    ("duplicate_imports", duplicate_imports),
    ("illegal_static_local", illegal_static_local),
]


def apply_all(text: str):
    """Apply every fix in sequence (best-effort).  Returns (new_text, applied)."""
    applied = []
    cur = text
    for name, fn in FIXES:
        nxt = fn(cur)
        if nxt != cur:
            applied.append(name)
            cur = nxt
    return cur, applied
