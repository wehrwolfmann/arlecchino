// SynthObfuscator — ASM post-processor that injects source-ILLEGAL member collisions
// into already-compiled .class files, producing a synthetic "obfuscated" jar for the
// Arlecchino Stage Э1 test fixture.
//
// Nothing is downloaded or vendored: ASM (org.ow2.asm, BSD-3-Clause) is INVOKED against
// the jars already present under ~/.arlecchino/engines/asm (same jars MemberRenamer is
// built against).  This mirrors how arlecchino/java/MemberRenamer.java is compiled+run.
//
// Usage:  java -cp <asm jars>:. SynthObfuscator <classesDir> <out.jar>
//
// Injected collisions (the renamer must resolve every one):
//   (a) duplicate field name, different descriptor   — Engine: add field `a` of type I
//   (b) return-type-only method overload             — Engine: add `flag(I)I` next to
//                                                       `flag(I)Z`
//   (e) MIDP callback override + colliding twin       — Screen: re-parent to Canvas, and
//                                                       add `keyPressed(I)I` next to the
//                                                       genuine callback `keyPressed(I)V`
// (c) type-shadow and (d) const-interface-shadow are already source-legal in the base
// sources (a field named like an in-jar type; an instance field shadowing an inherited
// constant) and need no bytecode surgery — the renamer handles them from the clean jar.
import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.jar.*;

import org.objectweb.asm.*;
import org.objectweb.asm.tree.*;

public final class SynthObfuscator {

    public static void main(String[] args) throws Exception {
        if (args.length < 2) {
            System.err.println("usage: SynthObfuscator <classesDir> <out.jar>");
            System.exit(2);
        }
        Path classesDir = Paths.get(args[0]);
        Path outJar = Paths.get(args[1]);

        Map<String, byte[]> out = new TreeMap<>();
        try (var stream = Files.walk(classesDir)) {
            for (Path p : (Iterable<Path>) stream.filter(x -> x.toString().endsWith(".class"))::iterator) {
                String internal = classesDir.relativize(p).toString()
                        .replace(File.separatorChar, '/');
                internal = internal.substring(0, internal.length() - ".class".length());
                byte[] data = Files.readAllBytes(p);
                out.put(internal, transform(internal, data));
            }
        }

        Files.createDirectories(outJar.toAbsolutePath().getParent());
        Manifest mf = new Manifest();
        mf.getMainAttributes().put(Attributes.Name.MANIFEST_VERSION, "1.0");
        try (JarOutputStream jos = new JarOutputStream(
                new BufferedOutputStream(Files.newOutputStream(outJar)), mf)) {
            for (Map.Entry<String, byte[]> e : out.entrySet()) {
                jos.putNextEntry(new JarEntry(e.getKey() + ".class"));
                jos.write(e.getValue());
                jos.closeEntry();
            }
        }
        System.out.println("SynthObfuscator: wrote " + out.size() + " class(es) -> " + outJar);
    }

    static byte[] transform(String internal, byte[] data) {
        ClassReader cr = new ClassReader(data);
        ClassNode cn = new ClassNode();
        cr.accept(cn, 0);

        if (internal.equals("Engine")) {
            injectDupField(cn);
            injectReturnTypeOverload(cn, "flag", "(I)Z", "(I)I");
        } else if (internal.equals("Screen")) {
            // (e) re-parent to a MIDP Canvas so keyPressed(I)V is a genuine callback...
            cn.superName = "javax/microedition/lcdui/Canvas";
            // ...and add a colliding return-type-only twin that the renamer MUST rename
            // (the callback itself must stay locked).
            injectReturnTypeOverload(cn, "keyPressed", "(I)V", "(I)I");
        }

        ClassWriter cw = new ClassWriter(0);   // no frames/maxs recompute needed
        cn.accept(cw);
        return cw.toByteArray();
    }

    /** (a) Add a second field named like an existing one but with a different descriptor. */
    static void injectDupField(ClassNode cn) {
        // existing `a` is Ljava/lang/String; — add an `a` of type I (int).
        cn.fields.add(new FieldNode(Opcodes.ACC_PRIVATE, "a", "I", null, null));
    }

    /** (b)/(e) Clone a method, keeping name+params but changing the return type, with a
     *  trivial valid body for the new descriptor (so the bytecode is well-formed). */
    static void injectReturnTypeOverload(ClassNode cn, String name,
                                         String fromDesc, String toDesc) {
        MethodNode tmpl = null;
        for (MethodNode mn : cn.methods) {
            if (mn.name.equals(name) && mn.desc.equals(fromDesc)) { tmpl = mn; break; }
        }
        if (tmpl == null) throw new IllegalStateException(
                "template method not found: " + name + fromDesc + " in " + cn.name);

        MethodNode twin = new MethodNode(tmpl.access, name, toDesc, null, null);
        String ret = toDesc.substring(toDesc.indexOf(')') + 1);
        InsnList code = new InsnList();
        // minimal valid body returning a default value of the new return type
        switch (ret) {
            case "I": case "Z": case "B": case "C": case "S":
                code.add(new InsnNode(Opcodes.ICONST_0));
                code.add(new InsnNode(Opcodes.IRETURN));
                break;
            case "J":
                code.add(new InsnNode(Opcodes.LCONST_0));
                code.add(new InsnNode(Opcodes.LRETURN));
                break;
            case "V":
                code.add(new InsnNode(Opcodes.RETURN));
                break;
            default:   // reference type
                code.add(new InsnNode(Opcodes.ACONST_NULL));
                code.add(new InsnNode(Opcodes.ARETURN));
        }
        twin.instructions = code;
        twin.maxStack = 2;
        twin.maxLocals = 2;   // this + int arg
        cn.methods.add(twin);
    }
}
