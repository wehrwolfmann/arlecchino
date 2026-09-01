// TwinObf — ASM post-processor that adds a colliding RETURN-TYPE-ONLY twin next to a
// named method, forcing the renamer to consider renaming the genuine override.
//
// Nothing is vendored: ASM (org.ow2.asm, BSD-3-Clause) is INVOKED against the jars
// already present under ~/.arlecchino/engines/asm — the same jars MemberRenamer is
// built against.
//
// Usage:  java -cp <asm jars>:. TwinObf <classesDir> <out.jar>
import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.jar.*;

import org.objectweb.asm.*;
import org.objectweb.asm.tree.*;

public final class TwinObf {
    public static void main(String[] a) throws Exception {
        Path dir = Paths.get(a[0]);
        Path outJar = Paths.get(a[1]);
        Map<String, byte[]> out = new TreeMap<>();
        try (var st = Files.walk(dir)) {
            for (Path p : (Iterable<Path>) st.filter(x -> x.toString().endsWith(".class"))::iterator) {
                String in = dir.relativize(p).toString().replace(File.separatorChar, '/');
                in = in.substring(0, in.length() - 6);
                out.put(in, transform(in, Files.readAllBytes(p)));
            }
        }
        Files.createDirectories(outJar.toAbsolutePath().getParent());
        Manifest mf = new Manifest();
        mf.getMainAttributes().put(Attributes.Name.MANIFEST_VERSION, "1.0");
        try (JarOutputStream jos = new JarOutputStream(new BufferedOutputStream(
                Files.newOutputStream(outJar)), mf)) {
            for (Map.Entry<String, byte[]> e : out.entrySet()) {
                jos.putNextEntry(new JarEntry(e.getKey() + ".class"));
                jos.write(e.getValue());
                jos.closeEntry();
            }
        }
        System.out.println("TwinObf: wrote " + out.size() + " class(es) -> " + outJar);
    }

    static byte[] transform(String in, byte[] data) {
        ClassReader cr = new ClassReader(data);
        ClassNode cn = new ClassNode();
        cr.accept(cn, 0);
        if (in.equals("Listener")) {
            twin(cn, "itemStateChanged", "(Ljavax/microedition/lcdui/Item;)V",
                 "(Ljavax/microedition/lcdui/Item;)I");
        } else if (in.equals("Widget")) {
            twin(cn, "keyPressed", "(I)V", "(I)I");
        }
        ClassWriter cw = new ClassWriter(0);
        cn.accept(cw);
        return cw.toByteArray();
    }

    static void twin(ClassNode cn, String name, String from, String to) {
        MethodNode t = null;
        for (MethodNode mn : cn.methods) {
            if (mn.name.equals(name) && mn.desc.equals(from)) { t = mn; break; }
        }
        if (t == null) throw new IllegalStateException("no " + name + from);
        MethodNode w = new MethodNode(t.access, name, to, null, null);
        String ret = to.substring(to.indexOf(')') + 1);
        InsnList c = new InsnList();
        if (ret.equals("I")) {
            c.add(new InsnNode(Opcodes.ICONST_0));
            c.add(new InsnNode(Opcodes.IRETURN));
        } else {
            c.add(new InsnNode(Opcodes.RETURN));
        }
        w.instructions = c;
        w.maxStack = 2;
        w.maxLocals = 2;
        cn.methods.add(w);
    }
}
