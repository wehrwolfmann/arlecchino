// Implements a callback contract that is NOT in the renamer's MIDP_CALLBACKS denylist.
// Only the PRECISE resolver path (the generated clean-room stub on the classpath) can
// know that `itemStateChanged` is a framework override and must stay locked.
public class Listener implements javax.microedition.lcdui.ItemStateListener {
    public void itemStateChanged(javax.microedition.lcdui.Item item) {}
}
