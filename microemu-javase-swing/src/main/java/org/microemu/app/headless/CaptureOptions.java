/*
 *  arlecchino-midp-rt: разбор CLI-флагов headless-захвата кадров.
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 *
 *  Свои флаги (снимаются ДО передачи остатка в org.microemu.app.Common):
 *
 *    --frames-dir DIR    включить захват: PNG-кадры + index.json в DIR
 *    --frames N          сколько кадров снять (по умолчанию 30)
 *    --period-ms MS      период между кадрами (по умолчанию 100)
 *    --boot-wait-ms MS   пауза после старта MIDlet до первого кадра (2000)
 *    --inputs FILE       JSON-скрипт ввода [{t_ms,key,action},...]
 *                        (формат difftest Arlecchino, docs/difftest_format.md)
 *    --rms DIR           file-backed RMS ПЛОСКО в DIR (расширение upstream
 *                        `--rms (file|memory)`: значение-путь = каталог)
 *
 *  Плюс автостарт: если включён захват и последний аргумент — существующий
 *  .jar/.jad, MIDlet-1 достаётся из манифеста/джада и аргумент переписывается
 *  в class-name-инвокацию (--appclasspath jar [--propertiesjad jad] Class) —
 *  headless-Launcher без этого не репейнтится и кадры остаются пустыми.
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

	public File rmsDir;

	/** Последняя цель запуска (jar/jad/класс) — попадает в index.json. */
	public String target = "";

	/** Остаток аргументов — передаётся в Common.initParams как есть. */
	public List<String> runtimeArgs = new ArrayList<String>();

	public boolean enabled() {
		return framesDir != null;
	}

	public static final String USAGE = "headless capture: "
			+ "[--frames-dir DIR] [--frames N] [--period-ms MS] [--boot-wait-ms MS] "
			+ "[--inputs FILE] [--rms (file|memory|DIR)] [--no-instrument] "
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
		if (o.enabled()) {
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

	// ─────────────────────── автостарт по MIDlet-1 ───────────────────────────
	private void rewriteAutoStart() {
		if (runtimeArgs.isEmpty()) {
			return;
		}
		String last = (String) runtimeArgs.get(runtimeArgs.size() - 1);
		String lower = last.toLowerCase();
		if (!lower.endsWith(".jar") && !lower.endsWith(".jad")) {
			return; // уже класс — Common сам инстанцирует и стартует
		}
		File f = new File(last);
		if (!f.isFile()) {
			return; // Common выдаст свою ошибку «не найден»
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

	/** 'Имя, /icon.png, the.Class' -> 'the.Class' (или null). */
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

	/** Минимальный парсер .jad/манифеста: 'Key: value' + строки-продолжения. */
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
