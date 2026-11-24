/*
 *  arlecchino-midp-rt: a test MIDlet for the headless rendering smoke test
 *  (Canvas: fillRect + drawString; System.out checks the Injected redirect
 *  of the ASM instrumentation).
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
 */
import javax.microedition.midlet.MIDlet;
import javax.microedition.lcdui.*;

public class TestMidlet extends MIDlet {
    static class C extends Canvas {
        // the last keyCode — a fixture for checking --inputs (the bottom bar
        // changes colour by the pressed key: the frame after input != the frame before)
        private int lastKey = 0;

        protected void paint(Graphics g) {
            g.setColor(0xFF0000);
            g.fillRect(0, 0, getWidth(), getHeight());
            g.setColor(0xFFFFFF);
            g.drawString("HELLO", 10, 10, Graphics.TOP | Graphics.LEFT);
            g.setColor((lastKey * 0x010305) & 0xFFFFFF);
            g.fillRect(0, getHeight() - 12, getWidth(), 12);
            System.out.println("PAINT_CALLED " + getWidth() + "x" + getHeight());
        }

        protected void keyPressed(int keyCode) {
            lastKey = keyCode;
            System.out.println("KEY_PRESSED " + keyCode);
            repaint();
        }
    }
    protected void startApp() {
        System.out.println("START_APP");
        C c = new C();
        Display.getDisplay(this).setCurrent(c);
        c.repaint();
        // No auto-destroy: SmokeLaunch dumps the frame and ends the process itself
    }
    protected void pauseApp() {}
    protected void destroyApp(boolean u) {}
}
