# Debugging Integration — Landscape Survey

**Status: none of this was implemented.** No debugger, no instrumentation agent, no
debug-info generator and no in-game console exist in Arlecchino. This document is a
landscape survey of what already exists outside the project, kept for its measurements,
its effort estimates and — above all — for the options that were examined and rejected,
so that they are not researched a second time. The research was carried out on
**2026-06-09**, against the earlier Python implementation of the toolkit; all effort
estimates below therefore describe the amount of work as it was sized at that time and
are reproduced as language-neutral line counts.

**Scope**: approaches to integrating debugging capabilities into Arlecchino
(multi-decompiler J2ME → Java reconstruction toolkit).

---

## Executive Summary

The survey identified **9 adoption-ready ideas** spanning 3 categories:

1. **Production-ready tools** (decomp2dbg, Dogbolt, meta-decompilation)
2. **Active research with prototypes** (DIRTY, VarBERT, DIRE)
3. **Infrastructure patterns** (JDWP, Frida, record-replay, Javassist instrumentation)

Eight of the nine are recorded below. One surveyed option was dropped as out of scope for
this project and is not described here; the remaining prototypes are renumbered so the
ranking stays coherent.

**Confidence calibration** (the convention used throughout this document):

- Production tools: **~95%** (shipped, documented, tested in the wild)
- Research prototypes: **~70%** (papers + GitHub code exist; not always plug-and-play)
- Emerging ideas (e.g. time-travel debugging): **~50%** (proof-of-concept phase, not
  battle-tested for J2ME)

Per-item confidence figures are given inline for each approach that carries one, and are
collected again in the [confidence table](#confidence-levels).

---

## 1. Decompiler ↔ Debugger Integration

### 1.1 decomp2dbg — production bridge

**What**: plugin that syncs decompiler-derived symbols (functions, variables, types) into
GDB/LLDB as source-level symbols.
**Maturity**: production (2023–present, PyPI package, active issues).

**Specifics**:

- **Architecture**: bidirectional via XMLRPC — the decompiler backend (IDA, Ghidra,
  Binary Ninja, angr) runs a server; the GDB/LLDB client polls or receives live updates.
- **Symbol mapping**: functions and globals → GDB native symbols; local variables →
  convenience variables (`$v4`); stack/register → dereference.
- **On breakpoint**: prints the decompiled line and auto-imports the variables reachable
  at that address.

**Adoptability for Arlecchino**:

- Strengths:
  - Already integrates Ghidra (which can consume Java bytecode via CFR/Vineflower).
  - Multi-decompiler backend architecture — fits the multi-decompiler merge philosophy.
  - XMLRPC is language-agnostic, so the client side can be implemented in any language.
- Weakness: **no Java-specific support**. decomp2dbg targets compiled binaries
  (C/C++/Rust); it does not know about JDWP or JVM stack frames. **A custom backend would
  be required** to map Arlecchino's reconstructed Java source → debuggable bytecode.

**Adoption path**:

1. Write a decomp2dbg backend plugin that consumes Arlecchino's merged source and type
   information.
2. Generate a synthetic JDWP symbol table (see §1.3) and expose it to jdb/IDEs via JPDA.
3. Effort: ~200–300 lines of orchestration code plus JDWP marshalling.

**Source**: [GitHub](https://github.com/mahaloz/decomp2dbg),
[PyPI](https://pypi.org/project/decomp2dbg/)

---

### 1.2 Ghidra Debugger — native integration

**What**: Ghidra's built-in reverse debugger (available since v10.0) that steps decompiled
pseudo-code in sync with GDB.
**Maturity**: production (part of official Ghidra, documented).

**Specifics**:

- On each GDB breakpoint, Ghidra auto-updates the decompiler view, highlighting the active
  line.
- Hovering over a variable makes Ghidra attempt to unwind the stack and show the live
  value.
- Supports GDB and LLDB via wrapper scripts.

**Adoptability for Arlecchino**:

- Strength: works out of the box if Arlecchino outputs to the Ghidra project format.
- **Weakness (rejection reason)**: Ghidra's decompiler (Sleigh) is closed-source binary
  analysis and is not tuned for J2ME bytecode. It would have to be fed pre-decompiled Java
  source rather than the original `.jar`.

**Verdict**: **low priority / skip** — requires Ghidra infrastructure setup, and
decomp2dbg is the more portable route to the same result.

**Source**:
[clearbluejar blog](https://clearbluejar.github.io/posts/decompilation-debugging-pretending-all-binaries-come-with-source-code/),
[Ghidra docs](https://pwndbg.re/2025.05.30/tutorials/decompiler-integration/ghidra-integration/)

---

### 1.3 JDWP + JDI integration — the Java-native path

**What**: Java Debug Wire Protocol (JDWP) + Java Debug Interface (JDI) — the standard Java
debugging API.
**Maturity**: production (JDK core, 20+ years stable).

**Specifics**:

- **JDWP**: a low-level wire protocol — command/reply packets over TCP.
- **JDI**: a high-level API that abstracts JDWP — set breakpoints, read/write variables,
  invoke methods.
- jdb (command line) and every IDE (IntelliJ, Eclipse, VS Code) use JDI to debug live
  JVMs.
- **Security posture**: no authentication whatsoever; security is provided externally, by
  a firewall or an SSH tunnel.
- Attaches to a running JVM without recompilation via
  `-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=5005`.

**Adoptability for Arlecchino**:

- Strengths:
  - **Native Java approach** — Arlecchino outputs Java source, and JDWP is made for Java.
  - Ubiquitous: every IDE and debugger supports it.
- **The gap**: JDWP debugs *running bytecode*, not source code. This is the specific
  reason it is not directly usable here. Arlecchino would have to:
  - compile reconstructed Java → `.class`;
  - map reconstructed source lines → bytecode offsets (via synthetic DWARF or class file
    attributes);
  - integrate source-line mapping into the IDE if acting as an IDE plugin (IntelliJ has an
    API; Eclipse and VS Code are less documented).

**Adoption path** (highest priority of the survey):

1. **Phase 1**: compile Arlecchino-merged source → bytecode using `javac`.
2. **Phase 2**: generate `.source` file attributes in the `.class` files that map to the
   reconstructed source (not to the original J2ME sources).
3. **Phase 3**: launch the JVM with JDWP, attach jdb or an IDE, step and inspect
   variables.
4. **Phase 4** (optional): an IDE plugin that shows Arlecchino's consensus view
   (multi-decompiler side-by-side) in the left margin, with breakpoints synced to source.
5. Effort: ~400 lines of orchestration code (bytecode rewriting) plus ~200 lines if the
   IDE plugin is built.

**Source**: [Baeldung guide](https://www.baeldung.com/java-application-remote-debugging),
[Oracle JDI](https://docs.oracle.com/javase/8/docs/technotes/guides/jpda/jdwp-spec.html)

---

## 2. Runtime Instrumentation & In-Game Debug Overlays

### 2.1 Javassist + Java agents — bytecode instrumentation

**What**: the Javassist library plus the JDK javaagent mechanism, for runtime bytecode
rewriting.
**Maturity**: production (Apache-licensed, 2000+ GitHub stars, widely used in APM tools).

**Specifics**:

- **Javassist**: a high-level API (`insertBefore`, `insertAfter`, method redefinition) —
  more convenient than raw ASM.
- **Java agents**: attach to the JVM at startup (`-javaagent:debug-agent.jar`) or at
  runtime (Attach API).
- Can inject debug print statements, trace calls, hook exception handlers and modify
  control flow.
- Supports class redefinition for hot-fixes during a debugging session.

**Adoptability for Arlecchino**:

- Strength: gives a debug console and traces inside reconstructed J2ME games without
  touching the source.
- Use cases:
  - inject trace logging:
    `System.out.println("Entered " + var + " with state=" + gameState);`
  - hook method entry/exit to log the call stack;
  - wrap exception handlers to capture and log errors.

**Adoption path**:

1. Write a Javassist-based agent that instruments the reconstructed bytecode on load.
2. The agent injects calls into a `DebugConsole` class (new, part of an Arlecchino
   runtime).
3. `DebugConsole` listens on a TCP socket and displays either in a separate window or as
   an in-game overlay.
4. Effort: ~300 lines of Javassist code plus ~200 lines of console UI.

**Example scenario**: a mobile game freezes; the agent logs the last 100 method calls,
variable values and GC overhead.

**Source**:
[Baeldung Java instrumentation](https://blogs.sap.com/2016/03/09/java-bytecode-instrumentation-using-agent-breaking-into-java-application-at-runtime/),
[Javassist guide](https://blog.jakubholy.net/2010/06/25/implementing-build-time-instrumentation-with-javassist/)

---

### 2.2 In-game debug console pattern — UI overlay

**What**: an overlay HUD showing debug logs, a command input and a variable inspector,
after the pattern used by game engines.
**Maturity**: production patterns (Unity asset store, open-source examples).

**Specifics**:

- Examples: Consolation (Unity), yapf-consolation, DeveloperConsole (DavidF-Dev).
- Toggled by a key (backtick, F12, F1).
- Displays Javassist-injected logs, or polls JDI for variable state.
- Supports command execution: `set breakpoint 0x1234`, `print my_var`, `reload_sprites`
  and so on.

**Adoptability for Arlecchino**:

- Fit: J2ME games are usually emulated (KEmulator, J2ME Loader), so the emulator's
  canvas/graphics layer can be hooked.
- Implementation: a Swing/JavaFX window, or injection into the game canvas (harder;
  requires Javassist graphics redirection).

**Adoption path**:

1. Javassist injects calls to `DebugConsole.log(msg)`.
2. `DebugConsole` opens a Swing `JFrame` with a text area (logs) and an input field
   (commands).
3. Commands delegate to JDI or to internal state inspection.
4. Effort: ~150 lines of Swing UI plus ~200 lines of command parser.

**Source**: [Consolation](https://github.com/mminer/consolation),
[Unity Debug Console](https://github.com/yasirkula/UnityIngameDebugConsole)

---

## 3. Multi-Decompiler Consensus & Quality Improvement

### 3.1 Meta-decompilation — ensemble voting

**What**: academic work showing that voting across multiple decompilers (CFR, Vineflower,
jadx, Procyon) significantly improves reconstruction quality.
**Maturity**: research (peer-reviewed 2020, cited 100+ times); prototype implementations
exist.

**Specifics**:

- **Problem**: no single Java decompiler handles all bytecode patterns. **6 out of 8
  decompilers have cases that they alone can handle — 276 of 2397 classes in the study.**
- **Solution**: a multi-decompiler ensemble — try each decompiler; if one produces
  syntactically valid Java, use it, otherwise try the next.
- **Result**: **95% recompilability**, against **44–84% for the individual tools**.
- **Algorithm**:
  1. Decompile the class with all tools in parallel.
  2. Run `javac` on each output (validates syntax).
  3. Use the first that compiles; if all fail, merge the ASTs of the best-effort outputs.

**Adoptability for Arlecchino**:

- **Perfect fit**: Arlecchino *already does this*. The paper validates the approach
  empirically.
- Enhancement: add confidence scoring.
  - Assign a weight to each decompiler (CFR: 0.9, Vineflower: 0.85, …) based on local
    performance metrics.
  - Consensus mechanism: if N decompilers produce identical code for a method, mark it
    "high confidence" (suitable for auto-generation); if one decompiler differs, flag the
    method for manual review.

**Adoption path**:

1. Quantify decompiler agreement per method/class (already present in the diff logic).
2. Add a confidence score to the output: `// Confidence: 4/4 decompilers agree`.
3. In the debug console: filter methods by confidence; sort "most uncertain" first for
   manual inspection.
4. Effort: ~100 lines.

**Paper**:
[Java decompiler diversity and its application to meta-decompilation](https://arxiv.org/pdf/2005.11315)

---

### 3.2 Decompiler Explorer / Dogbolt — web UI for comparison

**What**: a web interface (dogbolt.org) showing decompiler outputs side by side.
**Maturity**: production (live service, open-source, MIT license).

**Specifics**:

- Upload a binary; view Ghidra, IDA, Binary Ninja, Boomerang and RetDec outputs in tabs.
- No consensus algorithm; pure visualisation.
- Can be self-hosted via Docker.

**Adoptability for Arlecchino**:

- **Mismatch (rejection reason)**: Dogbolt targets *compiled binaries* (x86, ARM), not
  Java bytecode. CFR, Vineflower and jadx are not integrated.
- Workaround if pursued: fork Dogbolt and add Java decompiler backends — ~500 lines to
  integrate CFR and Vineflower via subprocess.
- Better alternative: write a lightweight Arlecchino-native web UI — ~100 lines of HTML/JS
  plus ~50 lines of server code.

**Verdict**: **low priority / skip** — the existing CLI is adequate; a web UI is only
warranted if UX becomes the bottleneck.

**Source**: [Decompiler Explorer](https://dogbolt.org/),
[GitHub](https://github.com/decompiler-explorer/decompiler-explorer)

---

## 4. Statistical Variable & Type Recovery

Three published research systems recover meaningful variable names from decompiler output
by means of trained statistical models. All three were trained on C binaries, which is the
central caveat for every one of them.

### 4.1 DIRTY (DecompIled variable ReTYper)

**What**: a trained statistical model that refines decompiler output by recovering
meaningful variable names and types.
**Maturity**: research + prototype (USENIX Security 2022, code released).

**Specifics**:

- **Input**: a decompiled C function with poor names such as `rax`, `local_c`.
- **Output**: recommended variable names and types drawn from **48k+ known types**.
- **Training corpus**: **~1M GitHub C functions**.
- **Accuracy**: **66.4% of original names, 75.8% of original types** on a held-out test
  set.
- **Released implementation**: Python.

**Adoptability for Arlecchino**:

- **Mismatch**: DIRTY was trained on C; Java bytecode is not C binaries. Method names,
  type information and local variable usage differ significantly.
- Idea: retrain DIRTY on Java reconstructed code (corpus: GitHub Java projects with
  source + decompiled pairs).
  - Feasibility: ~2–4 weeks (1M Java functions, GPU cluster).
  - Alternative: use the pre-trained C model and fine-tune it for Java in ~1 week on a
    smaller corpus.
- Quick win: apply DIRTY to *variable names* only — types are less relevant, since
  Arlecchino already has bytecode type information.

**Adoption path**:

1. Collect a corpus: 10k J2ME `.jar` files (Arlecchino's input) + CFR/Vineflower
   decompiled output + original source where available.
2. Train or fine-tune a DIRTY variant on that corpus.
3. Post-process Arlecchino output: feed each method to the model, replace auto-generated
   names (`v0`, `local_c`) with suggestions.
4. Effort: ~2–4 weeks training from scratch; ~1 week fine-tuning.
5. Expected boost: variable clarity **+10–20%** (speculative, not validated on J2ME).

**Confidence**: **~60%** — untested on J2ME bytecode; a C-trained model may not
generalise.

**Source**: [USENIX 2022 paper](https://www.usenix.org/system/files/sec22-chen-qibin.pdf)

---

### 4.2 VarBERT — variable naming via transfer learning

**What**: a model that predicts variable names by learning from human-written source code
and is then fine-tuned on decompiled code.
**Maturity**: research (IEEE S&P 2024, open-source).

**Specifics**:

- **Training**: pre-trained on **5.2M human source functions**; fine-tuned on IDA/Ghidra
  decompiled C.
- **Task**: given a decompiled function with slots like `int v0, char *v1, ...`, predict a
  meaningful name from the vocabulary ("count", "ptr", "index").
- **Accuracy**: **54.43% (IDA), 54.49% (Ghidra)** on O2-optimised binaries.
- **Bonus capability**: can classify whether a variable is human-created or
  compiler-generated, at **~80–90% F1**.

**Adoptability for Arlecchino**:

- **Same caveat as DIRTY**: trained on binary decompilation, not Java.
- Advantage over DIRTY: the VarBERT framework is generic, so retraining on Java is easier.
- Quick adoption: use pre-trained VarBERT on Arlecchino output as-is — roughly a **50%**
  chance it generalises; empirically unknown.
- Proper adoption: fine-tune on 5k J2ME decompiled functions plus the original variable
  names, where source is available.

**Adoption path**:

1. Collect a J2ME corpus (the same one as for DIRTY).
2. Fine-tune VarBERT in 2–3 days on a single GPU.
3. Integrate via a thin wrapper process: feed it Arlecchino output, get name suggestions
   back.
4. Effort: ~200 lines of integration code plus 2–3 days of GPU time.

**Confidence**: **~70%** — the model is general-purpose, but Java-specific validation is
needed.

**Source**: [IEEE S&P 2024](https://sefcom.asu.edu/publications/varbert-oakland24.pdf),
[GitHub](https://github.com/sefcom/VarBERT)

---

### 4.3 DIRE — variable renaming ensemble

**What**: an ensemble of lexical and graph-based models for variable name prediction.
**Maturity**: research (2022, ACM TSE); dataset published.

**Specifics**:

- **Two models**:
  1. Lexical — captures sequential context, like DIRTY.
  2. Graph — captures data-flow graph structure.
- **Training**: **164k binaries** from GitHub C projects.
- **Accuracy**: **74.3% variable name recovery — the highest of the three systems
  surveyed.**
- **Unique value**: analyses how training data provenance (which GitHub projects, which
  compiler flags) affects performance.

**Adoptability for Arlecchino**:

- Same position as VarBERT: a generic ensemble approach, easier to retrain on Java than
  DIRTY.
- Advantage: the graph model could exploit Arlecchino's AST representation.
- Effort: ~4 weeks to retrain and validate on J2ME.

**Verdict**: same adoption path as VarBERT, but **lower priority than VarBERT** —
slightly more complex for similar accuracy.

**Source**: [ACM TSE 2022](https://dl.acm.org/doi/pdf/10.1145/3546946),
[GitHub](https://github.com/pcyin/dire)

---

## 5. Dynamic Analysis & Time-Travel Debugging

### 5.1 Frida — runtime instrumentation on mobile

**What**: a dynamic instrumentation toolkit — not a debugger, but a hook engine.
**Maturity**: production (NowSecure; widely used in mobile security).

**Specifics**:

- Injects JavaScript into a running process (Android, iOS, Windows, macOS, Linux).
- Hooks any method, Java or native, at runtime.
- Spies on arguments and return values, modifies behaviour, logs calls.
- Needs no source code; works on compiled and obfuscated apps.

**Adoptability for Arlecchino**:

- Useful for J2ME game debugging:
  - J2ME games commonly run in emulators (KEmulator, J2ME Loader on Android) or on a JVM.
  - Frida can hook the JVM's method dispatch to log game state, player position, collision
    checks and so on.
- Workflow:
  1. Load the J2ME game in an emulator.
  2. Attach Frida to the JVM process.
  3. Use a Frida script to hook reconstructed class methods (`game.update()`,
     `player.move(x, y)`).
  4. Log or intercept the calls.

**Adoption path**:

1. Generate Frida hook templates from Arlecchino output (method signatures → Frida
   JavaScript).
2. The user selects methods to trace in the debug console.
3. Frida injects the hooks on the fly.
4. Effort: ~200 lines for the hook-template generator plus integration.

**Confidence**: **~85%** — Frida is battle-tested and the J2ME integration is
straightforward.

**Source**: [Frida docs](https://frida.re/),
[Medium guide](https://can-ozkan.medium.com/dynamic-instrumentation-on-android-with-frida-a-practical-guide-878f492144ff)

---

### 5.2 Record-and-replay (rr, Pernosco, WinDbg TTD) — time-travel debugging

**What**: record an entire program execution — every instruction, memory access and
register change — then replay it backwards and forwards and query the execution history.

**Maturity**:

- **rr** (Linux): production (Mozilla, open-source).
- **Pernosco**: commercial (Mozilla engineers); an omniscient debugging UI built on rr.
- **WinDbg TTD** (Windows): production (Microsoft).

**Specifics**:

- **Recording overhead: ~10–20x slowdown.**
- Benefits:
  - replay a bug 1000x; set breakpoints retroactively; inspect any past state;
  - LINQ queries over the trace (WinDbg): *"find all instructions that wrote to address
    X"*;
  - deterministic replay — the same input yields an identical execution.

**Adoptability for Arlecchino**:

- **Not a primary goal**, but useful for heisenbug debugging: a J2ME game that crashes
  intermittently can be recorded over 10 runs, filtered for the crash and replayed
  deterministically.
- Integration:
  1. Arlecchino outputs reconstructed source.
  2. Compile to bytecode; run in a JVM under rr.
  3. Replay in jdb — GDB does not understand Java, but an rr trace is JVM-agnostic.

**Verdict**: **deferred** — low priority initially; revisit only if determinism problems
appear.

**Source**: [rr](https://rr-project.org/),
[WinDbg TTD](https://learn.microsoft.com/en-us/archive/blogs/windbg/time-travel-debugging-in-windbg-preview)

---

### 5.3 Symbolic execution (angr) — automated constraint solving

**What**: treat decompiled code as symbolic constraints and solve them to find inputs that
trigger specific paths.
**Maturity**: research + production tooling (angr is widely used in CTFs and security
research).

**Specifics**:

- Instead of stepping through code, solve: *"what input causes this branch?"*
- Concolic testing mixes symbolic (constraints) with concrete (real values).
- Use case: find inputs that unlock a cheat menu, bypass a licence check and similar.

**Adoptability for Arlecchino**:

- **An advanced feature with low immediate utility** for game debugging.
- Niche use: if a J2ME game has obfuscated input validation, angr could recover the
  key/password.
- Integration effort is minimal, since angr is standalone; Arlecchino could merely suggest
  angr recipes in the debug menu.

**Verdict**: **deferred to Phase 2.**

**Source**: [angr documentation](https://docs.angr.io/),
[Defeating obfuscation with angr](https://napongizero.github.io/blog/Defeating-Code-Obfuscation-with-Angr)

---

## 6. Source-Level Debug Info Generation

### 6.1 DWARF debug format — source mapping for reconstructed code

**What**: the standard debugging metadata format used by GDB, LLDB, Valgrind and others.
Maps memory addresses → source locations, variable locations and type information.
**Maturity**: production (DWARF 5 standardised; compilers emit it).

**Relevant sections**:

- `.debug_info` — type and variable definitions (Debugging Information Entries, DIEs).
- `.debug_line` — the line number table (address → file/line mapping).
- `.debug_str` — the string pool (variable names, file names).
- `.debug_ranges`, `.debug_loc` — variable location expressions (register + offset).

**How debuggers use it**: on a breakpoint at address `0x1234`, look up `.debug_line` →
`file.java:42`, then show the source context.

**Adoptability for Arlecchino**:

- Powerful, but complex: reconstructed source lines have no natural correspondence to
  bytecode offsets, and CFR and Vineflower map them differently.
- **Proposed workaround**: generate synthetic DWARF with a 1:1 mapping (reconstructed line
  N → bytecode offset M). Not exact, but enough for a debugger to step through Arlecchino
  source.
- Effort: ~400 lines (DWARF writer) plus ~100 lines (bytecode offset calculator).
- Payoff: if Arlecchino emits ELF or JVM `.class` files, the output plugs into any
  DWARF-aware debugger.

**Adoption path** (medium priority):

1. Extract the mapping from CFR/Vineflower: which reconstructed source line corresponds to
   which bytecode offset.
2. Write a DWARF generator (the survey assumed the `pyelftools` library, matching the
   then-current Python implementation).
3. Embed the result into compiled `.class` files or an ELF wrapper.
4. Test with GDB on the reconstructed bytecode.
5. Effort: ~2 weeks.

**Confidence**: **~75%** — the DWARF standard is solid, but the bytecode ↔ source mapping
has edge cases.

**Source**: [LLVM Source-Level Debugging](https://llvm.org/docs/SourceLevelDebugging.html),
[DWARF Wikipedia](https://en.wikipedia.org/wiki/DWARF)

---

## 7. Existing Multi-Decompiler Work (For Reference)

### 7.1 Decompiler benchmark datasets

Several academic efforts catalogue decompiler behaviour:

- **"The Strengths and Behavioral Quirks of Java Bytecode Decompilers"** (2019): evaluates
  CFR, JD-Core, Procyon, Vineflower and others on **2,041 classes**. Finding: **CFR is the
  most reliable at 84% syntactic correctness**, but no single tool handles all patterns.
- **DecompileBench** (2025): **1M binary-source pairs**; benchmarks both accuracy and
  recompilability.

**Relevance**: validates Arlecchino's multi-decompiler philosophy empirically. The
findings can be used to weight the decompilers in the merge step.

**Source**: [Strengths and Quirks](https://arxiv.org/pdf/1908.06895)

---

## Prototype Recommendations

Ranked by (practical impact × feasibility × uniqueness). Line counts below were sized
against the earlier Python implementation and are quoted as language-neutral estimates.

### #1: JDWP source-level debug integration (2–3 weeks)

**The Java-native approach.**

**What to build**:

- Compile Arlecchino-reconstructed source → bytecode using `javac`.
- Generate synthetic JDWP source mappings (bytecode offset ↔ source line).
- Bundle as a JVM launcher plus an integrated jdb CLI console, or as an IDE plugin for
  IntelliJ/Eclipse/VS Code.

**Why it ranks first**:

- Perfect fit: Arlecchino outputs Java, and JDWP is Java's native debugging standard.
- Ubiquitous: every IDE and tool supports JDWP.
- Covers **80% of debugging workflows**: step, watch variables, set breakpoints, inspect
  objects.
- Arlecchino can act as a debugger backend inside any IDE.

**Workflow**:

```
user: arlecchino decompile allods.jar → allods_src/
user: arlecchino debug allods_src/   # starts the JVM with JDWP, opens the IDE
user: [IDE] sets a breakpoint in allods/Game.java:42
user: [IDE] steps through doPlayerMove(), inspects player.health
```

**Effort breakdown**:

| Component | Estimate |
|---|---|
| Bytecode compilation + synthetic source mapping | ~400 lines (`javac` subprocess + file handling) |
| JVM + JDWP launcher | ~100 lines (subprocess management, JDWP handshake) |
| IDE plugin (optional, high ROI) | ~200 lines of IDE SDK code |

**Deliverable**: compile Arlecchino output → bytecode; generate synthetic source mappings
(class file attributes); bundle a JVM launcher plus jdb UI or IDE plugin. Demo: step
through the reconstructed Allods game, inspect `player.health`, set breakpoints.

**Key technical detail**: CFR and Vineflower do not map every bytecode offset to source.
Arlecchino must infer the mapping — **likely 85–90% fidelity**. Test on real J2ME games
before shipping.

**Timeline**: 2–3 weeks.

---

### #2: Javassist runtime instrumentation + in-game debug console (1–2 weeks)

**The dynamic analysis angle.**

**What to build**:

- A Java agent (using the Javassist library) that instruments Arlecchino-compiled bytecode
  at JVM load time.
- The agent injects calls to `DebugConsole.log(...)` before and after method entry, in
  exception handlers and on variable writes.
- A Swing-based in-game console: toggles on F12, shows logs, accepts commands
  (`trace method`, `inspect var`, `set_breakpoint`, `reload_sprites`).

**Why it ranks second**:

- Orthogonal to #1: complements static debugging with dynamic tracing.
- No recompilation needed — the agent attaches to the JVM at startup.
- Unique angle: an in-game console is not offered by competitors for J2ME ports.
- Works on the live running game: pause and inspect mid-execution.

**Workflow**:

```
user: java -javaagent:arlecchino-agent.jar allods_game.jar
[game starts with Javassist hooks injected]
user: press F12 → the in-game console opens
user: type "trace game.update" → every call to game.update() logs args + return
user: type "inspect player.health" → watch the variable in real time
```

**Effort breakdown**:

| Component | Estimate |
|---|---|
| Javassist agent + hook generation | ~250 lines |
| `DebugConsole` class (Swing UI) | ~200 lines |
| Command parser | ~100 lines |
| **Total** | **~550 lines** |

**Deliverable**: a Java agent using Javassist (auto-injects on JVM startup); a Swing-based
debug console (logs + command input); commands `trace(class, method)`, `inspect(var)`,
`set_breakpoint()`. Demo: enable tracing for `game.update()` and observe every state
change; run a `game.reset()` command from the console.

**Key technical details**:

- Javassist can redefine methods at runtime, so the agent injects without touching the
  original `.class` files.
- **Logging overhead: ~5–10% slowdown**, acceptable for debug builds.

**Timeline**: 1–2 weeks.

---

## Summary Table

| **Idea** | **Maturity** | **Effort** | **Impact** | **Java fit** | **Recommended** |
|---|---|---|---|---|---|
| decomp2dbg | Production | 2 weeks | High | No (but adaptable) | Phase 2 |
| Ghidra Debugger | Production | 1 week setup | Medium | Low | Skip |
| **JDWP / JDI** | **Production** | **2–3 weeks** | **High** | **Perfect** | **#1 priority** |
| **Javassist instrumentation** | **Production** | **1–2 weeks** | **High** | **Perfect** | **#2 priority** |
| In-game console | Pattern | 1 week | Medium | Yes | Part of #2 |
| Meta-decompilation | Research | 1 week (integrate) | Medium | Yes | Already in Arlecchino |
| Dogbolt / Decompiler Explorer | Production | 2 weeks (fork) | Low | Requires modification | Skip |
| DIRTY | Research | 2–4 weeks (retrain) | Medium | C-trained | Phase 2 |
| VarBERT | Research | 1 week (fine-tune) | Medium | C-trained | Phase 2 |
| DIRE | Research | 4 weeks (retrain) | Medium | C-trained | Phase 2 |
| Frida | Production | 1–2 weeks | Medium | Yes | Phase 2 |
| rr / TTD | Production | 2 weeks | Low | Advanced | Phase 3 |
| Symbolic execution (angr) | Research | 1 week | Low | Yes | Phase 3 |
| DWARF debug info | Production | 2 weeks | Medium | Complex | Phase 2 |

---

## Implementation Roadmap

**Weeks 1–3**: #1 (JDWP + source mapping)
- Branch: `feature/jdwp-debugger`
- Deliverable: an IDE can step through `Allods.java` and watch `player.health`.
- Test: record a video of the debugging session.

**Weeks 4–5**: #2 (Javassist agent + in-game console)
- Branch: `feature/runtime-instrumentation`
- Deliverable: the F12 console shows game method traces in real time.
- Test: inject a deliberate bug and watch it trace in the console.

---

## Validation Checklist

- [ ] #1: JDWP steps through ≥ 10 methods in Allods without missing lines.
- [ ] #1: IDE breakpoints work; variable inspection shows correct values.
- [ ] #2: agent injection adds < 10% runtime overhead.
- [ ] #2: the in-game console captures ≥ 95% of method calls (spot-check).

---

## Architectural Vision (Post-Prototypes)

An Arlecchino debugger suite, sketched for Phases 3–4:

```
Arlecchino CLI
  ↓ (output)
  ├─→ Reconstructed .java source
  │    ├─→ [JDWP phase] Compile → bytecode + synthetic source map
  │    └─→ [IDE plugin] Show in IntelliJ with confidence scores
  │
  ├─→ Javassist agent JAR
  │    ├─→ Auto-instrument on JVM load
  │    └─→ In-game console (Swing JFrame)
  │
  ├─→ Frida hook templates
  │    └─→ User selects methods to trace dynamically
  │
  └─→ Debug metadata (DWARF, class file attributes)
       └─→ Feeds GDB, LLDB and other tools
```

**User workflow**:

1. Arlecchino decompiles the J2ME game → merged Java source + consensus view.
2. The user clicks "Debug" → auto-compile, start the JVM with JDWP and the Javassist
   agent.
3. An IDE (IntelliJ) or a jdb console opens; the user steps through the reconstructed
   code.
4. The in-game console shows logs; typing `trace game.update` streams method calls.

---

## Known Risks

1. **Bytecode ↔ source fidelity** (#1): the CFR/Vineflower mapping is incomplete and may
   skip some bytecode regions. Mitigation: instrument the bytecode and compare against an
   actual execution trace.
2. **Agent overhead** (#2): large games may become slow. Mitigation: selective
   instrumentation — the user picks which methods to trace.

---

## Caveats & Unknowns

1. **Bytecode ↔ source mapping fidelity**: CFR and Vineflower do not map every bytecode
   offset to reconstructed source. A synthetic mapping may be incomplete, leaving some
   bytecode un-steppable. Mitigation: test on real J2ME games.

2. **Generalisation of the trained models**: DIRTY, VarBERT and DIRE were all trained on C
   binaries; **their performance on Java is unvalidated**. Recommendation: **validate on 10
   sample J2ME games before any heavy investment.**

3. **JDWP for non-compiled code**: JDWP is designed to debug JVM bytecode at runtime,
   whereas Arlecchino output is reconstructed *source*. Mapping reconstructed lines to
   bytecode offsets is non-trivial. Workaround: synthetic line tables, or source mapping
   on the IDE side.

4. **Security**: **JDWP has no authentication, and Javassist agents can be abused.** Debug
   builds should be run isolated — in a Docker container or on a separate machine.

---

## Confidence Levels

| Task | Confidence | Validation method |
|---|---|---|
| #1 JDWP integration | 85% | Test on Allods + 2 other J2ME games |
| #1 Source mapping | 75% | Compare reconstructed line → bytecode offset; cross-check against a bytecode trace |
| #2 Javassist overhead | 90% | Profile with JMH; measure frame rate impact |
| #2 In-game console UI | 80% | User feedback; polish the Swing layout |

---

## Success Criteria

- **#1**: a user can debug a reconstructed J2ME game in an IDE as if it were
  source-available.
- **#2**: the in-game console shows live execution traces with no breakpoint recompile
  needed.

---

## References

1. **decomp2dbg**: [GitHub](https://github.com/mahaloz/decomp2dbg)
2. **Ghidra Debugger**: [Ghidra Docs](https://ghidra.re),
   [clearbluejar blog](https://clearbluejar.github.io/posts/decompilation-debugging-pretending-all-binaries-come-with-source-code/)
3. **JDWP / JDI**: [Oracle JPDA](https://docs.oracle.com/javase/8/docs/technotes/guides/jpda/),
   [Baeldung](https://www.baeldung.com/java-application-remote-debugging)
4. **Javassist**: [Javassist Guide](https://www.javassist.org/),
   [Baeldung Instrumentation](https://blogs.sap.com/2016/03/09/java-bytecode-instrumentation-using-agent-breaking-into-java-application-at-runtime/)
5. **Meta-Decompilation**: [arXiv:2005.11315](https://arxiv.org/pdf/2005.11315)
6. **DIRTY**: [USENIX Security 2022](https://www.usenix.org/system/files/sec22-chen-qibin.pdf)
7. **VarBERT**: [IEEE S&P 2024](https://sefcom.asu.edu/publications/varbert-oakland24.pdf),
   [GitHub](https://github.com/sefcom/VarBERT)
8. **DIRE**: [ACM TSE 2022](https://dl.acm.org/doi/pdf/10.1145/3546946)
9. **Decompiler Explorer / Dogbolt**: [dogbolt.org](https://dogbolt.org/),
   [GitHub](https://github.com/decompiler-explorer/decompiler-explorer)
10. **Frida**: [Frida Docs](https://frida.re/),
    [Mobile Debugging Guide](https://can-ozkan.medium.com/dynamic-instrumentation-on-android-with-frida-a-practical-guide-878f492144ff)
11. **DWARF**: [LLVM SourceLevelDebugging](https://llvm.org/docs/SourceLevelDebugging.html),
    [Wikipedia](https://en.wikipedia.org/wiki/DWARF)
12. **Record-and-Replay**: [rr](https://rr-project.org/),
    [WinDbg TTD](https://learn.microsoft.com/en-us/windows-hardware/drivers/debuggercmds/time-travel-debugging-overview)
13. **Symbolic Execution**: [angr Docs](https://docs.angr.io/)
14. **Java Decompiler Evaluation**: [arXiv:1908.06895](https://arxiv.org/pdf/1908.06895)
15. **In-Game Consoles**: [Consolation (Unity)](https://github.com/mminer/consolation),
    [DeveloperConsole](https://github.com/DavidF-Dev/Unity-DeveloperConsole)
