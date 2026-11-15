// The canvas-size and RMS-trace fixture for midp-rt (tests/midp_rt_canvas.rs).
//
// TEMPLATE: the test substitutes @FULL@ with `false` or `true` before compiling.
//
//   * the canvas asks for its size in its constructor before and after
//     setFullScreenMode(@FULL@), and once more in paint(); every answer goes to
//     the trace as a fillRect (`fillRect 0 0 <in paint>`, `fillRect 1 1 <before>`,
//     `fillRect 2 2 <after>`);
//   * startApp() writes two records, reads them back through an enumeration and
//     asks for the record count once, so the trace shows exactly the RMS calls
//     the game makes, and none the runtime makes for itself while it enumerates
//     or saves the store.
import javax.microedition.lcdui.Canvas;
import javax.microedition.lcdui.Display;
import javax.microedition.lcdui.Graphics;
import javax.microedition.midlet.MIDlet;
import javax.microedition.rms.RecordEnumeration;
import javax.microedition.rms.RecordStore;

public class SizeMidlet extends MIDlet {
    protected void startApp() {
        SizeCanvas canvas = new SizeCanvas();
        try {
            RecordStore rs = RecordStore.openRecordStore("sizestore", true);
            rs.addRecord(new byte[] { 1, 2, 3 }, 0, 3);
            rs.addRecord(new byte[] { 4, 5 }, 0, 2);
            RecordEnumeration e = rs.enumerateRecords(null, null, false);
            while (e.hasNextElement()) {
                e.nextRecord();
            }
            rs.getNumRecords();
            rs.closeRecordStore();
        } catch (Exception ex) {
            // the RMS half must not kill the size half
        }
        Display.getDisplay(this).setCurrent(canvas);
        canvas.repaint();
    }

    protected void pauseApp() {}

    protected void destroyApp(boolean unconditional) {}
}

class SizeCanvas extends Canvas {
    private final int w0, h0, w1, h1;

    SizeCanvas() {
        w0 = getWidth();
        h0 = getHeight();
        setFullScreenMode(@FULL@);
        w1 = getWidth();
        h1 = getHeight();
    }

    protected void paint(Graphics g) {
        g.setColor(0x000000);
        g.fillRect(0, 0, getWidth(), getHeight());
        g.fillRect(1, 1, w0, h0);
        g.fillRect(2, 2, w1, h1);
    }
}
