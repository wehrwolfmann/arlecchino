// Default-package helper type referenced by App.  After FlattenFixture re-parents App
// into package `Container`, App can no longer NAME this default-package type in source —
// the irreducible cross-package error that --flatten-packages resolves.
public class Lib {
    int value;
    public Lib() { this.value = 42; }
    int get() { return value; }
}
