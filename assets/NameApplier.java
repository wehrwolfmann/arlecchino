// NameApplier — applies a fully planned rename table to a jar (Arlecchino ai-rename,
// apply-names).
//
// All the decisions (which names, override groups, locks, conflicts) are made on the
// Rust side; this helper is deliberately mechanical.  It reads a tab-separated table
//
//   class      <old internal name>  <new internal name>
//   method     <owner>  <name>  <descriptor>  <new name>
//   field      <owner>  <name>  <new name>
//   local      <owner>  <method>  <descriptor>  <label>  <new name>
//   labels     on                 (give every unnamed local/parameter its label)
//   classpath  <jar or dir>       (extra types for the local-variable type analysis)
//   labels-out <file>             (write the label table there)
//
// where every (owner, member) pair through which a renamed member is REFERENCED is
// listed explicitly, and rewrites every class with ASM's ClassRemapper +
// SimpleRemapper: declarations, references, descriptors, signatures, inner-class
// attributes and frames.  Class entries are written under their new names; other
// entries are copied unchanged, except signature files (META-INF/*.SF|*.RSA|*.DSA|
// *.EC), which a rewrite would invalidate anyway.
//
// Local variables and parameters.  Obfuscated jars carry no LocalVariableTable, so
// decompilers invent `var4`.  For a method without one, the helper runs ASM's data-flow
// analyser, splits every local slot into stretches of one value type, and LABELS them
// deterministically: `v<slot>` for the first type a slot holds, `v<slot>b`, `v<slot>c`
// … for the others (`this` is never labelled).  A `local` line names one label; with
// `labels on` the unnamed ones keep the label itself.  The names go into a synthesized
// LocalVariableTable, which every engine reads, so the decompiled source shows them.
// Instructions are never changed.  A method whose analysis fails keeps no table.
//
// usage: NameApplier <in.jar> <out.jar> <table.tsv>
//
// ASM (org.ow2.asm, BSD-3-Clause) is invoked, not vendored. This file is original.

import java.io.*;
import java.net.URL;
import java.net.URLClassLoader;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.zip.*;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.Type;
import org.objectweb.asm.commons.ClassRemapper;
import org.objectweb.asm.commons.SimpleRemapper;
import org.objectweb.asm.tree.*;
import org.objectweb.asm.tree.analysis.*;

public final class NameApplier {
    /** (owner.name+desc) -> (label -> new name) */
    static final Map<String, Map<String, String>> LOCALS = new HashMap<>();
    static boolean labelAll = false;
    static final List<String> CLASSPATH = new ArrayList<>();
    static String labelsOut = null;
    static final StringBuilder LABELS = new StringBuilder();
    static int namedLocals = 0, labelledLocals = 0, unknownLabels = 0, failedMethods = 0;

    public static void main(String[] args) throws Exception {
        if (args.length != 3) {
            System.err.println("usage: NameApplier <in.jar> <out.jar> <table.tsv>");
            System.exit(2);
        }
        Map<String, String> map = new HashMap<>();
        int nc = 0, nm = 0, nf = 0;
        try (BufferedReader br = new BufferedReader(new InputStreamReader(
                new FileInputStream(args[2]), StandardCharsets.UTF_8))) {
            String line;
            while ((line = br.readLine()) != null) {
                if (line.isEmpty() || line.startsWith("#")) continue;
                String[] p = line.split("\t");
                switch (p[0]) {
                    case "class":  map.put(p[1], p[2]); nc++; break;
                    case "method": map.put(p[1] + "." + p[2] + p[3], p[4]); nm++; break;
                    case "field":  map.put(p[1] + "." + p[2], p[3]); nf++; break;
                    case "local":
                        LOCALS.computeIfAbsent(p[1] + "." + p[2] + p[3], k -> new HashMap<>()).put(p[4], p[5]);
                        break;
                    case "labels": labelAll = p.length > 1 && p[1].equals("on"); break;
                    case "classpath": CLASSPATH.add(p[1]); break;
                    case "labels-out": labelsOut = p[1]; break;
                    default: throw new IOException("bad table line: " + line);
                }
            }
        }
        // pass 1: read everything (the type analysis needs the jar's own hierarchy)
        List<String> names = new ArrayList<>();
        List<byte[]> datas = new ArrayList<>();
        List<Long> times = new ArrayList<>();
        try (ZipInputStream in = new ZipInputStream(new BufferedInputStream(new FileInputStream(args[0])))) {
            ZipEntry e;
            while ((e = in.getNextEntry()) != null) {
                if (e.isDirectory()) continue;
                names.add(e.getName());
                datas.add(in.readAllBytes());
                times.add(e.getTime());
            }
        }
        Hierarchy hier = new Hierarchy();
        for (int i = 0; i < names.size(); i++) {
            String n = names.get(i);
            if (n.endsWith(".class") && !n.toUpperCase(Locale.ROOT).startsWith("META-INF/")) hier.add(datas.get(i));
        }
        hier.loader = new URLClassLoader(CLASSPATH.stream().map(NameApplier::url).toArray(URL[]::new), null);

        SimpleRemapper remapper = new SimpleRemapper(map);
        int classes = 0;
        boolean localsWanted = labelAll || !LOCALS.isEmpty();
        try (ZipOutputStream out = new ZipOutputStream(new BufferedOutputStream(new FileOutputStream(args[1])))) {
            Set<String> written = new HashSet<>();
            for (int i = 0; i < names.size(); i++) {
                String name = names.get(i);
                byte[] data = datas.get(i);
                String upper = name.toUpperCase(Locale.ROOT);
                if (upper.startsWith("META-INF/") && (upper.endsWith(".SF") || upper.endsWith(".RSA")
                        || upper.endsWith(".DSA") || upper.endsWith(".EC"))) continue;
                if (name.endsWith(".class") && !upper.startsWith("META-INF/")) {
                    ClassReader cr = new ClassReader(data);
                    ClassWriter cw = new ClassWriter(0);
                    if (localsWanted) {
                        ClassNode cn = new ClassNode();
                        cr.accept(cn, 0);
                        nameLocals(cn, hier);
                        cn.accept(new ClassRemapper(cw, remapper));
                    } else {
                        cr.accept(new ClassRemapper(cw, remapper), 0);
                    }
                    data = cw.toByteArray();
                    name = remapper.map(cr.getClassName()) != null
                            ? remapper.map(cr.getClassName()) + ".class" : name;
                    classes++;
                }
                if (!written.add(name)) throw new IOException("duplicate entry after rename: " + name);
                ZipEntry oe = new ZipEntry(name);
                if (times.get(i) != -1) oe.setTime(times.get(i));
                out.putNextEntry(oe);
                out.write(data);
                out.closeEntry();
            }
        }
        if (labelsOut != null) {
            try (Writer w = new OutputStreamWriter(new FileOutputStream(labelsOut), StandardCharsets.UTF_8)) {
                w.write("# owner\tmethod\tdescriptor\tlabel\ttype\tparameter\n");
                w.write(LABELS.toString());
            }
        }
        System.out.println("rewrote " + classes + " classes (table: " + nc + " classes, "
                + nm + " method refs, " + nf + " field refs; locals: " + namedLocals + " named, "
                + labelledLocals + " labelled, " + unknownLabels + " unknown labels, "
                + failedMethods + " methods not analysed)");
    }

    static URL url(String p) {
        try {
            return new File(p).toURI().toURL();
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    // ── local variables ──────────────────────────────────────────────────────────

    static final class Seg {
        final int slot, start;
        int end;
        Type type; // null while only `null` was stored
        String exact; // declared type of a parameter (the analyser widens booleans to int)
        String desc() { return exact != null ? exact : type.getDescriptor(); }
        Seg(int slot, int start, Type type) { this.slot = slot; this.start = start; this.type = type; }
    }

    static Type typeOf(BasicValue v) {
        if (v == null || v == BasicValue.UNINITIALIZED_VALUE || v.getType() == null) return null;
        Type t = v.getType();
        if (t.getSort() == Type.METHOD) return null;
        return t;
    }

    static boolean isNullType(Type t) {
        return t != null && t.getSort() == Type.OBJECT && t.getInternalName().equals("null");
    }

    static void nameLocals(ClassNode cn, Hierarchy hier) {
        for (MethodNode mn : cn.methods) {
            if (mn.instructions.size() == 0) continue;
            if (mn.localVariables != null && !mn.localVariables.isEmpty()) continue; // real debug info
            String key = cn.name + "." + mn.name + mn.desc;
            Map<String, String> wanted = LOCALS.getOrDefault(key, Collections.emptyMap());
            if (!labelAll && wanted.isEmpty()) continue;
            Frame<BasicValue>[] frames;
            FlowAnalyzer a = new FlowAnalyzer(new HierVerifier(hier, cn), mn.instructions.size());
            try {
                frames = a.analyze(cn.name, mn);
            } catch (Throwable t) {
                failedMethods++;
                System.err.println("warning: " + key + ": not analysed (" + t.getMessage() + ")");
                continue;
            }
            boolean isStatic = (mn.access & Opcodes.ACC_STATIC) != 0;
            int argSlots = isStatic ? 0 : 1;
            Map<Integer, Type> argType = new HashMap<>();
            for (Type at : Type.getArgumentTypes(mn.desc)) {
                argType.put(argSlots, at);
                argSlots += at.getSize();
            }
            AbstractInsnNode[] insns = mn.instructions.toArray();
            int n = insns.length;
            BitSet[] live = liveness(insns, a.succ, mn.maxLocals);
            List<Seg> segs = new ArrayList<>();
            for (int slot = isStatic ? 0 : 1; slot < mn.maxLocals; slot++) {
                Seg cur = null;
                // a parameter is a variable from the start until it is first overwritten,
                // read or not; anything else only where its value is still to be read
                boolean paramAlive = slot < argSlots;
                for (int i = 0; i < n; i++) {
                    Frame<BasicValue> f = frames[i];
                    Type t = f == null ? null : typeOf(f.getLocal(slot));
                    if (paramAlive && i > 0 && storesTo(insns[i - 1], slot)) {
                        // the parameter ends where it is first overwritten
                        paramAlive = false;
                        if (cur != null) {
                            cur.end = i;
                            if (cur.type != null) segs.add(cur);
                            cur = null;
                        }
                    }
                    if (t != null && !paramAlive && !live[i].get(slot)) t = null;
                    if (isNullType(t)) {
                        if (cur == null) { cur = new Seg(slot, i, null); }
                        continue; // a stored null continues whatever the slot holds
                    }
                    if (cur != null && (t == null || (cur.type != null && !t.equals(cur.type)))) {
                        cur.end = i;
                        if (cur.type != null) segs.add(cur);
                        cur = null;
                    }
                    if (t != null) {
                        if (cur == null) {
                            cur = new Seg(slot, i, t);
                            if (i == 0 && argType.containsKey(slot)) cur.exact = argType.get(slot).getDescriptor();
                        }
                        else if (cur.type == null) cur.type = t;
                    }
                }
                if (cur != null) {
                    cur.end = n;
                    if (cur.type != null) segs.add(cur);
                }
            }
            if (segs.isEmpty()) continue;
            // labels: per slot, distinct types in order of first appearance
            Map<Integer, List<String>> typesBySlot = new TreeMap<>();
            for (Seg s : segs) {
                List<String> l = typesBySlot.computeIfAbsent(s.slot, k -> new ArrayList<>());
                if (!l.contains(s.desc())) l.add(s.desc());
            }
            Set<String> used = new HashSet<>();
            Map<String, LabelNode> at = new HashMap<>();
            LabelNode endLabel = null;
            List<LocalVariableNode> table = new ArrayList<>();
            for (Seg s : segs) {
                int k = typesBySlot.get(s.slot).indexOf(s.desc());
                String label = "v" + s.slot + (k == 0 ? "" : String.valueOf((char) ('a' + k)));
                String name = wanted.get(label);
                if (name == null && !labelAll) continue;
                boolean param = s.slot < argSlots && k == 0 && s.start == 0;
                // the analyser widens boolean/byte/char/short to int; a parameter's
                // declared type is known exactly
                String desc = s.desc();
                if (used.add(label)) {
                    LABELS.append(cn.name).append('\t').append(mn.name).append('\t').append(mn.desc)
                            .append('\t').append(label).append('\t').append(desc)
                            .append('\t').append(param ? "1" : "0").append('\n');
                    if (name != null) namedLocals++; else labelledLocals++;
                }
                LabelNode start = labelAt(mn, insns, s.start, at);
                LabelNode end;
                if (s.end >= n) {
                    if (endLabel == null) { endLabel = new LabelNode(); mn.instructions.add(endLabel); }
                    end = endLabel;
                } else {
                    end = labelAt(mn, insns, s.end, at);
                }
                table.add(new LocalVariableNode(name != null ? name : label, desc, null, start, end, s.slot));
            }
            for (String l : wanted.keySet()) {
                if (!used.contains(l)) {
                    unknownLabels++;
                    System.err.println("warning: " + key + ": no local labelled " + l);
                }
            }
            if (!table.isEmpty()) mn.localVariables = table;
        }
    }

    /** Records the control-flow successors the analyser discovers. */
    static final class FlowAnalyzer extends Analyzer<BasicValue> {
        final List<List<Integer>> succ = new ArrayList<>();

        FlowAnalyzer(Interpreter<BasicValue> in, int n) {
            super(in);
            for (int i = 0; i < n; i++) succ.add(new ArrayList<>());
        }

        @Override
        protected void newControlFlowEdge(int insn, int successor) {
            succ.get(insn).add(successor);
        }

        @Override
        protected boolean newControlFlowExceptionEdge(int insn, int successor) {
            succ.get(insn).add(successor);
            return true;
        }
    }

    static boolean storesTo(AbstractInsnNode in, int slot) {
        if (in instanceof VarInsnNode) {
            int op = in.getOpcode();
            return op >= Opcodes.ISTORE && op <= Opcodes.ASTORE && ((VarInsnNode) in).var == slot;
        }
        return false;
    }

    /** live[i] = slots whose value at instruction i is read later on some path. */
    static BitSet[] liveness(AbstractInsnNode[] insns, List<List<Integer>> succ, int maxLocals) {
        int n = insns.length;
        BitSet[] use = new BitSet[n], def = new BitSet[n], live = new BitSet[n];
        for (int i = 0; i < n; i++) {
            use[i] = new BitSet(maxLocals);
            def[i] = new BitSet(maxLocals);
            live[i] = new BitSet(maxLocals);
            AbstractInsnNode in = insns[i];
            int op = in.getOpcode();
            if (in instanceof VarInsnNode) {
                int v = ((VarInsnNode) in).var;
                if (op >= Opcodes.ILOAD && op <= Opcodes.ALOAD || op == Opcodes.RET) use[i].set(v);
                else def[i].set(v);
            } else if (in instanceof IincInsnNode) {
                use[i].set(((IincInsnNode) in).var);
            }
        }
        boolean changed = true;
        while (changed) {
            changed = false;
            for (int i = n - 1; i >= 0; i--) {
                BitSet out = new BitSet(maxLocals);
                for (int s : succ.get(i)) out.or(live[s]);
                out.andNot(def[i]);
                out.or(use[i]);
                if (!out.equals(live[i])) {
                    live[i] = out;
                    changed = true;
                }
            }
        }
        return live;
    }

    static LabelNode labelAt(MethodNode mn, AbstractInsnNode[] insns, int i, Map<String, LabelNode> cache) {
        String k = String.valueOf(i);
        LabelNode l = cache.get(k);
        if (l != null) return l;
        if (insns[i] instanceof LabelNode) {
            l = (LabelNode) insns[i];
        } else {
            l = new LabelNode();
            mn.instructions.insertBefore(insns[i], l);
        }
        cache.put(k, l);
        return l;
    }

    // ── a class hierarchy without loading classes ────────────────────────────────

    static final class Info {
        String superName;
        String[] interfaces;
        boolean isInterface;
    }

    static final class Hierarchy {
        final Map<String, Info> known = new HashMap<>();
        final Set<String> missing = new HashSet<>();
        ClassLoader loader;

        void add(byte[] data) {
            ClassReader cr = new ClassReader(data);
            Info i = new Info();
            i.superName = cr.getSuperName();
            i.interfaces = cr.getInterfaces();
            i.isInterface = (cr.getAccess() & Opcodes.ACC_INTERFACE) != 0;
            known.put(cr.getClassName(), i);
        }

        Info get(String name) {
            Info i = known.get(name);
            if (i != null || missing.contains(name)) return i;
            byte[] data = null;
            String res = name + ".class";
            try (InputStream in = open(res)) {
                if (in != null) data = in.readAllBytes();
            } catch (IOException e) {
                data = null;
            }
            if (data == null) {
                missing.add(name);
                return null;
            }
            add(data);
            return known.get(name);
        }

        InputStream open(String res) {
            InputStream in = loader == null ? null : loader.getResourceAsStream(res);
            if (in == null) in = ClassLoader.getSystemResourceAsStream(res);
            return in;
        }

        /** Is `sub` a subtype of `sup`?  Unknown links answer yes (lenient, like the verifier). */
        boolean subtype(String sub, String sup) {
            if (sub.equals(sup) || sup.equals("java/lang/Object")) return true;
            Deque<String> work = new ArrayDeque<>();
            Set<String> seen = new HashSet<>();
            work.add(sub);
            while (!work.isEmpty()) {
                String c = work.poll();
                if (!seen.add(c)) continue;
                if (c.equals(sup)) return true;
                Info i = get(c);
                if (i == null) return true;
                if (i.superName != null) work.add(i.superName);
                if (i.interfaces != null) work.addAll(Arrays.asList(i.interfaces));
            }
            return false;
        }
    }

    static final class HierVerifier extends SimpleVerifier {
        final Hierarchy h;

        HierVerifier(Hierarchy h, ClassNode cn) {
            super(Opcodes.ASM9, Type.getObjectType(cn.name),
                    cn.superName == null ? null : Type.getObjectType(cn.superName),
                    ifaces(cn), (cn.access & Opcodes.ACC_INTERFACE) != 0);
            this.h = h;
        }

        static List<Type> ifaces(ClassNode cn) {
            List<Type> l = new ArrayList<>();
            for (String s : cn.interfaces) l.add(Type.getObjectType(s));
            return l;
        }

        @Override
        protected boolean isInterface(Type t) {
            if (t.getSort() != Type.OBJECT) return false;
            Info i = h.get(t.getInternalName());
            return i != null && i.isInterface;
        }

        @Override
        protected Type getSuperClass(Type t) {
            if (t.getSort() != Type.OBJECT) return Type.getObjectType("java/lang/Object");
            Info i = h.get(t.getInternalName());
            if (i == null || i.superName == null) {
                return t.getInternalName().equals("java/lang/Object") ? null : Type.getObjectType("java/lang/Object");
            }
            return Type.getObjectType(i.superName);
        }

        @Override
        protected boolean isAssignableFrom(Type t, Type u) {
            if (t.equals(u)) return true;
            if (u.getSort() == Type.OBJECT && u.getInternalName().equals("null")) {
                return t.getSort() == Type.OBJECT || t.getSort() == Type.ARRAY;
            }
            if (t.getSort() == Type.OBJECT && t.getInternalName().equals("null")) return false;
            if (t.getSort() == Type.ARRAY) {
                if (u.getSort() != Type.ARRAY) return false;
                Type te = Type.getType(t.getDescriptor().substring(1));
                Type ue = Type.getType(u.getDescriptor().substring(1));
                if ((te.getSort() == Type.OBJECT || te.getSort() == Type.ARRAY)
                        && (ue.getSort() == Type.OBJECT || ue.getSort() == Type.ARRAY)) {
                    return isAssignableFrom(te, ue);
                }
                return te.equals(ue);
            }
            if (t.getSort() != Type.OBJECT) return false;
            String tn = t.getInternalName();
            if (u.getSort() == Type.ARRAY) {
                return tn.equals("java/lang/Object") || tn.equals("java/lang/Cloneable")
                        || tn.equals("java/io/Serializable");
            }
            if (u.getSort() != Type.OBJECT) return false;
            return h.subtype(u.getInternalName(), tn);
        }
    }
}
