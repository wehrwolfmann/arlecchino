// Overrides a.c(); its field x must not take a name already used up the hierarchy.
public class k extends a {
    int x;

    public k(int v) {
        b = v;
        x = v + 1;
    }

    public int c() {
        return b + x;
    }
}
