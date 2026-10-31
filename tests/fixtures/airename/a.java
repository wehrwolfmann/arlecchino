// Synthetic fixture for tests/ai_rename.rs (original work): a hand-written,
// ProGuard-style "obfuscated" hierarchy.  `a` plays an abstract shape.
public abstract class a {
    protected int b;

    public abstract int c();

    public int d() {
        return c() * 2;
    }

    public String toString() {
        return "a" + c();
    }
}
