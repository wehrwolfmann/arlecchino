/*
 *  arlecchino-midp-rt: input script for headless capture.
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
 *
 *  The file format is a JSON array of events (Arlecchino difftest format,
 *  docs/difftest_format.md):
 *
 *      [{"t_ms": 4500, "key": "FIRE", "action": "tap"}, ...]
 *
 *  t_ms counts from the END of boot-wait (the start of frame capture);
 *  instead of t_ms there may be "tick": N — the tick number (frame boundaries, the
 *  "F N" lines of the trace); such a scenario is deterministic and needs --ticks mode
 *  (see TickDriver). Mixing t_ms and tick in one file is not allowed;
 *  key — a key name (UP/DOWN/LEFT/RIGHT, NUM0-9, STAR, POUND, FIRE/SELECT,
 *  SOFT_LEFT/SOFT_RIGHT, CLR) or a number = a raw MIDP keyCode;
 *  action — press | release | tap (= press immediately followed by release).
 *
 *  The JSON parser is minimal (an array of flat objects: strings/numbers/literals) —
 *  made exactly for this format, with no external dependencies.
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
		public long tick = -1; // >= 0 — a tick-based event (--ticks mode)
		public Object key;     // String (a name) or Number (a raw MIDP keyCode)
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

	/** All events of the scenario are tick-based (an empty scenario fits both modes). */
	public static boolean byTick(List<Event> events) {
		return !events.isEmpty() && events.get(0).tick >= 0;
	}

	// ───────────────────────────── injection ─────────────────────────────────
	public static void inject(Event e) {
		String action = (e.action == null) ? "press" : e.action;
		if (!action.equals("press") && !action.equals("release") && !action.equals("tap")) {
			System.err.println("inputs: unknown action " + action + " — skipped");
			return;
		}
		if (action.equals("press") || action.equals("tap")) {
			fire(e.key, true);
		}
		if (action.equals("release") || action.equals("tap")) {
			fire(e.key, false);
		}
	}

	/**
	 * The canonical entry point for live injection of one key (a symbolic name from
	 * KEYMAP, e.g. "UP"/"SELECT"/"NUM5"). Reused by the ShellFrameBridge input back
	 * channel, so that the key mapping lives in one place instead of being
	 * duplicated. Thread-safe: below, everything goes to DisplayAccess.keyPressed,
	 * which puts the event into the EventDispatcher queue (or atomically updates
	 * GameCanvas), so it can be called from any thread.
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
			System.err.println("inputs: unknown key " + key + " — skipped");
			return;
		}
		J2SEButton btn = J2SEDeviceButtonsHelper.getButton(name);
		if (btn == null && name == ButtonName.DELETE) {
			btn = J2SEDeviceButtonsHelper.getButton(ButtonName.BACK_SPACE);
		}
		if (btn == null) {
			System.err.println("inputs: the device has no key " + key + " — skipped");
			return;
		}
		if (org.microemu.TickInput.isDelivering() && DeviceFactory.getDevice().hasRepeatEvents()) {
			// tick mode: on a device with auto-repeat, the release goes through a
			// timer (50 ms of wall time) — we send the raw keyCode synchronously
			rawKey(btn.getKeyCode(), press);
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

	// ──────────────────────────── mini JSON ──────────────────────────────────
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
		int byTick = 0;
		for (int i = 0; i < events.size(); i++) {
			if (events.get(i).tick >= 0) {
				byTick++;
			}
		}
		if (byTick != 0 && byTick != events.size()) {
			throw new IOException("inputs JSON: t_ms-based and tick-based events cannot be mixed in one file");
		}
		// the sort is stable: events of the same tick/moment keep the file order
		Collections.sort(events, new Comparator<Event>() {
			public int compare(Event a, Event b) {
				long x = (a.tick >= 0) ? a.tick : a.tMs;
				long y = (b.tick >= 0) ? b.tick : b.tMs;
				return (x < y) ? -1 : (x > y) ? 1 : 0;
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
			Object tk = obj.get("tick");
			if (tk != null) {
				if (!(tk instanceof Long) || ((Long) tk).longValue() < 0) {
					throw new IOException("inputs JSON: tick must be an integer >= 0, got " + tk);
				}
				if (t != null) {
					throw new IOException("inputs JSON: the event has both t_ms and tick — only one is allowed");
				}
				e.tick = ((Long) tk).longValue();
			}
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
				throw new IOException("inputs JSON: expected ',' or ']' at position " + (p.i - 1));
			}
			p.ws();
		}
		return out;
	}

	/** A pocket parser: an array of flat objects of string/number/literal. */
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
				throw new IOException("inputs JSON: unexpected end of file");
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
				throw new IOException("inputs JSON: expected '" + c + "' at position " + (i - 1));
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
					throw new IOException("inputs JSON: expected ',' or '}' at position " + (i - 1));
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
			throw new IOException("inputs JSON: unexpected character '" + c + "' at position " + i);
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
							throw new IOException("inputs JSON: truncated \\u");
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
				throw new IOException("inputs JSON: not a number: " + tok);
			}
		}
	}
}
