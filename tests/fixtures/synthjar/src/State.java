// An in-jar type whose simple name (`State`) is shadowed by a field in Engine,
// driving the (c) type-shadow collision.
public class State {
    int value;
    public State() { this.value = 0; }
}
