# Behavioural verification of the difftest harness

This document is the permanent record of how the behavioural harness was validated: the first real difftest run on an original-versus-rebuild pair (2026-06-10), the cross-engine A↔B validation between FreeJ2ME-plus and midp-rt (2026-06-11), and the ten-jar smoke run of the whole reference set through midp-rt (2026-06-11). All measurements below were taken in those sessions on the Python edition of the toolkit; the modules carried over one-for-one into the Rust tree and the file and command names used here are the Rust ones. Test-gate counts quoted from that period are Python-edition gate figures and are labelled as such.

---

## 1. First real difftest run — Allods, 2026-06-10

Stages A5/A6 of the runtime plan closed with the first run of `arlecchino difftest` on a real pair: an original jar versus the project's own flatten rebuild. This was the readiness criterion for Block A.

### Preparing the pair

1. **Original:** `~/Загрузки/J2ME/Allods_1_2_240x320.jar` (Nival, 240×320).
2. **Rebuild:** the full pipeline with the opt-in executability flag:

   ```
   arlecchino decompile ~/Загрузки/J2ME/Allods_1_2_240x320.jar \
       -o /tmp/arl_allods_flat --config arlecchino.local.toml --flatten-packages
   ```

   Result: merged_errors = 3, round-trip coverage 30/30 (100 %); 2 classes (`a.java`, `q.java`) did not recompile and were seeded with the **original bytecode** in `merged/_classes`, exactly as the round trip intends.
3. **Packing the executable rebuilt jar** — this recipe is not obvious from the code and must survive. The packer takes the resources and the manifest from `_rename/Allods_1_2_240x320.renamed.jar` (the flatten-renamed jar, whose manifest already points at the default-package `GameMIDlet`), and takes every `.class` from `merged/_classes`:

   - source jar (manifest + resources): `/tmp/arl_allods_flat/_rename/Allods_1_2_240x320.renamed.jar`
   - classes directory: `/tmp/arl_allods_flat/merged/_classes`
   - output: `/tmp/arl_allods_rebuilt.jar`

   In the Rust edition this is `arlecchino::difftest::pack_rebuilt_jar(src_jar, classes_dir, out_jar)` in `src/difftest.rs`. **The packing step currently has no CLI surface**: the subcommands are `setup`, `decompile`, `emulate`, `run` and `difftest`, and none of them packs a rebuilt jar. It is reachable only as a library call (and is covered by the unit test `pack_rebuilt_jar_replaces_classes_and_keeps_resources`), so a rebuilt jar for a difftest has to be produced through that function or by reproducing the recipe above by hand.

### Input script

4 × `FIRE` (the middle soft key) drives the game from the splash through the menu to the gameplay screen — 6 unique screen states over 70 frames × 150 ms:

```json
[
  {"t_ms": 4000, "key": "FIRE", "action": "tap"},
  {"t_ms": 5500, "key": "FIRE", "action": "tap"},
  {"t_ms": 7000, "key": "FIRE", "action": "tap"},
  {"t_ms": 8500, "key": "FIRE", "action": "tap"}
]
```

### Noise calibration (original twice, no input, 40 frames × 150 ms)

```
arlecchino difftest --calibrate ~/Загрузки/J2ME/Allods_1_2_240x320.jar \
    -o /tmp/arl_dt_calib --frames 40 --period-ms 150
```

| metric | value |
|---|---|
| screen-state sets | **EQUAL** (Jaccard 1.00, 3 of 3 states byte-identical) |
| order of first appearance | identical |
| max timing shift | 7 frames (variability of JVM/splash start-up) |
| frame-by-frame match by index | 70 % (timing-sensitive, expected to be <100 %) |
| RMS byte-for-byte | **NO** (timestamps inside the records) — hence the verdict compares RMS content only when it is stable in the baseline |

This confirms the model from the A2 spike: content is deterministic, timing is not.

### Verdict, orig vs rebuilt (with the input script, 70 frames × 150 ms)

```
arlecchino difftest ~/Загрузки/J2ME/Allods_1_2_240x320.jar \
    /tmp/arl_allods_rebuilt.jar --input inputs.json -o /tmp/arl_dt_allods \
    --frames 70 --period-ms 150
```

**VERDICT: PASS (exit 0)** — the rebuild behaves like the original, within the noise.

| comparison | Jaccard | order | shift | frame-by-frame (info) |
|---|---|---|---|---|
| baseline (orig vs orig) | 0.86 (orig_b caught one extra transient menu-transition frame) | same | 1 | 97 % |
| **orig vs rebuilt** | **1.00 — sets EQUAL** | same | 0 | 100 % |

- All 6 screen states (Nival splash → menu → … → gameplay screen with the hero) are **pixel-identical** in the rebuild (the frame sha256 values matched).
- RMS: the same 4 store files are created (identical paths); their content differs byte for byte, which is within the noise — it also differs between two runs of the original.
- No new exceptions in the emulator's stderr for the rebuild.
- The cross result (1.00) came out **cleaner than the baseline** (0.86) simply because orig_b caught a short-lived transition frame that did not land in the samples of the other two runs. Calibrating the verdict against the baseline is exactly what handles this.

Negative control — proof that difftest can FAIL — is covered by the A6 integration test: a recoloured synthetic MIDlet gives FAIL with `state-set similarity 0.00` (in the Rust tree: `tests/difftest_live.rs::difftest_fails_on_a_recoloured_pattern`).

### Conclusions reached

- The Block A readiness criterion was met: orig-vs-orig calibration works, orig-vs-merged (flatten) produces a verdict without crashing, and that verdict is PASS.
- Emulator noise on Allods is timing only (a start-up shift of up to 7 frames) plus RMS timestamps; the pixel content of the states is strictly deterministic, so the difftest signal is clean. The conclusion drawn at the time was that the A7 virtual clock was **not needed for the MVP**.
  > Superseded in part: the virtual clock was built anyway (2026-06-12) and later became load-bearing — see `docs/determinism-fixed-clock.md`, especially the 2026-07-30 StackMapTable addendum, where a rebuild with a modern `javac` could not be difftested at all until the transformer was fixed.
- Gate: 88 passed / 2 skipped (the live skips need `ARLECCHINO_REGRESS_DIR`) — a Python-edition gate figure; the 712→2 and 10/10 reference metrics were unaffected.

---

## 2. Cross-engine validation (the A↔B link), 2026-06-11

The final validation of Block B: **the same original jar** is run in two engines — FreeJ2ME-plus (the Block A reference) and midp-rt (the project's LGPL fork of MicroEmulator, Block B) — with the same input script, and compared **softly and structurally**:

- the sequence of **screen-state changes** (when and how often the picture changes), with tolerance `--state-tol` (default ±3);
- the presence or absence of **RMS activity** — the store names on disk differ by construction between the engines (FreeJ2ME-plus mangles `system` into `0a9e551506c3lzdgvt` plus a write file, midp-rt writes `system.rs`), so only the *fact* of writing is compared;
- **exceptions** logged by only one engine;
- blank versus rendered frames.

**Pixels are deliberately NOT compared across engines**: different skins, fonts and palettes make a Jaccard of ~0.00 legitimate, so the pixel figure is emitted as info only.

Implementation: `arlecchino difftest <orig.jar> --cross-engine [--state-tol N]` — `compare_cross_engine` in `src/difftest.rs` is a pure function with unit tests that need no emulator. Reports are written to `cross_engine_report.{md,json}`.

### Run: 4 jars, script 4 × FIRE (t = 2/4/6/8 s), 70 frames × 150 ms

| jar | verdict | freej2me: states/changes | midp-rt: states/changes | RMS in both | note |
|---|---|---|---|---|---|
| Allods_1_2_240x320 | **AGREE** | 6/13 | 7/14 | yes | changes nearly frame-synchronous (14, 16, 18, … 40 in both) |
| Liftboy_240x320_rus | **AGREE** | 49/48 | 51/51 | yes | animated title sequence, both traces about 50 states |
| GOTHIC-3 | DIVERGE¹ | 7/6 | 17/16 | yes | false signal: progress-bar animation granularity |
| Silent_Storm_240x320 | DIVERGE² | 1/0 + IllegalStateException | 6/6 | yes | **defect on the FreeJ2ME-plus side**, not midp-rt |

¹ **Gothic 3 — a legitimate difference, not a midp-rt defect.** Both engines walk the same path: HandyGames splash + progress bar → sound dialog → title → menu → intro dialogue «Ксардас!.. Помоги мне!!!» (the same final screen, checked against the frames of both runs). midp-rt redraws the loading progress bar more often (frames 7–14 each hash separately) while FreeJ2ME-plus coalesces the repaints — 17 versus 7 unique states for an identical trajectory. This is a known limitation of the soft metric: the state counter is sensitive to animation granularity. With `--state-tol 10` the pair converges to AGREE.

² **Silent Storm — the harness caught a real defect in the reference emulator.** FreeJ2ME-plus gets stuck on the Nival splash forever: its MMAPI (`org.recompile.mobile.PlatformPlayer.getControl` — "player is either CLOSED or UNREALIZED") repeatedly throws `IllegalStateException` out of the game's sound code (`joybits.SilentStorm.c.a`), and the game never advances (1 state, 0 changes over 10.5 s). midp-rt, with its stub JSR-135, gets through: splash → title «Операция Silent Storm» → menu «Новая игра / [КАМПАНИЯ] / СЕТЕВАЯ ИГРА» → «Руководство». This is an argument for keeping a second, independent reference: the divergence pointed at fragility in FreeJ2ME-plus, and midp-rt is the correct one here.

### Verdict

- The A↔B link works: "orig in the emulator vs orig in the shim" from the plan is implemented and has been run on real jars.
- 2 of 4 strict AGREE; both DIVERGE cases were analysed frame by frame and neither is a midp-rt defect (one is animation granularity, the other a bug in FreeJ2ME-plus itself).
- **No behavioural divergence of midp-rt against the reference was found** on this sample.

### Interactive windowed check (3 jars live, `arlecchino run`)

Windows were opened on screen (KDE Wayland/XWayland); input was scripted through uinput (ydotool — XTEST is blocked in Plasma 6 by the remote-control portal); screenshots at `/tmp/arl_interactive_{allods,gothic3,liftboy}.png`.

| jar | path | result |
|---|---|---|
| Allods | menu (◄►/select) → «Новая игра» → intro → **gameplay screen** (forest, road, character) | OK, exit 0, no exceptions |
| Gothic 3 | «Включить звук?» → menu → «Старт» → Xardas intro dialogue (multi-page) → **gameplay** (house, HUD, character turns with the arrow keys) | OK after the fix¹, exit 0 |
| Liftboy | «Звук вкл.?» (soft ✓) → menu «Начать игру» → «Вступление» → **the hotel scene** (tutorial, dialogue pages through) | OK, exit 0, no exceptions |

¹ **A windowed-mode defect in midp-rt was found and fixed.** GameCanvas games (Gothic 3) showed a grey window forever while the headless renderer worked. Root cause: `SwingDisplayComponent.getGraphicsSurface()` returned `null` until the first `repaintRequest`, so `GameCanvas.flushGraphics` → `J2SEDeviceDisplay.flushGraphics` silently dropped every frame of a fullscreen game. The fork had already fixed the same class of defect on the headless side (`NoUiDisplayComponent.getGraphicsSurface`, lazy surface creation); the fix was mirrored into the Swing component (repository arlecchino-midp-rt). After the fix: the fork's smoke test is OK (frames bit-for-bit, input arrives) and Gothic 3 plays through to gameplay in a window.

Observation (not a defect): moving with the arrow keys in Allods did not shift the view — the map view is static on the starting cell or blocked by an obstacle; the menu and the intro do respond to the same keys.

### Boundary of the method

- The state counter captures the trajectory but is sensitive to the redraw frequency of animations (Gothic 3). When a divergence appears, look at the frames by eye before blaming an engine.
- The "orig/merged in midp-rt" comparison (deterministic mode) is confirmed separately in part 3 below (Jaccard 1.00, frame-by-frame 100 %).

---

## 3. Ten-jar smoke run through midp-rt (headless), 2026-06-11

Stages B5'/B7': a smoke run of the real reference set through `arlecchino run --headless` on the **midp-rt** engine (the project's LGPL fork of MicroEmulator, in a separate process). This is an **INFO report, not a gate** — failures are recorded, not fixed.

### Method

```
run_headless_midp_rt(jar, out, max_frames=30, frame_period_ms=100,
                     boot_wait_s=2.0, width=240, height=320)
```

— equivalent to `arlecchino run --headless <jar> --frames 30 --size 240x320` (`src/midprun.rs`). No input script (start-up screens only), ASM instrumentation ON, RMS file-backed flat in `out/rms`, `-Duser.home` isolated. The "renders" criterion: 30 PNG frames written and at least some non-blank (not single-colour). "exceptions" are the Java exception names taken from the runtime log — some of them are ordinary noise from the games themselves.

### Results: **10/10 loaded and render**, 0 failures

| jar | run | frames | unique states | non-blank | exceptions in the log |
|---|---|---|---|---|---|
| Allods_1_2_240x320.jar | ok | 30 | 1 | 30/30 | — |
| Bruce_Lee_-_Iron_Fist_2D.jar | ok | 30 | 2 | 30/30 | — |
| Elven_Chronicles_v1_0_5ru_240x320.jar | ok | 30 | 5 | 28/30 | — |
| Flitzer_w900_rus.jar | ok | 30 | 2 | 30/30 | — |
| GOTHIC-3.JAR | ok | 30 | 11 | 30/30 | — |
| Get_Rich_or_Die_Tryin_ru_240x320.jar | ok | 30 | 2 | 30/30 | — |
| Liftboy_240x320_rus.jar | ok | 30 | 21 | 30/30 | — |
| Masha_i_Medved_240x320.jar | ok | 30 | 7 | 30/30 | RecordStoreNotFoundException |
| Silent_Storm_240x320.jar | ok | 30 | 2 | 30/30 | — |
| cyber_blood_rus.jar | ok | 30 | 1 | 30/30 | — |

### Interpretation notes (these prevent false alarms)

- Visual spot-check of the frames: GOTHIC-3 — frame plus the «Включить звук?» dialogue; Liftboy — title screen plus «Звук вкл.?»; Allods — the Nival splash (and with an input script of 2 × FIRE it reaches the gameplay screen — see the fork smoke test and the difftest below).
- **"unique states = 1" (Allods, cyber_blood) is not a defect** — it is a static splash during the first 3 seconds. Likewise, the 2 blank frames of Elven Chronicles are start-up before the first draw.
- **`RecordStoreNotFoundException` on Masha i Medved is not a crash** — it is the normal save probe on first launch (`openRecordStore(..., false)` against an empty store).
- No comparison against the Block A FreeJ2ME engine was made here (a different task); this is a smoke test of midp-rt specifically.

### Context from the same session

`arlecchino difftest --engine midp-rt` on the Allods pair, original versus the flatten rebuild (`/tmp/arl_allods_rebuilt.jar`, input script 4 × FIRE, 70 frames × 150 ms): **PASS** — Jaccard 1.00 (7 states, sets EQUAL), shift 0, **frame-by-frame 100 % in both the baseline and the cross comparison**. Unlike FreeJ2ME-plus (baseline 0.86 / 97 %), midp-rt was fully deterministic on one machine on this path; the RMS store files match by name, their content differing only in the game's own record timestamps, within the baseline noise.

The gate was unaffected: 115 passed / 2 skipped — a Python-edition gate figure — with the 712→2 and 10/10 reference metrics unchanged.

---

## Closing note: reconciling "midp-rt is deterministic" with the reported flakiness

Part 3 above calls midp-rt fully deterministic on one machine, while `docs/determinism-fixed-clock.md` records an unstable baseline on midp-rt that jumps between Jaccard 1.00 and 0.75 (one state out of seven) across three consecutive `--calibrate` runs. These are not contradictory measurements — they are **different paths**:

- The 2026-06-11 result above was measured **without** `--fixed-clock`: the jar is run as-is and midp-rt reproduces the run exactly.
- The 2026-07-30 result was measured **with** `--fixed-clock`, where the jar is ASM-instrumented first. On that path the residual noise comes from scheduling inside midp-rt itself, which the instrumentation does not touch, and it is to be addressed in Block B.

The full analysis of what the instrumentation does and does not guarantee is in `docs/determinism-fixed-clock.md`.
