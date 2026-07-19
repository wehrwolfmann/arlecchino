# Arlecchino — meta-decompiler & J2ME port toolkit

A **meta-decompiler** for Java plus a small **J2ME / MIDP porting toolkit**.
It runs several decompilers on the same `.jar`, **normalises their output to one
canonical class-name space**, compiles each variant, and assembles a *best-of*
source tree — for every class (optionally every method) it keeps the variant with
the fewest compilation errors. It then **auto-fixes** common decompiler artifacts,
**round-trips** the result back to bytecode to measure coverage, can **deobfuscate**
for readability, and can **emulate** the original jar in a bundled J2ME runtime.

Its headline trick for **ProGuard-obfuscated** jars is a **bytecode-first canonical
member renamer** (ASM): before decompiling, it rewrites the jar so every field and
method has a Java-*source-legal*, unique name — the single biggest source of
uncompilable decompiler output on real obfuscated jars. On the Allods J2ME game jar
this takes the honest isolated error count from **712 → 2** (see below): the renamer
fixes member-collision **and** type-name-shadowing illegalities in bytecode, and a
small cross-engine auto-fix layer clears the few remaining decompiler type-inference
artifacts. The only 2 residual errors are an irreducible package-layout constraint in
the jar itself (a `Container.GameMIDlet` referencing default-package types — not
expressible in Java source without flattening packages, a porting decision left to
the user).

> **Credit / inspiration.** The core idea (decompiler diversity → per-class/per-method
> best-fragment selection) comes from *“Java Decompiler Diversity and its Application
> to Meta-decompilation”* by Harrand, Soto-Valero, Monperrus, Baudry
> ([arXiv:2005.11315](https://arxiv.org/abs/2005.11315)) and their research prototype
> *Arlecchino*. **This is an independent clean-room reimplementation** — none of their
> code is used. Key difference: the original measures quality as **AST distance to the
> original source** (it targets open-source projects); this tool measures quality as
> **compilability** (number of `javac` errors), so it works on **obfuscated jars with
> no source** — the common real-world case (e.g. porting old J2ME/MIDP games).

## Install

Pure-stdlib core (needs a JDK 11+ on `PATH` for `java`/`javac`/`javap`/`jar`):

```bash
git clone … && cd arlecchino
pip install -e .            # installs the `arlecchino` command
# optional, only for the experimental per-method merge:
pip install javalang
```

You can also run it without installing: `python3 -m arlecchino …`
(`python3 -m arlecchino <jar>` is shorthand for the `decompile` subcommand).

## Commands

```
arlecchino setup     [--engines-root DIR] [--only vineflower,cfr,…]
                     [--freej2me-prebuilt P | --freej2me-src DIR] [--no-freej2me]
arlecchino decompile <jar> [-o OUT] [-cp DEPS] [--config C] [-j N]
                     [--midp-stub JAR | --no-midp-stub]
                     [--rename-members | --no-rename-members]
                     [--per-method] [--deobf] [--no-autofix] [--no-roundtrip]
arlecchino emulate   <jar> [--freej2me P] [-W 176] [-H 220] [-s 2]
```

`arlecchino <jar> …` is shorthand for `arlecchino decompile <jar> …` (back-compat).

### `setup` — get the engines + emulator

Downloads **Vineflower, CFR, Procyon, jadx** from their GitHub releases and builds
or fetches **FreeJ2ME** into `~/.arlecchino/engines/`. It also downloads **ASM**
(`org.ow2.asm`, BSD-3-Clause) from Maven Central into `~/.arlecchino/engines/asm/`
and compiles the bytecode member-renamer helper against it. Each tool keeps its own
license; nothing is vendored in this repo.

### `decompile` — multi-decompile + best-of merge

0. **Bytecode member-rename (AUTO).** If the jar has Java-*source-illegal* member
   collisions and ASM + the helper are available, rewrite the jar so every member is
   uniquely named *before* decompiling (see *Bytecode member renamer* below). The
   **renamed jar then becomes the API reference** for every later step, so the honest
   metric and round-trip stay consistent. Toggle with `--rename-members` /
   `--no-rename-members`.
1. Decompiles with all configured engines (in **parallel**, `-j`).
2. **Normalises names** so every engine lands in one canonical name space — in
   particular jadx's `defpackage` wrapper is un-wrapped so jadx (often the
   cleanest) joins the merge (see *Name normalisation* below).
3. Scores each class **in isolation** against the original jar as the API, which
   avoids `javac`'s batch **error-masking** (see *Honest error counts*) and ranks
   the engines honestly.
4. **Best-of merge:** per class, keeps the variant with the fewest isolated errors.
5. **Auto-fix pass:** rewrites universal artifacts and keeps a fix only if it
   strictly lowers the error count (never makes things worse).
6. **Round-trip check:** recompiles the merge to `.class` and reports how many of
   the original classes are covered, plus a coarse member-count diff vs the
   original bytecode.
7. Writes `merged/`, `report.md`, `report.json`, and per-engine trees under `-o`.

### `emulate` — run the jar in FreeJ2ME

Thin wrapper around FreeJ2ME's AWT front-end:
`java -jar freej2me.jar <midlet.jar> W H scale`.

## Features (status)

| # | Feature | Status |
|---|---|---|
| 1 | **Name normalisation across engines** (jadx → canonical) | **Done.** jadx keeps the original short names but wraps them in `defpackage`; we un-wrap, strip `package defpackage;` / `JADX INFO` comments and rewrite references, so jadx joins the merge as a first-class donor. |
| 2 | **Auto-fix artifacts** | **Done (universal subset).** `fold_interface_static_init` (constant-interface `static{}` → inline initialisers, *keeping it an interface* so `implements X` stays valid; preferred on ties over the class fallback for whole-tree consistency), `const_interface_to_class` fallback, `scratch_temp_dead_array_type` (a synthetic `varNNNNN` declared as one array type but dead-stored as another → widen the dead decl to `Object`), `resolve_unrepresentable_casts` (recover Vineflower's `(<unrepresentable>)` cast type from a sibling decompiler that named it), duplicate-import dedup, illegal `static` on locals. Each kept only if it lowers the error count. |
| 3 | **Per-method merge** | **Experimental** (`--per-method`, needs `javalang`). Splices method **and constructor** bodies by signature, isolated-scored. Works on clean cases; on this jar the cross-engine auto-fixes resolve the residuals more reliably than whole-method splicing (engines disagree on local/field names, so a strict-improvement splice rarely lands). |
| 4 | **Round-trip check** | **Done (pragmatic).** Coverage = original classes that compile; broken classes are seeded from original bytecode so dependents still resolve. Coarse member/method-count diff via `javap`. Not a semantic/behaviour proof. |
| 5 | **Auto-download engines + FreeJ2ME** | **Done.** `setup` pulls the four decompilers from GitHub releases and builds FreeJ2ME from source with `javac` (no Ant), or copies a prebuilt jar. Degrades gracefully offline. |
| 6 | **Config file + parallelism** | **Done.** TOML/JSON engine config (`engines.default.toml`, `~/.arlecchino/engines.toml`, or `--config`). Decompilers and per-class compiles run in a thread pool (`-j`). |
| 7 | **Deobfuscation pass** | **Partial** (`--deobf`). Heuristic MIDP-role annotation in `merged/` + a separate `jadx --deobf` tree. |
| 8 | **Bytecode member renamer** (ProGuard descriptor-overloads + type-name shadows) | **Done.** ASM pass rewrites the jar so every field/method is source-legal and unique *before* decompiling, preserving virtual dispatch and locking external (MIDP/JDK) overrides. Also fixes **type-name shadowing** (field named like a referenced in-jar type) and **constant-interface-inherited shadowing**. **712 → 2** isolated errors on Allods (the 2 are an irreducible package-layout constraint, not a renamer gap). See below. |

## Honest error counts (and why v1's numbers differ from a naive run)

`javac` is **all-or-nothing per invocation**: a single early structural error
(e.g. an illegal interface initializer) aborts attribution of the *other* files,
**masking** their real errors. A naive whole-tree compile of a badly obfuscated jar
can therefore report a deceptively tiny number. v1 instead **compiles each class in
isolation** against the original jar (which supplies the canonical API of every
sibling) and sums the per-class errors — an honest, unmaskable metric. It also
auto-adds a **MIDP stub** (FreeJ2ME's jar) to the classpath so
`javax.microedition.*` resolves instead of inflating/masking counts; override with
`-cp`, `--midp-stub`, or `--no-midp-stub`.

## Example (real result — Allods, an obfuscated J2ME game jar)

True isolated error totals (lower = better), **member-renamer OFF**:

| engine | classes | isolated errors |
|---|---|---|
| cfr | 30 | 978 |
| vineflower | 30 | 1053 |
| jadx | 32 | 1066 |
| procyon | 30 | 1183 |

With the **member-renamer ON** (default AUTO) every engine's raw count collapses
(procyon 1183 → 1, cfr 978 → 16, vineflower 1053 → 11, jadx 1066 → 7), and the
best-of merge drops from 712 to **2** (see *Bytecode member renamer* below).

**Best-of merge, member-renamer OFF (`--no-rename-members`): 712** — ~27 % fewer
than the best single engine, 100 % round-trip class coverage. The bulk of those 712
are caused by ProGuard descriptor-overloaded members (next section).

**Best-of merge, member-renamer ON (default AUTO): 2**, with **100 % (32/32)
round-trip class coverage** and the whole 31-class default-package tree compiling
together. Turning the renamer on also collapses every *single* engine's raw count
(e.g. procyon 1183 → 1, cfr 978 → 16, jadx 1066 → 7) — direct evidence the rewrite
makes the decompiled source compilable. The renamer now also fixes **type-name
shadowing** (a field named like an in-jar type it references, e.g. `class j {int o;}`
that uses `o.c` where `o` is also a type — the field shadows the type in source,
yielding 57 spurious `int cannot be dereferenced`) and **constant-interface-inherited
shadowing** (an `implements`-ed constant interface whose constant collides with an
inherited instance field). A tiny cross-engine auto-fix layer then resolves the last
decompiler artifacts: a synthetic-temp dead-store array-type conflict, and
Vineflower's `<unrepresentable>` casts (recovered from a sibling decompiler that named
the type). See below for the honest residual.

## Bytecode member renamer (the ProGuard descriptor-overload fix)

ProGuard reuses the **same member name with different descriptors** — many
fields/methods share a name with *different types*. This is legal in JVM bytecode
(members are keyed by name **and** descriptor) but **illegal in Java source**, so
decompiler output won't compile: `cannot find symbol`, `reference … is ambiguous`,
`… cannot be dereferenced`, `variable … is already defined`. Source-level textual
renaming can't fix it — a *use* site doesn't know which overloaded declaration it
binds to. So we rename in **bytecode, before decompiling**:

* A small Java helper (`arlecchino/java/MemberRenamer.java`) using **ASM** loads every
  class, builds the class/hierarchy model, and detects **only** the source-illegal
  collisions: (a) two fields in one class with the same name; (b) two methods in one
  class with the same name + parameter types but a different return type. `<init>` /
  `<clinit>` are never touched.
* **Hierarchy-aware:** methods linked by override (same name + full descriptor across
  in-jar super/sub/interface types) are unioned into one virtual group and renamed as a
  unit, so dispatch is preserved.
* **External-override locking (critical):** if a method's name + descriptor is declared
  by a supertype/interface **not in the jar** (`java/*`, `javax/microedition/*` MIDP),
  its group is **locked** and never renamed — `run()`, `paint(Graphics)`,
  `keyPressed(int)`, `startApp()`, … If a locked method collides with another, the
  **other** collider is renamed instead. External supertypes are resolved by reflection,
  loading the MIDP stub jar on the resolver classpath; unresolvable externals fall back
  to a well-known MIDP/JDK callback denylist (so we are safe offline without
  blanket-locking — which would prevent fixing genuine return-type collisions).
* **New-name scheme** (deterministic, minimal churn, collision-free): the first
  occurrence keeps its name; each other collider becomes `<orig>_<tag>` where `<tag>` is
  the first 8 hex digits of an FNV-1a-64 hash of the member's full descriptor (numeric
  `_2`, `_3`… bump only if still taken). Only actual colliders are renamed.
* All references — including shared NameAndType constant-pool entries — are rewritten by
  a custom `Remapper` + `ClassRemapper` over every class. A `mapping.txt`
  (`owner.oldName desc -> newName`) is written next to the renamed jar.

On Allods: 32 classes → **355 field renames, 50 method renames** (291 descriptor-
overload + 56 type-shadow + 8 constant-interface-shadow); the renamed jar verifies
(all 32 classes load through the JVM verifier, 0 failures), has **zero** remaining
duplicate-field / return-type collisions, and **no MIDP callback was renamed**
(verified: `GameMIDlet.startApp/pauseApp/destroyApp` and `i`'s
`run/showNotify/hideNotify/keyPressed/keyReleased` are all untouched).

#### Type-name-shadow field rename

A second source-illegality from single-namespace obfuscation: a class `C` declares a
field named `X` *and* references an in-jar **type** also named `X` via `X.member` /
`new X()` / `(X)cast`. Legal in bytecode (field access and type references are keyed
independently); in Java **source** the field `X` is in scope and shadows the type, so
`X.c` parses as `(this.X).c` → `int cannot be dereferenced`. The renamer detects this
precisely — rename `C.X` only when an in-jar type has simple name `X` **and** `C`
actually references that type's internal name in its bytecode — and renames the
**field** (the type can't move). On Allods this alone clears all 57 `int cannot be
dereferenced` (classes `j` 41, `e` 15, `i` 1).

#### Constant-interface-inherited-shadow field rename

A constant interface `I` (all-static fields) that a class hierarchy `implements` leaks
its constants into the implementor's member scope, where they collide with an inherited
instance field — `reference to b is ambiguous` (both the interface constant and the
superclass field match). The renamer renames the colliding **constant** in the
interface (a read-only, dispatch-irrelevant member). On Allods this clears the
ambiguous-reference residuals (`z`, `w`).

### Honest residual (2 errors, isolated metric)

After the renamer + the cross-engine auto-fix layer + the experimental per-method
merge, the honest isolated total is **2 errors, both in `Container/GameMIDlet`**, and
both are the *same* irreducible problem: `GameMIDlet` lives in package `Container` but
references default-package types (`i`, `a`) — and **the Java language has no syntax for
a named-package class to name an unnamed-package type** (JLS §6.5/§7.4.3). This is a
property of the original jar's package layout, not a decompiler or renamer defect, and
cannot be fixed by renaming a member or editing source text. Resolving it requires a
**package-flattening porting decision** (move everything into one package, as a working
manual port does) — deliberately left to the user, not baked into the generic tool.

> Calibration: the **2** is the honest isolated, unmaskable per-class count (each
> class compiled alone against the **renamed** jar + MIDP stub as the API), and was
> reproduced by an independent isolated re-measure of all 32 classes (31 clean,
> `GameMIDlet` = 2). The 31 default-package classes also compile **together** as a tree
> (whole-tree errors = 2, i.e. only `GameMIDlet`). Round-trip coverage (100 %, 32/32)
> is a structural smoke test — it proves every class compiles against the renamed jar
> API (broken classes seeded from bytecode so dependents resolve), **not** behavioural
> equivalence. We did not build or run the game.

> Note: an earlier prototype reported “1 error” on this jar — that was a `javac`
> **masking artifact** (one interface error hid ~700 real ones). The 712 / 2 figures
> here are the honest unmasked isolated counts; the merge genuinely beats every single
> engine.

## Configuration

Copy `arlecchino/engines.default.toml` to `~/.arlecchino/engines.toml` (or pass
`--config`) and edit paths/args. Each `[[engine]]` has `name`, `type`
(`names`/`renamed`), `kind` (`jar`/`tool`), `path`, `args` (with `{jar}`/`{out}`),
optional `sources_subdir` and `normalize` (`none`/`jadx`).

## License

**Apache-2.0** — see [LICENSE](LICENSE) and [NOTICE](NOTICE). Independent clean-room
work; third-party decompilers, FreeJ2ME, and ASM (BSD-3-Clause, used by the member
renamer) are invoked / downloaded as separate processes / artifacts under their own
licenses and are not bundled.
