# Determinism: the fixed-clock instrumentation

This document is the permanent record of the deterministic-clock instrumentation of MIDlets (`--fixed-clock`): what the ASM transformer rewrites, what it deliberately leaves alone, what was measured on the reference game, and — most importantly — where the honest boundary of the guarantee lies. The core work was done and measured on 2026-06-12 (FreeJ2ME-plus, the game Allods); the StackMapTable addendum at the end was found and fixed on 2026-07-30 (a second corpus jar, midp-rt). Measurements quoted here are historical figures from the Python edition of the toolkit; the implementation carried over module-for-module into the Rust tree, and the file names below are the Rust ones.

Stage A7 of the runtime plan was implemented by **route (b)** — ASM instrumentation of the MIDlet — and explicitly **not** by forking FreeJ2ME-plus (route (a), a GPL fork, was rejected). It is a frame-oriented determinism reserve for running difftest on a noisy emulator.

## Components

| component | file |
|---|---|
| ASM transformer (clean-room, Apache-2.0) | `assets/DeterministicClockTransformer.java` |
| orchestration (same shape as the member renamer) | `src/fixedclock.rs` |
| shared compiler for the ASM helpers | `src/rename.rs` (`compile_helper`, parameterised by helper class) |
| build during setup | `src/setup_engines.rs` (compiles both helpers) |
| CLI flag + env | `arlecchino difftest --fixed-clock` / `ARLECCHINO_FIXED_CLOCK=1` (`src/cli.rs`) |
| difftest integration | `src/difftest.rs` (`calibrate` / `difftest` / `cross_engine`, `_fixed_clock_jar`) |
| unit tests | 4 tests added for this stage |

## What the transformer rewrites

- `INVOKESTATIC System.currentTimeMillis()J` / `nanoTime()J` → `arlecchino/det/Det.currentTimeMillis` / `nanoTime` — a **virtual time counter**: every call advances the clock by a fixed step (1 virtual millisecond per call), starting from a fixed epoch. A clock-polling animation loop therefore advances deterministically, and repeating the same number of calls yields the same timestamps.
- `INVOKESPECIAL java/util/Random.<init>()V` → push `FIXED_SEED` + `<init>(J)V`.
- `INVOKESPECIAL java/util/Random.<init>(J)V` → `POP2` of the original seed, then `FIXED_SEED`.
- `INVOKESTATIC Math.random()D` → `Det.random()` (a seeded `Random.nextDouble`).
- The runtime class `arlecchino/det/Det` is **generated with ASM and injected** into the output jar, so the emulator needs no classpath entry and no external dependency.

### What is deliberately not touched

`System.gc`, identity `hashCode` and `Thread` timing are left alone. For the frame-by-frame picture on these MIDlets they are irrelevant: frames are sampled on request, not off the clock.

### Reuse of the ASM base

The same `<engines>/asm` (ASM 9.7, BSD-3-Clause, invoked and not vendored) and the same `_classes/` directory as the member renamer. `setup` compiles both helpers through one code path (`compile_helper` was generalised with a helper-class parameter).

## Live test on Allods (2026-06-12)

Original: `path/to/Allods_1_2_240x320.jar`. Input: 4 × FIRE (splash → menu → gameplay screen), 70 frames × 150 ms.

### Bytecode

```
DeterministicClockTransformer: clock=10 new Random()=0 new Random(seed)=3
  Math.random()=0 (runtime class -> arlecchino/det/Det)
```

- 10 `System.currentTimeMillis` call sites intercepted (0 residual), 3 `Random(seed)` constructions re-seeded;
- bytecode verification of all 29 MIDlet classes — **0 verify failures**.

### Runtime determinism (the strictest criterion)

With the wall-clock sampling delay disabled (`deterministic_capture`): **8 independent runs (4 pairs × 2, each a separate JVM launch) produced ONE AND THE SAME complete sequence of 70 frames**, a single sequence sha `e1d9f9348b96716a`, with the frames byte-identical (sha256) across runs.

### Standard wall-clock capture loop (runs sequential, machine idle)

| | Jaccard of screen-state sets (6 runs) | frame-by-frame |
|---|---|---|
| original (BEFORE) | {0.86, 0.86, 1.00, 0.71, …} — noisy, 1.00 reached ~2 of 6 | 96–100 % |
| fixed-clock (AFTER) | {0.86, **1.00, 1.00, 1.00, 1.00, 1.00**} — 5 of 6 | 96–100 % |

## Calibrated confidence — the honest boundary

- **What was verified:** bytecode (javap + verify), runtime bit-for-bit reproduction (8 runs, separate JVMs), orig-vs-orig Jaccard BEFORE vs AFTER (6 runs each), 4 unit tests, and the full test gate (123 passed / 2 skipped — a Python-edition gate figure, up from 119/2 by the 4 new tests).
- **Where determinism is COMPLETE (high confidence):** a single JVM run of the instrumented jar is reproducible bit for bit — the MIDlet's own time and RNG logic no longer depends on the wall clock. Proven by the 8 runs collapsing to one sequence sha and by the separate-JVM unit test.
- **Where it is NOT guaranteed (the honest boundary):** 100 % frame accuracy under the standard capture loop or under load. The residual ±1-frame noise is the **phase of sampling relative to the FreeJ2ME-plus rendering thread**, which the harness does not control (op-15 returns the current front buffer; how many game ticks the MIDlet thread managed between two requests depends on the OS scheduler). The instrumentation removes ONE of the two sources of non-determinism — the MIDlet's clock and RNG. The second one can only be cured by controlling scheduling: either a fork (route (a), rejected) or the project's own midp-rt engine (Block B), where the tick is issued by the harness.
- **Counter-finding (recorded so it is not repeated):** removing the wall-clock sleep from the capture loop (back-to-back op-15) does NOT improve stability on FreeJ2ME-plus — it makes it WORSE, because it catches a still-loading MIDlet at random phases. The automatic difftest mode therefore keeps the standard wall-clock cadence; `deterministic_capture` remains an explicit escape hatch in `diffrun::run_headless` (`src/diffrun.rs`) and is never switched on automatically.

## Nothing broken

- Full gate: **123 passed / 2 skipped** (previously 119/2; +4 tests for this stage) — a Python-edition gate figure.
- midp-rt difftest and the regression set were unaffected.
- The default (non-fixed) mode is byte-identical: the flag is opt-in, and without it the transformer does not run and prints nothing.

## Clean-room / licences

- FreeJ2ME-plus (GPL) — behaviour is observed only; it is neither modified nor copied. All of the code for this stage is original and Apache-2.0.
- ASM (BSD-3-Clause) — invoked, not vendored.

---

## Addendum, 2026-07-30: StackMapTable frames (class files with major ≥ 50)

Found on a second corpus jar. The transformer read classes with `ClassReader.SKIP_FRAMES` but wrote them with a `ClassWriter` carrying only `COMPUTE_MAXS` — that is, it **discarded the StackMapTable and put nothing in its place**.

- On ORIGINAL phone jars (major 48/49) there are no frames and none are needed, so the defect never showed: every earlier measurement in this document remains valid.
- On a tree rebuilt with a modern `javac` (major 61) the instrumented jar stopped loading:
  `VerifyError: Expecting a stackmap frame at branch target 150`.
  The very thing difftest exists for — comparing an original against a rebuild — was impossible on a modern build. The temporary workaround was to build with `-source 7 -target 7 -XDstringConcat=inline` and patch the class major version down to 49.

### How it was fixed

- **Main path — PRESERVE the frames.** None of the edits change the shape of the stack at a frame point (a call is replaced by a call with the same descriptor; the seed push/pop happens strictly between instructions), so the original frames remain correct — they only have to not be lost. For major ≥ 50 the class is read with flags `0` instead of `SKIP_FRAMES`; ASM re-emits every frame at its shifted offset, while `COMPUTE_MAXS` still recomputes `max_stack`.
- **Below major 50 — exactly the previous behaviour** (`SKIP_FRAMES`): a desktop JVM verifies such classes by type inference and ignores the CLDC `StackMap` attribute. Checked byte for byte: every class entry of that instrumented jar matches the pre-fix output.
- **Fallback path — `COMPUTE_FRAMES`** (`--recompute-frames`, and also automatically when a class with major ≥ 50 has branches but no frames at all, because an obfuscator stripped them). `COMPUTE_FRAMES` calls `getCommonSuperClass`, which needs a loader that sees both the jar's own classes and the MIDP API. The `Hierarchy` class resolves super/interface links **by reading `.class` resources** from a loader spanning [the input jar] + [`--cp`] + [the transformer's own loader], without initialising classes; `fixedclock::api_classpath` (`src/fixedclock.rs`) supplies `midp-rt.jar` / the emulator jar automatically. Unresolved types are **reported**, not swallowed — without `--cp` the output shows `Unresolved: [javax/microedition/rms/RecordStore]`.
- The injected `arlecchino/det/Det.class` was given a fixed mtime, so instrumenting the same jar twice is now reproducible bit for bit.
- The same latent defect was cleaned out of `assets/MemberRenamer.java` (which wrote `ClassWriter(0)` + `SKIP_FRAMES`): renaming members does not change the shape of the stack at all, so the frames are simply carried through, and `ClassRemapper` remaps the types inside them along the way.

### Verification of the fix

- Before the fix, `difftest --engine midp-rt --fixed-clock` on a major-61 jar: `VerifyError`, Jaccard 0.00, frame-by-frame 0 %, VERDICT FAIL.
- After: no `VerifyError` in any run log; the jar works WITHOUT the `-source 7` crutch and without the class-major patch.
- The reference run without `--fixed-clock` was reproduced: orig vs rebuilt (major 61) — Jaccard 1.00, frame-by-frame 100 %.
- Full gate: **128 passed / 2 skipped** (+6 frame tests; the 2 skips need external artefacts) — a Python-edition gate figure.

### Remaining honest boundary (open, tracked separately)

`--fixed-clock` on midp-rt **does not give a fully deterministic run**: the orig-vs-orig baseline noise jumps between Jaccard 1.00 and 0.75 (one state out of seven) from launch to launch — measured with three consecutive `--calibrate` runs. Because of this the verdict of `difftest --fixed-clock` on midp-rt is unstable. It has nothing to do with stack maps (the original jar is instrumented byte for byte as before): the major-49 jar from the old crutch gives exactly the same 0.75. The source is scheduling inside midp-rt itself, and it is to be addressed in Block B.

> Note on an earlier statement: the first difftest run (2026-06-10, recorded in `docs/behavioural-verification.md`) concluded that virtual clocks were not needed for the MVP, because the emulator noise on Allods was timing-only. That conclusion held for that pair; this stage was built anyway as a reserve for noisier games, and the addendum above shows it later became load-bearing on rebuilt trees with modern class files.

> Note on a second earlier statement: the ten-jar report of 2026-06-11 (also in `docs/behavioural-verification.md`) called midp-rt fully deterministic on one machine. That measurement was taken **without** `--fixed-clock`; with the instrumentation enabled the baseline is the flaky 1.00/0.75 described above. See the closing note of that document.

---

## Addendum, 2026-11-14: deterministic input (Block B, `midp-rt --ticks`)

The instrumentation above fixes the MIDlet's clock and RNG, but on midp-rt input still arrived by the wall clock: the capture loop injected an event when `now >= t_ms`, the runtime queued it on its event thread, and which game tick saw it depended on the scheduler. Comparing a reference run with a translated port therefore drifted by about one frame around every key press. Block B removes that source by letting the harness, not the scheduler, decide when an event is seen.

### The tick

A **tick** is a frame boundary: every `Displayable.paint()` through the runtime's display (Canvas games) and every `GameCanvas.flushGraphics()` (GameCanvas games). Tick `n` is the trace line `F n`; tick 0 is the first frame. `flushGraphics()` became a frame boundary in the trace too (an `F n` line, written before the buffer reaches the screen). Before this, GameCanvas games had no frame boundaries in the trace at all.

### Running it

```sh
java -Djava.awt.headless=true -Dmidprt.trace=trace.txt -cp midp-rt.jar:lib/asm-9.8.jar \
    org.microemu.app.Headless --ticks 340 --inputs ticks.json \
    [--frames-dir out/frames] [--tick-timeout-ms 30000] fixed.jar
```

* `--ticks N` runs exactly N ticks (`F 0` … `F N-1`), then writes `index.json`, prints `TICKS_DONE N` and exits with 0. The game's code does not run after the last boundary, so the trace tail and the `# histogram` are reproducible too.
* `--frames-dir` captures one PNG per tick (`frame_i` = the screen after tick i was drawn), and `index.json` gets a `"ticks": N` field. `--frames`, `--period-ms` and `--boot-wait-ms` have no effect in this mode.
* In this mode `F` lines carry no wall time: `F n` instead of `F n t=…`. The canonicaliser (`canon.sh`) reads both forms.
* `--tick-timeout-ms` is a watchdog, and the only thing in this mode that reads the wall clock. If no new tick comes within that time, the run exits with 4 and prints `TICKS_STALLED` (for example, the game stopped drawing).
* Exit codes: 0 done, 2 bad arguments or scenario, 3 capture error, 4 stalled.

### Scenario format

The existing `--inputs` JSON array, with `tick` in place of `t_ms`:

```json
[
  {"tick": 259, "key": "FIRE", "action": "press"},
  {"tick": 261, "key": "FIRE", "action": "release"},
  {"tick": 300, "key": "NUM5", "action": "tap"}
]
```

* `tick` is an integer ≥ 0. `key` and `action` are unchanged: a key name (`UP/DOWN/LEFT/RIGHT`, `NUM0`–`NUM9`, `STAR`, `POUND`, `FIRE`/`SELECT`/`SOFT3`, `SOFT_LEFT`, `SOFT_RIGHT`, `CLR`) or a raw MIDP key code. The action is `press`, `release` or `tap` (press then release on the same tick).
* Events on the same tick are delivered in file order.
* Backward compatible: a file with `t_ms` events behaves exactly as before without `--ticks`. One file cannot mix `t_ms` and `tick`. A `t_ms` file under `--ticks` is refused (exit 2), and so is a `tick` file without it, so a stale scenario never silently falls back to wall-clock delivery. An event at or beyond tick N is reported and not delivered.

### Where and how an event is delivered

At boundary `n`, all on the thread that draws the frame:

1. the screen after tick `n-1` is captured; if `n == N`, the run ends here;
2. `F n` is written to the trace;
3. the events with `tick == n` are delivered **synchronously**. `keyPressed`/`keyReleased` (and a soft-key `Command`) run right here instead of in the event queue, and GameCanvas key states are latched immediately. The trace shows `keyPressed K` straight after `F n`;
4. frame `n` is drawn.

A Canvas game's own thread is parked in `serviceRepaints()` during this, and a GameCanvas game's thread is the one calling `flushGraphics()`. Either way the game sees the event strictly between the logic of tick `n-1` and the drawing of tick `n`, whatever the OS scheduler does. A `repaint()` requested from a handler during delivery is not queued, because frame `n` is drawn right after. A queued extra paint would race the game thread and add a stray tick. On a device with key auto-repeat, key releases go through a 50 ms timer, so tick mode sends the raw key code synchronously instead.

### Verification

* `tests/tick_input.rs` compiles midp-rt from the checkout and runs the looping synthetic MIDlet (`SynthMidlet` with `LOOP = true`: a game-style `repaint` + `serviceRepaints` thread whose frames record the paint index at which the last press arrived). Two runs with the same scenario give byte-identical traces and frame indexes. The press scheduled for tick 5 is the first line after `F 5`, frames 0–4 show no press, and every frame from 5 on shows "seen on paint 5". A `t_ms` scenario under `--ticks` is refused.
* Pilot (one commercial J2ME game from the local corpus, not distributed; fixed-clock instrumented): see the measurement note at the end of this addendum.

### Honest boundary

* Determinism holds when the game's frames come from one thread, in the normal `repaint` + `serviceRepaints` or `flushGraphics` loop. A game that calls `repaint()` without `serviceRepaints()`, paints from several threads, or drives logic from `Timer`/`callSerially`, still has a scheduler-dependent number of ticks. The watchdog does not detect that. Two runs then simply differ, and a byte comparison of two runs is the check.
* What a game does before tick 0 is outside the tick clock. A loop thread that starts before its screen is actually current spins for a scheduler-dependent number of turns (`repaint` / `serviceRepaints` lines with no frame). The synthetic MIDlet therefore starts ticking only after its first frame.
* The runtime's own images (the four `ChoiceGroup` check/radio icons, created whenever that class happens to be initialised) no longer go through `Image.createImage`, so they no longer appear in the trace as `I n` lines. Before, their ids raced with the game's own images. The old `DROP_IDS` workaround in the canonicaliser is not needed for new traces.
* Pointer events and key auto-repeat are not scripted in this mode.
* `repaint(x, y, w, h)` from a handler during delivery is dropped in favour of the frame that follows. If the game paints only a sub-rectangle on that frame, the dropped region is not redrawn.

### Pilot measurement (2026-11-15)

One commercial J2ME game from the local corpus (not distributed), fixed-clock instrumented, 240×320, `--ticks 340` with four scripted events (FIRE press/release on ticks 259/261 and 300/302). Two separate JVM launches gave **byte-identical traces** (32 129 lines, 340 frame boundaries, each `keyPressed`/`keyReleased` straight after its `F n`) and byte-identical frame hashes for all 340 captured frames. The first attempt differed only in the ids of the runtime's own `ChoiceGroup` icons. That race is fixed above.
