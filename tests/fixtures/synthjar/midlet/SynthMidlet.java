// Stage A6 (PLAN_runtime.md, Block A) — the synthetic deterministic test MIDlet.
//
// This is a TEMPLATE: `synthjar.build_midlet()` substitutes @COLOR@ with a hex
// colour literal and @LOOP@ with `false` or `true` before compiling (javac
// --release 8, classpath = the generated clean-room MIDP stub
// ~/.arlecchino/midp-stub.jar).  Varying the colour yields an
// intentionally-different jar — the difftest FAIL fixture.
//
// Behaviour is fully deterministic by construction (no clock, no randomness):
//   * paint() draws a checkerboard in PATTERN_COLOR on black;
//   * the bottom bar's colour encodes the LAST key code received (key-reaction
//     fixture for the A3 input-replay test);
//   * startApp() writes one fixed record into RecordStore "synthstore"
//     (RMS-diff fixture for A4).
//
// With LOOP = false there are no threads: the canvas paints only on request.
// With LOOP = true a game-style loop thread drives the frames
// (repaint + serviceRepaints, like a real game tick), and paint() also draws
// `fillRect <paint index at which the last key press arrived> 0 1 1`, so a
// trace shows on which tick the game saw the press (deterministic-input
// fixture for `midp-rt --ticks`).
import javax.microedition.lcdui.Canvas;
import javax.microedition.lcdui.Display;
import javax.microedition.lcdui.Graphics;
import javax.microedition.midlet.MIDlet;
import javax.microedition.rms.RecordStore;

public class SynthMidlet extends MIDlet {
    static final boolean LOOP = @LOOP@;
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
        if (!LOOP) {
            canvas.repaint();
        }
        // LOOP: the tick thread starts in showNotify() and begins ticking only
        // after the first frame — a loop that ticks earlier spins on a
        // not-yet-shown screen for a scheduler-dependent number of turns
    }

    protected void pauseApp() {}

    protected void destroyApp(boolean unconditional) {}
}

class SynthCanvas extends Canvas implements Runnable {
    static final int PATTERN_COLOR = 0x@COLOR@;
    private int lastKey = 0;
    private volatile int paints = 0;   // paint() calls so far (LOOP only)
    private int pressedAt = -1;
    private Thread loop;

    protected void showNotify() {
        if (SynthMidlet.LOOP && loop == null) {
            loop = new Thread(this);
            loop.start();
            repaint();
        }
    }

    public void run() {
        while (paints == 0) {      // wait for the first frame (see startApp)
            try {
                Thread.sleep(2);
            } catch (InterruptedException e) {
                return;
            }
        }
        while (true) {
            repaint();
            serviceRepaints();
            try {
                Thread.sleep(2);
            } catch (InterruptedException e) {
                return;
            }
        }
    }

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
        if (SynthMidlet.LOOP) {
            g.fillRect(pressedAt, 0, 1, 1);
            paints++;
        }
    }

    protected void keyPressed(int keyCode) {
        lastKey = keyCode;
        pressedAt = paints;
        repaint();
    }
}
