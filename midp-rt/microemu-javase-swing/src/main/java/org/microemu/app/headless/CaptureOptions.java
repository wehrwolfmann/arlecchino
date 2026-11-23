/*
 *  arlecchino-midp-rt: parsing of the headless frame capture CLI flags.
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
 *
 *  Our own flags (stripped BEFORE the rest is passed to org.microemu.app.Common):
 *
 *    --frames-dir DIR    enable capture: PNG frames + index.json in DIR
 *    --frames N          how many frames to take (default 30)
 *    --period-ms MS      period between frames (default 100)
 *    --boot-wait-ms MS   pause after the MIDlet starts before the first frame (2000)
 *    --inputs FILE       JSON input script [{t_ms,key,action},...]
 *                        (Arlecchino difftest format, docs/difftest_format.md);
 *                        with --ticks — events by tick [{tick,key,action},...]
 *    --ticks N           deterministic mode: run exactly N ticks
 *                        (frame boundaries, see org.microemu.TickInput) and exit;
 *                        input is delivered synchronously on its tick, a frame
 *                        (with --frames-dir) is taken on every tick,
 *                        --frames/--period-ms/--boot-wait-ms have no effect
 *    --tick-timeout-ms MS  watchdog for --ticks mode: exit with code 4 if
 *                        no new tick arrived within MS ms of wall time (30000)
 *    --rms DIR           file-backed RMS FLAT in DIR (extension of upstream
 *                        `--rms (file|memory)`: a path value = the directory)
 *
 *  Plus autostart: if capture is on and the last argument is an existing
 *  .jar/.jad, MIDlet-1 is taken from the manifest/jad and the argument is rewritten
 *  into a class-name invocation (--appclasspath jar [--propertiesjad jad] Class) —
 *  without this the headless Launcher never repaints and the frames stay blank.
 */
package org.microemu.app.headless;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStreamReader;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.jar.JarFile;
import java.util.jar.Manifest;

public class CaptureOptions {

	public File framesDir;

	public int frames = 30;

	public int periodMs = 100;

	public int bootWaitMs = 2000;

	public File inputsFile;

	/** Tick mode: how many ticks to run (0 = the normal wall-clock mode). */
	public int ticks = 0;

	public int tickTimeoutMs = 30000;

	public File rmsDir;

	/** The last launch target (jar/jad/class) — goes into index.json. */
	public String target = "";

	/** The remaining arguments — passed to Common.initParams as is. */
	public List<String> runtimeArgs = new ArrayList<String>();

	public boolean enabled() {
		return framesDir != null;
	}

	public boolean tickMode() {
		return ticks > 0;
	}

	public static final String USAGE = "headless capture: "
			+ "[--frames-dir DIR] [--frames N] [--period-ms MS] [--boot-wait-ms MS] "
			+ "[--inputs FILE] [--ticks N] [--tick-timeout-ms MS] [--rms (file|memory|DIR)] [--no-instrument] "
			+ "[--resizableDevice W H] (game.jar | game.jad | MIDletClass)";

	public static CaptureOptions parse(String[] args) {
		CaptureOptions o = new CaptureOptions();
		for (int i = 0; i < args.length; i++) {
			String a = args[i];
			if (a.equals("--frames-dir")) {
				o.framesDir = new File(value(args, ++i, a));
			} else if (a.equals("--frames")) {
				o.frames = intValue(args, ++i, a);
			} else if (a.equals("--period-ms")) {
				o.periodMs = intValue(args, ++i, a);
			} else if (a.equals("--boot-wait-ms")) {
				o.bootWaitMs = intValue(args, ++i, a);
			} else if (a.equals("--ticks")) {
				o.ticks = intValue(args, ++i, a);
				if (o.ticks < 1) {
					throw new IllegalArgumentException("--ticks: ожидается число >= 1\n" + USAGE);
				}
			} else if (a.equals("--tick-timeout-ms")) {
				o.tickTimeoutMs = intValue(args, ++i, a);
			} else if (a.equals("--inputs")) {
				o.inputsFile = new File(value(args, ++i, a));
			} else if (a.equals("--rms")) {
				String v = value(args, ++i, a);
				if (v.equals("file") || v.equals("memory")) {
					o.runtimeArgs.add(a);
					o.runtimeArgs.add(v);
				} else {
					o.rmsDir = new File(v);
				}
			} else {
				o.runtimeArgs.add(a);
			}
		}
		if (o.frames < 1 || o.periodMs < 0 || o.bootWaitMs < 0) {
			throw new IllegalArgumentException("--frames/--period-ms/--boot-wait-ms: "
					+ "ожидаются неотрицательные числа (--frames >= 1)\n" + USAGE);
		}
		if (!o.runtimeArgs.isEmpty()) {
			o.target = (String) o.runtimeArgs.get(o.runtimeArgs.size() - 1);
		}
		if (o.enabled() || o.tickMode()) {
			o.rewriteAutoStart();
		}
		return o;
	}

	private static String value(String[] args, int i, String flag) {
		if (i >= args.length) {
			throw new IllegalArgumentException(flag + ": не хватает значения\n" + USAGE);
		}
		return args[i];
	}

	private static int intValue(String[] args, int i, String flag) {
		try {
			return Integer.parseInt(value(args, i, flag));
		} catch (NumberFormatException e) {
			throw new IllegalArgumentException(flag + ": ожидается число, получено "
					+ args[i] + "\n" + USAGE);
		}
	}

	// ─────────────────────── autostart via MIDlet-1 ──────────────────────────
	private void rewriteAutoStart() {
		if (runtimeArgs.isEmpty()) {
			return;
		}
		String last = (String) runtimeArgs.get(runtimeArgs.size() - 1);
		String lower = last.toLowerCase();
		if (!lower.endsWith(".jar") && !lower.endsWith(".jad")) {
			return; // already a class — Common instantiates and starts it itself
		}
		File f = new File(last);
		if (!f.isFile()) {
			return; // Common will report its own "not found" error
		}
		String cls = null;
		File jar = f;
		File jad = null;
		try {
			if (lower.endsWith(".jad")) {
				Map<String, String> attrs = foldManifest(f);
				cls = midlet1Class((String) attrs.get("MIDlet-1"));
				String jarRef = (String) attrs.get("MIDlet-Jar-URL");
				if (cls == null || jarRef == null || jarRef.indexOf("://") >= 0) {
					cls = null;
				} else {
					jar = new File(f.getParentFile(), jarRef);
					jad = f;
					if (!jar.isFile()) {
						cls = null;
					}
				}
			} else {
				JarFile jf = new JarFile(f);
				try {
					Manifest m = jf.getManifest();
					if (m != null) {
						cls = midlet1Class(m.getMainAttributes().getValue("MIDlet-1"));
					}
				} finally {
					jf.close();
				}
			}
		} catch (Exception e) {
			cls = null;
		}
		if (cls == null) {
			System.err.println("headless: MIDlet-1 не найден в " + last
					+ " — останется Launcher-список (в headless он не репейнтится, "
					+ "кадры будут пустыми)");
			return;
		}
		runtimeArgs.remove(runtimeArgs.size() - 1);
		runtimeArgs.add("--appclasspath");
		runtimeArgs.add(jar.getPath());
		if (jad != null) {
			runtimeArgs.add("--propertiesjad");
			runtimeArgs.add(jad.getPath());
		}
		runtimeArgs.add(cls);
	}

	/** 'Name, /icon.png, the.Class' -> 'the.Class' (or null). */
	static String midlet1Class(String entry) {
		if (entry == null) {
			return null;
		}
		String[] parts = entry.split(",");
		if (parts.length < 3 || parts[2].trim().length() == 0) {
			return null;
		}
		return parts[2].trim();
	}

	/** A minimal .jad/manifest parser: 'Key: value' + continuation lines. */
	static Map<String, String> foldManifest(File f) throws Exception {
		Map<String, String> attrs = new HashMap<String, String>();
		BufferedReader r = new BufferedReader(
				new InputStreamReader(new FileInputStream(f), "UTF-8"));
		try {
			String key = null;
			String line;
			while ((line = r.readLine()) != null) {
				if (line.trim().length() == 0) {
					key = null;
					continue;
				}
				if (line.startsWith(" ") && key != null) {
					attrs.put(key, attrs.get(key) + line.substring(1));
					continue;
				}
				int colon = line.indexOf(':');
				if (colon < 0) {
					key = null;
					continue;
				}
				key = line.substring(0, colon).trim();
				attrs.put(key, line.substring(colon + 1).trim());
			}
		} finally {
			r.close();
		}
		return attrs;
	}
}
