// DeterministicClockTransformer — ASM bytecode rewriter that makes a J2ME MIDlet's
// observable behaviour a pure function of how many times it asks the clock / RNG,
// independent of the host's wall clock.  This is Arlecchino Stage A7 (PLAN_runtime.md,
// "A7. Детерминизм"), path (b): instrument the MIDlet itself instead of forking the
// (GPL) emulator.
//
// WHY
// ---
// FreeJ2ME-plus samples frames by wall clock and runs the MIDlet on real threads with
// the real system clock, so two replays of the same input drift (baseline Jaccard 0.86
// / 97 % per-frame on Allods).  A MIDlet whose animations/timeouts/RNG are driven by
// `System.currentTimeMillis()` / `System.nanoTime()` / `new Random()` is therefore
// nondeterministic.  If we make those sources deterministic, the MIDlet's state becomes
// a function of the *call sequence* alone — so two replays produce byte-identical frames
// even on this noisy emulator.
//
// WHAT IT REWRITES (only these — minimal, behaviour-preserving for the deterministic run)
//   (1) INVOKESTATIC java/lang/System.currentTimeMillis ()J
//       INVOKESTATIC java/lang/System.nanoTime         ()J
//         -> INVOKESTATIC <RuntimeClass>.currentTimeMillis/nanoTime ()J
//       Our clock is a virtual counter that advances a FIXED STEP per call (so a clock-
//       polling animation loop advances deterministically), seeded at a fixed origin.
//   (2) INVOKESPECIAL java/util/Random.<init> ()V   (new Random() — time-seeded by the JDK)
//         -> push FIXED_SEED ; INVOKESPECIAL java/util/Random.<init> (J)V
//   (3) INVOKESPECIAL java/util/Random.<init> (J)V   (explicit seed — usually currentTimeMillis)
//         -> POP2 the incoming seed ; push FIXED_SEED ; INVOKESPECIAL <init> (J)V
//       (popping a side-effect-free seed expression's RESULT is safe here: the seed on
//        Allods is `System.currentTimeMillis()` or a small arithmetic of it — no observable
//        side effects beyond the clock we already virtualise.  This is opt-in fixed-clock
//        mode only; the normal pipeline never runs this pass.)
//   (4) INVOKESTATIC java/lang/Math.random ()D
//         -> INVOKESTATIC <RuntimeClass>.random ()D   (our seeded generator)
//
// We DON'T touch System.gc (no behavioural value for frame determinism), Object.hashCode
// (identity hashes don't reach the framebuffer for these MIDlets), or Thread timing
// (sleeps don't change rendered state when frames are sampled by request, not by clock).
//
// RUNTIME CLASS
//   A tiny class `arlecchino/det/Det` is GENERATED (via ASM) and injected into the output
//   jar, so the rewritten MIDlet is self-contained — no extra classpath at emulation time.
//   It holds: a static `long virtualNanos` advanced FIXED_STEP_NS per clock call, and a
//   static java.util.Random seeded with FIXED_SEED for Math.random().  All static, reset
//   implicitly per JVM (each headless run is a fresh JVM, so each run starts at the same
//   virtual origin -> identical sequences).
//
// STACKMAP FRAMES (class file major >= 50)
//   Original phone jars are major 48/49: the old type-inferencing verifier, no
//   StackMapTable at all.  A jar REBUILT by a modern javac is major 50+, where every
//   branch target MUST carry a frame or the class fails to load with
//   `VerifyError: Expecting a stackmap frame at branch target N`.  So the rewrite has to
//   keep the frames intact.  Two paths, both handled here:
//
//   * PRESERVE (default).  Every rewrite above is stack-shape-preserving AT FRAME
//     POINTS: we only swap a call for one with the same descriptor, or push/pop the
//     Random seed strictly between two instructions.  The incoming frames therefore stay
//     valid verbatim; we just must not throw them away.  We read WITHOUT
//     `ClassReader.SKIP_FRAMES` so ASM re-emits each frame at its new (shifted) bytecode
//     offset.  For major <= 49 there are no frames to visit, so this path is exactly the
//     old behaviour.
//   * RECOMPUTE (fallback / `--recompute-frames`).  If a major>=50 class carries branches
//     but NO frames at all (stripped by an obfuscator), preserving nothing is not enough —
//     we re-run the class through `ClassWriter.COMPUTE_FRAMES`.  That calls
//     `getCommonSuperClass`, which needs a loader that sees BOTH the jar's own classes and
//     the MIDP API (`javax.microedition.*`) — neither is on our classpath.  `Hierarchy`
//     below supplies exactly that: it resolves super/interface links by READING .class
//     resources from a loader over the input jar plus any `--cp` entries (the caller
//     passes midp-rt.jar / the emulator jar), never by initialising the class.
//
// CLEAN-ROOM: ASM (org.ow2.asm, BSD-3-Clause) is INVOKED, not vendored; the FreeJ2ME-plus
// emulator is observed for behaviour only (GPL) and never modified.  All logic here is
// original Arlecchino code (Apache-2.0).
//
// USAGE:  java -cp <asm jars>:. DeterministicClockTransformer \
//              [--cp <jar>[:<jar>...]] [--recompute-frames] <in.jar> <out.jar>
//
// Author: Wehrwolfmann.

import java.io.*;
import java.net.*;
import java.util.*;
import java.util.jar.*;

import org.objectweb.asm.*;

public final class DeterministicClockTransformer {

    // The injected runtime class: deterministic clock + RNG, no external deps.
    static final String RT_INTERNAL = "arlecchino/det/Det";

    // Virtual-time knobs.  Origin is an arbitrary fixed epoch; step is the per-call
    // advance.  A non-zero step means a `while (now - start < T)` busy/animation loop
    // still terminates deterministically (each poll moves the clock forward), while a
    // replay of the SAME number of polls yields the SAME timestamps.
    static final long FIXED_ORIGIN_NANOS = 1_000_000_000L * 1_000L;  // 1000 s since vt-epoch
    static final long FIXED_STEP_NANOS   = 1_000_000L;               // +1 ms of virtual time / clock call
    static final long FIXED_SEED         = 0x5DEECE66DL ^ 0xA110D5L;  // any constant; stable across runs

    // Class-file major of the first Java version that mandates StackMapTable
    // (Java 6 = 50).  Everything at or below 49 uses the old inferencing verifier
    // and carries no frames — the pre-frames J2ME jars we normally instrument.
    static final int MAJOR_STACKMAPS = 50;

    // Fixed mtime for the injected runtime entry (2000-01-01 UTC — the zip format
    // cannot represent anything before 1980), so instrumenting the same jar twice
    // yields the same bytes.
    static final long RT_ENTRY_TIME = 946_684_800_000L;

    public static void main(String[] args) throws Exception {
        DeterministicClockTransformer t = new DeterministicClockTransformer();
        List<String> positional = new ArrayList<>();
        for (int i = 0; i < args.length; i++) {
            String a = args[i];
            if (a.equals("--cp") && i + 1 < args.length) {
                t.addClasspath(args[++i]);
            } else if (a.equals("--recompute-frames")) {
                t.forceRecomputeFrames = true;
            } else {
                positional.add(a);
            }
        }
        if (positional.size() < 2) {
            System.err.println("usage: DeterministicClockTransformer "
                    + "[--cp <jar>[" + File.pathSeparator + "<jar>...]] "
                    + "[--recompute-frames] <in.jar> <out.jar>");
            System.exit(2);
        }
        t.run(new File(positional.get(0)), new File(positional.get(1)));
    }

    int rewrittenClock = 0, rewrittenRandomNoArg = 0, rewrittenRandomSeed = 0, rewrittenMathRandom = 0;
    // How each class was handled, for the one-line report.
    int framesPreserved = 0, framesRecomputed = 0;
    /** Extra classpath entries for hierarchy resolution (MIDP API jars, …). */
    final List<File> hierarchyClasspath = new ArrayList<>();
    /** `--recompute-frames`: always rebuild frames instead of preserving them. */
    boolean forceRecomputeFrames = false;
    /** Super/interface resolver behind getCommonSuperClass; built in run(). */
    Hierarchy hierarchy;

    void addClasspath(String spec) {
        for (String part : spec.split(File.pathSeparator)) {
            part = part.trim();
            if (part.isEmpty()) continue;
            File f = new File(part);
            if (f.exists()) hierarchyClasspath.add(f);
            else System.err.println("DeterministicClockTransformer: --cp entry not found, "
                    + "ignored: " + part);
        }
    }

    void run(File in, File out) throws IOException {
        File outDir = out.getParentFile();          // null for a bare file name
        if (outDir != null) outDir.mkdirs();
        // The frame recomputation path resolves types out of the jar itself first,
        // then the caller-supplied MIDP/emulator jars, then this JVM's own classes.
        hierarchy = new Hierarchy(in, hierarchyClasspath);
        boolean rtPresent = false;
        try (JarFile jf = new JarFile(in);
             JarOutputStream jos = new JarOutputStream(
                     new BufferedOutputStream(new FileOutputStream(out)))) {
            Enumeration<JarEntry> e = jf.entries();
            while (e.hasMoreElements()) {
                JarEntry je = e.nextElement();
                if (je.isDirectory()) continue;
                byte[] data;
                if (je.getName().endsWith(".class")) {
                    try (InputStream is = jf.getInputStream(je)) {
                        data = transformClass(is.readAllBytes());
                    }
                } else {
                    try (InputStream is = jf.getInputStream(je)) {
                        data = is.readAllBytes();
                    }
                }
                if (je.getName().equals(RT_INTERNAL + ".class")) rtPresent = true;
                JarEntry ne = new JarEntry(je.getName());
                ne.setTime(je.getTime());
                jos.putNextEntry(ne);
                jos.write(data);
                jos.closeEntry();
            }
            if (!rtPresent) {
                JarEntry rt = new JarEntry(RT_INTERNAL + ".class");
                // Fixed timestamp: without it the injected entry gets the wall clock and
                // two instrumentations of the same jar differ byte-wise — awkward for a
                // pass whose whole point is reproducibility.
                rt.setTime(RT_ENTRY_TIME);
                jos.putNextEntry(rt);
                jos.write(generateRuntimeClass());
                jos.closeEntry();
            }
        }
        System.out.println("DeterministicClockTransformer: clock=" + rewrittenClock
                + " new Random()=" + rewrittenRandomNoArg
                + " new Random(seed)=" + rewrittenRandomSeed
                + " Math.random()=" + rewrittenMathRandom
                + " frames: kept=" + framesPreserved + " recomputed=" + framesRecomputed
                + " (runtime class -> " + RT_INTERNAL + ")");
        hierarchy.reportUnresolved();
    }

    // ---- per-class transform --------------------------------------------------
    /** Class-file major version straight from the header (bytes 6..7). */
    static int majorVersion(byte[] cls) {
        return ((cls[6] & 0xFF) << 8) | (cls[7] & 0xFF);
    }

    byte[] transformClass(byte[] in) {
        boolean framesMandatory = majorVersion(in) >= MAJOR_STACKMAPS;
        if (framesMandatory && forceRecomputeFrames) return recomputeFrames(in);

        // Default: rewrite while CARRYING THE EXISTING FRAMES THROUGH.  For a major>=50
        // class the read flags are 0, NOT SKIP_FRAMES — dropping its StackMapTable is
        // exactly what produced `VerifyError: Expecting a stackmap frame at branch
        // target N` on javac-rebuilt jars.  Our edits never change the stack shape at a
        // frame point, so the original frames stay correct; ASM just re-emits them at
        // the shifted offsets.  COMPUTE_MAXS still recomputes max_stack, which the
        // extra LDC2_W of the Random rewrites can raise.
        //
        // Below 50 we keep SKIP_FRAMES, i.e. the pre-fix behaviour byte for byte.  Those
        // are the original phone jars: the desktop JVM verifies them by inference and
        // ignores the CLDC-preverifier `StackMap` attribute entirely, so carrying it
        // would change nothing at run time while perturbing every jar the existing A7
        // reference numbers were measured on.
        FrameStats stats = new FrameStats();
        ClassReader cr = new ClassReader(in);
        ClassWriter cw = new ClassWriter(cr, ClassWriter.COMPUTE_MAXS);
        cr.accept(detVisitor(cw, stats), framesMandatory ? 0 : ClassReader.SKIP_FRAMES);

        // A major>=50 class with branches but NO frames at all had them stripped before
        // us; there is nothing to preserve, so build them from scratch instead.
        if (framesMandatory && stats.branches > 0 && stats.frames == 0)
            return recomputeFrames(in);
        if (framesMandatory) framesPreserved++;
        return cw.toByteArray();
    }

    /** Fallback: rebuild the whole StackMapTable via COMPUTE_FRAMES, resolving the
     *  type hierarchy through {@link Hierarchy} (jar + --cp + this JVM). */
    byte[] recomputeFrames(byte[] in) {
        ClassReader cr = new ClassReader(in);
        ClassWriter cw = new ClassWriter(cr, ClassWriter.COMPUTE_FRAMES) {
            @Override protected String getCommonSuperClass(String type1, String type2) {
                return hierarchy.commonSuperClass(type1, type2);
            }
        };
        // COMPUTE_FRAMES discards visited frames anyway; SKIP_FRAMES avoids parsing them.
        cr.accept(detVisitor(cw, new FrameStats()), ClassReader.SKIP_FRAMES);
        framesRecomputed++;
        return cw.toByteArray();
    }

    ClassVisitor detVisitor(ClassWriter cw, FrameStats stats) {
        return new ClassVisitor(Opcodes.ASM9, cw) {
            @Override public MethodVisitor visitMethod(int access, String name, String desc,
                                                       String sig, String[] ex) {
                MethodVisitor mv = super.visitMethod(access, name, desc, sig, ex);
                return new DetMethodVisitor(mv, stats);
            }
        };
    }

    /** Per-class tally used to spot a major>=50 class whose frames were stripped. */
    static final class FrameStats {
        int frames = 0;     // StackMapTable entries seen
        int branches = 0;   // jumps / switches / handlers — i.e. things that need frames
    }

    final class DetMethodVisitor extends MethodVisitor {
        private final FrameStats stats;

        DetMethodVisitor(MethodVisitor mv, FrameStats stats) {
            super(Opcodes.ASM9, mv);
            this.stats = stats;
        }

        @Override public void visitFrame(int type, int nLocal, Object[] local,
                                         int nStack, Object[] stack) {
            stats.frames++;
            super.visitFrame(type, nLocal, local, nStack, stack);
        }

        @Override public void visitJumpInsn(int opcode, Label label) {
            stats.branches++;
            super.visitJumpInsn(opcode, label);
        }

        @Override public void visitTableSwitchInsn(int min, int max, Label dflt, Label... labels) {
            stats.branches++;
            super.visitTableSwitchInsn(min, max, dflt, labels);
        }

        @Override public void visitLookupSwitchInsn(Label dflt, int[] keys, Label[] labels) {
            stats.branches++;
            super.visitLookupSwitchInsn(dflt, keys, labels);
        }

        @Override public void visitTryCatchBlock(Label start, Label end, Label handler, String type) {
            stats.branches++;
            super.visitTryCatchBlock(start, end, handler, type);
        }

        @Override public void visitMethodInsn(int opcode, String owner, String name,
                                              String desc, boolean itf) {
            // (1) System.currentTimeMillis()/nanoTime() -> our virtual clock
            if (opcode == Opcodes.INVOKESTATIC && owner.equals("java/lang/System")
                    && desc.equals("()J")
                    && (name.equals("currentTimeMillis") || name.equals("nanoTime"))) {
                super.visitMethodInsn(Opcodes.INVOKESTATIC, RT_INTERNAL, name, "()J", false);
                rewrittenClock++;
                return;
            }
            // (4) Math.random() -> our seeded generator
            if (opcode == Opcodes.INVOKESTATIC && owner.equals("java/lang/Math")
                    && name.equals("random") && desc.equals("()D")) {
                super.visitMethodInsn(Opcodes.INVOKESTATIC, RT_INTERNAL, "random", "()D", false);
                rewrittenMathRandom++;
                return;
            }
            // (2)/(3) java.util.Random constructors -> fixed seed
            if (opcode == Opcodes.INVOKESPECIAL && owner.equals("java/util/Random")
                    && name.equals("<init>")) {
                if (desc.equals("()V")) {
                    // new Random(): push FIXED_SEED, call <init>(J)V instead.
                    super.visitLdcInsn(Long.valueOf(FIXED_SEED));
                    super.visitMethodInsn(Opcodes.INVOKESPECIAL, "java/util/Random",
                                          "<init>", "(J)V", false);
                    rewrittenRandomNoArg++;
                    return;
                }
                if (desc.equals("(J)V")) {
                    // new Random(seed): drop the incoming seed, substitute FIXED_SEED.
                    super.visitInsn(Opcodes.POP2);                 // discard original long seed
                    super.visitLdcInsn(Long.valueOf(FIXED_SEED));  // push fixed seed
                    super.visitMethodInsn(Opcodes.INVOKESPECIAL, "java/util/Random",
                                          "<init>", "(J)V", false);
                    rewrittenRandomSeed++;
                    return;
                }
            }
            super.visitMethodInsn(opcode, owner, name, desc, itf);
        }
    }

    // ---- generate the injected runtime class via ASM --------------------------
    //
    // Equivalent Java source (hand-emitted as bytecode so no javac is needed at
    // transform time and the class is JDK-version-neutral for the J2ME runtime):
    //
    //   package arlecchino.det;
    //   public final class Det {
    //       private static long vt = FIXED_ORIGIN_NANOS;
    //       private static final java.util.Random RNG = new java.util.Random(FIXED_SEED);
    //       public static long nanoTime()          { long v = vt; vt += FIXED_STEP_NANOS; return v; }
    //       public static long currentTimeMillis() { long v = vt; vt += FIXED_STEP_NANOS; return v / 1000000L; }
    //       public static double random()          { return RNG.nextDouble(); }
    //   }
    //
    // Target class version 50.0 (Java 6) — comfortably runnable by every emulator JVM.
    byte[] generateRuntimeClass() {
        ClassWriter cw = new ClassWriter(ClassWriter.COMPUTE_MAXS);
        cw.visit(Opcodes.V1_6, Opcodes.ACC_PUBLIC | Opcodes.ACC_FINAL | Opcodes.ACC_SUPER,
                 RT_INTERNAL, null, "java/lang/Object", null);

        cw.visitField(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC, "vt", "J", null, null).visitEnd();
        cw.visitField(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_FINAL,
                      "RNG", "Ljava/util/Random;", null, null).visitEnd();

        // <clinit>: vt = FIXED_ORIGIN_NANOS; RNG = new Random(FIXED_SEED);
        MethodVisitor cl = cw.visitMethod(Opcodes.ACC_STATIC, "<clinit>", "()V", null, null);
        cl.visitCode();
        cl.visitLdcInsn(Long.valueOf(FIXED_ORIGIN_NANOS));
        cl.visitFieldInsn(Opcodes.PUTSTATIC, RT_INTERNAL, "vt", "J");
        cl.visitTypeInsn(Opcodes.NEW, "java/util/Random");
        cl.visitInsn(Opcodes.DUP);
        cl.visitLdcInsn(Long.valueOf(FIXED_SEED));
        cl.visitMethodInsn(Opcodes.INVOKESPECIAL, "java/util/Random", "<init>", "(J)V", false);
        cl.visitFieldInsn(Opcodes.PUTSTATIC, RT_INTERNAL, "RNG", "Ljava/util/Random;");
        cl.visitInsn(Opcodes.RETURN);
        cl.visitMaxs(0, 0);
        cl.visitEnd();

        // private constructor (utility class)
        MethodVisitor ct = cw.visitMethod(Opcodes.ACC_PRIVATE, "<init>", "()V", null, null);
        ct.visitCode();
        ct.visitVarInsn(Opcodes.ALOAD, 0);
        ct.visitMethodInsn(Opcodes.INVOKESPECIAL, "java/lang/Object", "<init>", "()V", false);
        ct.visitInsn(Opcodes.RETURN);
        ct.visitMaxs(0, 0);
        ct.visitEnd();

        // long tick(): long v = vt; vt += STEP; return v;   (shared by both clocks)
        emitTick(cw);
        // long nanoTime() { return tick(); }
        emitClockAccessor(cw, "nanoTime", false);
        // long currentTimeMillis() { return tick() / 1_000_000; }
        emitClockAccessor(cw, "currentTimeMillis", true);

        // double random() { return RNG.nextDouble(); }
        MethodVisitor rnd = cw.visitMethod(Opcodes.ACC_PUBLIC | Opcodes.ACC_STATIC,
                                           "random", "()D", null, null);
        rnd.visitCode();
        rnd.visitFieldInsn(Opcodes.GETSTATIC, RT_INTERNAL, "RNG", "Ljava/util/Random;");
        rnd.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/util/Random", "nextDouble", "()D", false);
        rnd.visitInsn(Opcodes.DRETURN);
        rnd.visitMaxs(0, 0);
        rnd.visitEnd();

        cw.visitEnd();
        return cw.toByteArray();
    }

    /** private static long tick() { long v = vt; vt = vt + STEP; return v; } */
    void emitTick(ClassWriter cw) {
        MethodVisitor mv = cw.visitMethod(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC,
                                          "tick", "()J", null, null);
        mv.visitCode();
        mv.visitFieldInsn(Opcodes.GETSTATIC, RT_INTERNAL, "vt", "J");   // [vt]
        mv.visitVarInsn(Opcodes.LSTORE, 0);                            // v = vt
        mv.visitFieldInsn(Opcodes.GETSTATIC, RT_INTERNAL, "vt", "J");
        mv.visitLdcInsn(Long.valueOf(FIXED_STEP_NANOS));
        mv.visitInsn(Opcodes.LADD);
        mv.visitFieldInsn(Opcodes.PUTSTATIC, RT_INTERNAL, "vt", "J");   // vt += STEP
        mv.visitVarInsn(Opcodes.LLOAD, 0);
        mv.visitInsn(Opcodes.LRETURN);                                 // return v
        mv.visitMaxs(0, 0);
        mv.visitEnd();
    }

    /** public static long <name>() { long v = tick(); return millis ? v/1e6 : v; } */
    void emitClockAccessor(ClassWriter cw, String name, boolean millis) {
        MethodVisitor mv = cw.visitMethod(Opcodes.ACC_PUBLIC | Opcodes.ACC_STATIC,
                                          name, "()J", null, null);
        mv.visitCode();
        mv.visitMethodInsn(Opcodes.INVOKESTATIC, RT_INTERNAL, "tick", "()J", false);
        if (millis) {
            mv.visitLdcInsn(Long.valueOf(1_000_000L));
            mv.visitInsn(Opcodes.LDIV);
        }
        mv.visitInsn(Opcodes.LRETURN);
        mv.visitMaxs(0, 0);
        mv.visitEnd();
    }

    // ---- type hierarchy for COMPUTE_FRAMES ------------------------------------
    //
    // ASM's default getCommonSuperClass does `Class.forName` on the writer's own
    // classloader.  That loader sees neither the MIDlet's classes (they live in the jar
    // we are rewriting, not on our classpath) nor the MIDP API (`javax.microedition.*`),
    // so it blows up with ClassNotFoundException on perfectly ordinary merges.
    //
    // This resolver answers the same question WITHOUT loading anything: it fetches the
    // raw `<name>.class` resource from a loader over [input jar] + [--cp entries] +
    // [our own loader] and reads the super/interface links out of the header.  No static
    // initialiser ever runs, and a J2ME class that could not be linked on a desktop JVM
    // (e.g. one extending javax.microedition.lcdui.Canvas) resolves fine.  Only when a
    // type is genuinely nowhere to be found do we fall back to java/lang/Object — and
    // that is REPORTED, not swallowed, so a missing --cp shows up instead of silently
    // widening the frames.
    static final class Hierarchy {
        private final ClassLoader loader;
        private final Map<String, Node> cache = new HashMap<>();
        private final Set<String> unresolved = new TreeSet<>();

        static final class Node {
            final String superName;        // null for java/lang/Object and interfaces' implicit super
            final String[] interfaces;
            final boolean isInterface;
            Node(String superName, String[] interfaces, boolean isInterface) {
                this.superName = superName;
                this.interfaces = interfaces == null ? new String[0] : interfaces;
                this.isInterface = isInterface;
            }
        }

        Hierarchy(File inputJar, List<File> extra) {
            List<URL> urls = new ArrayList<>();
            addUrl(urls, inputJar);
            for (File f : extra) addUrl(urls, f);
            loader = new URLClassLoader(urls.toArray(new URL[0]),
                                        Hierarchy.class.getClassLoader());
        }

        private static void addUrl(List<URL> urls, File f) {
            try {
                if (f != null && f.exists()) urls.add(f.toURI().toURL());
            } catch (MalformedURLException e) {
                System.err.println("DeterministicClockTransformer: bad classpath entry "
                        + f + " (" + e.getMessage() + ")");
            }
        }

        /** Super/interfaces of an internal name, or null when it cannot be found. */
        private Node node(String internalName) {
            if (cache.containsKey(internalName)) return cache.get(internalName);
            Node n = readFromResource(internalName);
            if (n == null) n = readFromReflection(internalName);
            if (n == null) unresolved.add(internalName);
            cache.put(internalName, n);
            return n;
        }

        /** Preferred path: parse the class header straight out of the .class bytes. */
        private Node readFromResource(String internalName) {
            try (InputStream is = loader.getResourceAsStream(internalName + ".class")) {
                if (is == null) return null;
                ClassReader cr = new ClassReader(is);
                return new Node(cr.getSuperName(), cr.getInterfaces(),
                                (cr.getAccess() & Opcodes.ACC_INTERFACE) != 0);
            } catch (IOException e) {
                return null;
            }
        }

        /** Fallback for JDK types whose .class is not exposed as a resource: reflect on
         *  the class WITHOUT initialising it. */
        private Node readFromReflection(String internalName) {
            try {
                Class<?> c = Class.forName(internalName.replace('/', '.'), false, loader);
                Class<?> sup = c.getSuperclass();
                Class<?>[] ifs = c.getInterfaces();
                String[] names = new String[ifs.length];
                for (int i = 0; i < ifs.length; i++) names[i] = internal(ifs[i]);
                return new Node(sup == null ? null : internal(sup), names, c.isInterface());
            } catch (ClassNotFoundException | LinkageError e) {
                return null;
            }
        }

        private static String internal(Class<?> c) { return c.getName().replace('.', '/'); }

        /** Is `sub` a subtype of `sup` (classes and interfaces alike)? */
        private boolean isAssignable(String sup, String sub) {
            Deque<String> todo = new ArrayDeque<>();
            Set<String> seen = new HashSet<>();
            todo.add(sub);
            while (!todo.isEmpty()) {
                String cur = todo.poll();
                if (cur.equals(sup)) return true;
                if (!seen.add(cur)) continue;
                Node n = node(cur);
                if (n == null) continue;
                if (n.superName != null) todo.add(n.superName);
                Collections.addAll(todo, n.interfaces);
            }
            return false;
        }

        String commonSuperClass(String type1, String type2) {
            if (type1.equals(type2)) return type1;
            final String OBJECT = "java/lang/Object";
            if (type1.equals(OBJECT) || type2.equals(OBJECT)) return OBJECT;
            Node n1 = node(type1), n2 = node(type2);
            if (n1 == null || n2 == null) return OBJECT;   // reported via unresolved
            if (isAssignable(type1, type2)) return type1;
            if (isAssignable(type2, type1)) return type2;
            // Interfaces have no useful common supertype in the verifier's model.
            if (n1.isInterface || n2.isInterface) return OBJECT;
            String cur = type1;
            while (true) {
                Node n = node(cur);
                if (n == null || n.superName == null) return OBJECT;
                cur = n.superName;
                if (isAssignable(cur, type2)) return cur;
            }
        }

        /** Make a missing --cp visible instead of silently widening frames to Object. */
        void reportUnresolved() {
            if (unresolved.isEmpty()) return;
            System.err.println("DeterministicClockTransformer: could not resolve "
                    + unresolved.size() + " type(s) while recomputing frames; they were "
                    + "widened to java/lang/Object. Pass the MIDP API jar via --cp. "
                    + "Unresolved: " + unresolved);
        }
    }
}
