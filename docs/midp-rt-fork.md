# midp-rt: origin of the MIDP runtime fork

This is the record of how and why `midp-rt/` — the MIDP 2.0 / CLDC 1.1 runtime that
Arlecchino drives as a separate process — came to exist. The adoption research behind it was
carried out on **2026-06-10** against local clones of the MicroEmulator mirrors, and every
compatibility claim below was measured live on that date with **JDK 17.0.19** on Linux. The
fork is not a rewrite: it is an adapted, selectively imported and modernised copy of
MicroEmulator, vendored into this repository as a git subtree under `midp-rt/`, built into its
own `midp-rt.jar` artefact, and never linked into the Apache-2.0 Rust crate.

---

## 1. The B0 decision — why an adapted code base and not a bespoke shim

Decision B0 (2026-06-10) settled the fork in the runtime plan: **Block B is built on the code
base of the old MicroEmulator, adapted for this project — not on a hand-written "CLDC-lite"
shim of its own.**

The reasoning: MicroEmulator is a complete, mature MIDP 2.0 implementation (lcdui, lcdui.game,
midlet lifecycle, rms, io, pki) whose rendering path is plain Java2D; writing the same surface
from scratch was estimated at weeks of work and would have reproduced, badly, semantics that
already exist and are known to run real phone games. Against that stands one cost: the licence.

The obligations that come with the decision, stated at B0 and unchanged since:

* the fork is kept as a **separate module / jar** — a separate artefact, exactly like the
  decompiler engines that `setup` fetches — and **its sources are published**;
* the fork's code is **never mixed into the Apache-2.0 code** of Arlecchino; all interaction
  happens across the boundary of a separate process / separate jar;
* the project's own ABI stub, `src/midp_stub.rs`, stays as it is: it exists for the member
  renamer and for compilation, and is a different artefact with a different role from the
  runtime.

The remaining B-stages were re-read under that decision: the API-surface scan (B1) survives as
a completeness checklist for the fork; the specification stage (B2) narrows to a specification
of the *adaptation* (JDK 17 compatibility, headless rendering, RMS path, CLI integration); and
B3–B5 become edits inside the fork instead of new code.

> **What actually happened.** The one deviation from the shape sketched at B0 is packaging:
> instead of a wholly separate repository, the fork lives as a git subtree in `midp-rt/` inside
> this repository. The licence boundary is unaffected — nothing under `midp-rt/` is compiled,
> linked or embedded into the Rust crate, Cargo excludes it from the packaged crate, and the
> jar is invoked only as a separate operating-system process. Two trees in one repository is
> mere aggregation; the LGPL boundary is still the process boundary.

---

## 2. Mirror comparison and choice of fork base

Three mirrors of MicroEmulator were compared on 2026-06-10. `.java` file counts are counts of
source files in the tree at the stated HEAD.

| Repository                                                    | HEAD                                       | Date                                                     | `.java` files          | Verdict                          |
|---------------------------------------------------------------|--------------------------------------------|----------------------------------------------------------|------------------------|----------------------------------|
| **barteo/microemu** (canonical, author Bartek Teodorczyk)     | `e47b656d983deaa67810c776662444483bdbf134` | 2023-03-28 (code ~2010, later commits are CI cosmetics)  | 547 (in `microemulator/`) | ✅ **FORK BASE**                 |
| nikita36078/microemu (fork of barteo; author of J2ME-Loader)  | `eb7e7d38bf842fc51777a671d425b03eb7e7535e` | 2018-03-28                                               | 455                    | 🍒 source of cherry-picked fixes |
| tisoft/microemu                                               | `bf5dffebca4e`                             | 2010-11-04                                               | —                      | stale SVN snapshot, not needed   |

* **barteo** is the most complete: the `microemulator/` monorepo (modules listed in §4) plus
  `incubator/`, `me-extras/` and `microemu-maemo/` (not needed). This is upstream
  "MicroEmulator 3.0.0-SNAPSHOT" (`build.version`) — the latest state of the project after
  release 2.0.4, and therefore the base of the fork.
* **nikita36078** is barteo minus android/iphone/webstart, plus five useful commits. It was
  rejected as a *base* because the fork is trimmed and carries no extensions history; its
  commits are taken as patches on top of barteo instead.

Clones used for the study: `~/research/microemu-ref/barteo` (depth 50) and
`~/research/microemu-ref/nikita36078` (depth 5).

## 3. Commits cherry-picked from the other mirror

Five commits from nikita36078/microemu @ `eb7e7d38bf842fc51777a671d425b03eb7e7535e`, visible in
the shallow clone, were carried over by hand onto the barteo base:

| Commit    | Subject                          | Touches                                                |
|-----------|----------------------------------|--------------------------------------------------------|
| `eb7e7d3` | Fix Nokia games loading issue    | `MIDletClassLoader`                                    |
| `70c8503` | Fix GameCanvas drawing           | `J2SEDeviceDisplay` / `GameCanvas` / `Image` — 6 files  |
| `e7c9da6` | Improve exceptions handling      | —                                                      |
| `5b36135` | Delete unused classloaders       | —                                                      |
| `6575842` | Remove ClosedConnection          | —                                                      |

The research named "Fix GameCanvas drawing" and "Fix Nokia games loading" as the minimum worth
taking; in the end all five were applied and are recorded in `midp-rt/NOTICE`.

## 4. Per-module licence map

`COPYING` in the upstream repository states "either **LGPL 2.1** or the **Apache License
2.0**" — but that covers "most source files" only, and the tree is in fact mixed. The counts
below come from reading the licence headers of the files under each module's `src/main`.

| Module                     | Total | Dual LGPL 2.1 / AL 2.0 | **LGPL-only** | Notes                                                                                                                                                                                                                                                        |
|----------------------------|-------|------------------------|---------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| microemu-cldc              | 19    | 19                     | 0             |                                                                                                                                                                                                                                                              |
| microemu-midp              | ~97   | ~78                    | **19**        | LGPL-only = © 2005 Andres Navarro: **all of `lcdui.game.*`** (GameCanvas / Sprite / LayerManager / TiledLayer / Layer) plus the lcdui widgets (Alert, Form, List, ChoiceGroup, Command, Font, Gauge, Item, ImageItem, DateField, CustomItem, Choice, ItemCommandListener) and `org.microemu.GameCanvasKeyAccess` |
| microemu-javase            | 75    | 74                     | **1**         | `org/microemu/device/impl/Polygon.java`                                                                                                                                                                                                                      |
| microemu-javase-swing      | 57    | 57                     | 0             |                                                                                                                                                                                                                                                              |
| ext/microemu-jsr-135 (MMAPI)| 37   | 22                     | **15**        | LGPL-only: nearly all of `javax.microedition.media.control` / `protocol`, plus TimeBase and `J2SEVolumeControl`                                                                                                                                               |
| ext/microemu-nokiaui       | 7     | 7                      | 0             |                                                                                                                                                                                                                                                              |
| ext/microemu-siemensapi    | 32    | 32                     | 0             |                                                                                                                                                                                                                                                              |

A few files carry no header at all (for example `lcdui/DisplayUtils.java`, `device/ui/*UI.java`,
`util/ExtendedRecordListener.java`); they are read as dual-licensed per `COPYING`, and the fork
gives them proper headers.

**Conclusion.** The assembled runtime jar is, taken as a whole, **LGPL 2.1**: the LGPL-only
files govern the aggregate artefact, and the dual-licensed remainder is compatible with that.
This matches decision B0 exactly — a separate LGPL module/jar, fork sources published, not one
line inside Arlecchino's Apache-2.0 code.

**And the escape hatch.** Should an Apache-only core ever be wanted, only the **35 LGPL-only
files** (19 + 1 + 15) would have to be dropped or rewritten; everything else is already dual.

> **What actually happened to the MMAPI numbers.** The 15 LGPL-only files in
> `ext/microemu-jsr-135` were never imported: the fork's JSR-135 layer was written from scratch
> against the public JSR-135 specification instead (`javax.microedition.media[.control]` plus
> real players in `org.microemu.media`). The count that governs the shipped jar is therefore
> the ~20 LGPL-only files in `microemu-midp` and `microemu-javase`; the escape-hatch figure of
> 35 is the number as measured across the whole upstream candidate set on 2026-06-10.

## 5. Module keep / drop list

```
microemulator/
├── api/                      # signature tests for CLDC 1.0/1.1, MIDP 2.0 (tests only) — NOT NEEDED
├── microemu-cldc/            # ✅ javax.microedition.io (Connector, *Connection) — 19 files
├── microemu-midp/            # ✅ CORE: javax.microedition.{lcdui, lcdui.game, midlet, rms, io, pki}
│                             #    + org.microemu.{MIDletBridge, device.*, util.*} (RecordStore etc.)
├── microemu-injected/        # ✅ 1 file (Injected.java — System.out redirect in instrumented code)
├── microemu-javase/          # ✅ org.microemu.app.Common (all the jar/jad launch logic),
│                             #    app.classloader.* (MIDletClassLoader + ASM instrumentation),
│                             #    app.util.FileRecordStoreManager / util.MemoryRecordStoreManager,
│                             #    device/impl/*, device resources (default/resizable device.xml + png)
├── microemu-javase-swing/    # ✅ PARTIALLY: org/microemu/device/j2se/* (J2SEDeviceDisplay,
│                             #    J2SEGraphicsSurface=BufferedImage, J2SEFontManager, J2SEInputMethod…),
│                             #    org/microemu/app/Headless.java + ui/noui/NoUiDisplayComponent ✅,
│                             #    app/Main.java + ui/swing/* (the Swing GUI window) — keep for interactive use,
│                             #    applet/* — DROP
├── microemu-javase-applet/   # ❌ applets — drop
├── microemu-javase-swt/      # ❌ SWT — drop (already disabled in the pom)
├── microemu-extensions/
│   ├── microemu-jsr-135/     # ⚠️ MMAPI: interfaces + BasicPlayer/audio — take (Phase 2 = B8)
│   ├── microemu-nokiaui/     # ✅ take (real games need the Nokia UI API)
│   ├── microemu-siemensapi/  # ✅ take (same for Siemens)
│   ├── microemu-jsr-75/120/82 # ❌ FileConnection/SMS/Bluetooth — not until demand shows (B1 scan)
│   └── microemu-device-*/    # ❌ device skins — not needed (resizable + default already in javase)
├── microemu-examples/, microemu-tests/, microemu-webstart/  # ❌
└── microemulator/            # the assembly module — replaced by a build script of the fork
```

There is exactly one external dependency: **ASM** (upstream pins the ancient `asm:asm:3.1`, and
only `app.classloader` uses it). There are no Swing dependencies outside `javase-swing`;
rendering is plain Java2D / `BufferedImage`.

> **What actually happened to two of these lines.** `microemu-injected` was *not* copied in the
> end: its single class `org.microemu.Injected` is duplicated (a different variant of the same
> FQCN) inside `microemu-javase`, and the javase variant was taken. `microemu-jsr-135` was not
> imported either — see the note in §4; the fork's MMAPI layer is original work written against
> the public specification, and by 2026-06-12 it played real sound (MIDI via `javax.sound.midi`,
> WAV/AU/AIFF via `javax.sound.sampled`, `Manager.playTone` via a tone generator), with a
> no-op player for unsupported formats so that MMAPI games load instead of dying with
> `NoClassDefFoundError`.

## 6. Toolchain compatibility — measured live on 2026-06-10

The Arlecchino toolchain is **JDK 17.0.19** (`javac --release 8` for the round-trip — see the
`javac.*.args` files). What was actually run, in `/tmp/me-smoke`, against the barteo HEAD
classes:

1. ✅ `javac 17 --release 8` compiles **cldc + midp + api + javase + javase-swing** (minus the
   injected duplicate, minus applet) with **0 errors** — 255 files, warnings only.
2. ✅ Headless launch of a test MIDlet (`Canvas.paint` + `drawString` + `repaint`) on JDK 17:
   `java org.microemu.app.Headless --appclassloader system TestMidlet` → `START_APP`,
   `PAINT_CALLED 176x176`. It works both with `-Djava.awt.headless=true` and **with no DISPLAY
   at all** — rendering goes to a `BufferedImage` and fonts resolve through fontconfig.
3. ❌→✅ Launching a **jar file** (the case that matters here): `MIDletClassLoader` always runs
   classes through ASM instrumentation, and **ASM 3.1 crashes on Java 8 bytecode** — class file
   **major 52**, which is what merged jars are after `--flatten-packages` rebuilds them with
   `--release 8`. Original phone jars are major 47–49 and ASM 3.1 reads those fine. The
   workaround was confirmed on the spot: `MIDletClassLoader.instrumentMIDletClasses = false`
   (a public static field) makes the jar load and render headless. The instrumentation only
   buys a `System.out` redirect and exception capture, which is not critical here; the proper
   fix in the fork is to upgrade ASM 3.1 → 9.x — a familiar tool, roughly 5 files under
   `app/classloader/*` to rewrite onto the new ASM API.
4. ✅ A MIDlet **in the default package** loads and starts — meaning the project's
   flatten-merged jars (Allods: `Container.GameMIDlet` plus default-package classes) pass by
   construction. The `Common` CLI accepts `{jar file}` | `{jad}` | `{class name}` directly, and
   already has `--rms (file|memory)` and `--resizableDevice W H`.

> **What actually happened with ASM.** The two-step mitigation was executed in full: the
> `--no-instrument` CLI flag first, then the real upgrade to **ASM 9.8** (BSD-3-Clause, shipped
> as a separate jar in `midp-rt/lib/`, not compiled into `midp-rt.jar`), with
> `org.microemu.app.classloader` (`ChangeCallsClassVisitor`, `ChangeCallsMethodVisitor`) ported
> to the ASM 9 API. Instrumentation now works on original phone bytecode (major 45–49) and on
> modern bytecode (major 52+) alike. One caveat carried forward: `ClassWriter(0)` preserves the
> input class's stack map frames as they are, and the non-standard `--enhanceCatchBlock` option
> (off by default) injects bytecode into catch blocks, which on major 51+ may need frames
> recomputed — the upgrade was not verified with that option on.

## 7. The headless launch result, and the risk it retired

Item 2 above is the single most valuable measurement of the study. Before it, the open question
was whether a Swing-era MIDP runtime could render at all without a window server — the whole
premise of frame-capture diff testing depended on the answer. It renders with
`java.awt.headless=true` and with no `DISPLAY` in the environment, because the drawing surface
is a `BufferedImage` and font metrics come from fontconfig. **The "Swing/AWT needs a display"
risk was retired on 2026-06-10** and no Xvfb, no virtual framebuffer and no window manager
appears anywhere in the runtime path.

What remains of that risk is narrow and documented: font metrics may differ between JDK builds
and platforms, so golden-frame tests use a perceptual tolerance rather than byte equality
across machines.

> **What actually happened.** Headless rendering of *GameCanvas* games needed one more fix that
> the study had not foreseen: upstream `J2SEDeviceDisplay.flushGraphics` cast the display
> component hard to `SwingDisplayComponent`, which in headless mode silently killed the game
> thread with a `ClassCastException` — GameCanvas frames never reached the listener. The fork
> introduces a shared interface `J2SEGraphicsSurfaceDisplayComponent`, implemented by both
> `SwingDisplayComponent` and `NoUiDisplayComponent`, and drops the hard cast.

## 8. The build: Maven was not resurrected

The upstream POMs date from 2010 and pull from repositories that no longer exist. Since plain
compilation was already proven (§6, item 1: 255 files, 0 errors), the assembly module was
dropped and replaced with a **simple `javac` script**, `midp-rt/build.sh`, compiling with
`--release 8` on JDK 17 into `midp-rt/build/midp-rt.jar`. No Maven, no Gradle, no dependency
resolution at build time; the one external jar (ASM) sits in `midp-rt/lib/`.

## 9. LGPL 2.1 obligations that shape the arrangement

These are the obligations the whole arrangement exists to satisfy:

1. **Separate artefact.** The fork is its own module and its own jar (`midp-rt.jar`); nothing
   from it is compiled, linked or embedded into the Arlecchino binary.
2. **Separate process.** Arlecchino talks to it only by spawning `java -cp midp-rt.jar …`,
   exactly as it talks to the decompiler engines. The licence boundary is the process boundary.
3. **Published sources.** The fork's sources are published, and travel with their own `COPYING`,
   `COPYING-LGPL-2.1`, `COPYING-AL-2.0`, `CREDITS`, `LICENSE` and `NOTICE`.
4. **NOTICE recording origin and changes.** LGPL 2.1 §2a requires the modified files to carry
   notice of the change; `midp-rt/NOTICE` records the upstream origin (barteo/microemu @
   `e47b656d983deaa67810c776662444483bdbf134`), the five cherry-picked commits with their
   hashes, and a dated list of every change made against upstream. It also carries the
   third-party attribution found inside MicroEmulator itself:
   `org/microemu/device/impl/Polygon.java` is not original MicroEmulator code but was extracted
   from `uk.ac.leeds.ccg.geotools.GeoPolygon` of the GeoTools project (© The Centre for
   Computational Geography, University of Leeds, 2002), distributed under LGPL 2.1 and used
   here on the same terms.
5. **No LGPL code in the Apache-2.0 tree.** Not one line. Violating this is risk №1 of the
   legal strategy, not a housekeeping detail.

## 10. The adaptation plan as it stood (B1'–B5')

Recorded as it was written on 2026-06-10, because it is the reasoning that produced the fork.
Its correspondence to the original plan: B1' ≈ B1 + B2 (the scan stays a checklist),
B2'–B3' = B3 + B4 + B5 (adapting instead of writing from scratch), B4' = B6, B5' = B7.

* **B1' — import the fork** (S–M): move the ✅ modules of §5 from barteo @ `e47b656d` into the
  new module; drop applet / swt / webstart / examples / jsr-75/120/82 / skins; keep the upstream
  headers and `COPYING*`; write a NOTICE giving the origin (microemu @ commit) and the changes
  (LGPL 2.1 §2a). Cherry-pick the five nikita36078 fixes (at minimum "Fix GameCanvas drawing"
  and "Fix Nokia games loading"). Build with a plain `javac` script — the 2010 Maven POM pulls
  dead repositories, and compilation is already proven — with `--release 8`. Test: the jar
  builds and the §6 smoke passes.
* **B2' — JDK 17 modernisation** (M): ASM 3.1 → 9.x in `app/classloader` (port
  `ClassPreprocessor` and the `ChangeCalls*Visitor`s to the ASM 9 API), **or** ship the proven
  `--no-instrument` flag first and make the ASM upgrade the second commit; walk the deprecations
  (`StringBuffer`/`Vector` stay — they work); give the header-less files licence headers. Test:
  both paths — instrumented for original jars, raw for merged ones — run the synthetic MIDlet.
* **B3' — headless frame rendering + input** (M–L; new code *inside* the LGPL fork): an entry
  point added by the fork on top of `Headless` / `NoUiDisplayComponent`, hanging a
  `DisplayRepaintListener` that dumps `J2SEGraphicsSurface.getImage()` (a `BufferedImage`) to
  PNG plus an index JSON in the Block A format (frame → file → sha256); key injection done
  programmatically through `J2SEInputMethod` (MIDP key codes from the A3 input script); RMS via
  `--rms file` with a configurable directory (`FileRecordStoreManager` already exists) for the
  A4 RMS diff. CLI: `java -jar midp-rt.jar --headless --frames-dir D --inputs S <jar>`. Test:
  a golden frame of the synthetic MIDlet (A6), and a RecordStore round-trip.
* **B4' — integration into Arlecchino** (S–M, on the Apache side): `setup` fetches or builds
  `midp-rt.jar` (pinned to a fork tag, with an ENV override following the engine pattern); a
  subcommand `arlecchino run <jar> [--shim|--freej2me] [--headless …]` spawns a **separate java
  process** (LGPL boundary = process boundary); `emulate` is left alone. Test: CLI smoke; the
  gate metrics 712 → 2 and 10/10 unchanged.
* **B5' — real corpus and polish** (M): run the 10-jar regression set (an INFO report, not a
  gate); the flatten-merged Allods jar must start and draw a screen; fix against actual
  failures (the completeness checklist being the B1 scan plus the signatures in
  `src/midp_stub.rs`). MMAPI / sound (jsr-135 `BasicPlayer`) only if something fails on it,
  otherwise Phase 2 (B8).

> **What actually happened.** All five stages were carried out, with the packaging change noted
> in §1 and the MMAPI change noted in §4. The headless capture entry point of B3' grew a richer
> flag set than sketched here and is documented separately in
> `docs/headless-capture-protocol.md`; the CLI landed as `arlecchino run` and
> `arlecchino difftest --engine midp-rt` in `src/cli.rs`, `src/midprun.rs` and `src/difftest.rs`,
> with jar discovery in `src/setup_engines.rs`. Sound did not wait for B8: MMAPI was needed by
> Allods and was implemented on 2026-06-12.

## 11. Risks and where they stand

| # | Risk | State and mitigation |
|---|------|----------------------|
| 1 | **ASM 3.1 vs modern bytecode** — the main one | Crash **confirmed** on merged jars (major 52) on 2026-06-10. Two-step mitigation, both steps also confirmed: the `no-instrument` flag (worked immediately) and then the ASM 9 upgrade (~5 files). Residual risk low; the upgrade to ASM 9.8 has since landed. |
| 2 | ⚖️ **LGPL obligations** | The aggregate jar is LGPL 2.1 because of the LGPL-only files (§4). Mandatory: published fork sources, NOTICE of changes, separate artefact and process, not one line in the Apache code. A breach is risk №1 of the legal strategy. |
| 3 | **Swing/AWT headless** | **RETIRED** — verified with no DISPLAY and with `java.awt.headless=true` (§7). Residual: font metrics may vary between JDKs and platforms, so golden tests use a perceptual tolerance. |
| 4 | **repaint / thread semantics** (`callSerially`, `serviceRepaints` coalescing) | This is where behaviour diverges from real handsets. Mitigated by comparing checkpoint frames rather than every frame in diff tests. |
| 5 | **EOL-2010 code base** | The networking/io parts (`org.microemu.microedition.io`) and MMAPI are weak. Bounded by the B1 API-surface scan and the rule "fix against actual failures". The Maven build is not resurrected — a `javac` script replaces it. |

## 12. B8 — scope consciously deferred

B8 was marked Phase 2, optional, explicitly **not part of the MVP**:

* a minimal JSR-135 (tones plus WAV/MIDI through `javax.sound`);
* pointer events;
* `lcdui.game.*`, if the B1 scan showed demand for it;
* an LWJGL rendering backend.

> **What actually happened.** Two of the four were pulled forward. `lcdui.game.*` came in with
> the imported `microemu-midp` module from the start (the B1 scan did show demand — GameCanvas,
> Sprite and LayerManager are common in real games), and JSR-135 went past the "minimal" mark on
> 2026-06-12 to real MIDI, WAV/AU/AIFF and tone playback, with sound off by default in headless
> mode so that frame diffs stay deterministic and independent of the audio device. Pointer
> events and the LWJGL backend remain deferred.

## 13. Research artefacts

* `~/research/microemu-ref/barteo` — the fork base, commit
  `e47b656d983deaa67810c776662444483bdbf134`.
* `~/research/microemu-ref/nikita36078` — the source of the cherry-picked fixes, commit
  `eb7e7d38bf842fc51777a671d425b03eb7e7535e`.
* `/tmp/me-smoke/` — the compilation and runtime smoke of §6 (255 classes, the test MIDlet, a
  `Launch.java` with `instrumentMIDletClasses=false`); temporary, and reproducible from §6 in
  minutes.

## 14. See also

* `docs/runtime-research.md` — the upstream feasibility study of 2026-06-10 that preceded this
  adoption, including the candidate-runtime licence matrix and the rejected alternatives.
* `docs/headless-capture-protocol.md` — the frame-capture protocol the runtime actually speaks.
* `docs/determinism-fixed-clock.md` — the bytecode-rewriting route to deterministic replay.
* `midp-rt/NOTICE`, `midp-rt/README.md` — the authoritative, travelling record of origin and
  changes.
