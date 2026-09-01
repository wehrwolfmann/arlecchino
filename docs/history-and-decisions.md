# History and standing decisions

Where the current shape of the toolkit came from: the staged work plans that produced
it, the invariants those plans imposed and that still hold, the architectural
boundaries that are permanent, and the things that were built once and deliberately
not carried into the Rust edition.

Everything below is history except the two sections marked **standing** — those are
rules, not records.

---

## 1. Standing: the invariants

These were laid down before the first improvement stage and have governed every change
since:

* **A fix is accepted only if it strictly lowers the error count.** Never a fix that
  makes things worse, never a fix accepted on aesthetics.
* **Graceful degradation.** Offline, without ASM, without the compiled helper — the
  tool still runs and says what it could not do. It never hard-fails on a missing
  optional prerequisite.
* **No vendoring of third-party artifacts.** The work is clean-room and Apache-2.0;
  decompilers, ASM, the emulator and the MIDP runtime are downloaded or invoked, never
  copied into the tree. (The one exception is the MIDP runtime *source* subtree under
  `midp-rt/`, which is separately licensed, never compiled into this crate, and
  documented in `NOTICE` and `docs/midp-rt-fork.md`.)
* **Every stage carries a test that catches its own regression.** A stage without such
  a test is not finished.

## 2. Standing: the gate discipline

Before **every** merge into the pipeline, re-measure the reference numbers. Originally
that meant two things: the headline jar's isolated error count, and the ten-jar
regression corpus. **If a number moves — stop, investigate, explain.** A number that
moved for a reason nobody can state is a defect, not an improvement.

The live gate now is `regress/expected.txt` plus `regress/run.sh` (see the README).
The historical numbers the plans were written against are recorded in section 6.

## 3. Standing: the Java boundary is permanent

From the 2026-06-24 port analysis, and still true:

> The tool by its nature **orchestrates someone else's Java ecosystem** (several
> decompilers, `javac`, `javap`, `jar`, FreeJ2ME / MicroEmulator) and **manipulates
> bytecode through ASM**, a Java library. Neither reproduces cheaply in another
> language: there are no ASM equivalents (the `noak` / `cafebabe` crates are raw and
> cover *reading*, not safe rewriting with supertype resolution by reflection), and
> the decompilers and `javac` / `javap` are external processes anyway.

The consequence, stated at the time and unchanged: **the Python/Java boundary is the
Rust/Java boundary. The port moved the wrapper; it did not remove the JVM.** A JDK is
still required on the user's machine, so "single binary, zero dependencies" was never
fully achievable and was never claimed.

What stays Java, permanently:

* `assets/MemberRenamer.java` (~48 KB, the ASM bytecode member renamer) — the core
  feature;
* `assets/DeterministicClockTransformer.java` (ASM, the fixed-clock instrumentation);
* the generated clean-room MIDP/CLDC stub — in the Rust edition the signature text is a
  blob in `src/midp_stub.rs` that is written out and handed to `javac`;
* the decompilers themselves, `javac` / `javap` / `jar`, FreeJ2ME, and the MicroEmulator
  fork.

**midp-rt is not ported.** It is somebody else's upstream fork (LGPL), integrated as a
`git subtree` with its history preserved. It is too large to reimplement and it is not
this project's code. The LGPL boundary is the process boundary: the orchestrator
launches it as an external `java` process. See `docs/midp-rt-fork.md`.

### What the port was expected to buy, and what it was not

Written before the work, and worth keeping as a calibration of expectations:

> If the goal is functionality or correctness: the port gives no new functionality, and
> the regression risk is high — the subtle auto-fix and semantic-diff heuristics, the
> masked-literal scanner, the `javac` edge cases. If the goal is speed and a single
> binary with no interpreter runtime: it helps.

The named highest risk was: *"correctness of auto-fix and semantic-diff — the subtlest
part; masked-literal offsets and brace matching must match 1:1 or the merge silently
breaks."*

The validation strategy chosen in response is the one the test suite still uses:
**golden comparison against the reference implementation on the same fixtures**, with
"the error counts, the ranking and the reports match" as the definition of done. That
is what `tests/behaviour_parity.rs`, `tests/parity.rs`, `tests/golden/` and
`regress/expected.txt` are.

The port analysis' own verdict at the time — *"a full 1:1 port is inadvisable; a hybrid
is advisable"* — was overtaken by events: the phase it called optional and high-risk
(the behavioural-verification modules) was completed, and the Rust tree now carries
`semdiff.rs`, `difftest.rs`, `diffrun.rs`, `midprun.rs`, `permethod.rs`, `fixedclock.rs`
and `autofix.rs`. The *reasoning* above survives; the verdict does not.

---

## 4. The pipeline improvement stages (all completed)

Ten stages, run in three priority bands: trust first, then performance, then features.
Stage 1 went first deliberately — without a safety net there is no honest way to
"accompany each stage with a test".

| # | What | Outcome |
|---|---|---|
| 1 | Test scaffold: an on-the-fly synthetic obfuscated jar, a ten-jar regression runner, CI | Done. The synthetic jar generator is now `tests/fixtures/synthjar/` + `tests/common/`; the runner is `regress/run.sh` |
| 2 | Pin every engine to an exact release tag; honour `GITHUB_TOKEN` | Done — `src/setup_engines.rs`. Downloads ask for `releases/tags/<tag>`, never `releases/latest` |
| 3 | Semantic round-trip (`--semdiff`) | Done — `src/semdiff.rs` |
| 4 | Member renamer: remove a dead loop, precompute override-group membership | Done. **Acceptance criterion: the produced `mapping.txt` byte-for-byte identical before and after**, on the synthetic jar and on the reference game |
| 5 | In-process `javax.tools.JavaCompiler` backend | Done in the reference implementation; **not ported** — see section 5 |
| 6 | Isolated-compile cache | Done in the reference implementation; **not ported** — see section 5 |
| 7 | Mask string, char and comment literals before structural brace matching | Done — `src/autofix.rs`, pinned by `tests/autofix_mask.rs` |
| 8 | Always put the generated clean-room MIDP/CLDC stub on the renamer's resolver classpath | Done — `src/midp_stub.rs`, `rename::resolver_classpath` |
| 9 | `--flatten-packages` | Done — opt-in, default byte-identical |
| 10 | Evaluate the per-method merge; mark it experimental | Done: **off by default, labelled experimental** |

### The risk ranking, stated in advance

The two stages judged most dangerous to the headline number were 5 (it rewrites how
compilation happens) and 6 (it caches its result) — because that is the path that
*produces* the number. Both were required to be behind a toggle, off by default, until
their output was proven **identical** to plain `javac`. Stage 9 was the third: it
changes naming, so it too is opt-in, and the expectation was recorded before the work
("with the flag on, expect the residual to vanish entirely; the default does not
change"). That is exactly how all three landed.

## 5. Built, measured, and deliberately not ported

Two performance features exist in the reference implementation and have no counterpart
in the Rust edition. Neither was dropped for being wrong — both were proven not to
change any number. They are recorded here so the work is not repeated from scratch if
the cost of compilation ever becomes the bottleneck again.

### In-process javac backend

The merge pipeline scores thousands of candidate compiles, and by default each one is a
fresh `javac` subprocess. The reference implementation added an opt-in in-process
backend driving `javax.tools.JavaCompiler` with a reusable `StandardJavaFileManager`.
The design constraint that made it safe: it returned the **same `javac` diagnostic
text**, parsed by the **same regular expression**, so per-file error counts were
byte-identical to the subprocess path. It stayed **off by default** until equivalence
was confirmed across the whole CI JDK matrix.

Not ported. The Rust edition has one compile path: `core::compile_file_isolated` shells
out to `javac`.

### Isolated-compile cache

An opt-in, per-process, LRU-bounded memoisation of the isolated compile, never
persisted to disk, reset at each jar's merge. The key is what matters:

```
sha256(source_text + "\0" + api_classpath)
```

Keying on the *text* rather than the path is what makes it hit at all — the auto-fix and
per-method probes write the same source to many different temporary files. Keying on the
classpath too is what keeps it correct. The default cache bound was 20 000 entries.

It was verified to change nothing: the headline number and the full regression set were
**identical cache-on versus cache-off**.

Not ported. There is no compile cache in `src/core.rs`.

---

## 6. The runtime plan (Blocks A and B, both completed)

Two independent blocks, both shippable on their own.

**Block A — behavioural diff-testing against a reference emulator.** Headless frame
capture, input replay, RMS diffing, a noise-calibrated verdict, and finally the virtual
clock. The research conclusion that shaped the whole design:

> FreeJ2ME-plus is **non-deterministic out of the box** (`System.currentTimeMillis()`
> plus an unseeded RNG). The first version is therefore built **without** determinism:
> catch gross divergences — a crash, a black screen, a diverged record store,
> consistently different frames — and defer frame-exact accuracy.

That deferred item became the fixed-clock stage; see `docs/determinism-fixed-clock.md`.
The measurements are in `docs/behavioural-verification.md` and the capture mechanism in
`docs/headless-capture-protocol.md`.

Block A's completion note carries one caveat worth keeping: the reference game's merged
jar is only *executable* after `--flatten-packages` — the residual errors in the default
path are about compilability, not executability.

**Block B — a MIDP runtime of our own.** Decided on 2026-06-10 to be an adaptation of
the existing MicroEmulator code base rather than a bespoke shim, with obligations that
still bind: the fork is a **separate module and jar**, its sources are **published**,
its code is **never mixed** into this Apache-2.0 code base, and interaction happens only
across a process boundary. The full reasoning, the licence map that justifies it and the
JDK-17 compatibility findings are in `docs/midp-rt-fork.md`.

**The link between them** was the point of both: `difftest` can compare three pairs —
original against merged in the reference emulator; the original in the reference
emulator against the original in our runtime (which validates our runtime); and original
against merged in our runtime, deterministically.

### Block B, phase 2 — consciously deferred, never done

* JSR-135 minimum: tone generation plus wav/midi playback through `javax.sound`;
* pointer (stylus/touch) events;
* the `lcdui.game.*` package;
* an LWJGL rendering backend in place of Swing.

### Block B risks still live

* The runtime's upstream code base was end-of-life in 2010; its networking, IO and media
  layers are its weakest parts.
* API-surface creep was cut off by an up-front scan of what the corpus actually calls,
  plus a rule to implement on actual failures rather than speculatively.
* Games that spawn their own threads are non-deterministic under any JVM scheduler. For
  diff purposes, compare **checkpoint frames**, not every frame.
* Font metrics can differ between JDKs and platforms, so any golden image test needs a
  perceptual tolerance rather than an exact match.

---

## 7. Historical reference numbers

Two sets of numbers appear in this project's history and they are **not** in conflict —
they measure different configurations. Do not quote one as a correction of the other.

**The headline, June 2026.** The reference game's honest isolated error count went from
**712 to 2**, measured with a four-engine local configuration whose CFR was a
`0.153-SNAPSHOT` development build (bundled with jd-gui-duo, never released upstream).
With `--flatten-packages` the residual went to **0**. The conditions attached to that
measurement are set out in the README's example section.

**The live gate, September 2026.** `regress/expected.txt`, measured with the six-engine
`regress/engines.6.toml`, the pinned released CFR 0.152, and a recorded JDK and binary
revision — and cross-checked jar for jar against the reference implementation on the
same day, same corpus, same configuration, each edition in its own isolated home and
engine root. All eleven jars agreed on both numbers.

The ten-jar corpus the plans refer to is `Allods`, `Bruce_Lee`, `Elven_Chronicles`,
`Flitzer`, `GOTHIC-3`, `Get_Rich`, `Liftboy`, `Masha_i_Medved`, `Silent_Storm`,
`cyber_blood_rus`. The jars themselves are **never committed** — they are supplied at
run time.

### Appendix: the configuration the headline was measured with

The 712 → 2 figure was produced with a machine-local `arlecchino.local.toml` that was
never committed anywhere — it existed as a single copy on one disk. Since the engine set
and, in particular, the unreleased CFR build are part of what the number *means*, the
file is recorded here verbatim. It is not a working configuration for the Rust edition
(the live gate uses `regress/engines.6.toml`); it is the provenance of a number.

```toml
# Local test config: points at the engines already present under
# ~/Загрузки/allods_re/tools (so `decompile --config arlecchino.local.toml`
# works without running `arlecchino setup`).  Not for distribution.

engines_root = "~/Загрузки/allods_re/tools"

[[engine]]
name = "vineflower"
type = "names"
kind = "jar"
path = "vineflower.jar"
args = ["{jar}", "{out}"]

[[engine]]
name = "cfr"
type = "names"
kind = "jar"
path = "jdguiduo/lib/cfr-0.153.jar"
args = ["{jar}", "--outputdir", "{out}",
        "--renamedupmembers", "true", "--renameillegalidents", "true",
        "--silent", "true"]

[[engine]]
name = "procyon"
type = "names"
kind = "jar"
path = "procyon.jar"
args = ["-jar", "{jar}", "-o", "{out}"]

[[engine]]
name = "jadx"
type = "names"
kind = "tool"
path = "jadx/bin/jadx"
args = ["--no-res", "-d", "{out}", "{jar}"]
sources_subdir = "sources"
normalize = "jadx"
```

Note `jdguiduo/lib/cfr-0.153.jar` — the development snapshot with no upstream release.
`assets/engines.default.toml` pins the released 0.152 instead, which is one reason the
live gate reports different numbers for the same jar.

## 8. Continuous integration

The reference implementation ran a **JDK 11 / 17 / 21 matrix** with `fail-fast: false`,
on push, on pull request and on manual dispatch. Its design rationale is worth keeping
whatever the language: the job needs **no private jars and no decompiler engines** —
the synthetic obfuscated jar is generated from `javac` plus ASM, ASM is fetched from
Maven Central by `arlecchino setup --only asm`, and every engine-dependent test skips
cleanly. Because every gate number depends on `javac` behaviour, the JDK matrix is the
part that matters most.

The Rust tree currently has no CI configuration. `cargo test` plus `regress/run.sh` are
the gate.
