// Overrides the WELL-KNOWN `keyPressed(I)V` callback, which the denylist covers even
// with no stub at all — the offline minimum.
public class Widget extends javax.microedition.lcdui.Canvas {
    protected void paint(javax.microedition.lcdui.Graphics g) {}
    protected void keyPressed(int keyCode) {}
}
