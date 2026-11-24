# arlecchino-midp-rt

An LGPL fork of [MicroEmulator](https://github.com/barteo/microemu) — a J2ME runtime (MIDP 2.0 / CLDC 1.1)
for running mobile games on a desktop JVM. Used by the **Arlecchino** project as a
**separate process** (a separate artifact `midp-rt.jar`; the code of this repository never gets
into Arlecchino's Apache-2.0 code — the LGPL boundary = the process boundary).

## Origin

- **Base:** [barteo/microemu](https://github.com/barteo/microemu)
  @ `e47b656d983deaa67810c776662444483bdbf134` (MicroEmulator 3.0.0-SNAPSHOT,
  the last state of the upstream code, ~2010).
- **Fixes:** 5 commits from the fork
  [nikita36078/microemu](https://github.com/nikita36078/microemu)
  @ `eb7e7d38bf842fc51777a671d425b03eb7e7535e` (by the author of J2ME-Loader), carried over by hand:
  - Remove ClosedConnection (`6575842`)
  - Fix GameCanvas drawing (`70c8503`)
  - Improve exceptions handling (`e7c9da6`)
  - Delete unused classloaders (`5b36135`)
  - Fix Nokia games loading issue (`eb7e7d3`)

The full list of changes relative to upstream is in [NOTICE](NOTICE) and the git history.

## License

**GNU LGPL 2.1** (see [LICENSE](LICENSE)). Most upstream files are
dual-licensed LGPL 2.1 / Apache 2.0 (see `COPYING`, `COPYING-AL-2.0`),
but ~35 files (all of `javax.microedition.lcdui.game.*`, some lcdui widgets,
`device/impl/Polygon.java`) are LGPL 2.1 only, so the combined artifact
is distributed under the terms of LGPL 2.1. The fork's sources are published.

## Contents (what is taken from upstream)

| Module | Contents |
|---|---|
| `microemu-cldc` | `javax.microedition.io` (Connector, *Connection) |
| `microemu-midp` | core: `javax.microedition.{lcdui, lcdui.game, midlet, rms, io, pki}`, `org.microemu.*` |
| `microemu-javase` | jar/jad launching (`org.microemu.app.Common`), classloader + ASM instrumentation, RMS managers, device resources (default/resizable) |
| `microemu-javase-swing` | Java2D rendering (`org.microemu.device.j2se.*`, BufferedImage), headless launching (`org.microemu.app.Headless`, `NoUiDisplayComponent`), Swing GUI (`org.microemu.app.Main`) |
| `microemu-extensions/microemu-nokiaui` | Nokia UI API (`com.nokia.mid.*`) |
| `microemu-extensions/microemu-siemensapi` | Siemens API (`com.siemens.mp.*`) |

Dropped from upstream: applet, SWT, webstart, android, iphone, device skins,
JSR-75/120/82, examples/tests, the Maven build. JSR-135 (MMAPI) sound in the fork
is written from scratch from the public JSR-135 specification:
`javax.microedition.media[.control]` (`Manager`/`Player`/`Controllable`/
`VolumeControl`/`ToneControl` etc.) + real players in
`org.microemu.media`: **MIDI** → `MidiPlayer` (javax.sound.midi, volume
by intercepting CC7), **WAV/AU/AIFF** → `SampledPlayer` (javax.sound.sampled,
MASTER_GAIN volume), `Manager.playTone` → `ToneGenerator`. The media type
is determined by the content-type or the data signature (MThd/RIFF/.snd/FORM);
unsupported formats (AMR, MP3 etc.) get a safe no-op
`NoopPlayer` with a correct state machine — the game runs without sound and without
`NoClassDefFoundError`. The system property `org.microemu.media.noop=true`
forces sound off; headless mode (`org.microemu.app.Headless`)
turns that on by default — frame diff tests are deterministic and do not depend
on an audio device.
`microemu-injected` was not copied: its only class `org.microemu.Injected`
is duplicated (another variant of the same FQCN) in `microemu-javase` — the javase variant is used.

## Building

Requires JDK 17 (compiled with `--release 8`). The upstream Maven build is dead and not used.

```sh
./build.sh        # → build/midp-rt.jar
```

## Running

```sh
# headless (the default Main-Class), RMS in memory
java -cp build/midp-rt.jar:lib/asm-9.8.jar org.microemu.app.Headless <game.jar|game.jad|MIDletClass>

# Swing window
java -cp build/midp-rt.jar:lib/asm-9.8.jar org.microemu.app.Main <game.jar>
```

Useful CLI flags: `--rms (file|memory)`, `--resizableDevice W H`,
`--appclassloader system` (for a MIDlet class on the classpath),
`--no-instrument` (turn off ASM instrumentation of MIDlet classes).

## Headless frame capture (for Arlecchino's difftest)

`org.microemu.app.Headless` has a capture mode (the fork's own flags, stripped
before the rest is passed to `Common`):

```sh
java -Djava.awt.headless=true -cp build/midp-rt.jar:lib/asm-9.8.jar \
    org.microemu.app.Headless \
    --frames-dir out/cap --frames 30 --period-ms 100 --boot-wait-ms 2000 \
    --inputs script.json --rms out/rms game.jar
```

- `--frames-dir DIR` — turns capture on: PNG frames `frame_NNNN.png` +
  `index.json` (sha256 of the frame's raw RGB24 bytes, dimensions, a `blank` flag =
  all pixels of one colour); when capture ends the process exits by itself;
- `--frames N` / `--period-ms MS` / `--boot-wait-ms MS` — how many frames,
  the sampling period and the pause after the MIDlet starts before the first frame;
- `--inputs FILE` — a JSON input script `[{"t_ms":4500,"key":"FIRE","action":"tap"},…]`
  (Arlecchino's difftest format, `docs/difftest_format.md` in arlecchino-remake);
  `t_ms` counts from the end of boot-wait; key names: `UP/DOWN/LEFT/RIGHT`,
  `NUM0`–`NUM9`, `STAR`, `POUND`, `FIRE`/`SELECT`/`SOFT3`, `SOFT_LEFT`,
  `SOFT_RIGHT`, `CLR`; a number instead of a name = a raw MIDP keyCode;
- `--ticks N` — deterministic mode: exactly N ticks (a tick = a frame boundary,
  the `F n` line of the trace: `paint()` or `GameCanvas.flushGraphics()`), then
  exit; scenario events are given by a `tick` field instead of `t_ms`
  (`[{"tick":259,"key":"FIRE","action":"press"},…]`) and are delivered
  synchronously right after `F n`, before frame n is drawn; with `--frames-dir` —
  one frame per tick; the wall clock plays no part (except for the watchdog
  `--tick-timeout-ms MS`, 30000 by default, exit 4). Details —
  `docs/determinism-fixed-clock.md` (section "deterministic input") in Arlecchino;
- `--rms DIR` — an extension of upstream `--rms (file|memory)`: a path value =
  file-backed RMS **flat** in DIR (no `suite-<name>` — store files are comparable
  between the original and the rebuilt jar even with different MIDlet class names);
- autostart: if the last argument is a `.jar`/`.jad`, the class from `MIDlet-1`
  is taken from the manifest/jad and a class-name invocation is substituted
  (the headless Launcher never repaints, so without autostart the frames would be blank).

## ASM instrumentation

`MIDletClassLoader` runs the game's classes through ASM instrumentation
(redirecting `System.out`/`System.err` into `org.microemu.Injected`, replacing
`Thread`/`Timer`/`TimerTask`, `getResourceAsStream`). **ASM 9.8** is bundled
(BSD-3-Clause, a separate jar in `lib/`, see NOTICE); the code of
`org.microemu.app.classloader.*` is ported from ASM 3.1 to the ASM 9 API.
Instrumentation works both on the original phone jars (old bytecode,
major 45–49) and on modern bytecode (major 52+) — the ASM 3.1 built into upstream
failed on major 52+ (historical limitation B1', lifted in B2').

It is turned off with the CLI flag `--no-instrument` (the System.out redirect
and exception interception are lost; not critical for the runtime).

Note: `ClassWriter(0)` keeps the input class's stack map frames as
they are; the non-standard option `--enhanceCatchBlock` (off by default)
inserts bytecode into catch blocks and on major 51+ may require recomputing
frames — the upgrade was not tested with it.

## Smoke test

```sh
./smoke/smoke.sh   # instrumentation ON; three phases:
                   #  1) a test MIDlet, major 52 (modern bytecode),
                   #  2) an original phone jar (ARLECCHINO_SMOKE_JAR, optional),
                   #  3) headless capture: 12 PNG + index.json, NUM5 input gives
                   #     a new screen state (>=2 unique sha256);
                   # phases 1–2: two headless runs, a frame dump, sha256 comparison
```

Phase 2 runs only when `ARLECCHINO_SMOKE_JAR=<game.jar>` points at an original phone
jar; otherwise it is skipped with a message.

Sound (JSR-135) is checked by a separate live smoke test — you hear it from the speakers:

```sh
javac --release 8 -cp build/midp-rt.jar -d smoke/out smoke/MmapiSmoke.java
java -cp build/midp-rt.jar:smoke/out MmapiSmoke <file.mid>
# the Allods scenario: createPlayer/realize/prefetch/setLoopCount(-1)/start,
# then VolumeControl setLevel/setMute and Manager.playTone
```

Recorded facts (2026-06-10/11, JDK 17.0.19, Linux), all with ASM 9
instrumentation ON:

- a test MIDlet, **major 52** (Canvas: fillRect + drawString + a key bar,
  176×176): two independent headless runs give a **bit-for-bit identical
  PNG frame** — sha256
  `00d27a40f5686289c20438f26cb88df9cff1275b9b811e5aa78843e82bf85e16`
  (2026-06-11, after the key bar was added to TestMidlet);
- the original phone jar **Allods / Rage of Mages** (Nival 2005,
  176×220, class file **major 45**, GameCanvas + MMAPI): the Nival Interactive
  splash renders, two runs bit-for-bit — sha256
  `ad301c11ef8224c8e0609a3144d4f402b5f999a7b0cdebcd5d6dde1599c7838b`;
- headless capture (phase 3 + a live 240×320 run): Allods
  `Allods_1_2_240x320.jar` with a 2×`FIRE` input script reaches the gameplay
  screen (30 frames, 6 unique states, 0 blank), the RMS store
  `system.rms` is written flat into the `--rms` directory.

Rendering is deterministic (important for the checkpoint diffs of Arlecchino's Block A / A7).
Caveat: checked on one machine/JDK; font metrics may differ between
platforms/JDK versions — cross-platform determinism is not claimed.

Headless rendering of GameCanvas games works thanks to a fork fix: upstream
`J2SEDeviceDisplay.flushGraphics` hard-cast the display component to
`SwingDisplayComponent` and in headless silently killed the game thread with a
`ClassCastException` — a common interface
`J2SEGraphicsSurfaceDisplayComponent` (Swing + NoUi) was introduced.
