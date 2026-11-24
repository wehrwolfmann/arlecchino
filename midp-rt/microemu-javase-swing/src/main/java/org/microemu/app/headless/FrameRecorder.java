/*
 *  arlecchino-midp-rt: capture of headless rendering frames into PNG + index.json.
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
 *
 *  Listens to NoUiDisplayComponent repaints (the latest surface), and the capture
 *  loop samples it every period-ms: a PNG frame frame_NNNN.png, the sha256 of the
 *  RAW RGB24 bytes (not of the PNG container) and a blank flag (all pixels
 *  of one colour — nothing is drawn on screen). The result is index.json
 *  (the format mirrors frames.json from arlecchino/diffrun.py).
 *
 *  Input events (InputScript) are injected best-effort by wall clock: before
 *  the frame for which now >= t_ms; t_ms counts from the end of boot-wait.
 *  The deterministic mode (--ticks) takes frames not here but at tick
 *  boundaries — see TickDriver.
 */
package org.microemu.app.headless;

import java.awt.image.BufferedImage;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.List;

import javax.imageio.ImageIO;

import org.microemu.app.ui.DisplayRepaintListener;
import org.microemu.device.Device;
import org.microemu.device.DeviceFactory;
import org.microemu.device.j2se.J2SEGraphicsSurface;

public class FrameRecorder implements DisplayRepaintListener {

	private volatile J2SEGraphicsSurface lastSurface;

	private final File framesDir;

	private static class Frame {
		int index;
		String file;
		String sha256;
		int width;
		int height;
		boolean blank;
	}

	private final List<Frame> recorded = new ArrayList<Frame>();

	public FrameRecorder(File framesDir) {
		this.framesDir = framesDir;
		framesDir.mkdirs();
	}

	public void repaintInvoked(Object repaintObject) {
		lastSurface = (J2SEGraphicsSurface) repaintObject;
	}

	/**
	 * Main loop: boot-wait, then `frames` frames every period-ms with
	 * injection of input events; index.json at the end. Returns the exit code.
	 */
	public int captureLoop(CaptureOptions opts) {
		List<InputScript.Event> events;
		try {
			events = (opts.inputsFile != null)
					? InputScript.parse(opts.inputsFile)
					: new ArrayList<InputScript.Event>();
		} catch (IOException e) {
			System.err.println("inputs: " + e.getMessage());
			return 2;
		}
		if (InputScript.byTick(events)) {
			System.err.println("inputs: tick-based events require --ticks N mode");
			return 2;
		}
		sleep(opts.bootWaitMs);
		try {
			long t0 = System.currentTimeMillis();
			int next = 0;
			for (int i = 0; i < opts.frames; i++) {
				long now = System.currentTimeMillis() - t0;
				while (next < events.size() && events.get(next).tMs <= now) {
					InputScript.inject(events.get(next++));
				}
				snapshot(i);
				sleep(opts.periodMs);
			}
			writeIndex(opts);
		} catch (Exception e) {
			e.printStackTrace();
			return 3;
		}
		System.out.println("FRAMES_WRITTEN " + recorded.size() + " -> "
				+ new File(framesDir, "index.json").getPath());
		return 0;
	}

	private static void sleep(long ms) {
		try {
			Thread.sleep(ms);
		} catch (InterruptedException e) {
			Thread.currentThread().interrupt();
		}
	}

	// ───────────────────────────── one frame ─────────────────────────────────
	void snapshot(int index) throws Exception {
		BufferedImage img = copyCurrent();
		int w = img.getWidth();
		int h = img.getHeight();
		int[] px = img.getRGB(0, 0, w, h, null, 0, w);
		byte[] rgb = new byte[px.length * 3];
		boolean blank = true;
		for (int i = 0; i < px.length; i++) {
			int p = px[i];
			rgb[i * 3] = (byte) (p >> 16);
			rgb[i * 3 + 1] = (byte) (p >> 8);
			rgb[i * 3 + 2] = (byte) p;
			if ((p & 0xFFFFFF) != (px[0] & 0xFFFFFF)) {
				blank = false;
			}
		}
		Frame f = new Frame();
		f.index = index;
		f.file = String.format("frame_%04d.png", new Object[] { Integer.valueOf(index) });
		f.sha256 = hex(MessageDigest.getInstance("SHA-256").digest(rgb));
		f.width = w;
		f.height = h;
		f.blank = blank;
		ImageIO.write(img, "png", new File(framesDir, f.file));
		recorded.add(f);
	}

	/** A copy of the current surface (under its lock) or a black LCD-sized frame. */
	private BufferedImage copyCurrent() {
		J2SEGraphicsSurface s = lastSurface;
		if (s == null) {
			int w = 240, h = 320;
			Device device = DeviceFactory.getDevice();
			if (device != null) {
				w = device.getDeviceDisplay().getFullWidth();
				h = device.getDeviceDisplay().getFullHeight();
			}
			return new BufferedImage(w, h, BufferedImage.TYPE_INT_RGB);
		}
		synchronized (s) {
			BufferedImage src = s.getImage();
			BufferedImage copy = new BufferedImage(src.getWidth(), src.getHeight(),
					BufferedImage.TYPE_INT_RGB);
			copy.getGraphics().drawImage(src, 0, 0, null);
			return copy;
		}
	}

	private static final char[] HEX = "0123456789abcdef".toCharArray();

	private static String hex(byte[] digest) {
		StringBuilder b = new StringBuilder(digest.length * 2);
		for (int i = 0; i < digest.length; i++) {
			b.append(HEX[(digest[i] >> 4) & 0xF]).append(HEX[digest[i] & 0xF]);
		}
		return b.toString();
	}

	// ───────────────────────────── index.json ────────────────────────────────
	void writeIndex(CaptureOptions opts) throws IOException {
		StringBuilder b = new StringBuilder();
		b.append("{\n");
		b.append("  \"engine\": \"midp-rt\",\n");
		b.append("  \"jar\": ").append(q(opts.target)).append(",\n");
		b.append("  \"frame_period_ms\": ").append(opts.periodMs).append(",\n");
		b.append("  \"boot_wait_ms\": ").append(opts.bootWaitMs).append(",\n");
		if (opts.tickMode()) {
			// frame i = the screen after tick i is drawn (frame_period_ms has no effect)
			b.append("  \"ticks\": ").append(opts.ticks).append(",\n");
		}
		b.append("  \"rms_dir\": ")
				.append(opts.rmsDir == null ? "null" : q(opts.rmsDir.getPath()))
				.append(",\n");
		b.append("  \"frames\": [\n");
		for (int i = 0; i < recorded.size(); i++) {
			Frame f = recorded.get(i);
			b.append("    {\"index\": ").append(f.index)
					.append(", \"file\": ").append(q(f.file))
					.append(", \"sha256\": ").append(q(f.sha256))
					.append(", \"width\": ").append(f.width)
					.append(", \"height\": ").append(f.height)
					.append(", \"blank\": ").append(f.blank)
					.append(i + 1 < recorded.size() ? "},\n" : "}\n");
		}
		b.append("  ]\n}\n");
		Writer w = new OutputStreamWriter(
				new FileOutputStream(new File(framesDir, "index.json")), "UTF-8");
		try {
			w.write(b.toString());
		} finally {
			w.close();
		}
	}

	/** JSON string escaping. */
	private static String q(String s) {
		StringBuilder b = new StringBuilder(s.length() + 2);
		b.append('"');
		for (int i = 0; i < s.length(); i++) {
			char c = s.charAt(i);
			if (c == '"' || c == '\\') {
				b.append('\\').append(c);
			} else if (c < 0x20) {
				b.append(String.format("\\u%04x", new Object[] { Integer.valueOf(c) }));
			} else {
				b.append(c);
			}
		}
		return b.append('"').toString();
	}
}
