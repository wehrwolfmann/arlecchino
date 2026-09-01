// Base source for the synthetic obfuscated jar (Stage Э1 test fixture).
//
// These classes are compiled with javac (legal Java), then POST-PROCESSED with ASM
// (SynthObfuscator) to inject source-ILLEGAL member collisions that mirror what
// ProGuard-style obfuscation produces.  Nothing here is downloaded/vendored — the
// fixture is generated on the fly.
//
// All types live in the default package, mirroring single-letter obfuscated jars.
public class Engine {
    // (a) dup-field seed: SynthObfuscator adds a SECOND field also named `a` but with a
    //     different descriptor (I), making two `a` fields in this class — illegal Java.
    String a = "hello";

    // (b) return-type-only overload seed: SynthObfuscator clones `flag(I)Z` into a twin
    //     `flag(I)I` (same name+params, different return) — illegal Java.
    public boolean flag(int x) { return x > 0; }

    // (c) type-shadow seed: a field named `State` (the simple name of an in-jar type),
    //     and the body references the State type by simple name so the renamer's
    //     "referenced && shadows a type" rule fires.
    int State = 7;
    State maker() { return new State(); }

    int compute() {
        return (flag(State) ? 1 : 0) + a.length();
    }
}
