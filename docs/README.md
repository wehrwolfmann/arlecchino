# Documentation

Research reports, measurements and decision records. The README covers *how to use* the
tool; this directory covers *why it is the way it is* — the numbers behind the claims,
the protocols that had to be reverse-engineered, and the alternatives that were examined
and rejected. None of it is derivable from reading the source.

| document | what it is |
|---|---|
| [`history-and-decisions.md`](history-and-decisions.md) | The standing invariants and the gate discipline; the permanent Java/JVM boundary and why the port did not remove it; the ten pipeline stages and the two features that were built, measured and deliberately not ported; the historical reference numbers and the conditions attached to each |
| [`headless-capture-protocol.md`](headless-capture-protocol.md) | How frames are captured with no window, X server or native code: the FreeJ2ME-plus libretro pipe protocol, the three traps in it, the measured determinism of frame *content* versus frame *timing*, and how key names differ between the two engines |
| [`behavioural-verification.md`](behavioural-verification.md) | How the difftest harness was validated: the first real original-versus-rebuild run with its noise calibration, the cross-engine comparison (including the case where the harness caught a defect in the reference emulator), and the ten-jar smoke run with the notes that prevent false alarms |
| [`determinism-fixed-clock.md`](determinism-fixed-clock.md) | The `--fixed-clock` instrumentation: what is rewritten, what is deliberately left alone, the measured results — and the honest boundary of the guarantee. Includes the StackMapTable regression that made instrumented modern class files fail to verify, and its fix |
| [`midp-rt-fork.md`](midp-rt-fork.md) | Where the vendored MIDP runtime came from: mirror selection with commit hashes, the per-file licence map that determines the whole arrangement, the module keep/drop list, and the live JDK-17 compatibility findings |
| [`runtime-research.md`](runtime-research.md) | The feasibility study that preceded the runtime work: the emulator front-end landscape, the determinism verdict, the candidate-runtime licence matrix and the licence filter rule, and the costed reuse approaches. Kept mainly for the alternatives it rejected — two of its own recommendations were later overruled by measurement, which is marked in place |
| [`debugging-research.md`](debugging-research.md) | A landscape survey of adding source-level debugging. **Nothing in it was implemented.** Kept for the published measurements it collects and for the options examined and rejected |

Related material outside this directory:

* `../tests/README.md` — what the test suite pins, and what skips where;
* `../regress/expected.txt` — the live regression gate and the exact conditions it was
  measured under;
* `../NOTICE` — the attribution and clean-room statements.
