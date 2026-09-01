# Headless frame capture: the FreeJ2ME-plus pipe protocol

How `arlecchino difftest` and `arlecchino run --headless` get pixels out of
FreeJ2ME-plus without a window, an X server or any native code. This is the report of
the 2026-06-10 spike that settled the approach, plus the protocol notes it produced.
The implementation lives in `src/diffrun.rs`.

## The choice: speak the libretro front-end's pipe protocol directly

The plan named three candidates for headless capture: (a) an SDL2 dummy video driver,
(b) Xvfb plus AWT, (c) a minimal libretro front-end in C or Rust.

The spike found a fourth, strictly simpler option and took it. FreeJ2ME-plus's
"libretro core" is a thin C wrapper around a **Java process, `freej2me-lr.jar`**
(Main-Class `org.recompile.freej2me.Libretro`), which the wrapper drives over an
ordinary **stdin/stdout pipe protocol**. Nothing stops us from speaking that protocol
ourselves. So the toolkit launches `freej2me-lr.jar` directly and talks to it: no SDL,
no X server, no Xvfb, no native code. The emulator renders into its own internal
front-buffer and hands over raw RGB24 frames on request.

Options (a) and (b) were never implemented and never even measured — (c′) produced
frames immediately and is strictly less machinery. (This overrules the SDL2
recommendation of the earlier feasibility study; see `docs/runtime-research.md`.)

`freej2me-lr.jar` is installed by `arlecchino setup` (pinned release 1.52).

## The protocol

Clean-room notes taken by observing freej2me-plus 1.52.

**Child process argv** — 26 positional settings
(`width height rotate phone fps sound … doja ignore_volume`).

**stdin** — 5-byte commands `[op, b1..b4]`, where `b1..b4` is a big-endian `u32`:

| op | meaning |
|---|---|
| `11` | set the RMS directory (path length, then the path bytes) |
| `10` | load and run a jar (same length-prefixed encoding) |
| `15` | request a frame |
| `3` / `2` | joypad key down / up |
| `5` / `4` / `6` | pointer down / up / drag |

**stdout** — `+READY\n` after start; then, for each frame request, a **16-byte
header** — magic `0xFE`, `w:2`, `h:2`, `rotation:1`, `vibration:8`, `restart:1`,
`encoding:1` — followed by `w*h*3` bytes of RGB24.

## Three traps, all reproducible, all handled

1. **The encoding restart.** `MobilePlatform.load()` compares the JVM's `file.encoding`
   string with the one the MIDlet wants (default `ISO_8859_1`). On a Linux/UTF-8 locale
   that mismatches for **every** jar, so the emulator answers with a *restart frame*
   (`w = h = 0`, `restart = 1`) and goes to sleep. Cure: launch the JVM with
   `-Dfile.encoding=ISO_8859_1`, and handle the restart frame by relaunching with the
   encoding named in the header (`Shift_JIS` / `EUC_KR`). The restart frame carries a
   full-LCD payload that must be drained before relaunching, or the pipe desynchronises.
2. **Non-ASCII paths.** The front-end runs the jar path through
   `URLDecoder.decode(path, textEncoding)`. A Cyrillic path (`~/Загрузки/…`) turns to
   mush and the emulator reports "File not found". Cure: an ASCII symlink inside the
   output directory.
3. **stdout noise.** A MIDlet may print to `System.out`. The reader therefore scans
   forward to the `0xFE` magic byte rather than assuming the header starts where the
   previous frame ended (tolerance 1 MiB).

## Determinism, measured

Two runs of `Allods_1_2_240x320.jar`, 60 frames × 200 ms, 2 s boot wait. The run is a
Nival splash followed by a two-phase blinking screen. Unique frame hashes:

| screen state | sha256 |
|---|---|
| Nival splash | `5619c6da9171b0cad5e0036e9ded0a944afb73ddee7ff55485ed2e271f126cb9` |
| post-splash, phase A | `d849e9a2a4164103745df80200b9dd6cbefcc3751cc451f92a331c387220aaf5` |
| post-splash, phase B | `713b9ebfb8a06b86553798e45a77aaf600c97c56bf396e6dd66b83d4d49e5342` |

* The **hash sets of the two runs are equal** — 3 of 3 states byte-identical. The pixel
  render of each screen state is deterministic.
* **Frame-by-frame by index: 45/60 (75 %)** — state transitions shift by ±1 frame (the
  splash ends at frame 19 in run 1 and at frame 20 in run 2).

**Verdict: content is deterministic, timing is not.** For the harness this is better
than expected: comparing *sets and sequences of screen states* gives a clean signal
with no pixel noise, and the original-versus-original calibration run is needed only to
size the timing tolerances. Frame-exact replay needs more than this — see
`docs/determinism-fixed-clock.md`.

## Input replay and RMS, confirmed on the same protocol

* **Input.** Commands `3` / `2` carry joypad indices — D-pad `0`–`3`, middle soft key
  `7`, `NUM5` = `18` (the `standard getMobileKey` branch of the front-end). A script of
  `{t_ms, key, action}` events drove Allods from the splash all the way to the **game
  screen** (the hero on the map): 6 screen states that a run without input never
  reaches.
* **RMS.** Command `11` redirects record stores into a directory of our choosing. On
  startup Allods creates a store (`rms/Allods/…lzdgvt.{rms,1,2}` plus
  `config/Allods/game.conf`); reaching the game changes the `.rms`, which the
  snapshot/diff pass reports as added/changed.

## Key names across the two engines

The same input-script format is accepted by both `--engine freej2me` and
`--engine midp-rt`, and the key names are the same, but two things differ on the
midp-rt side because it replays through the runtime itself (`--inputs` of its headless
mode) rather than through a libretro joypad:

* `FIRE` / `SELECT` / `SOFT3` map onto the MicroEmulator device's **SELECT** button
  (the middle key, game action FIRE); `CLR` maps onto **DELETE**.
* A **raw integer** in `key` means a **MIDP keyCode** (e.g. `53` = NUM5) — *not* a
  libretro joypad index. A script that hard-codes joypad indices is therefore silently
  reinterpreted when run through `--engine midp-rt`; use the symbolic names in any
  script meant for both engines.
