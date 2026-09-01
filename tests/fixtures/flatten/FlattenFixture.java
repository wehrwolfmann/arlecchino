// FlattenFixture — builds the Stage Э9 cross-package test jar.
//
// It compiles nothing itself; it takes two already-compiled DEFAULT-package classes
// (`App` and `Lib`, both legal Java referencing each other in the default package) and
// RE-PARENTS `App` into a NAMED package `Container` via ASM, WITHOUT touching its
// references to the default-package type `Lib`.  The result mirrors the irreducible
// Allods situation: a named-package class (`Container.App`) that references a
// default-package type (`Lib`) — uncompilable in Java source (JLS §6.5/§7.4.3), because
// there is no syntax for a named-package class to name an unnamed-package type.
//
// It also writes a manifest whose `MIDlet-1` entry names the re-parented class
// (`Container.App`), so the flatten pass's manifest rewrite is exercised.
//
// ASM (org.ow2.asm, BSD-3-Clause) is INVOKED, not vendored — same jars as MemberRenamer.
//
// Usage:  java -cp <asm jars>:. FlattenFixture <classesDir> <out.jar>
import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.jar.*;

import org.objectweb.asm.*;
import org.objectweb.asm.commons.ClassRemapper;
import org.objectweb.asm.commons.Remapper;

public final class FlattenFixture {

    public static void main(String[] args) throws Exception {
        if (args.length < 2) {
            System.err.println("usage: FlattenFixture <classesDir> <out.jar>");
            System.exit(2);
        }
        Path classesDir = Paths.get(args[0]);
        Path outJar = Paths.get(args[1]);

        // Remap ONLY the App type -> Container/App; Lib stays in the default package.
        final Map<String, String> typeMap = new HashMap<>();
        typeMap.put("App", "Container/App");
        Remapper remapper = new Remapper() {
            @Override public String map(String internalName) {
                return typeMap.getOrDefault(internalName, internalName);
            }
        };

        Map<String, byte[]> out = new TreeMap<>();
        try (var stream = Files.walk(classesDir)) {
            for (Path p : (Iterable<Path>) stream
                    .filter(x -> x.toString().endsWith(".class"))::iterator) {
                String internal = classesDir.relativize(p).toString()
                        .replace(File.separatorChar, '/');
                internal = internal.substring(0, internal.length() - ".class".length());
                byte[] data = Files.readAllBytes(p);
                ClassReader cr = new ClassReader(data);
                ClassWriter cw = new ClassWriter(0);
                cr.accept(new ClassRemapper(cw, remapper), 0);
                String outName = typeMap.getOrDefault(internal, internal);
                out.put(outName, cw.toByteArray());
            }
        }

        Manifest mf = new Manifest();
        mf.getMainAttributes().put(Attributes.Name.MANIFEST_VERSION, "1.0");
        // J2ME entry attribute naming the re-parented class (dotted FQN).
        mf.getMainAttributes().putValue("MIDlet-1", "Demo,/icon.png,Container.App");

        Files.createDirectories(outJar.toAbsolutePath().getParent());
        try (JarOutputStream jos = new JarOutputStream(
                new BufferedOutputStream(Files.newOutputStream(outJar)), mf)) {
            for (Map.Entry<String, byte[]> e : out.entrySet()) {
                jos.putNextEntry(new JarEntry(e.getKey() + ".class"));
                jos.write(e.getValue());
                jos.closeEntry();
            }
        }
        System.out.println("FlattenFixture: wrote " + out.size() + " classes to " + outJar);
    }
}
