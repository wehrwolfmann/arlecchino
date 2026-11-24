// (f) self-name type-shadow seed.
// A field named like its OWN class is legal (`class Tally { static int Tally; }`), and
// javac compiles the unqualified `bump()` below to INVOKESTATIC Tally.bump.  A
// decompiler, however, prints that call as `Tally.bump()`, which in source means
// `(the int field Tally).bump()`.  The renamer must move the field.
public class Tally {
    static int Tally = 3;

    static int bump() { return Tally + 1; }

    int use() { return bump(); }
}
