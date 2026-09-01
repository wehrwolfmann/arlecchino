// (e) MIDP-override seed.
// Compiled as a plain class; SynthObfuscator then re-parents it to
// javax/microedition/lcdui/Canvas and marks `keyPressed(I)V` as a real MIDP callback
// override.  SynthObfuscator ALSO injects a return-type-only collider `keyPressed(I)I`
// in the SAME class: the renamer must LOCK the genuine `keyPressed(I)V` callback (leave
// it untouched) and rename the OTHER (unlocked) collider instead.
public class Screen {
    int last;

    // becomes the MIDP callback override after re-parenting (locked, never renamed)
    public void keyPressed(int code) { this.last = code; }

    int read() { return last; }
}
