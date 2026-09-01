# Runtime research: headless emulation and MIDP shim feasibility

This is the feasibility study that preceded the runtime work, carried out on **2026-06-10**
on Arch/CachyOS with JDK 17 and the existing AWT `freej2me.jar`. It covers two blocks: Block A,
differential behavioural testing against a reference emulator (FreeJ2ME-plus), and Block B, a
MIDP → desktop runtime that runs MIDlets as ordinary desktop Java applications. Its lasting
value is not its recommendations — several of those were later overruled by measurement, and
each such place is marked below — but the **alternatives it rejected and the licence matrix it
established**, which still govern what may and may not be linked into this project.

---

## Block A — headless FreeJ2ME-plus and differential testing

### 1.1 Headless emulation capabilities

**The FreeJ2ME-plus frontend landscape**, as it stood in June 2026:

* **AWT (`freej2me.jar`)** — a standalone GUI; it can take screenshots with `Ctrl+C` and scale
  with `+`/`-`. Headless operation through `java.awt.headless=true` is **theoretically possible
  but untested and undocumented** in FreeJ2ME-plus. No evidence of AWT headless rendering in
  practice.
* **Libretro (`freej2me_libretro.so` / `.dll`)** — needs a libretro frontend (RetroArch,
  RetroEmu) to act as the window manager. Libretro cores abstract the display into a framebuffer
  callback (`retro_video_refresh_t`), which makes headless operation **plausible but not
  zero-effort**: it requires writing a minimal libretro frontend (or extending RetroArch) to
  dump frames to disk without a window.
* **SDL2 (`freej2me-sdl.jar`)** — built for libtas and joystick use. SDL2 can render offscreen
  via `SDL_CreateWindowWithProperties` (hidden flag) plus `SDL_GetWindowSurface`, then dump to
  PNG or raw. The **most practical headless path**, but it requires compiling SDL2 support
  (libSDL2 2.24.0+) and binding it from Java.

**Assessment as written:** libretro with a custom frontend, or SDL2 with headless surfacing, are
viable; AWT headless is a dead end (the GUI toolkit is bound to X11). **Effort: medium** — 2–3
weeks to build a minimal libretro/SDL2 harness. The study's final recommendation for Block A was
**SDL2 headless plus frame diffing, with no determinism** (see §"Final recommendations" below).

> ⚠️ **Editorial note — this recommendation was overruled by measurement.** The SDL2 path was
> not taken. The implementation drives the **libretro front-end's stdin/stdout pipe protocol
> directly**: frames and events cross a pipe between the Rust side and the emulator front-end.
> There is **no SDL, no X server, no Xvfb and no native code** anywhere in the capture path — the
> "must write a minimal libretro frontend in C or Rust, ~200–300 LOC" premise of §1.1 and of the
> Block A MVP below turned out to be unnecessary, because the existing front-end could simply be
> spoken to over pipes. The costing in this section (2–3 weeks for an SDL2 or libretro harness)
> should be read as historical. The protocol that was actually implemented is documented in
> `docs/headless-capture-protocol.md`; the driver lives in `src/diffrun.rs`.

### 1.2 Determinism

**System clock and RNG seeding.**

* FreeJ2ME-plus **provides no built-in determinism control**. Like stock J2ME it exposes
  `System.currentTimeMillis()` to applications, which reads the real system clock — every run
  gets a different timestamp.
* Java's `java.util.Random` seeds from `System.currentTimeMillis()` by default, so RNG is
  non-deterministic across runs.
* The FreeJ2ME-plus source (the TASEmulators fork on GitHub) has **no documented clock
  injection, no RNG seeding override and no time-freezing mechanism**.

**Savestate behaviour.**

* Libretro cores support savestates (serialised emulator state), but **determinism across
  savestate loads depends on whether the core resets all timers and RNG to a canonical seed** —
  FreeJ2ME-plus does not. Restoring a savestate resumes from exactly that moment; replaying from
  it requires re-injecting **the same input sequence *and* the same clock deltas**.
* **Bounded determinism** (the RetroArch checkpointing idea): if a run diverges because of timer
  jitter, that can be logged and checked. Not practical for J2ME with no clock anchor.

**Workarounds — all invasive:**

1. **Patch FreeJ2ME-plus** to accept a `FIXED_CLOCK_MS` environment variable or system property
   and feed virtual time to `System.currentTimeMillis()`.
   *Trade-off:* requires modifying and maintaining a fork of the emulator, and fixes nothing for
   any other engine.
2. **Bytecode rewriting on the MIDlet**: inject a shim that intercepts `System.currentTimeMillis()`
   calls at load time (an ASM bytecode transform).
   *Trade-off:* viable but brittle — it only works on applications that call
   `System.currentTimeMillis()` without reflection.
3. **Libretro core serialisation plus checkpoints**: periodically dump the full emulator state
   and the elapsed frame count, compare a per-checkpoint hash, and flag divergence as
   non-determinism.
   *Trade-off:* feasible, but requires deep libretro integration.

**Verdict:** FreeJ2ME-plus is **not deterministic out of the box**. Differential testing is
possible (frame-by-frame hash comparison), but **reproducible replay requires either patching
the emulator or rewriting the application's bytecode**. Building a deterministic harness is a
**separate ~3-week effort** on top of headless rendering.

> **What actually happened.** Route **(2), bytecode rewriting, is the one that was chosen** —
> deliberately, precisely because it does not fork the emulator and therefore works against any
> engine the project drives. It rewrites `System.currentTimeMillis()` and `nanoTime()` to a
> virtual counter, `new Random()` / `new Random(seed)` to a fixed seed, and `Math.random()` to a
> seeded source, reusing the project's existing ASM infrastructure. It is documented in
> `docs/determinism-fixed-clock.md`; the code is `assets/DeterministicClockTransformer.java`
> orchestrated from `src/fixedclock.rs`, with the shared compile helper in `src/rename.rs`.
> The "brittle, reflection-free calls only" caveat above still stands and is the honest limit of
> the approach.

### 1.3 Input replay and frame capture

**Libretro input recording (`.bsv`).** RetroArch records button presses in a binary format with
optional savestate checkpoints (`retro_serialize` snapshots at configurable intervals). Libretro
cores expose a frame-per-call interface: each `retro_run()` advances one frame, and input is
polled through the `retro_input_poll_t` callback. **Practical for FreeJ2ME-plus:** a custom
libretro frontend can record inputs to `.bsv` and then replay them deterministically — but
**only if the core honours virtual clock injection**, which brings the question back to
§1.2: it does not.

**Frame capture.**

* Libretro: the `retro_video_refresh_t` callback receives a framebuffer pointer, which can be
  written straight to PNG or raw. **Works without modification**, framerate agnostic.
* SDL2: `SDL_GetWindowSurface`, or `SDL_CreateTexture` plus `SDL_RenderReadPixels`, gives a raw
  RGBA buffer to write out as PNG through an external library. **Straightforward.**
* AWT: a screenshot to file via `Robot.createScreenCapture()` — a JVM feature, independent of
  FreeJ2ME-plus. It works but requires X11 (no Xvfb needed if libretro or SDL2 is used instead).

**Assessment:** frame capture is **trivial**; input record/replay is **straightforward** in
libretro; **deterministic replay is blocked without clock injection**.

### 1.4 RMS (RecordStore) extraction

* FreeJ2ME-plus stores RMS in a configurable directory — by default `~/.freej2me/rms/`, or a
  per-platform variant such as `/emuelec/configs/freej2me/rms/`.
* Each game gets a subdirectory; stores are named with prefixes such as `_c`, `_k`, `_o` (the
  store ID encoded as a single character).
* **Diffing:** copy the RMS directory before and after the run and compare bytewise, or with
  `diff`, to detect changes in game state.

**Practical for testing:** before/after RMS snapshots give ground truth for save-state
validation, and the whole cycle automates — capture the initial RMS, run with input replay,
capture the final RMS, compare.

### Block A summary and MVP

| Aspect                  | Status        | Effort | Notes                                            |
|-------------------------|---------------|--------|--------------------------------------------------|
| **Headless rendering**  | ✅ Viable     | 2–3w   | Libretro + custom frontend, or SDL2 offscreen    |
| **Frame capture**       | ✅ Trivial    | <1w    | Libretro callback or SDL2 readback               |
| **Input replay**        | ✅ Viable     | 1–2w   | `.bsv` format + libretro framework               |
| **Determinism**         | ⚠️ Not OOB    | 3–4w   | Requires emulator patching or bytecode rewriting |
| **RMS diffing**         | ✅ Easy       | <1w    | Filesystem snapshots + byte comparison           |

**MVP for Block A (without determinism), as proposed:**

1. Build a **minimal libretro frontend** (C or Rust, ~200–300 LOC), or use **SDL2 offscreen**
   mode.
2. Implement frame dumping to `.rgba` or `.png`.
3. Record input sequences and frame numbers in CSV or JSON.
4. Run the application twice with the same inputs, capture frames, hash-compare the outputs.
5. **Limitations:** this only detects gross frame mismatches; frame-perfect regression testing
   requires determinism, which is out of scope for the MVP.

**If determinism is essential:** fork FreeJ2ME-plus and add an `ARLECCHINO_FIXED_CLOCK`
environment variable to inject virtual time. Effort: +2 weeks.

> **What actually happened.** Steps 1 and 2 were superseded — no custom frontend and no SDL2;
> the existing libretro front-end is driven over its stdin/stdout pipe protocol
> (`docs/headless-capture-protocol.md`). Step 3 landed as a JSON input script. The
> `ARLECCHINO_FIXED_CLOCK` name survived, but as the environment variable of the *bytecode
> rewriting* route, not of an emulator fork.

---

## Block B — MIDP → desktop shim (a CLDC/MIDP runtime on the JDK)

### 2.1 Candidate runtimes and licence analysis

| Runtime                      | License                             | Type              | Status         | Linkable Library | Notes                                                                        |
|------------------------------|-------------------------------------|-------------------|----------------|------------------|------------------------------------------------------------------------------|
| **MicroEmulator-minimal**    | LGPL 2.1 / Apache 2.0 (dual)        | Emulator          | Active (GH)    | ⚠️ Partial       | Pure Java CLDC/MIDP 2.0; Swing GUI included; no modular export               |
| **MicroEmulator (orig)**     | LGPL 2.1+                           | Emulator          | ⚠️ EOL (2010)  | ✅ Yes           | Classic; Swing desktop support; last release 2.0.4; source available          |
| **FreeJ2ME-plus**            | **Proprietary / mixed**             | Emulator          | Active (GH)    | ❌ No            | Designed as an emulator, not a runtime library                                |
| **J2ME-Loader**              | Apache 2.0                          | Android emulator  | Active         | ❌ No            | Android-specific; converts MIDP → Android via the Mascot Capsule framework    |
| **phoneME Feature**          | **GPLv2**                           | CLDC/MIDP stack   | ⚠️ Archived    | ⚠️ Conditional   | Sun's official runtime; GPL (not LGPL) = **problematic for commercial linking** |
| **J2ME Polish**              | **GPLv2 + commercial**              | Build framework   | Active         | ❌ No            | An SDK for app development, not a runtime; GPL = problematic                  |
| **KEmulator**                | MPL 2.0 (nnmod)                     | Emulator          | Stale          | ❌ No            | Decompiled original; monolithic emulator                                      |

**The licence filter (permissive-only path)** — the rule this matrix encodes, and the rule the
project still follows:

* ✅ **LGPL 2.1+** — linkable, provided attribution is given and the source and licence are
  included. In this project it is taken one step further: LGPL code is not linked at all, only
  run as a separate process and shipped as a separate artefact.
* ✅ **Apache 2.0** — fully permissive, attribution required.
* ❌ **GPLv2 / GPLv2+** — makes Arlecchino GPL-viral (it would infect the whole shim). Usable
  **only as a separate process**, never as a linked library. This is why phoneME Feature — the
  official Sun CLDC/MIDP reference implementation, and technically the most authoritative
  candidate on the list — is rejected as code and kept only as a specification reference.

**Remaining candidates for a linkable library:**

1. **MicroEmulator (classic)** — LGPL 2.1, Swing-based, mature, last updated 2010.
2. **MicroEmulator-minimal** — LGPL 2.1 / Apache 2.0, cleaner code base, maintained on GitHub.

> **What actually happened.** The canonical **barteo/microemu** tree was taken as the fork base
> rather than either of the two shortlisted forks — it is the most complete, and the "minimal"
> forks turned out to be trimmed rather than cleaner. The full comparison, the licence map read
> from the actual file headers (which contradicts the repository's own `COPYING` in detail), and
> the adoption record are in `docs/midp-rt-fork.md`.

### 2.2 MicroEmulator architecture and reusability

**Core modules:**

* `microemu-cldc` — the CLDC 1.0/1.1 runtime (`java.lang`, `java.io`, `java.util`).
* `microemu-midp` — the MIDP 2.0 implementation (`javax.microedition.lcdui`,
  `javax.microedition.midp`).
* `microemu-javase` — the Swing backend for desktop rendering.
* `microemu-*.jar` for the optional JSRs (FileConnection, Media, and so on).

**Entry point:** create a `Display`, call `setCurrent(Displayable)` to switch screens, implement
`Canvas` for drawing. The MIDlet lifecycle is managed by a `MIDletRunner` or similar harness.

**Limitations noted at the time:**

* Monolithic — all JSR modules compile together, and stripping down to a minimal
  Canvas/Display/RecordStore set is hard.
* The Swing binding is baked into the Swing module; decoupling it is possible but non-trivial.
* **It is not designed as a headless or embeddable library**; the GUI is integral.

> **What actually happened.** The third limitation proved to be wrong in the way that mattered
> most. MicroEmulator already ships `org.microemu.app.Headless` and a `NoUiDisplayComponent`,
> and on 2026-06-10 a test MIDlet was rendered on JDK 17 with `java.awt.headless=true` and no
> `DISPLAY` at all. Rendering goes to a `BufferedImage`; Swing is only the interactive host.
> Stripping the tree down was also less painful than feared: the modules were imported
> selectively (see `docs/midp-rt-fork.md` §5).

**Three reuse approaches, as costed:**

1. **Approach A (full MicroEmulator)** — bundle `microemu-cldc.jar` and `microemu-midp.jar` as a
   Maven/Gradle dependency, and supply a Swing or LWJGL canvas wrapper.
   **Effort: low (~3–5 days). Limitation:** forces a Swing / heavyweight GUI.
2. **Approach B (minimal extract)** — take the MicroEmulator source and extract only `Canvas`,
   `Display`, `Graphics`, `RecordStore` and `MIDlet`; strip the JSR dependencies; rebuild as a
   lean `arlecchino-midp-runtime.jar`.
   **Effort: medium (~2–3 weeks). Benefit:** full control, no Swing overhead.
3. **Approach C (reference only)** — use MicroEmulator as a **specification reference**, not as
   code, and implement Canvas/Display from scratch, linking natively.
   **Effort: high (~6–8 weeks). Benefit:** no licensing entanglement, and an optimised result.

**Chosen: Approach A**, in the form the study recommended — reuse the MicroEmulator code base
rather than re-implement it — and this is what decision B0 (2026-06-10) ratified.

> **What actually happened.** The realised shape is A with a slice of B: not a Maven dependency
> on the 2.0.4 artefacts, but a selectively imported, modernised fork built by a plain `javac`
> script (the 2010 POMs pull dead repositories), shipped as its own `midp-rt.jar` and invoked as
> a separate process. Approach C was never revisited; the licensing entanglement it was meant to
> avoid is handled by the process boundary instead, at a fraction of the cost.

### 2.3 A per-game minimal shim (the path not taken)

The study also sketched a from-scratch shim, sized against a bytecode analysis phase: scan a
given MIDlet jar with ASM or `javap` to find which `javax.microedition.*` classes are actually
imported, which methods are called on `Display`, `Canvas` and `Graphics`, whether it uses
`RecordStore`, `Image` or `Font`, and how it uses threads (`Thread.sleep()`, `Runnable`). A
typical Canvas game turned out to need very little:

```java
// Minimal Canvas game
extends Canvas implements Runnable
  - paint(Graphics g)
  - keyPressed(int keyCode)
  - run() { while (running) { repaint(); Thread.sleep(33); } }
```

The sketched shim was a `Canvas`/`Display`/`Graphics`/`RecordStore` skeleton — some 20–30 core
drawing methods on `Graphics`, a `Display.getDisplay()`/`setCurrent()` pair, and a file-backed
`RecordStore` under `~/.arlecchino/rms/` — hosted in a Swing frame, with a `KeyListener`
translating AWT key codes to MIDP key codes (a mapping table of ~30 LOC) and frame callbacks
from a Swing `Timer` or a `ScheduledExecutorService`. **Realistic MVP: 2–3 weeks**, expected to
work for 95% of 2D games; determinism remained a separate concern.

> **What actually happened.** Decision B0 rejected this route on 2026-06-10 in favour of
> adapting MicroEmulator — the same 2–3 weeks buys a complete, battle-tested MIDP surface
> instead of a partial one. The bytecode-analysis idea survived as the B1 API-surface scan, used
> as a completeness checklist for the fork, and the ABI stub `src/midp_stub.rs` keeps the
> signature list for the renamer and for compilation.

### 2.4 Existing implementations as reference

* **J2ME-Loader (Android)** — converts MIDP bytecode through the **Mascot Capsule** library
  (proprietary, not open source) to Android APIs. **Not reusable as a reference** for a desktop
  shim; the architecture is Android-specific. The lesson it carries is that MIDP → native
  conversion is viable but heavy.
* **MicroEmulator (2010)** — the gold standard for MIDP on the desktop, full source available
  (GitHub, SourceForge archives). **Reuse path: Approach A (bundle the jar) or Approach B
  (extract the key classes).**
* **phoneME Feature (archived Sun runtime)** — the official CLDC/MIDP reference implementation,
  archived on GitHub (magicus/phoneME). **Not reusable** directly because of GPLv2, which would
  restrict Arlecchino to a separate-process arrangement at best. **Reusable as specification:**
  the MIDP 2.0 spec is public and can be implemented from independently.

### 2.5 RecordStore implementation (file-backed)

The on-disk format designed for the shim:

```
~/.arlecchino/rms/
  {appName}/
    {storeName}.rms       # Binary: [record_id:4 | len:4 | data:len | ...]
    {storeName}.idx       # Index:  [record_id:4 | offset:8 | len:4 | ...]
```

The minimal implementation is an in-memory `Map<Integer, byte[]>` cache behind
`openRecordStore(name, create)` / `getRecord(recordId)` / `setRecord(recordId, data, offset,
len)`, with the store directory created under `System.getProperty("user.home")` +
`.arlecchino/rms/` + the current application name, and a `flush()` serialising the record map
back to the `.rms` file on every write.

**Effort:** under one week for a working prototype.

> **What actually happened.** The runtime uses MicroEmulator's own `FileRecordStoreManager`
> rather than this format, extended by the fork so that a directory path given to `--rms` holds
> the store files *flat* — with no `suite-<name>` level — so that stores stay comparable between
> an original and a rebuilt jar even when the MIDlet class names differ. That flatness is what
> makes the Block A RMS diff work.

### Rendering backend comparison

| # | Backend                | Assessment                                                                    |
|---|------------------------|-------------------------------------------------------------------------------|
| 1 | **Swing Canvas**       | Easy, works on every JDK, slow. `JPanel` + `paintComponent(Graphics2D)`. ~50 LOC wrapper. |
| 2 | **LWJGL**              | Faster, OpenGL, adds an LWJGL dependency. ~150 LOC wrapper.                    |
| 3 | **Native (C via JNI)** | Fastest, but a cross-platform compilation burden. Not recommended for the MVP. |

Swing/AWT via `Graphics2D` was judged adequate for 2D games and chosen for the MVP.

> **What actually happened.** Java2D remained the renderer, and the "slow" verdict never became a
> problem for the corpus in scope. The important detail the table misses is that the Java2D path
> renders into a `BufferedImage` and therefore needs no window at all, which is what makes
> headless frame capture possible. LWJGL stayed deferred.

### Block B summary and MVP path

| Feature                      | Approach                                   | License   | Effort | Notes                          |
|------------------------------|--------------------------------------------|-----------|--------|--------------------------------|
| **Canvas/Display/Graphics**  | A: Bundle MicroEmulator MIDP + Swing wrapper | LGPL 2.1 | 3–5d   | Low risk, well-tested          |
| **RecordStore**              | File-backed (custom)                        | MIT       | 3–5d   | Simple file I/O                |
| **Input handling**           | Swing KeyListener → MIDP keycodes           | -         | 2d     | ~30 LOC mapping                |
| **Rendering**                | Swing/AWT (Graphics2D)                      | -         | 3d     | Adequate for 2D games          |
| **Determinism**              | Out of scope for MVP                        | -         | -      | Not required for basic functionality |

**Recommended MVP: Approach A + Swing (2–3 weeks)**

1. **Week 1:** clone the MicroEmulator source, extract the CLDC + MIDP jars, verify that they
   compile on JDK 17.
2. **Week 2:** build the wrapper — a `MidletRunner` loading the `.jar` through a
   `URLClassLoader`, instantiating the MIDlet and calling its lifecycle methods; a `SwingCanvas`
   panel painting the MIDP `Canvas`; key-code mapping and the RecordStore file backend.
3. **Test:** run 5–10 simple MIDP games (Snake, Tetris clones) on the desktop. ✅ Success = the
   games run.

**Limitations of the MVP, stated up front:**

* Swing is slower than LWJGL or a native backend.
* Multi-threading (if the application uses `new Thread()`) relies on JVM thread scheduling, which
  is not deterministic.
* No audio / JSR-135 support initially — no MediaPlayer.
* Touch input (stylus) is mapped to the mouse only.

**Path to completeness, if needed:** add an LWJGL rendering backend (~2w more); implement audio
natively (~2w); full JSR support (FileConnection, Messaging, and so on) — not realistic within
Arlecchino's scope.

---

## Final recommendations (as written on 2026-06-10)

### Block A

**Recommended path: SDL2 headless + frame diffing, no determinism.** Extend the FreeJ2ME-plus
SDL2 frontend for offscreen rendering (compile SDL2 with `SDL_HINT_VIDEO_DRIVER=dummy`, dump
frames to `.rgba` or `.png` via `SDL_RenderReadPixels`, record inputs in a simple CSV of
`frame_no,key_code,is_press`), then run a differential harness that runs the original twice with
the same inputs and hash-compares the frames — accepting that the two runs will not match
exactly because of non-determinism, while still catching visual artefacts and crashes through
frame-hash changes. **Time investment: ~3 weeks** for the harness plus the test framework.
**Determinism, if required as Phase 2:** fork FreeJ2ME-plus, add clock injection and RNG
seeding — an additional 2–3 weeks.

> ⚠️ **Editorial note.** Both halves of this were overruled by measurement. The capture path
> uses the libretro front-end's stdin/stdout pipe protocol, with no SDL and no X server
> (`docs/headless-capture-protocol.md`), and determinism was solved by bytecode rewriting rather
> than by forking the emulator (`docs/determinism-fixed-clock.md`). The one idea from this
> paragraph that survived intact and proved essential is **noise calibration**: run the original
> twice with the same script first, so that the baseline non-determinism of *that particular jar*
> is known before the rebuilt jar is judged against it. The harness lives in `src/difftest.rs`
> and `src/diffrun.rs`.

### Block B

**Recommended path: MicroEmulator bundle + Swing wrapper (LGPL-compliant)** — take
`microemu-cldc` and `microemu-midp` (the study suggested version 2.0.4 as a Maven dependency),
wrap them in a desktop runner that loads the MIDlet, hosts it in a frame and forwards input, and
back RecordStore with files under `~/.arlecchino/rms/{appname}/{storename}.dat`. **Time
investment: ~2–3 weeks** for a working MVP tested on 10 simple games.

**Licensing compliance, as specified:** attribute MicroEmulator in the LICENSE file and in the
UI splash; include the full LGPL 2.1 and Apache 2.0 licence texts; no GPL entanglement, since
MicroEmulator is LGPL and therefore safe to link.

> **What actually happened.** The compliance arrangement went further than "safe to link":
> the fork is never linked at all. It is a separate artefact (`midp-rt.jar`) run as a separate
> operating-system process, with its sources published and a NOTICE recording origin and
> changes, and not one line of it inside the Apache-2.0 code. The version taken was not 2.0.4
> but the canonical tree at commit `e47b656d983deaa67810c776662444483bdbf134`
> ("3.0.0-SNAPSHOT"). See `docs/midp-rt-fork.md`.

---

## Risk summary

| Block | Risk                       | Mitigation                                                        |
|-------|----------------------------|-------------------------------------------------------------------|
| **A: Non-determinism**  | Frame-for-frame replay impossible without patching | Accept for the MVP; plan determinism as Phase 2. |
| **A: Libretro complexity** | A custom libretro frontend is non-trivial       | Use the SDL2 path instead (lower learning curve). |
| **B: MicroEmulator EOL** | Last release 2010, may have JDK 17 issues        | Test early; the GitHub forks (microemu-minimal) are newer. |
| **B: Swing overhead**   | Performance cost for animated games              | Acceptable for the initial MVP; an LWJGL backend can follow. |
| **B: LGPL attribution** | Compliance burden                                | Document in LICENSE and UI; no code-level complexity. |

> **What actually happened.** Risk A-1 was closed by bytecode rewriting rather than deferred;
> risk A-2 dissolved once the existing front-end's pipe protocol proved drivable, so no custom
> frontend and no SDL2 were needed; risk B-1 was measured and largely retired on 2026-06-10
> (compiles clean on JDK 17, renders headless), leaving one real defect — ASM 3.1 crashing on
> class-file major 52 — which was fixed by upgrading to ASM 9.8. Risk B-3 turned out to be more
> than a documentation task: LGPL compliance is what dictates the separate-artefact,
> separate-process architecture.

---

## References

* **FreeJ2ME-Plus:** https://github.com/TASEmulators/freej2me-plus
* **MicroEmulator (classic):** https://github.com/barteo/microemu (archived); forks
  ichisadashioko/microemulator, lolo-san/microemu-minimal
* **MicroEmulator LGPL licence:** https://github.com/ichisadashioko/microemulator/blob/master/LICENSE.LGPL
* **J2ME-Loader (reference):** https://github.com/nikita36078/J2ME-Loader (Apache 2.0)
* **phoneME archive:** https://github.com/magicus/phoneME (GPLv2 — do not link)
* **Libretro specification:** https://github.com/libretro/libretro.h
* **MIDP 2.0 JSR-118 spec:** https://docs.oracle.com/javame/config/cldc/ref-impl/midp2.0/jsr118/

---

## Conclusion (as written on 2026-06-10)

**Block A** is feasible in 3–4 weeks with frame-based diffing, but lacks determinism without
modifying the emulator; the SDL2 headless path is practical.

**Block B** is feasible in 2–3 weeks by bundling MicroEmulator with a Swing wrapper —
LGPL-compliant, mature code, adequate for most 2D games.

**Both blocks are independently shippable MVPs.** Determinism (A) and LWJGL rendering (B) are
enhancement candidates for Phase 2.

> **Epilogue.** Both blocks shipped. The feasibility verdicts held; two of the three routes
> recommended to get there did not. Block A capture is a pipe protocol against the libretro
> front-end rather than an SDL2 harness; Block A determinism arrived in Phase 1 rather than
> Phase 2, by bytecode rewriting rather than an emulator fork; Block B became an adapted,
> published LGPL fork run as a separate process rather than a bundled dependency. LWJGL is still
> deferred. The parts of this study that aged best are the ones that are facts rather than
> plans: the licence matrix, the GPLv2 exclusion, the RMS storage layout, and the honest verdict
> that nothing here is deterministic unless something is rewritten.
