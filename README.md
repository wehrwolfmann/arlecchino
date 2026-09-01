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
member-renamer helper against it. Each tool keeps its own license; nothing is vendored
in this repo.

Every engine is **pinned to an exact release tag** — the download asks for
`releases/tags/<tag>`, never `releases/latest`, so an upstream release cannot silently
shift the headline metric. Override a pin with `ARLECCHINO_<ENGINE>_VERSION`
(`ARLECCHINO_FREEJ2ME_PLUS_VERSION` for the emulator). Set `GITHUB_TOKEN` / `GH_TOKEN`
to lift the anonymous 60 req/h GitHub API rate limit.

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
   lowers the error count (never makes things worse).
6. **Round-trip check:** recompiles the merge to `.class` and reports how many of the
   original classes are covered, plus a coarse member-count diff vs the original
   bytecode.
7. **Quality metrics:** syntactic correctness, cross-decompiler agreement and a
   structural behavioural check. This stage recompiles and cross-compares every class
   and costs minutes on a big jar — `--no-metrics` skips it; everything the merge
   itself reports is already printed before it.
8. Writes `merged/`, `report.md`, `report.json`, and per-engine trees under `-o`.

`decompile --semdiff` adds a **semantic round-trip**: per method, the symbolic
reference sets of the original and the recompiled bytecode are compared (owner
dropped, `StringBuffer`/`StringBuilder` unified, renamer mapping reconciled), so an
auto-fix that silently changed semantics is flagged even when member counts match.

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
  Defaults to `press`.

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

## Bytecode member renamer

ProGuard reuses the **same member name with different descriptors** — legal in JVM
bytecode (members are keyed by name **and** descriptor) but **illegal in Java source**,
so decompiler output won't compile. Source-level textual renaming can't fix it — a use
site doesn't know which overloaded declaration it binds to. So the renamer works in
**bytecode, before decompiling**: a small ASM helper (`assets/MemberRenamer.java`,
compiled by `setup`) loads every class, detects the source-illegal collisions (duplicate
field names; return-type-only method overloads), unions override groups so virtual
dispatch is preserved, **locks** members bound by an external (MIDP/JDK) contract, and
rewrites all references (including shared NameAndType constant-pool entries). It also
fixes **type-name shadowing** (a field named like an in-jar type it references) and
**constant-interface-inherited shadowing**.

### `--flatten-packages` (opt-in)

A named-package class cannot name a type that lives in the **default (unnamed)**
package — a Java language rule, not a decompiler gap, so a jar that mixes the two keeps
an irreducible residual of `cannot find symbol` errors no renamer can remove.
`--flatten-packages` takes the porting decision a manual port takes: the same ASM pass
also remaps **types**, moving every in-jar class into the default package. All type
references are rewritten consistently (extends/implements, descriptors,
`NEW`/`CHECKCAST`/`INSTANCEOF`, field and method owners, `LDC` class constants,
signatures), each class's **jar entry path follows its new name**, and the manifest
entry-class attribute (`MIDlet-1:`, `Main-Class:`) is rewritten with it. Colliding
simple names keep the first and get a stable `_<8hex>` suffix.

Caveat: the pass rewrites type *references*, not string **constants** — a jar that
resolves classes from a package-qualified `Class.forName("pkg.Foo")` **literal** would
need those literals updated by hand. Without the flag the renamed jar is bit-for-bit
what it was, so the default path is unaffected.

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
