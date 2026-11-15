// The synthetic rendering MIDlet: every pixel-exact drawing path of the port's
// software renderer (jrt::raster) on one screen, so `port-rust` + `frame-diff`
// can compare the translation's frames with midp-rt's pixel by pixel.
//
// Images are made by the test itself (tests/port_rust.rs): /rgba.png is RGBA
// with opaque, half-transparent and fully transparent pixels, /pal.png is a
// palette image with a tRNS chunk.  Nothing here reads a clock or a random
// number; the picture depends only on the paint index and the last key, so the
// frames are deterministic under midp-rt's `--ticks` mode.

import javax.microedition.lcdui.Canvas;
import javax.microedition.lcdui.Display;
import javax.microedition.lcdui.Graphics;
import javax.microedition.lcdui.Image;
import javax.microedition.midlet.MIDlet;

public class RenderMidlet extends MIDlet {
    private RenderCanvas canvas;

    protected void startApp() {
        if (canvas == null) {
            canvas = new RenderCanvas();
            canvas.setFullScreenMode(true);
        }
        Display.getDisplay(this).setCurrent(canvas);
    }

    protected void pauseApp() {}

    protected void destroyApp(boolean unconditional) {}
}

class RenderCanvas extends Canvas implements Runnable {
    private Image rgba;
    private Image pal;
    private Image buffer;
    private int lastKey = 0;
    private volatile int paints = 0;
    private Thread loop;

    RenderCanvas() {
        try {
            rgba = Image.createImage("/rgba.png");
            pal = Image.createImage("/pal.png");
        } catch (Exception e) {
            rgba = null;
        }
    }

    protected void showNotify() {
        if (loop == null) {
            loop = new Thread(this);
            loop.start();
            repaint();
        }
    }

    public void run() {
        while (paints == 0) {
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
        int w = getWidth();
        int h = getHeight();
        int t = paints;
        g.setColor(0x203040);
        g.fillRect(0, 0, w, h);
        if (rgba == null) {
            return;
        }
        // anchors of drawImage
        g.drawImage(rgba, 10 + t % 50, 20, Graphics.TOP | Graphics.LEFT);
        g.drawImage(rgba, w / 2, 60, Graphics.HCENTER | Graphics.VCENTER);
        g.drawImage(pal, w - 1, h - 1, Graphics.BOTTOM | Graphics.RIGHT);
        g.drawImage(pal, 0, 40, 0);
        // the eight transforms of drawRegion, with the four corner anchors
        int[] anchors = { Graphics.TOP | Graphics.LEFT, Graphics.TOP | Graphics.RIGHT,
                Graphics.BOTTOM | Graphics.LEFT, Graphics.VCENTER | Graphics.HCENTER };
        for (int tr = 0; tr < 8; tr++) {
            g.drawRegion(rgba, 2, 1, 9, 6, tr, 20 + tr * 26, 100, anchors[tr % 4]);
            g.drawRegion(pal, 1, 0, 5, 7, (tr + t) % 8, 20 + tr * 26, 125, Graphics.TOP | Graphics.LEFT);
        }
        // clip set under a translation stays where it was set
        g.translate(30, 150);
        g.setClip(0, 0, 60, 40);
        g.translate(5, 5);
        g.setColor(0xff8800);
        g.fillRect(-20, -20, 200, 200);
        g.drawImage(rgba, 40, 20, Graphics.TOP | Graphics.LEFT);
        g.translate(-35, -155);
        g.setClip(0, 0, w, h);
        g.clipRect(120, 150, 50, 30);
        g.setColor(0x33cc99);
        g.fillRect(100, 140, 100, 100);
        g.setClip(0, 0, w, h);
        // outlines: midp-rt draws a rectangle as four lines
        g.setColor(0xffffff);
        g.drawRect(100, 190, 40, 20);
        g.drawLine(0, 220, w - 1, 220);
        g.drawLine(50 + t % 20, 200, 50 + t % 20, 230);
        // a mutable image starts transparent under midp-rt
        if (buffer == null) {
            buffer = Image.createImage(32, 16);
            Graphics b = buffer.getGraphics();
            b.setColor(0x00ff00);
            b.fillRect(4, 4, 24, 8);
            b.drawImage(rgba, 0, 0, Graphics.TOP | Graphics.LEFT);
        }
        g.drawImage(buffer, 150, 240, Graphics.TOP | Graphics.LEFT);
        // the last key code as a colour
        g.setColor((lastKey * 0x123457) & 0xffffff);
        g.fillRect(0, h - 12, w, 12);
        paints++;
    }

    protected void keyPressed(int keyCode) {
        lastKey = keyCode;
        repaint();
    }
}
