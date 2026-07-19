// MemberRenamer — bytecode-first canonical member renamer for Arlecchino.
//
// PURPOSE
// -------
// ProGuard-style obfuscation reuses the SAME member name with DIFFERENT descriptors,
// which is legal in JVM bytecode (members are keyed by name+descriptor) but ILLEGAL
// in Java source. That makes decompiler output uncompilable. This tool rewrites a jar
// so every field/method has a Java-source-LEGAL, unique name, while preserving virtual
// dispatch and never touching members bound by an external (not-in-jar) contract
// (java/*, javax/microedition/* MIDP callbacks, etc.). The renamed jar can then be fed
// to the normal multi-decompiler pipeline and the output compiles.
//
// It rewrites ALL references (including splitting shared NameAndType constant-pool
// entries) via ASM's ClassRemapper + a custom Remapper.
//
// COLLISIONS HANDLED (and ONLY these — minimal churn):
//   (a) FIELDS: two+ fields in the SAME class with the SAME name (Java forbids
//       duplicate field names regardless of type). Keep one deterministically; rename
//       the rest.
//   (b) METHODS: two+ methods in the SAME class with the same name AND same PARAMETER
//       types but a different RETURN type (return-type-only overload — illegal in Java).
//       Keep one deterministically; disambiguate the rest. (Normal overloads — same
//       name, different params — are legal Java and are left alone.)
//   <init>/<clinit> are never renamed.
//
// HIERARCHY-AWARE METHOD GROUPING
//   Methods linked by override (same name+FULL descriptor across super/sub/interface,
//   for in-jar types) are unioned into one virtual group and renamed as a unit, so
//   dispatch is preserved.
//
// EXTERNAL-OVERRIDE LOCKING (critical)
//   If a method's name+descriptor matches a method declared in any supertype/interface
//   NOT present in the jar (java/*, javax/microedition/*, etc.), the whole virtual
//   group is LOCKED and never renamed (run(), paint(Graphics), keyPressed(int),
//   startApp()...). If a locked method collides with another method in the same class,
//   the OTHER (unlocked) collider is renamed instead. Fields are class-local in Java so
//   they are never externally locked.
//
// NEW-NAME SCHEME (deterministic, minimal churn, collision-free)
//   The first occurrence (by declaration order, then descriptor) keeps its original
//   name. Each further collider gets:  <orig> + "_" + tag(descriptor)
//   where tag = first 8 hex chars of a stable hash of the *full descriptor* (for
//   methods: the method descriptor "(params)ret"; for fields: the field type
//   descriptor), with non-identifier chars already excluded by hashing. If that name is
//   somehow still taken in the class/group, a numeric "_2", "_3"... suffix is appended
//   until unique. Result is always a valid Java identifier and unique within its class.
//
// OUTPUT
//   <out.jar> renamed jar; <mapping.txt> with lines "owner.oldName desc -> newName".
//
// ASM (org.ow2.asm, BSD-3-Clause) is invoked, not vendored. Logic here is original.

import java.io.*;
import java.net.URL;
import java.net.URLClassLoader;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.jar.*;
import java.util.zip.*;

import org.objectweb.asm.*;
import org.objectweb.asm.tree.*;
import org.objectweb.asm.commons.ClassRemapper;
import org.objectweb.asm.commons.Remapper;

public final class MemberRenamer {

    // ---- class model ----------------------------------------------------------
    static final class Info {
        final ClassNode node;
        Info(ClassNode n) { this.node = n; }
        String name()       { return node.name; }
        String superName()  { return node.superName; }
        List<String> ifaces() { return node.interfaces == null ? Collections.emptyList() : node.interfaces; }
    }

    final Map<String, Info> classes = new LinkedHashMap<>();   // internal name -> Info
    // method-key = owner '#' name desc ; union-find parent map for virtual groups
    final Map<String, String> uf = new HashMap<>();
    // mapping decisions: keyed by "owner#name desc" -> newName (methods)
    final Map<String, String> methodRenames = new HashMap<>();
    // field renames: "owner#name fieldDesc" -> newName
    final Map<String, String> fieldRenames = new HashMap<>();
    // ordered mapping lines for the mapping file
    final List<String> mappingLines = new ArrayList<>();
    int typeShadowCount = 0;   // fields renamed because they shadowed an in-jar type
    int constShadowCount = 0;  // interface constants renamed (inherited-shadow)

    static String mkey(String owner, String name, String desc) { return owner + "#" + name + " " + desc; }

    // Classloader used to RESOLVE external supertypes via reflection (java/* always;
    // plus any framework jars — e.g. a MIDP stub — passed as the 4th argument).  This
    // lets us recognise genuine MIDP overrides (Canvas.paint, MIDlet.startApp, ...) so
    // we LOCK exactly those and nothing more.
    ClassLoader resolver = MemberRenamer.class.getClassLoader();

    // ---- entry ----------------------------------------------------------------
    public static void main(String[] args) throws Exception {
        if (args.length < 3) {
            System.err.println("usage: MemberRenamer <in.jar> <out.jar> <mapping.txt> "
                    + "[framework_cp]");
            System.exit(2);
        }
        MemberRenamer mr = new MemberRenamer();
        if (args.length >= 4 && !args[3].isEmpty()) mr.installResolver(args[3]);
        mr.run(new File(args[0]), new File(args[1]), new File(args[2]));
    }

    /** Add framework jars (e.g. a MIDP/CLDC stub) to the reflection classloader so
     *  javax.microedition.* supertypes resolve and their contracts can be detected. */
    void installResolver(String classpath) {
        List<URL> urls = new ArrayList<>();
        for (String p : classpath.split(File.pathSeparator)) {
            if (p.isEmpty()) continue;
            File f = new File(p);
            if (!f.exists()) continue;
            try { urls.add(f.toURI().toURL()); } catch (Exception ignore) {}
        }
        if (!urls.isEmpty())
            resolver = new URLClassLoader(urls.toArray(new URL[0]),
                                          MemberRenamer.class.getClassLoader());
    }

    void run(File in, File out, File mapping) throws Exception {
        load(in);
        buildVirtualGroups();
        planMethodRenames();
        planFieldRenames();
        planTypeShadowFieldRenames();
        planConstInterfaceShadowRenames();
        writeRenamed(in, out);
        writeMapping(mapping);
        System.out.println("MemberRenamer: classes=" + classes.size()
                + " methodRenames=" + methodRenames.size()
                + " fieldRenames=" + fieldRenames.size()
                + " (typeShadowFields=" + typeShadowCount
                + " constIfaceShadow=" + constShadowCount + ")");
    }

    // ---- load -----------------------------------------------------------------
    void load(File in) throws IOException {
        try (JarFile jf = new JarFile(in)) {
            Enumeration<JarEntry> e = jf.entries();
            while (e.hasMoreElements()) {
                JarEntry je = e.nextElement();
                if (je.isDirectory() || !je.getName().endsWith(".class")) continue;
                try (InputStream is = jf.getInputStream(je)) {
                    ClassReader cr = new ClassReader(is);
                    ClassNode cn = new ClassNode();
                    cr.accept(cn, ClassReader.SKIP_FRAMES);
                    classes.put(cn.name, new Info(cn));
                }
            }
        }
    }

    boolean inJar(String internalName) { return classes.containsKey(internalName); }

    // ---- union-find for virtual override groups -------------------------------
    String find(String k) {
        String p = uf.getOrDefault(k, k);
        if (!p.equals(k)) { p = find(p); uf.put(k, p); }
        return p;
    }
    void union(String a, String b) {
        String ra = find(a), rb = find(b);
        if (!ra.equals(rb)) uf.put(ra, rb);
    }

    /**
     * Link, by union-find, every in-jar method declaration to the same-named-and-
     * descriptored method it overrides/inherits from an in-jar supertype or interface.
     * A method is "virtual" for grouping if it is not static and not <init>/<clinit>.
     * Static/private methods don't participate in dispatch but renaming them must still
     * be consistent at the call site; since each is keyed by its own owner#name+desc and
     * we rewrite all references via the Remapper, treating them as singleton groups is
     * correct. We only UNION across the hierarchy for instance methods.
     */
    void buildVirtualGroups() {
        for (Info info : classes.values()) {
            for (MethodNode mn : info.node.methods) {
                String k = mkey(info.name(), mn.name, mn.desc);
                uf.putIfAbsent(k, k);
                if (isCtor(mn.name) || isStatic(mn.access)) continue;
                // walk all in-jar supertypes/interfaces that declare the same name+desc
                for (String sup : allSupertypes(info.name())) {
                    Info si = classes.get(sup);
                    if (si == null) continue;
                    for (MethodNode sm : si.node.methods) {
                        if (sm.name.equals(mn.name) && sm.desc.equals(mn.desc)
                                && !isStatic(sm.access)) {
                            union(k, mkey(sup, sm.name, sm.desc));
                        }
                    }
                }
            }
        }
    }

    /** All in-jar AND external supertype internal names, transitively (super + ifaces). */
    Set<String> allSupertypes(String name) {
        Set<String> seen = new LinkedHashSet<>();
        Deque<String> work = new ArrayDeque<>();
        Info self = classes.get(name);
        if (self == null) return seen;
        if (self.superName() != null) work.push(self.superName());
        for (String i : self.ifaces()) work.push(i);
        while (!work.isEmpty()) {
            String s = work.pop();
            if (s == null || !seen.add(s)) continue;
            Info si = classes.get(s);
            if (si != null) {
                if (si.superName() != null) work.push(si.superName());
                for (String i : si.ifaces()) work.push(i);
            }
            // external types: we cannot introspect them with ASM nodes, but for LOCKING
            // we only need to know the name+desc exists in an external supertype. That
            // check is done in isExternallyLocked() using core reflection on the runtime
            // (java/*) and a conservative name-prefix rule for MIDP/other externals.
        }
        return seen;
    }

    /**
     * A virtual group is LOCKED if ANY member method's name+desc is declared by a
     * supertype/interface that is NOT in the jar — i.e. it fulfils an external contract
     * (overrides java/lang/Runnable.run, javax.microedition.lcdui.Canvas.paint, MIDlet
     * .startApp, etc.). Renaming such a method would break the runtime shim.
     *
     * Detection: for each in-jar method, look at the class's transitive EXTERNAL
     * supertypes/interfaces (those whose internal name is not in the jar). If any of
     * them — resolved via the JDK's own classes when possible — declares a method with
     * the same name+desc, the group is locked. For externals we cannot resolve (most
     * MIDP/javax.microedition classes aren't on this JDK), we apply a conservative rule:
     * any non-private instance method whose immediate or transitive supertype chain
     * leaves the jar is treated as potentially-overriding and is LOCKED. This is the
     * safe direction (never rename a real external override); it only costs us the
     * chance to rename a method that merely *looks* like an override — which is fine,
     * because such a method has no descriptor collision to fix in the first place (we
     * only ever rename actual colliders, and we always prefer renaming the unlocked
     * collider).
     */
    final Map<String, Boolean> lockCache = new HashMap<>();

    boolean groupLocked(String anyMemberKey) {
        String root = find(anyMemberKey);
        Boolean c = lockCache.get(root);
        if (c != null) return c;
        boolean locked = false;
        // collect all members of this group
        for (Map.Entry<String, String> e : uf.entrySet()) {
            // iterate keys whose root is this root
        }
        // Build membership by scanning all method keys.
        for (Info info : classes.values()) {
            for (MethodNode mn : info.node.methods) {
                String k = mkey(info.name(), mn.name, mn.desc);
                if (!find(k).equals(root)) continue;
                if (methodHasExternalContract(info.name(), mn)) { locked = true; break; }
            }
            if (locked) break;
        }
        lockCache.put(root, locked);
        return locked;
    }

    // Well-known MIDP/CLDC + java.* callback signatures (name+full-descriptor) that an
    // in-jar class may override.  Used ONLY as a fallback when an external supertype
    // cannot be resolved via reflection (no MIDP stub on the resolver classpath).  With
    // a stub present, step 1 already locks these precisely; this list just guarantees
    // safety offline.  Keyed "name desc".
    static final Set<String> MIDP_CALLBACKS = new HashSet<>(Arrays.asList(
        // MIDlet
        "startApp ()V", "pauseApp ()V", "destroyApp (Z)V",
        // Runnable / Thread
        "run ()V",
        // Canvas / GameCanvas / Displayable / CustomItem
        "paint (Ljavax/microedition/lcdui/Graphics;)V",
        "keyPressed (I)V", "keyReleased (I)V", "keyRepeated (I)V",
        "pointerPressed (II)V", "pointerReleased (II)V", "pointerDragged (II)V",
        "showNotify ()V", "hideNotify ()V", "sizeChanged (II)V",
        // CommandListener / ItemCommandListener
        "commandAction (Ljavax/microedition/lcdui/Command;Ljavax/microedition/lcdui/Displayable;)V",
        "commandAction (Ljavax/microedition/lcdui/Command;Ljavax/microedition/lcdui/Item;)V",
        // PlayerListener
        "playerUpdate (Ljavax/microedition/media/Player;Ljava/lang/String;Ljava/lang/Object;)V",
        // common java.lang.Object overrides (never collide on return type, but keep safe)
        "hashCode ()I", "toString ()Ljava/lang/String;", "equals (Ljava/lang/Object;)Z",
        "clone ()Ljava/lang/Object;", "compareTo (Ljava/lang/Object;)I"
    ));

    /** True if (owner.name+desc) is declared by an external (not-in-jar) supertype —
     *  i.e. the method fulfils a runtime contract and must NOT be renamed. */
    boolean methodHasExternalContract(String owner, MethodNode mn) {
        if (isCtor(mn.name) || isStatic(mn.access)) return false;
        if (isPrivate(mn.access)) return false;     // private methods never override
        Set<String> exts = externalSupertypes(owner);
        if (exts.isEmpty()) return false;           // purely in-jar hierarchy: safe to rename
        // 1) precise: resolve each external supertype via reflection (covers java/* and,
        //    when a MIDP stub is on the resolver classpath, javax.microedition.*).
        boolean anyUnresolvable = false;
        for (String sup : exts) {
            if (jdkDeclaresMethod(sup, mn.name, mn.desc)) return true;
            if (!sup.equals("java/lang/Object") && !jdkResolvable(sup)) anyUnresolvable = true;
        }
        // 2) fallback ONLY for unresolvable externals: lock just the well-known callback
        //    signatures (so dispatch to a framework we can't see isn't broken).  We do
        //    NOT blanket-lock every method — that would prevent fixing real return-type
        //    collisions (e.g. i.a(int):Z vs i.a(int):I).
        if (anyUnresolvable && MIDP_CALLBACKS.contains(mn.name + " " + mn.desc)) return true;
        return false;
    }

    /** External (not-in-jar) supertypes/interfaces, transitively, for `name`. */
    Set<String> externalSupertypes(String name) {
        Set<String> ext = new LinkedHashSet<>();
        Deque<String> work = new ArrayDeque<>();
        Set<String> seen = new HashSet<>();
        Info self = classes.get(name);
        if (self == null) return ext;
        if (self.superName() != null) work.push(self.superName());
        for (String i : self.ifaces()) work.push(i);
        while (!work.isEmpty()) {
            String s = work.pop();
            if (s == null || !seen.add(s)) continue;
            Info si = classes.get(s);
            if (si != null) {                       // still in-jar, keep walking
                if (si.superName() != null) work.push(si.superName());
                for (String i : si.ifaces()) work.push(i);
            } else {                                // external boundary
                ext.add(s);
                // also climb external supertypes via reflection so we catch e.g.
                // MIDlet -> (its supers) and Canvas -> Displayable -> ... when on JDK
                for (String up : jdkSupertypes(s)) ext.add(up);
            }
        }
        return ext;
    }

    // ---- reflection helpers (resolve java/* AND framework/MIDP contracts) ------
    boolean jdkResolvable(String internal) {
        try { Class.forName(internal.replace('/', '.'), false, resolver); return true; }
        catch (Throwable t) { return false; }
    }
    Set<String> jdkSupertypes(String internal) {
        Set<String> out = new LinkedHashSet<>();
        try {
            Class<?> c = Class.forName(internal.replace('/', '.'), false, resolver);
            for (Class<?> s = c.getSuperclass(); s != null; s = s.getSuperclass())
                out.add(s.getName().replace('.', '/'));
            collectIfaces(c, out);
        } catch (Throwable ignore) {}
        return out;
    }
    void collectIfaces(Class<?> c, Set<String> out) {
        for (Class<?> i : c.getInterfaces()) {
            if (out.add(i.getName().replace('.', '/'))) collectIfaces(i, out);
        }
    }
    boolean jdkDeclaresMethod(String internal, String name, String desc) {
        try {
            Class<?> c = Class.forName(internal.replace('/', '.'), false, resolver);
            for (java.lang.reflect.Method m : c.getMethods())
                if (m.getName().equals(name) && Type.getMethodDescriptor(m).equals(desc)) return true;
            for (java.lang.reflect.Method m : c.getDeclaredMethods())
                if (m.getName().equals(name) && Type.getMethodDescriptor(m).equals(desc)) return true;
        } catch (Throwable ignore) {}
        return false;
    }

    // ---- plan METHOD renames (return-type-only overloads) ---------------------
    void planMethodRenames() {
        for (Info info : classes.values()) {
            // group this class's methods by (name, paramTypes); a collision is 2+ with
            // different return type.
            Map<String, List<MethodNode>> byNameParams = new LinkedHashMap<>();
            for (MethodNode mn : info.node.methods) {
                if (isCtor(mn.name)) continue;
                String params = mn.desc.substring(0, mn.desc.indexOf(')') + 1); // "(...)"
                byNameParams.computeIfAbsent(mn.name + " " + params, k -> new ArrayList<>()).add(mn);
            }
            for (List<MethodNode> grp : byNameParams.values()) {
                if (grp.size() < 2) continue;
                // distinct return types?  collision only if returns differ
                Set<String> rets = new LinkedHashSet<>();
                for (MethodNode mn : grp) rets.add(mn.desc.substring(mn.desc.indexOf(')') + 1));
                if (rets.size() < 2) continue;  // (shouldn't happen — same full desc dup)
                disambiguateMethods(info, grp);
            }
        }
    }

    /**
     * Given 2+ methods in `info` that share name+params but differ in return type,
     * keep one and rename the others. Selection respects locking: a LOCKED method (its
     * virtual group fulfils an external contract) is never the one renamed. We keep the
     * first non-renamable-preferred occurrence and rename the rest; among renamable
     * candidates, deterministic order = declaration order then descriptor.
     */
    void disambiguateMethods(Info info, List<MethodNode> grp) {
        // sort deterministically: declaration order is preserved by ClassNode; tie-break desc
        List<MethodNode> sorted = new ArrayList<>(grp);
        sorted.sort(Comparator.comparing((MethodNode m) -> m.desc));
        // choose the KEEPER: prefer a locked one (must keep its name); else first.
        MethodNode keeper = null;
        for (MethodNode mn : sorted)
            if (groupLocked(mkey(info.name(), mn.name, mn.desc))) { keeper = mn; break; }
        if (keeper == null) keeper = sorted.get(0);

        Set<String> used = existingMethodNames(info);
        for (MethodNode mn : sorted) {
            if (mn == keeper) continue;
            String gkey = mkey(info.name(), mn.name, mn.desc);
            if (groupLocked(gkey)) {
                // both colliders locked? can't happen for valid bytecode (two external
                // overrides with same name+params+different return is itself illegal in
                // the supertype). Defensive: skip renaming a locked method.
                continue;
            }
            String newName = uniqueName(mn.name, mn.desc, used);
            renameWholeGroup(gkey, newName);
            used.add(newName);
        }
    }

    /** Apply one new name to EVERY method in the virtual group (preserve dispatch). */
    void renameWholeGroup(String anyKey, String newName) {
        String root = find(anyKey);
        for (Info info : classes.values()) {
            for (MethodNode mn : info.node.methods) {
                String k = mkey(info.name(), mn.name, mn.desc);
                if (!find(k).equals(root)) continue;
                if (methodRenames.containsKey(k)) continue;
                methodRenames.put(k, newName);
                mappingLines.add(info.name() + "." + mn.name + " " + mn.desc + " -> " + newName);
            }
        }
    }

    Set<String> existingMethodNames(Info info) {
        Set<String> s = new HashSet<>();
        for (MethodNode mn : info.node.methods) {
            String k = mkey(info.name(), mn.name, mn.desc);
            s.add(methodRenames.getOrDefault(k, mn.name));
        }
        return s;
    }

    // ---- plan FIELD renames (duplicate names in same class) -------------------
    void planFieldRenames() {
        for (Info info : classes.values()) {
            Map<String, List<FieldNode>> byName = new LinkedHashMap<>();
            for (FieldNode fn : info.node.fields)
                byName.computeIfAbsent(fn.name, k -> new ArrayList<>()).add(fn);
            for (Map.Entry<String, List<FieldNode>> e : byName.entrySet()) {
                List<FieldNode> grp = e.getValue();
                if (grp.size() < 2) continue;   // unique name -> nothing to do
                // keep the first declared; rename the rest deterministically.
                // (Fields are class-local in Java; they never fulfil an external
                //  override contract, so no locking needed. But to MINIMISE churn for
                //  externally-referenced fields we keep declaration order: callers
                //  referencing field#desc are rewritten by the Remapper regardless.)
                Set<String> used = existingFieldNames(info);
                for (int i = 1; i < grp.size(); i++) {
                    FieldNode fn = grp.get(i);
                    String newName = uniqueName(fn.name, fn.desc, used);
                    String k = mkey(info.name(), fn.name, fn.desc);
                    fieldRenames.put(k, newName);
                    used.add(newName);
                    mappingLines.add(info.name() + "." + fn.name + " " + fn.desc + " -> " + newName);
                }
            }
        }
    }

    Set<String> existingFieldNames(Info info) {
        Set<String> s = new HashSet<>();
        for (FieldNode fn : info.node.fields) {
            String k = mkey(info.name(), fn.name, fn.desc);
            s.add(fieldRenames.getOrDefault(k, fn.name));
        }
        return s;
    }

    // ---- plan FIELD renames (field name shadows an in-jar TYPE name) ----------
    //
    // A second, distinct source-legality hazard from single-namespace obfuscation
    // (ProGuard reuses single letters for BOTH types and members): a class C may
    // declare an instance/static field named `X` while ALSO referencing an in-jar
    // *type* whose simple name is `X` via `X.staticMember` / `new X()` / `(X)cast`.
    // In bytecode these never collide (field access and type references are keyed
    // independently). In Java SOURCE, however, the field `X` is in scope and SHADOWS
    // the type name, so `X.c` parses as `(this.X).c` -> "int cannot be dereferenced",
    // and `new X()` / casts fail to resolve. Decompilers faithfully reproduce this
    // (uncompilable) source; no textual post-fix can repair it reliably (a use site
    // cannot tell field-vs-type apart once shadowed). So we rename the offending
    // FIELD in bytecode, here, before decompiling.
    //
    // Trigger (precise, minimal churn): rename field C.X only when (a) some in-jar
    // type has simple name X, AND (b) C actually references that type's INTERNAL
    // name somewhere in its bytecode (instructions, signatures, field/var types,
    // annotations) — i.e. the shadow genuinely occurs. Fields whose name merely
    // happens to equal a type never referenced in C are left untouched.
    void planTypeShadowFieldRenames() {
        // simple type name -> internal name, for in-jar types (default-package: equal)
        Map<String, String> typeBySimple = new HashMap<>();
        for (String internal : classes.keySet()) {
            String simple = internal.substring(internal.lastIndexOf('/') + 1);
            // if two in-jar types share a simple name across packages, prefer the
            // default-package one (the obfuscated single-letter case we care about).
            typeBySimple.putIfAbsent(simple, internal);
            if (internal.indexOf('/') < 0) typeBySimple.put(simple, internal);
        }
        for (Info info : classes.values()) {
            Set<String> referenced = referencedInternalNames(info.node);
            Set<String> used = existingFieldNames(info);
            for (FieldNode fn : info.node.fields) {
                String k = mkey(info.name(), fn.name, fn.desc);
                if (fieldRenames.containsKey(k)) continue;  // already renamed (dup-name)
                String internal = typeBySimple.get(fn.name);
                if (internal == null) continue;             // name isn't a type's simple name
                if (internal.equals(info.name())) continue; // self-name field: `class j {int j;}` is legal in source
                if (!referenced.contains(internal)) continue; // type not used by simple name here
                String newName = uniqueName(fn.name, fn.desc, used);
                fieldRenames.put(k, newName);
                used.add(newName);
                typeShadowCount++;
                mappingLines.add(info.name() + "." + fn.name + " " + fn.desc
                        + " -> " + newName + "  (type-shadow)");
            }
        }
    }

    // ---- plan FIELD renames (constant-interface field inherited into an
    //      implementor where it collides with an instance field) ----------------
    //
    // A constant interface I (all fields static final) is legal bytecode, and so is
    // a class C `implements I` that ALSO has an instance field with the same simple
    // name as one of I's constants (the two live in different owners). In Java
    // SOURCE, however, C inherits I's constants into its member scope, so a bare
    // `this.x` / `x` inside C is "reference to x is ambiguous" between the inherited
    // constant and C's own/inherited instance field. ProGuard's single-letter reuse
    // makes this common (e.g. interface `o` exposing constants a,b,c,... while a
    // class hierarchy `n`<-`z implements o` already has a boolean field `b`).
    //
    // Fix: rename the colliding CONSTANT in the interface (the constant is only read
    // via `I.name` / inherited reads, all rewritten by the Remapper; renaming it
    // never changes dispatch). We never rename the instance field here (that would
    // ripple through more code); the constant is the cheaper, safer side to move.
    void planConstInterfaceShadowRenames() {
        for (Info iface : classes.values()) {
            if ((iface.node.access & Opcodes.ACC_INTERFACE) == 0) continue;
            if (iface.node.fields.isEmpty()) continue;
            // union of instance-field names of EVERY in-jar implementor (its own +
            // in-jar super chain) — any of these shadows a same-named constant.
            Set<String> shadowingNames = new HashSet<>();
            for (Info c : classes.values()) {
                if (c == iface) continue;
                if (implementsInterface(c.name(), iface.name()))
                    shadowingNames.addAll(instanceFieldNames(c.name()));
            }
            if (shadowingNames.isEmpty()) continue;
            Set<String> used = existingFieldNames(iface);
            // iterate ALL fields (a constant interface may legitimately have several
            // fields sharing a bytecode name; the dup-name pass renamed all but the
            // first — only the still-bare one needs the shadow rename here).
            for (FieldNode fn : iface.node.fields) {
                String k = mkey(iface.name(), fn.name, fn.desc);
                if (fieldRenames.containsKey(k)) continue;   // already moved
                String effective = fn.name;                  // its source-visible name
                if (!shadowingNames.contains(effective)) continue;
                String newName = uniqueName(fn.name, fn.desc, used);
                fieldRenames.put(k, newName);
                used.add(newName);
                constShadowCount++;
                mappingLines.add(iface.name() + "." + fn.name + " " + fn.desc
                        + " -> " + newName + "  (const-iface-shadow)");
            }
        }
    }

    /** True if in-jar class `cls` implements `iface` (directly or transitively, via
     *  super classes and super interfaces, all in-jar). */
    boolean implementsInterface(String cls, String iface) {
        Deque<String> work = new ArrayDeque<>();
        Set<String> seen = new HashSet<>();
        work.push(cls);
        while (!work.isEmpty()) {
            String s = work.pop();
            if (s == null || !seen.add(s)) continue;
            Info si = classes.get(s);
            if (si == null) continue;
            for (String i : si.ifaces()) {
                if (i.equals(iface)) return true;
                work.push(i);
            }
            if (si.superName() != null) work.push(si.superName());
        }
        return false;
    }

    /** Instance (non-static) field names of `cls` plus those inherited from its
     *  in-jar super-class chain. */
    Set<String> instanceFieldNames(String cls) {
        Set<String> out = new HashSet<>();
        String cur = cls;
        Set<String> guard = new HashSet<>();
        while (cur != null && guard.add(cur)) {
            Info ci = classes.get(cur);
            if (ci == null) break;
            for (FieldNode fn : ci.node.fields)
                if ((fn.access & Opcodes.ACC_STATIC) == 0) out.add(fn.name);
            cur = ci.superName();
        }
        return out;
    }

    /** Every in-jar internal type name referenced anywhere in `cn`'s bytecode:
     *  superclass/interfaces, field & method descriptors, local-var & instruction
     *  type operands (NEW/CHECKCAST/field & method owners). Used to decide whether a
     *  field genuinely shadows a referenced type. */
    Set<String> referencedInternalNames(ClassNode cn) {
        Set<String> out = new HashSet<>();
        if (cn.superName != null) addType(out, Type.getObjectType(cn.superName));
        if (cn.interfaces != null) for (String i : cn.interfaces) addType(out, Type.getObjectType(i));
        for (FieldNode fn : cn.fields) addDescTypes(out, fn.desc);
        for (MethodNode mn : cn.methods) {
            addDescTypes(out, mn.desc);
            if (mn.localVariables != null)
                for (LocalVariableNode lv : mn.localVariables) addDescTypes(out, lv.desc);
            if (mn.instructions != null) {
                for (AbstractInsnNode in : mn.instructions.toArray()) {
                    if (in instanceof TypeInsnNode)
                        addType(out, Type.getObjectType(((TypeInsnNode) in).desc));
                    else if (in instanceof FieldInsnNode) {
                        FieldInsnNode f = (FieldInsnNode) in;
                        addType(out, Type.getObjectType(f.owner));
                        addDescTypes(out, f.desc);
                    } else if (in instanceof MethodInsnNode) {
                        MethodInsnNode m = (MethodInsnNode) in;
                        addType(out, Type.getObjectType(m.owner));
                        addDescTypes(out, m.desc);
                    } else if (in instanceof MultiANewArrayInsnNode)
                        addDescTypes(out, ((MultiANewArrayInsnNode) in).desc);
                    else if (in instanceof LdcInsnNode) {
                        Object c = ((LdcInsnNode) in).cst;
                        if (c instanceof Type) addType(out, (Type) c);
                    }
                }
            }
        }
        out.retainAll(classes.keySet());   // keep only in-jar types
        return out;
    }

    void addType(Set<String> out, Type t) {
        if (t == null) return;
        if (t.getSort() == Type.ARRAY) t = t.getElementType();
        if (t.getSort() == Type.OBJECT) out.add(t.getInternalName());
    }
    void addDescTypes(Set<String> out, String desc) {
        if (desc == null) return;
        try {
            if (desc.startsWith("(")) {
                for (Type a : Type.getArgumentTypes(desc)) addType(out, a);
                addType(out, Type.getReturnType(desc));
            } else {
                addType(out, Type.getType(desc));
            }
        } catch (Exception ignore) {}
    }

    // ---- deterministic unique-name scheme -------------------------------------
    /** orig + "_" + 8-hex of stable hash(desc); numeric-bump if still taken. */
    String uniqueName(String orig, String desc, Set<String> used) {
        String tag = hex8(stableHash(desc));
        String base = orig + "_" + tag;
        String cand = base;
        int n = 2;
        while (used.contains(cand)) cand = base + "_" + (n++);
        return cand;
    }
    static long stableHash(String s) {        // FNV-1a 64-bit (deterministic across JVMs)
        long h = 0xcbf29ce484222325L;
        for (int i = 0; i < s.length(); i++) { h ^= s.charAt(i); h *= 0x100000001b3L; }
        return h;
    }
    static String hex8(long h) {
        String s = Long.toHexString(h);
        return s.length() >= 8 ? s.substring(0, 8) : ("00000000" + s).substring(s.length());
    }

    // ---- write renamed jar via ClassRemapper ----------------------------------
    void writeRenamed(File in, File out) throws IOException {
        Remapper remapper = new Remapper() {
            @Override public String mapMethodName(String owner, String name, String descriptor) {
                String nn = lookupMethod(owner, name, descriptor);
                return nn != null ? nn : name;
            }
            @Override public String mapInvokeDynamicMethodName(String name, String descriptor) {
                return name; // no indy in J2ME
            }
            @Override public String mapFieldName(String owner, String name, String descriptor) {
                String nn = fieldRenames.get(mkey(owner, name, descriptor));
                return nn != null ? nn : name;
            }
        };

        // copy non-class entries verbatim; transform classes.
        try (JarFile jf = new JarFile(in);
             JarOutputStream jos = new JarOutputStream(new BufferedOutputStream(new FileOutputStream(out)))) {
            Enumeration<JarEntry> e = jf.entries();
            while (e.hasMoreElements()) {
                JarEntry je = e.nextElement();
                if (je.isDirectory()) continue;
                byte[] data;
                if (je.getName().endsWith(".class")) {
                    try (InputStream is = jf.getInputStream(je)) {
                        ClassReader cr = new ClassReader(is);
                        ClassWriter cw = new ClassWriter(0);
                        ClassRemapper cv = new ClassRemapper(cw, remapper);
                        cr.accept(cv, ClassReader.SKIP_FRAMES);
                        data = cw.toByteArray();
                    }
                } else {
                    try (InputStream is = jf.getInputStream(je)) {
                        data = is.readAllBytes();
                    }
                }
                JarEntry ne = new JarEntry(je.getName());
                ne.setTime(je.getTime());
                jos.putNextEntry(ne);
                jos.write(data);
                jos.closeEntry();
            }
        }
    }

    /**
     * Resolve a method rename for a call site. The owner at the call site may be a
     * subtype that doesn't itself declare the method (inherited dispatch). We resolve by
     * walking owner + its in-jar supertypes for a matching declared key; the virtual
     * group guarantees all of them map to the same new name.
     */
    String lookupMethod(String owner, String name, String desc) {
        String direct = methodRenames.get(mkey(owner, name, desc));
        if (direct != null) return direct;
        // climb in-jar supertypes for an inherited declaration that was renamed
        if (inJar(owner)) {
            for (String sup : allSupertypes(owner)) {
                String nn = methodRenames.get(mkey(sup, name, desc));
                if (nn != null) return nn;
            }
        }
        return null;
    }

    void writeMapping(File mapping) throws IOException {
        Collections.sort(mappingLines);
        try (Writer w = new OutputStreamWriter(new FileOutputStream(mapping), StandardCharsets.UTF_8)) {
            for (String l : mappingLines) { w.write(l); w.write('\n'); }
        }
    }

    // ---- small access helpers -------------------------------------------------
    static boolean isCtor(String name)     { return name.equals("<init>") || name.equals("<clinit>"); }
    static boolean isStatic(int acc)       { return (acc & Opcodes.ACC_STATIC) != 0; }
    static boolean isPrivate(int acc)      { return (acc & Opcodes.ACC_PRIVATE) != 0; }
}
