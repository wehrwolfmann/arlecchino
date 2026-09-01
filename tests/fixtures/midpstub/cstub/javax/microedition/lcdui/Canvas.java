package javax.microedition.lcdui;

public abstract class Canvas extends Displayable {
    protected abstract void paint(Graphics g);
    protected void keyPressed(int k) {}
}
