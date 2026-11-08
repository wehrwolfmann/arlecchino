// (g) inherited type-shadow seed: inherits Frame's field `Part` and also names the
// TYPE Part, so in source the field shadows the type.  The field read below is
// compiled with owner Body (not Frame), which the renamer must resolve to Frame.
public class Body extends Frame {
    Object make() { return new Part(); }

    int weight() { return Part + 1; }
}
