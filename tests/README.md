# Test suite

The suite adds **no** pipeline logic. It exercises the existing code paths and pins
their behaviour, so that a change which silently moves a number is caught before it
reaches the regression gate.

Run everything with `cargo test`. There is nothing else to install beyond what
`arlecchino setup` already puts on the machine.

## Layout

| path | what it pins | needs |
|---|---|---|
| `src/**/#[cfg(test)]` | pure module units — engine pins and the `GITHUB_TOKEN` header, i18n tables, the launcher and uninstaller plans, the per-method signature scanner, quality metrics, the MIDP stub table | nothing (a couple skip without `javac`) |
| `tests/parity.rs` | `autofix` / `normalize` / `deobf` outputs against values captured from the reference implementation | nothing |
| `tests/autofix_mask.rs` | auto-fix robustness to braces, semicolons and keywords hiding inside string literals, char literals and comments — including a copy of the old raw brace matcher, kept only to *prove* the historical misfire | nothing |
| `tests/behaviour_parity.rs` | golden comparison of `difftest` / `diffrun` / `midprun` / `semdiff` against the reference implementation on the same inputs (`tests/golden/behaviour_parity.json`) | nothing |
| `tests/unit.rs` | config loading, `Engine::argv`, the collision pre-check on hand-assembled `.class` bytes, rebuilt-jar packing, the loud rejection of an unknown input key, mapping-file parsing, `.jad`/manifest garbage handling, runtime-classpath layouts, the missing-runtime hint | nothing |
| `tests/synthjar.rs` | the bytecode member renamer against a synthetic obfuscated jar: all five injected collisions, the byte-for-byte golden mapping, determinism, and the full rename → decompile → merge pipeline at 0 isolated errors | `javac`, ASM, the compiled helper; the last test also needs the engines |
| `tests/flatten.rs` | `--flatten-packages`: the cross-package error stands without the flag and is gone with it, the manifest entry class is rewritten, the naming is deterministic | `javac`, `java`, ASM, the compiled helper |
| `tests/fixedclock.rs` | the deterministic-clock instrumentation: what it rewrites, that two JVM launches agree byte for byte, that the default path is a no-op, and the StackMapTable regression on modern class files | `javac`, `java`, `javap`, ASM, the compiled transformer |
| `tests/semdiff_javap.rs` | the semantic round-trip end to end against a live `javap`: a semantics-changing fix is flagged, a mapping-reconciled rename is not, `variant_drifts` separates drift from cosmetics | `javac`, `javap` |
| `tests/midp_stub_locking.rs` | the generated clean-room MIDP stub makes callback locking precise — a callback that is not on the offline denylist is still locked when the stub resolves it | `javac`, `java`, ASM, the compiled helper |
| `tests/midp_rt.rs` | where `midp-rt.jar` is looked for, in what order, and the CLI's exit codes on the error paths | nothing (nothing is launched) |
| `tests/difftest_live.rs` | the harness end to end on a real MIDlet: self-versus-self passes, a recoloured copy fails, a key tap produces a new screen state, a run leaves a record store | `javac`, `java`, `midp-rt.jar` |
| `tests/common/` | the fixture builders shared by the above (not a test binary) | — |
| `tests/fixtures/` | Java sources for the generated fixtures — nothing pre-built is committed | — |
| `tests/golden/` | committed expected values | — |

The ten-jar regression corpus is a separate thing and lives in `regress/`; those jars
are **never committed** and are supplied at run time.

## Graceful degradation

Rust has no `pytest.skip`, so a test whose prerequisites are missing prints
`SKIP: <reason>` to stderr and returns green. Watch for those lines when a suite looks
suspiciously fast.

* no `javac` / no `java` / no ASM jars → the generated-fixture tests skip;
* no compiled ASM helper (`arlecchino setup`) → the renamer, flatten, fixed-clock and
  stub-locking assertions skip;
* no `javap` → the semantic-diff tests skip;
* no decompiler engines → the full-pipeline test skips;
* no `midp-rt.jar` → the live behavioural tests skip.

A bare machine with only `javac` plus ASM therefore still runs the unit tests, the
parity goldens and the whole renamer safety net.

## The synthetic obfuscated jar

`tests/common/build_synth_jar` compiles five small classes with `javac`, then
post-processes them with ASM (`tests/fixtures/synthjar/SynthObfuscator.java`, built
against the same ASM jars the real renamer uses) to inject the source-illegal member
collisions that ProGuard-style obfuscation produces. Nothing is vendored: the jar is
generated on every run.

| case | injected | expected renamer behaviour |
|---|---|---|
| (a) duplicate field, different descriptor | `Engine.a:I` next to `Engine.a:String` | the `int a` is renamed; the `String a` keeps its name |
| (b) return-type-only overload | `Engine.flag(I)I` next to `flag(I)Z` | the pair is disambiguated |
| (c) type-shadow | field `Engine.State:I` shadowing the in-jar type `State` | renamed, tagged `(type-shadow)` |
| (d) const-interface shadow | the constant `Consts.b` shadowed by the instance field `Holder.b` | the constant is renamed, tagged `(const-iface-shadow)` |
| (e) MIDP callback override plus twin | `Screen` re-parented to `Canvas`; `keyPressed(I)I` next to `keyPressed(I)V` | `keyPressed(I)V` stays **locked**; the twin `(I)I` is renamed |

`tests/golden/synth_mapping.txt` is the exact `mapping.txt` those five cases must
produce, compared byte for byte. Any change to how override-group membership is
enumerated, or to the lock predicate, or to the new-name scheme, shows up there
immediately.

The full-pipeline test additionally runs the real rename → decompile (every configured
engine) → `merge_isolated` and asserts **0** isolated errors. A minimal clean-room MIDP
`Canvas` stub is generated on the fly and put on the classpath so the re-parented
`Screen` resolves.

## A note on what these tests actually watch

The ASM helpers (`assets/MemberRenamer.java`,
`assets/DeterministicClockTransformer.java`) are compiled once by `arlecchino setup`
into `~/.arlecchino/engines/asm/_classes`, and the tests exercise **that installed
build**, not the sources in `assets/`. Editing an asset without re-running `setup`
therefore changes nothing the suite can see. To test a change to a helper, point `HOME`
at a scratch directory holding a copy of the ASM jars and compile the helper into
`<HOME>/.arlecchino/engines/asm/_classes` yourself.

Every test file here has been checked against a deliberately broken build: each one goes
red when the behaviour it claims to pin is removed. A test that cannot fail is worse
than no test, so keep that property when adding to the suite.
