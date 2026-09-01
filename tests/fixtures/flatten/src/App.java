// Compiled in the DEFAULT package (legal), then FlattenFixture re-parents it into
// package `Container` in BYTECODE while leaving its references to `Lib` pointing at the
// default-package type.  Source decompiled from the result is uncompilable until
// --flatten-packages moves App back into the default package.
public class App {
    Lib lib = new Lib();
    int run() { return this.lib.get() + 1; }
}
