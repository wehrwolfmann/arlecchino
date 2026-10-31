// NameApplier — applies a fully planned rename table to a jar (Arlecchino ai-rename).
//
// All the decisions (which names, override groups, locks, conflicts) are made on the
// Rust side; this helper is deliberately mechanical.  It reads a tab-separated table
//
//   class   <old internal name>  <new internal name>
//   method  <owner>  <name>  <descriptor>  <new name>
//   field   <owner>  <name>  <new name>
//
// where every (owner, member) pair through which a renamed member is REFERENCED is
// listed explicitly, and rewrites every class with ASM's ClassRemapper +
// SimpleRemapper: declarations, references, descriptors, signatures, inner-class
// attributes and frames.  Class entries are written under their new names; other
// entries are copied unchanged, except signature files (META-INF/*.SF|*.RSA|*.DSA|
// *.EC), which a rewrite would invalidate anyway.
//
// usage: NameApplier <in.jar> <out.jar> <table.tsv>
//
// ASM (org.ow2.asm, BSD-3-Clause) is invoked, not vendored. This file is original.

import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.zip.*;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.commons.ClassRemapper;
import org.objectweb.asm.commons.SimpleRemapper;

public final class NameApplier {
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
                    default: throw new IOException("bad table line: " + line);
                }
            }
        }
        SimpleRemapper remapper = new SimpleRemapper(map);
        int classes = 0;
        try (ZipInputStream in = new ZipInputStream(new BufferedInputStream(new FileInputStream(args[0])));
             ZipOutputStream out = new ZipOutputStream(new BufferedOutputStream(new FileOutputStream(args[1])))) {
            Set<String> written = new HashSet<>();
            ZipEntry e;
            while ((e = in.getNextEntry()) != null) {
                String name = e.getName();
                byte[] data = in.readAllBytes();
                String upper = name.toUpperCase(Locale.ROOT);
                if (upper.startsWith("META-INF/") && (upper.endsWith(".SF") || upper.endsWith(".RSA")
                        || upper.endsWith(".DSA") || upper.endsWith(".EC"))) continue;
                if (name.endsWith(".class") && !upper.startsWith("META-INF/")) {
                    ClassReader cr = new ClassReader(data);
                    ClassWriter cw = new ClassWriter(0);
                    cr.accept(new ClassRemapper(cw, remapper), 0);
                    data = cw.toByteArray();
                    name = remapper.map(cr.getClassName()) != null
                            ? remapper.map(cr.getClassName()) + ".class" : name;
                    classes++;
                }
                if (!written.add(name)) throw new IOException("duplicate entry after rename: " + name);
                ZipEntry oe = new ZipEntry(name);
                if (e.getTime() != -1) oe.setTime(e.getTime());
                out.putNextEntry(oe);
                out.write(data);
                out.closeEntry();
            }
        }
        System.out.println("rewrote " + classes + " classes (table: " + nc + " classes, "
                + nm + " method refs, " + nf + " field refs)");
    }
}
