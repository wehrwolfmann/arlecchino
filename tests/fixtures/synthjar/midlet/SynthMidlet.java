// Stage A6 (PLAN_runtime.md, Block A) — the synthetic deterministic test MIDlet.
//
// This is a TEMPLATE: `synthjar.build_midlet()` substitutes @COLOR@ with a hex
// colour literal before compiling (javac --release 8, classpath = the generated
// clean-room MIDP stub ~/.arlecchino/midp-stub.jar).  Varying the colour yields
// an intentionally-different jar — the difftest FAIL fixture.
//
// Behaviour is fully deterministic by construction (no threads, no clock, no
// randomness):
//   * paint() draws a checkerboard in PATTERN_COLOR on black;
//   * the bottom bar's colour encodes the LAST key code received (key-reaction
//     fixture for the A3 input-replay test);
//   * startApp() writes one fixed record into RecordStore "synthstore"
//     (RMS-diff fixture for A4).
import javax.microedition.lcdui.Canvas;
import javax.microedition.lcdui.Display;
import javax.microedition.lcdui.Graphics;
import javax.microedition.midlet.MIDlet;
import javax.microedition.rms.RecordStore;

public class SynthMidlet extends MIDlet {
    private SynthCanvas canvas;

    protected void startApp() {
        if (canvas == null) {
            canvas = new SynthCanvas();
            try {
                RecordStore rs = RecordStore.openRecordStore("synthstore", true);
                if (rs.getNumRecords() == 0) {
                    byte[] rec = new byte[] { 'A', 'R', 'L', '1' };
                    rs.addRecord(rec, 0, rec.length);
                }
                rs.closeRecordStore();
            } catch (Exception e) {
                // RecordStore failure must not kill the screen fixture
            }
        }
        Display.getDisplay(this).setCurrent(canvas);
        canvas.repaint();
    }

    protected void pauseApp() {}

    protected void destroyApp(boolean unconditional) {}
}

class SynthCanvas extends Canvas {
    static final int PATTERN_COLOR = 0x@COLOR@;
    private int lastKey = 0;

    protected void paint(Graphics g) {
        int w = getWidth(), h = getHeight();
        g.setColor(0x000000);
        g.fillRect(0, 0, w, h);
        g.setColor(PATTERN_COLOR);
        for (int y = 0; y < 8; y++) {
            for (int x = 0; x < 8; x++) {
                if (((x + y) & 1) == 0) {
                    g.fillRect(x * 16, y * 16, 16, 16);
                }
            }
        }
        // deterministic colour derived from the last key code (0 = none yet)
        g.setColor((lastKey * 0x123457) & 0xFFFFFF);
        g.fillRect(0, h - 24, w, 24);
    }

    protected void keyPressed(int keyCode) {
        lastKey = keyCode;
        repaint();
    }
}
