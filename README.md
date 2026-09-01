# Arlecchino — meta-decompiler & J2ME port toolkit

A **meta-decompiler** for Java plus a small **J2ME / MIDP porting toolkit**, written
in Rust. It runs several decompilers on the same `.jar`, **normalises their output to
one canonical class-name space**, compiles each variant, and assembles a *best-of*
source tree — for every class (optionally every method) it keeps the variant with the
fewest compilation errors. It then **auto-fixes** common decompiler artifacts,
**round-trips** the result back to bytecode to measure coverage, can **deobfuscate**
for readability, and can **emulate** the original jar in a bundled J2ME runtime.

Its headline trick for **ProGuard-obfuscated** jars is a **bytecode-first canonical
member renamer** (ASM): before decompiling, it rewrites the jar so every field and
method has a Java-*source-legal*, unique name — the single biggest source of
uncompilable decompiler output on real obfuscated jars.

There are two front-ends over the same core:

- **`arlecchino`** — the command-line tool (below);
- **`arlecchino-gui`** — a desktop control panel (steam_dark theme) that wraps the same
  commands with buttons, file pickers and a live log, plus a system-tray icon and an
  "add to application menu" action.

> **Credit / inspiration.** The core idea (decompiler diversity → per-class/per-method
> best-fragment selection) comes from *"Java Decompiler Diversity and its Application
> to Meta-decompilation"* by Harrand, Soto-Valero, Monperrus, Baudry
> ([arXiv:2005.11315](https://arxiv.org/abs/2005.11315)) and their research prototype
> *Arlecchino*. **This is an independent clean-room reimplementation** — none of their
> code is used. Key difference: the original measures quality as **AST distance to the
> original source**; this tool measures quality as **compilability** (number of `javac`
> errors), so it works on **obfuscated jars with no source** — the common real-world
> case (e.g. porting old J2ME/MIDP games).

## Install

### Prebuilt binary (recommended)

A prebuilt `arlecchino` (and `arlecchino-gui`) binary for Linux x86-64 is provided on
the project's **GitHub Releases** page — download it, `chmod +x`, and run. No compiler
needed. The only runtime requirement is a **JDK 11+** on `PATH`
(`java`/`javac`/`javap`/`jar`); `git` is needed only if you build FreeJ2ME from source.

### Build from source

Needs a Rust toolchain (edition 2021) and, at runtime, a JDK 11+ on `PATH`.

```bash
git clone … && cd arlecchino
cargo build --release                       # -> target/release/arlecchino  (CLI)
cargo build --release --features gui,tray   # -> target/release/arlecchino-gui  (GUI + tray)
```

## Commands

```
arlecchino setup     [--engines-root DIR] [--only vineflower,cfr,…]
                     [--freej2me-prebuilt P | --freej2me-src DIR] [--no-freej2me]
arlecchino decompile <jar> [-o OUT] [-cp DEPS] [--config C] [-j N]
                     [--midp-stub JAR | --no-midp-stub]
                     [--rename-members | --no-rename-members] [--flatten-packages]
                     [--per-method] [--deobf] [--no-autofix] [--no-roundtrip]
                     [--no-metrics]
                     [--semdiff] [--reject-semantic-drift]
arlecchino emulate   <jar> [--freej2me P] [-W 176] [-H 220] [-s 2]
arlecchino run       <jar|jad> [--midp-rt P] [--size WxH] [--rms-dir D]
                     [--no-instrument] [--trace FILE]
                     [--headless [-o OUT] [--frames N] [--period-ms N]
                                 [--boot-wait S] [--inputs SCRIPT]]
arlecchino difftest  <orig.jar> [rebuilt.jar] [--input SCRIPT] [-o OUT]
                     [--calibrate | --cross-engine [--state-tol N]]
                     [--engine freej2me|midp-rt] [--frames N] [--period-ms N]
                     [--boot-wait S] [-W 240] [-H 320]
                     [--shift-tol N] [--jaccard-margin F] [--no-baseline]
                     [--fixed-clock]
```

`arlecchino <jar> …` is shorthand for `arlecchino decompile <jar> …` (back-compat).

### `setup` — get the engines + emulator

Downloads **Vineflower, CFR, Procyon, jadx** (plus **JD-Core, Jode, Fernflower and
Dava**) from their GitHub releases and builds or fetches **FreeJ2ME** into
`~/.arlecchino/engines/`. It also downloads **ASM** (`org.ow2.asm`, BSD-3-Clause) from
Maven Central into `~/.arlecchino/engines/asm/` and compiles the bytecode
member-renamer helper against it.

It also **generates** a **clean-room MIDP 2.0 / CLDC 1.1 API stub** — `javac`-compiling
public `javax.microedition.*` signatures (ABI only, empty or throwing bodies) into
`~/.arlecchino/midp-stub.jar` — which is always placed on the renamer's resolver
classpath so framework callbacks lock **precisely**, degrading to the offline denylist
if `javac` is unavailable. Each tool keeps its own license; nothing — including the
MIDP stub — is downloaded or vendored in this repo.

#### Reproducibility — pinned engine versions

Every engine is **pinned to an exact release tag** — the download asks for
`releases/tags/<tag>`, never `releases/latest`, so an upstream release cannot silently
shift the headline metric.

| engine | pinned tag | asset | ENV override |
|---|---|---|---|
| Vineflower | `1.12.0` | `vineflower-1.12.0.jar` | `ARLECCHINO_VINEFLOWER_VERSION` |
| CFR | `0.152` | `cfr-0.152.jar` | `ARLECCHINO_CFR_VERSION` |
| Procyon | `v0.6.0` | `procyon-decompiler-0.6.0.jar` | `ARLECCHINO_PROCYON_VERSION` |
| jadx | `v1.5.5` | `jadx-1.5.5.zip` (`bin/jadx`) | `ARLECCHINO_JADX_VERSION` |
| JD-Core | `jd-cli-1.3.0-beta-1` | `jd-cli-…-dist.zip` | `ARLECCHINO_JD_CLI_VERSION` |
| ASM | `9.7` | Maven Central | `ARLECCHINO_ASM_VERSION` |
| FreeJ2ME-plus | `1.52` | `freej2me-v1.52.zip` (`freej2me.jar` + `freej2me-lr.jar`) | `ARLECCHINO_FREEJ2ME_PLUS_VERSION` |

Jode, Fernflower and Dava (Soot) are pinned by construction — they are single direct
downloads whose version is part of the URL, so they have no ENV override. jd-cli's
newest release is a beta; the pin freezes the one this port was measured against rather
than following wherever `releases/latest` happens to point.

Set **`GITHUB_TOKEN`** (or `GH_TOKEN`) and `setup` sends it as an
`Authorization: Bearer` header, lifting the anonymous 60 req/h GitHub API rate limit
(optional — anonymous still works). Downloads degrade gracefully when offline.

> The historical **712 → 2** headline (below) was originally measured with a **local
> CFR `0.153-SNAPSHOT`** development build bundled with jd-gui-duo. Upstream has no
> `0.153` release, so `setup` pins the latest *released* CFR tag, `0.152`. A local
> `arlecchino.local.toml` pointing at your own jars keeps using them and is unaffected
> by `setup`'s downloads and pins.

### `decompile` — multi-decompile + best-of merge

0. **Bytecode member-rename (AUTO).** If the jar has Java-*source-illegal* member
   collisions and ASM + the helper are available, rewrite the jar so every member is
   uniquely named *before* decompiling. The **renamed jar then becomes the API
   reference** for every later step. Toggle with `--rename-members` /
   `--no-rename-members`.
1. Decompiles with all configured engines (in **parallel**, `-j`).
2. **Normalises names** so every engine lands in one canonical name space — in
   particular jadx's `defpackage` wrapper is un-wrapped so jadx joins the merge.
3. Scores each class **in isolation** against the original jar as the API, which avoids
   `javac`'s batch **error-masking** and ranks the engines honestly.
4. **Best-of merge:** per class, keeps the variant with the fewest isolated errors.
5. **Auto-fix pass:** rewrites universal artifacts and keeps a fix only if it strictly
   lowers the error count (never makes things worse). Six passes, all in
   `src/autofix.rs`:
   * `fold_interface_static_init` — a constant interface whose `static {}` block assigns
     its own blank finals becomes inline initialisers, *staying an interface* so every
     `implements X` remains valid. Preferred over the class fallback on ties, for
     whole-tree consistency;
   * `const_interface_to_class` — the fallback when folding cannot apply;
   * `scratch_temp_dead_array_type` — a synthetic `varNNNNN` declared as one array type
     but dead-stored as another; the dead declaration is widened to `Object`;
   * `resolve_unrepresentable_casts` — recovers the cast type behind Vineflower's
     `(<unrepresentable>)` from a sibling decompiler that did name it;
   * `duplicate_imports` — dedup;
   * `illegal_static_local` — strips `static` from a local declaration.

   Every pass scans a **length-preserving masked view** of the source in which string
   literals, char literals and comments are blanked, so a `{`, `}`, `;` or keyword
   hiding inside one cannot move a block boundary or trigger a spurious edit
   (`tests/autofix_mask.rs`).
6. **Round-trip check:** recompiles the merge to `.class` and reports how many of the
   original classes are covered, plus a coarse member-count diff vs the original
   bytecode.
7. **Quality metrics:** syntactic correctness, cross-decompiler agreement and a
   structural behavioural check. This stage recompiles and cross-compares every class
   and costs minutes on a big jar — `--no-metrics` skips it; everything the merge
   itself reports is already printed before it.
8. Writes `merged/`, `report.md`, `report.json`, and per-engine trees under `-o`.

`--deobf` is **partial** and heuristic: it annotates classes in `merged/` with their
likely MIDP role and emits a separate `jadx --deobf` tree beside it. It does not recover
names.

`--per-method` is **experimental** and off by default. On the reference game the
cross-engine auto-fixes resolve the residuals more reliably than whole-method splicing —
engines disagree on local and field names, so a strict-improvement splice rarely lands.
It also carries one deliberate, measured divergence from the reference implementation:
methods are visited in key order rather than source order, which on that jar finds two
splices and reaches an isolated sum of 3 where source order finds one and stops at 4.

### `--semdiff` — semantic round-trip

The plain round-trip check proves only **coverage** (every original class produced a
compiled one) and **coarse counts** (members and methods per `javap -p`). An auto-fix
can change *semantics* without touching either — above all the three that rewrite
constructs sitting next to behaviour: the dead-array widening can flip `new int[]` to
`new char[]`, the static-block fold re-encodes `<clinit>`, and interface→class flips
call sites from `invokeinterface` to `invokevirtual`.

`decompile --semdiff` disassembles the original class (from the renamed jar — the
canonical API the whole pipeline already uses) and the recompiled one, method by method,
normalises the differences a decompile+recompile round-trip legitimately produces, and
**flags** methods whose normalised set of symbolic references differs in substance. It
never changes the merge rules.

**Why a set of symbolic references and not the opcode stream?** A strict `javap -c`
diff is hopelessly noisy: `javac` re-encodes decompiled source with different branch
polarity (`if_icmpge` ↔ `if_icmplt; return`), different local-variable slot numbers,
different `dup` / `x++` idioms, and elided re-loads — none of which change behaviour.
Calibrating a line-wise diff on the known-good reference merge produced hundreds of
false differences.

The signal that is stable **and** false-positive-free uses these eight rules:

| # | rule | why |
|---|---|---|
| N1 | compare method to method, keyed by a signature with access modifiers stripped | a method present on only one side is flagged |
| N2 | keep field/method refs as `(kind, name, descriptor)`, **drop the owner class** | `javac` re-resolves a member to whichever supertype *declares* it (`Field aa.H:I` vs `Field j.H:I`) — same member, cosmetic difference |
| N3 | keep the invoke **kind** distinct (`Method` vs `InterfaceMethod`), owner still dropped | so interface→class is still caught, while interface re-resolution is not a false flag |
| N4 | unify `StringBuffer` ↔ `StringBuilder` | decompilers emit modern `StringBuilder` for `+` that the obfuscated bytecode built with `StringBuffer` |
| N5 | capture primitive array creation `newarray <type>` | exactly what the dead-array widening can change |
| N6 | **exclude** `checkcast` / `instanceof` | decompilers insert redundant downcasts the original verified statically |
| N7 | **exclude** the synthetic `getClass():Class` | `javac`'s null-check idiom, which decompiled source does not reproduce |
| N8 | reconcile renamed members through `mapping.txt` (reverse map `newName → oldName`) | a member the renamer renamed must not look like a semantic change |

**Calibration.** With N1–N8 the known-good 712→2 reference merge yields **zero** false
flags across all **31** cleanly recompiled classes — including the three the merge did
auto-fix (a dead scratch array, a folded constant interface, a resolved cast), none of
which change semantics. Synthetic interface→class and array-element-type drifts *are*
flagged (`tests/semdiff_javap.rs`).

Everything degrades softly: with no `javap` the module reports `available = false` and
does nothing; a class that did not recompile is skipped and reported, not flagged.

`decompile --reject-semantic-drift` implies `--semdiff` and turns the report into a
decision: every class the merge auto-fixed is re-verified, and an auto-fix whose
recompiled bytecode drifts from the original is **discarded** in favour of the best
non-auto-fixed variant — but only when reverting does not raise that class's isolated
error count. It can therefore only ever tighten the existing "accept a fix only if it
strictly lowers the error count" rule, never relax it. Reverted classes are listed in
`report.json` under `semantic_drift_reverts`.

### `emulate` — run the jar in FreeJ2ME

Thin wrapper around FreeJ2ME's AWT front-end:
`java -jar freej2me.jar <midlet.jar> W H scale`.

`setup` also installs **`freej2me-lr.jar`** — FreeJ2ME-plus's libretro (pipe)
front-end. `src/diffrun.rs` drives it over plain stdin/stdout for **headless frame
capture**: no window, no X server, no SDL, no native code. That is the foundation of
the behavioural diff-testing harness; the protocol and its traps are documented in
[`docs/headless-capture-protocol.md`](docs/headless-capture-protocol.md).

**FreeJ2ME-plus is GPLv3** and is only ever run as a separate process — never linked,
never vendored.

### `run` — run the MIDlet in midp-rt

Launches the MIDlet in **midp-rt** (an LGPL fork of MicroEmulator) as a **separate
process** — the LGPL boundary is the process boundary; nothing LGPL is linked here.
`MIDlet-1` is parsed from the manifest/`.jad` so the MIDlet auto-starts instead of
waiting in the runtime's launcher. `--headless` captures PNG frames + `index.json`
instead of opening a window.

**Where midp-rt comes from.** Its sources are vendored in this repository under
`midp-rt/` (git subtree, LGPL 2.1 — see `midp-rt/COPYING-LGPL-2.1`), but the built
jar is not: `midp-rt/build/` is git-ignored. Build it once with
`arlecchino setup --only midp-rt` (which runs `midp-rt/build.sh` for you), or by
hand with `cd midp-rt && ./build.sh`. The jar is then looked up in this order:

1. `--midp-rt PATH`;
2. `$ARLECCHINO_MIDP_RT`;
3. `<engines root>/midp-rt.jar` (`~/.arlecchino/engines` by default);
4. `midp-rt.jar` next to the `arlecchino` binary;
5. `build/midp-rt.jar` inside any source checkout of the fork — `$ARLECCHINO_MIDP_RT_SRC`,
   `<engines root>/midp-rt`, a `midp-rt/` directory beside the binary or in any of its
   parent directories (so a `target/release/` build finds the in-tree subtree), or
   `~/Projects/arlecchino-midp-rt`.

If none of these hit, the command prints every path it checked and how to fix it.

`arlecchino difftest --engine midp-rt` runs the behavioural diff through midp-rt instead
of FreeJ2ME-plus — a second, independent reference engine. On one machine its rendering
is fully deterministic: on the reference game, original versus rebuild came out at
**Jaccard 1.00 and 100 % frame-by-frame in both the baseline and the cross comparison**,
where FreeJ2ME-plus gives 0.86 / 97 %. That determinism does **not** currently extend to
`--fixed-clock` on this engine — see
[`docs/determinism-fixed-clock.md`](docs/determinism-fixed-clock.md) for the measured
boundary.

### `difftest` — behavioural diff (does the rebuilt jar still behave the same?)

Runs both jars headless with the same input script and compares the **structure of
screen states** — the set of frame hashes, the order they first appear in and the
first-appearance frame indices (with a shift tolerance) — plus RMS side effects and
crash signals from the emulator's stderr. Frame *content* is deterministic, frame
*timing* is not, so a noise baseline (the same jar run twice) is measured first and
the verdict only fails on signals stronger than that noise.

* `--calibrate` — only the baseline (one jar, twice).
* `--cross-engine` — the same jar in FreeJ2ME-plus **and** midp-rt, compared softly
  (structure only; pixels never match across engines, so the pixel Jaccard is
  informational).
* `--fixed-clock` — ASM-instrument the jar(s) with a virtual clock and a fixed-seeded
  RNG first, so replays become frame-identical.

#### Input-script format

`run --inputs` and `difftest --input` take a JSON array of events:

```json
[
  {"t_ms": 4500, "key": "FIRE",  "action": "tap"},
  {"t_ms": 6000, "key": "DOWN",  "action": "press"},
  {"t_ms": 6300, "key": "DOWN",  "action": "release"},
  {"t_ms": 7500, "key": "NUM5",  "action": "tap"}
]
```

* `t_ms` — milliseconds from the **first captured frame** (i.e. from the end of
  `--boot-wait`), not from JVM start. The array need not be sorted. Replay is best
  effort against the wall clock: an event is injected before the first frame request
  with `now >= t_ms`, so it is not frame-exact — `--fixed-clock` maps `t_ms` onto a
  frame index instead and makes the MIDlet's own logic deterministic.
* `key` — one of `UP DOWN LEFT RIGHT`, `NUM0`…`NUM9`, `STAR`, `POUND`,
  `FIRE` (= `SOFT3`, the middle soft key), `SOFT_LEFT`, `SOFT_RIGHT`, `CLR` — or a raw
  integer joypad index of the libretro front-end.
* `action` — `press`, `release` or `tap` (a press immediately followed by a release).
  Defaults to `press`. An unknown key name is a hard error, not a dropped event.

Both engines take the same format, but two things differ under `--engine midp-rt`,
because the replay is done by the runtime itself rather than by a libretro joypad:

* `FIRE` / `SELECT` / `SOFT3` map onto the MicroEmulator device's **SELECT** button (the
  middle key, game action FIRE), and `CLR` maps onto **DELETE**;
* a **raw integer** in `key` means a **MIDP keyCode** (e.g. `53` = `NUM5`), *not* a
  libretro joypad index. A script that hard-codes joypad indices is therefore silently
  reinterpreted when run through midp-rt — use the symbolic names in any script meant
  for both engines.

## GUI

`arlecchino-gui` is a small desktop panel (built with eframe/egui, steam_dark theme).
It runs the same `decompile` / `emulate` / `setup` commands as subprocesses and streams
their output into a log pane, and calls the jadx-tree normaliser directly. It also
offers:

- **Add to application menu** — installs a freedesktop `.desktop` launcher and icon
  into your user XDG dirs (no root).
- **Uninstall…** — with a confirmation dialog, removes the installed binaries, launcher
  and icon.
- a **system-tray icon** (KDE/StatusNotifierItem) with *Open panel* / *Quit*.

### Application icon

The app icon is a gold venetian half-mask with the harlequin lozenge on the brow —
original artwork drawn from the public-domain *commedia dell'arte* imagery, shipped as
two self-contained SVGs embedded into the GUI binary:

- `assets/icon.svg` — master, installed to
  `~/.local/share/icons/hicolor/scalable/apps/arlecchino.svg`;
- `assets/icon-small.svg` — the same design with the engraving stripped, installed to
  `hicolor/16x16` and `hicolor/24x24`, because below ~32 px a whole ornament falls under
  one pixel and only muddies the silhouette.

To use a different icon, replace those files and rebuild, or replace the installed
copies in place. Keep them self-contained (no raster data, no fonts, no external
references) — the launcher writes the file verbatim.

## Language (EN / RU / DE)

All user-facing text (GUI and CLI framing/errors) is available in **English**,
**Russian** and **German**. Resolution order:

1. `ARLECCHINO_LANG=en|ru|de` — override (also how the GUI passes its language to the
   CLI subprocesses it starts);
2. the stored preference in `~/.config/arlecchino/settings.toml`
   (`language = "system" | "en" | "ru" | "de"`), written by the GUI's **Settings ->
   Language** menu — the default, `system`, keeps the locale behaviour below;
3. the system locale (`LC_ALL` → `LC_MESSAGES` → `LANG`; `ru*` → Russian, `de*` →
   German, everything else → English).

Switching the language in the GUI takes effect immediately — no restart — and is kept
for the next start. Technical output (report.json / report.md content, progress and
merge logs, file paths, flag names) is left stable and untranslated.

## Honest error counts

`javac` is **all-or-nothing per invocation**: a single early structural error aborts
attribution of the *other* files, **masking** their real errors. This tool instead
**compiles each class in isolation** against the original jar (which supplies the
canonical API of every sibling) and sums the per-class errors — an honest, unmaskable
metric. It also auto-adds a **MIDP stub** (FreeJ2ME's jar) to the classpath so
`javax.microedition.*` resolves; override with `-cp`, `--midp-stub`, or
`--no-midp-stub`.

A naive whole-tree compile of a badly obfuscated jar can therefore report a deceptively
tiny number. An early prototype of this tool reported "1 error" on the reference game —
that was a `javac` masking artifact in which one interface error hid roughly seven
hundred real ones. The figures below are the honest unmasked isolated counts, and on
them the merge genuinely beats every single engine.

## Example — measured on an obfuscated J2ME game jar

⚠️ **Conditions.** These numbers were measured in June 2026 on `Allods_1_2_240x320.jar`
with a **four-engine local configuration** whose CFR was the unreleased
`0.153-SNAPSHOT` development build (see the note under `setup`). They are the project's
headline result and are quoted here as such. They are **not** the live gate — that is
`regress/expected.txt`, measured later with six pinned engines and cross-checked jar for
jar against the reference implementation, and it reports different (equally valid)
figures for the same jar. Do not read the two as contradicting each other; see
[`docs/history-and-decisions.md`](docs/history-and-decisions.md).

Per engine, **member-renamer OFF**, isolated errors (lower is better):

| engine | classes | isolated errors |
|---|---|---|
| cfr | 30 | 978 |
| vineflower | 30 | 1053 |
| jadx | 32 | 1066 |
| procyon | 30 | 1183 |

With the **member-renamer ON** (the default AUTO) every engine's raw count collapses —
procyon 1183 → 1, cfr 978 → 16, vineflower 1053 → 11, jadx 1066 → 7.

* **Best-of merge, renamer OFF** (`--no-rename-members`): **712** — about 27 % fewer
  than the best single engine, with 100 % round-trip class coverage. The bulk of those
  712 are caused by ProGuard's descriptor-overloaded members.
* **Best-of merge, renamer ON** (default): **2**, with **100 % (32/32)** round-trip
  class coverage and the whole 31-class default-package tree compiling together.
* **With `--flatten-packages`: 0** — see below.

One finding worth naming: a field named like an in-jar type it references — e.g.
`class j { int o; }` that uses `o.c` where `o` is also a type — shadows the type in
source and yields **57 spurious `int cannot be dereferenced`** errors on its own.

## Bytecode member renamer

ProGuard reuses the **same member name with different descriptors** — legal in JVM
bytecode (members are keyed by name **and** descriptor) but **illegal in Java source**,
so decompiler output won't compile. Source-level textual renaming can't fix it — a use
site doesn't know which overloaded declaration it binds to. So the renamer works in
**bytecode, before decompiling**: a small ASM helper (`assets/MemberRenamer.java`,
compiled by `setup`) loads every class, detects the source-illegal collisions (duplicate
field names; return-type-only method overloads), unions override groups so virtual
dispatch is preserved, **locks** members bound by an external (MIDP/JDK) contract, and
rewrites all references (including shared NameAndType constant-pool entries).

**The new-name scheme.** Within a collision group the first occurrence keeps its name;
each other collider becomes `<orig>_<tag>`, where `<tag>` is the first 8 hex digits of an
FNV-1a-64 hash of the member's **full descriptor** (a numeric `_2`, `_3`, … bump follows
only if that is still taken). Only actual colliders are renamed. A `mapping.txt`
(`owner.oldName desc -> newName`) is written next to the renamed jar and is what
`--semdiff` reconciles against.

**Locking.** A member bound by an external contract — a MIDP or JDK callback the runtime
calls by name — must never be renamed. If a locked method collides with another, the
**other** collider is renamed instead. Resolution is done precisely, by reflecting
against the generated clean-room MIDP stub; a hard-coded denylist of the well-known
callbacks is kept **only** as an offline fallback, because blanket-locking would prevent
fixing genuine return-type collisions.

**Measured on the reference game:** 32 classes → **355 field renames and 50 method
renames** (291 descriptor overloads, 56 type shadows, 8 constant-interface shadows). The
renamed jar verifies — all 32 classes load through the JVM verifier with 0 failures — has
**zero** remaining duplicate-field or return-type collisions, and **no MIDP callback was
renamed** (verified: `GameMIDlet.startApp/pauseApp/destroyApp` and the canvas class's
`run/showNotify/hideNotify/keyPressed/keyReleased` are all untouched).

### Type-name-shadow field rename

`X.c` where `X` is both a field of the current class and an in-jar type parses as
`(this.X).c` — the field shadows the type, and the compiler then complains that a
primitive cannot be dereferenced. The renamer renames the field `C.X` only when an
in-jar type has simple name `X` **and** `C` actually references that type's internal
name in its bytecode, so a coincidental name match is left alone. On the reference game
this one rule clears all **57** `int cannot be dereferenced` errors (41 in class `j`,
15 in `e`, 1 in `i`).

### Constant-interface-inherited-shadow field rename

An instance field that shadows a constant inherited from an implemented constant
interface makes every unqualified use ambiguous. The renamer renames the colliding
**constant** in the interface — a read-only, dispatch-irrelevant member — rather than the
instance field. On the reference game this clears the remaining ambiguous-reference
errors.

### `--flatten-packages` (opt-in)

A named-package class cannot name a type that lives in the **default (unnamed)**
package — a Java language rule (**JLS §6.5 / §7.4.3**), not a decompiler gap, so a jar
that mixes the two keeps an irreducible residual of `cannot find symbol` errors no
renamer can remove. On the reference game that residual is exactly the honest **2**: both
errors are in `Container/GameMIDlet`, and both are the same problem — it lives in package
`Container` but references default-package types.
`--flatten-packages` takes the porting decision a manual port takes: the same ASM pass
also remaps **types**, moving every in-jar class into the default package. All type
references are rewritten consistently (extends/implements, descriptors,
`NEW`/`CHECKCAST`/`INSTANCEOF`, field and method owners, `LDC` class constants,
signatures), each class's **jar entry path follows its new name**, and the manifest
entry-class attribute (`MIDlet-1:`, `Main-Class:`) is rewritten with it. Colliding
simple names keep the first and get a stable `_<8hex>` suffix.

Caveat: the pass rewrites type *references*, not string **constants** — a jar that
resolves classes from a package-qualified `Class.forName("pkg.Foo")` **literal** would
need those literals updated by hand. That was checked on the reference game before the
flag was trusted there: it has **no** such `forName` literals (its only
`Container/GameMIDlet` occurrences are type descriptors) and it loads resources by
**path** (`getResourceAsStream("/res/…")`), not by class name — so for that jar the
flatten is a complete, round-trip-safe fix. Check the same two things before relying on
it elsewhere.

**Result on the reference game:** `--flatten-packages` moves `Container.GameMIDlet` into
the default package and its 2 cross-package errors vanish, removing the entire
irreducible residual; round-trip coverage stays 100 %. Without the flag the renamed jar
is **bit-for-bit** what it was, so the default path is unaffected.

### What the numbers do and do not prove

The isolated count is the honest, unmaskable per-class metric: each class compiled alone
against the **renamed** jar plus the MIDP stub as its API. On the reference game it was
reproduced by an independent isolated re-measure of all 32 classes (31 clean,
`GameMIDlet` = 2), and the 31 default-package classes also compile **together** as a tree
(whole-tree errors = 2, i.e. only `GameMIDlet`).

Round-trip coverage (100 %, 32/32) is a **structural smoke test**: it proves every class
compiles against the renamed jar's API, with classes that failed to recompile seeded
from the original bytecode so their dependents still resolve. It is **not** behavioural
equivalence. Behaviour is a separate question, answered by `difftest` — and it was
answered: the rebuilt jar reproduces the original's screen states exactly on the
reference game. See
[`docs/behavioural-verification.md`](docs/behavioural-verification.md).

## Regression gate

`regress/expected.txt` holds the per-jar merge numbers of the reference corpus, and
`regress/run.sh` re-measures them:

```sh
cargo build --release
regress/run.sh                 # or: regress/run.sh /path/to/corpus
```

Both numbers of the `RESULT merged:` line are compared (the run uses `--no-metrics`,
which is not part of the gate), and a mismatch is a non-zero exit. The measurement uses
`regress/engines.6.toml` — the shipped default engine set minus the two `kind = "class"`
entries — because those numbers were cross-checked jar for jar against the Python
edition, which cannot run those two. Keep that file in sync with
`assets/engines.default.toml` when an engine's invocation changes.

## Tests

`cargo test`. The suite pins behaviour rather than adding any; it needs nothing beyond
what `arlecchino setup` already installs, and every test whose prerequisites are missing
prints `SKIP: <reason>` and returns green instead of failing. What each file covers, and
what skips where, is in [`tests/README.md`](tests/README.md).

## Documentation

[`docs/`](docs/README.md) holds the research reports, measurements and decision records
behind the claims above — the capture protocol that had to be reverse-engineered, the
determinism boundary and its regression history, where the vendored MIDP runtime came
from and under which licences, the behavioural validation numbers, and the alternatives
that were examined and rejected. None of it is derivable from the source.

## Configuration

The engine configuration is TOML/JSON. Resolution order: `--config PATH` →
`./arlecchino.toml` → `~/.arlecchino/engines.toml` → the built-in default (embedded).
Each `[[engine]]` has `name`, `type` (`names`/`renamed`), `kind` (`jar`/`tool`), `path`,
`args` (with `{jar}`/`{out}`), optional `sources_subdir` and `normalize` (`none`/`jadx`).

## License

**Apache-2.0** — see [LICENSE](LICENSE) and [NOTICE](NOTICE). Independent clean-room
work; third-party decompilers, FreeJ2ME, and ASM (BSD-3-Clause, used by the member
renamer) are invoked / downloaded as separate processes / artifacts under their own
licenses and are not bundled.
