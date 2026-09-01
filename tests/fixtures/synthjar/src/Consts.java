// (d) constant-interface shadow seed.
// `Consts` is a constant interface (all fields static final).  `Holder implements Consts`
// also declares an INSTANCE field `b`, which (in Java source, not bytecode) shadows the
// inherited constant `b` -> "reference to b is ambiguous".  The renamer renames the
// CONSTANT side (const-iface-shadow).
public interface Consts {
    boolean b = true;
    int LIMIT = 10;
}
