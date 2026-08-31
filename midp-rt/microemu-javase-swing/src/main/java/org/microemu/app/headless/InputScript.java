/*
 *  arlecchino-midp-rt: скрипт ввода для headless-захвата.
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 *
 *  Формат файла — JSON-массив событий (формат difftest Arlecchino,
 *  docs/difftest_format.md):
 *
 *      [{"t_ms": 4500, "key": "FIRE", "action": "tap"}, ...]
 *
 *  t_ms отсчитывается от КОНЦА boot-wait (начала захвата кадров);
 *  key — имя клавиши (UP/DOWN/LEFT/RIGHT, NUM0-9, STAR, POUND, FIRE/SELECT,
 *  SOFT_LEFT/SOFT_RIGHT, CLR) либо число = сырой MIDP-keyCode;
 *  action — press | release | tap (= press и сразу release).
 *
 *  JSON-парсер минимальный (массив плоских объектов: строки/числа/литералы) —
 *  ровно под этот формат, без внешних зависимостей.
 */
package org.microemu.app.headless;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import org.microemu.DisplayAccess;
import org.microemu.MIDletAccess;
import org.microemu.MIDletBridge;
import org.microemu.device.DeviceFactory;
import org.microemu.device.impl.ButtonName;
import org.microemu.device.j2se.J2SEButton;
import org.microemu.device.j2se.J2SEDeviceButtonsHelper;
import org.microemu.device.j2se.J2SEInputMethod;

public class InputScript {

	public static class Event {
		public long tMs;
		public Object key;     // String (имя) либо Number (сырой MIDP-keyCode)
		public String action;  // press | release | tap
	}

	private static final Map<String, ButtonName> KEYMAP = new HashMap<String, ButtonName>();
	static {
		KEYMAP.put("UP", ButtonName.UP);
		KEYMAP.put("DOWN", ButtonName.DOWN);
		KEYMAP.put("LEFT", ButtonName.LEFT);
		KEYMAP.put("RIGHT", ButtonName.RIGHT);
		KEYMAP.put("FIRE", ButtonName.SELECT);
		KEYMAP.put("SELECT", ButtonName.SELECT);
		KEYMAP.put("SOFT3", ButtonName.SELECT);
		KEYMAP.put("SOFT_LEFT", ButtonName.SOFT1);
		KEYMAP.put("SOFT_RIGHT", ButtonName.SOFT2);
		KEYMAP.put("NUM0", ButtonName.KEY_NUM0);
		KEYMAP.put("NUM1", ButtonName.KEY_NUM1);
		KEYMAP.put("NUM2", ButtonName.KEY_NUM2);
		KEYMAP.put("NUM3", ButtonName.KEY_NUM3);
		KEYMAP.put("NUM4", ButtonName.KEY_NUM4);
		KEYMAP.put("NUM5", ButtonName.KEY_NUM5);
		KEYMAP.put("NUM6", ButtonName.KEY_NUM6);
		KEYMAP.put("NUM7", ButtonName.KEY_NUM7);
		KEYMAP.put("NUM8", ButtonName.KEY_NUM8);
		KEYMAP.put("NUM9", ButtonName.KEY_NUM9);
		KEYMAP.put("STAR", ButtonName.KEY_STAR);
		KEYMAP.put("POUND", ButtonName.KEY_POUND);
		KEYMAP.put("CLR", ButtonName.DELETE);
	}

	// ───────────────────────────── инжекция ──────────────────────────────────
	public static void inject(Event e) {
		String action = (e.action == null) ? "press" : e.action;
		if (action.equals("press") || action.equals("tap")) {
			fire(e.key, true);
		}
		if (action.equals("release") || action.equals("tap")) {
			fire(e.key, false);
		}
	}

	/**
	 * Канонический вход для живой инжекции одной клавиши (символическое имя из
	 * KEYMAP, напр. "UP"/"SELECT"/"NUM5"). Переиспользуется обратным каналом
	 * ввода ShellFrameBridge — чтобы маппинг клавиш жил в одном месте, а не
	 * дублировался. Поток-безопасно: ниже всё уходит в DisplayAccess.keyPressed,
	 * который кладёт событие в очередь EventDispatcher (или атомарно обновляет
	 * GameCanvas), так что вызывать можно из любого потока.
	 */
	public static void injectKey(String name, boolean press) {
		fire(name, press);
	}

	private static void fire(Object key, boolean press) {
		if (key instanceof Number) {
			rawKey(((Number) key).intValue(), press);
			return;
		}
		ButtonName name = (ButtonName) KEYMAP.get(String.valueOf(key).toUpperCase());
		if (name == null) {
			System.err.println("inputs: неизвестная клавиша " + key + " — пропущена");
			return;
		}
		J2SEButton btn = J2SEDeviceButtonsHelper.getButton(name);
		if (btn == null && name == ButtonName.DELETE) {
			btn = J2SEDeviceButtonsHelper.getButton(ButtonName.BACK_SPACE);
		}
		if (btn == null) {
			System.err.println("inputs: у устройства нет кнопки " + key + " — пропущена");
			return;
		}
		J2SEInputMethod im = (J2SEInputMethod) DeviceFactory.getDevice().getInputMethod();
		if (press) {
			im.buttonPressed(btn, '\0');
		} else {
			im.buttonReleased(btn, '\0');
		}
	}

	private static void rawKey(int keyCode, boolean press) {
		MIDletAccess ma = MIDletBridge.getMIDletAccess();
		if (ma == null) {
			return;
		}
		DisplayAccess da = ma.getDisplayAccess();
		if (da == null) {
			return;
		}
		if (press) {
			da.keyPressed(keyCode);
		} else {
			da.keyReleased(keyCode);
		}
	}

	// ──────────────────────────── мини-JSON ──────────────────────────────────
	public static List<Event> parse(File f) throws IOException {
		byte[] raw = new byte[(int) f.length()];
		InputStream in = new FileInputStream(f);
		try {
			int off = 0;
			while (off < raw.length) {
				int n = in.read(raw, off, raw.length - off);
				if (n < 0) {
					break;
				}
				off += n;
			}
		} finally {
			in.close();
		}
		List<Event> events = parseText(new String(raw, "UTF-8"));
		Collections.sort(events, new Comparator<Event>() {
			public int compare(Event a, Event b) {
				return (a.tMs < b.tMs) ? -1 : (a.tMs > b.tMs) ? 1 : 0;
			}
		});
		return events;
	}

	static List<Event> parseText(String text) throws IOException {
		P p = new P(text);
		List<Event> out = new ArrayList<Event>();
		p.ws();
		p.expect('[');
		p.ws();
		if (p.peek() == ']') {
			p.next();
			return out;
		}
		while (true) {
			Map<String, Object> obj = p.object();
			Event e = new Event();
			Object t = obj.get("t_ms");
			e.tMs = (t instanceof Number) ? ((Number) t).longValue() : 0L;
			e.key = obj.get("key");
			Object a = obj.get("action");
			e.action = (a == null) ? "press" : a.toString();
			out.add(e);
			p.ws();
			char c = p.next();
			if (c == ']') {
				break;
			}
			if (c != ',') {
				throw new IOException("inputs JSON: ожидалась ',' или ']' на позиции " + (p.i - 1));
			}
			p.ws();
		}
		return out;
	}

	/** Карманный разборщик: массив плоских объектов строка/число/литерал. */
	private static class P {
		final String s;
		int i;

		P(String s) {
			this.s = s;
		}

		void ws() {
			while (i < s.length() && Character.isWhitespace(s.charAt(i))) {
				i++;
			}
		}

		char peek() throws IOException {
			if (i >= s.length()) {
				throw new IOException("inputs JSON: неожиданный конец файла");
			}
			return s.charAt(i);
		}

		char next() throws IOException {
			char c = peek();
			i++;
			return c;
		}

		void expect(char c) throws IOException {
			if (next() != c) {
				throw new IOException("inputs JSON: ожидался '" + c + "' на позиции " + (i - 1));
			}
		}

		Map<String, Object> object() throws IOException {
			Map<String, Object> m = new HashMap<String, Object>();
			ws();
			expect('{');
			ws();
			if (peek() == '}') {
				next();
				return m;
			}
			while (true) {
				ws();
				String k = string();
				ws();
				expect(':');
				ws();
				m.put(k, value());
				ws();
				char c = next();
				if (c == '}') {
					break;
				}
				if (c != ',') {
					throw new IOException("inputs JSON: ожидалась ',' или '}' на позиции " + (i - 1));
				}
			}
			return m;
		}

		Object value() throws IOException {
			char c = peek();
			if (c == '"') {
				return string();
			}
			if (c == '-' || (c >= '0' && c <= '9')) {
				return number();
			}
			if (s.startsWith("true", i)) {
				i += 4;
				return Boolean.TRUE;
			}
			if (s.startsWith("false", i)) {
				i += 5;
				return Boolean.FALSE;
			}
			if (s.startsWith("null", i)) {
				i += 4;
				return null;
			}
			throw new IOException("inputs JSON: неожиданный символ '" + c + "' на позиции " + i);
		}

		String string() throws IOException {
			expect('"');
			StringBuilder b = new StringBuilder();
			while (true) {
				char c = next();
				if (c == '"') {
					return b.toString();
				}
				if (c == '\\') {
					char e = next();
					switch (e) {
					case 'n': b.append('\n'); break;
					case 't': b.append('\t'); break;
					case 'r': b.append('\r'); break;
					case 'b': b.append('\b'); break;
					case 'f': b.append('\f'); break;
					case 'u':
						if (i + 4 > s.length()) {
							throw new IOException("inputs JSON: оборванный \\u");
						}
						b.append((char) Integer.parseInt(s.substring(i, i + 4), 16));
						i += 4;
						break;
					default: b.append(e);
					}
				} else {
					b.append(c);
				}
			}
		}

		Number number() throws IOException {
			int start = i;
			boolean fp = false;
			while (i < s.length()) {
				char c = s.charAt(i);
				if (c == '.' || c == 'e' || c == 'E') {
					fp = true;
				}
				if (c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E'
						|| (c >= '0' && c <= '9')) {
					i++;
				} else {
					break;
				}
			}
			String tok = s.substring(start, i);
			try {
				return fp ? (Number) Double.valueOf(tok) : (Number) Long.valueOf(tok);
			} catch (NumberFormatException e) {
				throw new IOException("inputs JSON: не число: " + tok);
			}
		}
	}
}
