/*
 *  arlecchino-midp-rt: захват кадров headless-рендера в PNG + index.json.
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 *
 *  Слушает repaint'ы NoUiDisplayComponent (последняя поверхность), а цикл
 *  захвата сэмплирует её каждые period-ms: PNG-кадр frame_NNNN.png, sha256
 *  СЫРЫХ RGB24-байтов (а не PNG-контейнера) и признак blank (все пиксели
 *  одного цвета — на экране ничего не нарисовано). Итог — index.json
 *  (формат зеркалит frames.json из arlecchino/diffrun.py).
 *
 *  События ввода (InputScript) инжектятся best-effort по wall-clock: перед
 *  кадром, у которого now >= t_ms; t_ms отсчитывается от конца boot-wait.
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
	 * Главный цикл: boot-wait, затем frames кадров каждые period-ms с
	 * инжекцией событий ввода; index.json в конце. Возвращает exit-код.
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

	// ───────────────────────────── один кадр ─────────────────────────────────
	private void snapshot(int index) throws Exception {
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

	/** Копия текущей поверхности (под её замком) или чёрный кадр размера LCD. */
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
	private void writeIndex(CaptureOptions opts) throws IOException {
		StringBuilder b = new StringBuilder();
		b.append("{\n");
		b.append("  \"engine\": \"midp-rt\",\n");
		b.append("  \"jar\": ").append(q(opts.target)).append(",\n");
		b.append("  \"frame_period_ms\": ").append(opts.periodMs).append(",\n");
		b.append("  \"boot_wait_ms\": ").append(opts.bootWaitMs).append(",\n");
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

	/** JSON-экранирование строки. */
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
