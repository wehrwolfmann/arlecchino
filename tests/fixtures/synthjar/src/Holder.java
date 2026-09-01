// Implementor that supplies the shadowing instance field `b` for the (d) collision.
public class Holder implements Consts {
    boolean b;          // instance field; shadows the inherited constant Consts.b
    int n;

    public Holder() { this.b = false; this.n = LIMIT; }

    int get() { return b ? n : 0; }
}
