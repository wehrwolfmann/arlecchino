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
                     [--rename-members | --no-rename-members]
                     [--per-method] [--deobf] [--no-autofix] [--no-roundtrip]
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

Downloads **Vineflower, CFR, Procyon, jadx** from their GitHub releases and builds or
fetches **FreeJ2ME** into `~/.arlecchino/engines/`. It also downloads **ASM**
(`org.ow2.asm`, BSD-3-Clause) from Maven Central into `~/.arlecchino/engines/asm/` and
compiles the bytecode member-renamer helper against it. Each tool keeps its own
license; nothing is vendored in this repo.

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
7. Writes `merged/`, `report.md`, `report.json`, and per-engine trees under `-o`.

`decompile --semdiff` adds a **semantic round-trip**: per method, the symbolic
reference sets of the original and the recompiled bytecode are compared (owner
dropped, `StringBuffer`/`StringBuilder` unified, renamer mapping reconciled), so an
auto-fix that silently changed semantics is flagged even when member counts match.

### `emulate` — run the jar in FreeJ2ME

Thin wrapper around FreeJ2ME's AWT front-end:
`java -jar freej2me.jar <midlet.jar> W H scale`.

### `run` — run the MIDlet in midp-rt

Launches the MIDlet in **midp-rt** (an LGPL fork of MicroEmulator) as a **separate
process** — the LGPL boundary is the process boundary; nothing LGPL is linked here.
`MIDlet-1` is parsed from the manifest/`.jad` so the MIDlet auto-starts instead of
waiting in the runtime's launcher. `--headless` captures PNG frames + `index.json`
instead of opening a window.

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
