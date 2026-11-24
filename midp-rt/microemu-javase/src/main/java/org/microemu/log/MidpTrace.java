/*
 *  arlecchino-midp-rt: tracing of the game's MIDP calls (LGPL 2.1 — see LICENSE
 *  and NOTICE at the repository root).
 *
 *  Why: to move a game to another (native) engine you need to know exactly
 *  which MIDP methods it really calls and with which arguments. A trace
 *  taken under the shim serves as the reference: the sequence of drawing calls
 *  per frame + a histogram of the whole session.
 *
 *  Enabling: -Dmidprt.trace=<file> (or =- for stdout). Off by
 *  default: the only check is a static final flag ON, so
 *  ordinary runs have no overhead.
 *
 *  The format is line-based text (easy to diff and grep):
 *      F <frame number> t=<ms since trace start> — frame boundary (paint,
 *                                                GameCanvas.flushGraphics);
 *                                                in tick mode — without t=
 *      <method> <arg> <arg> ...                  — a call
 *      I <id> <w>x<h> <source>                   — Image creation
 *      P <id> <type> <N>b                        — Player creation (JSR-135)
 *      <method> "<store>" len=N hex=…            — a whole RMS record write
 *  At the end (shutdown hook) — a "# histogram" block with call counts per method.
 *
 *  Volume limit: drawing details are written only for the first
 *  -Dmidprt.trace.frames frames (200 by default), after that only a counter.
 *  Rare and important things — input, RMS, sound, Displayable changes, Image
 *  creation — are ALWAYS written (event/bytes), the limit does not apply to them.
 */
package org.microemu.log;

import java.io.BufferedWriter;
import java.io.FileWriter;
import java.io.IOException;
import java.io.PrintWriter;
import java.util.IdentityHashMap;
import java.util.Iterator;
import java.util.Map;
import java.util.TreeMap;

public final class MidpTrace {

	/** Path of the trace file or null. Read once when the class is loaded. */
	private static final String TARGET = System.getProperty("midprt.trace");

	/** Tracing is on. A final flag — the JIT drops the disabled branches. */
	public static final boolean ON = TARGET != null && TARGET.length() > 0;

	/** Volume limit: after N frames only the histogram is written. */
	private static final int MAX_FRAMES = Integer.getInteger("midprt.trace.frames",
			200).intValue();

	private static PrintWriter out;

	private static long t0;

	private static int frame = -1;

	private static boolean detail = true;

	private static final Map counts = new TreeMap();

	private static final Map imageIds = new IdentityHashMap();

	private static int nextImageId = 1;

	static {
		if (ON) {
			open();
		}
	}

	private MidpTrace() {
	}

	private static void open() {
		try {
			if ("-".equals(TARGET)) {
				out = new PrintWriter(System.out, true);
			} else {
				out = new PrintWriter(new BufferedWriter(new FileWriter(TARGET)));
			}
		} catch (IOException e) {
			Logger.warn("midprt.trace: cannot open " + TARGET + ": " + e);
			return;
		}
		t0 = System.currentTimeMillis();
		out.println("# midp-rt trace, frames with details: " + MAX_FRAMES);
		Runtime.getRuntime().addShutdownHook(new Thread(new Runnable() {
			public void run() {
				finish();
			}
		}, "midprt-trace-close"));
	}

	/** Frame boundary: called before Displayable.paint(). */
	public static synchronized void frame() {
		if (out == null) {
			return;
		}
		frame++;
		detail = frame < MAX_FRAMES;
		if (detail) {
			// arlecchino-midp-rt fork: in tick mode (headless --ticks) the wall
			// time is not written — the traces of two runs match byte for byte
			if (org.microemu.TickInput.active()) {
				out.println("F " + frame);
			} else {
				out.println("F " + frame + " t=" + (System.currentTimeMillis() - t0));
			}
		} else if (frame == MAX_FRAMES) {
			out.println("# ... details disabled after frame " + MAX_FRAMES
					+ ", the histogram keeps counting");
			out.flush();
		}
	}

	// ------------------------------------------------------- events (always)
	//
	// Input, RMS, sound, Displayable changes are rare and important: they are written
	// regardless of the detailed-frames limit.

	public static synchronized void event(String m) {
		hit(m);
		if (out != null) {
			out.println(m);
		}
	}

	public static synchronized void event(String m, int a) {
		hit(m);
		if (out != null) {
			out.println(m + " " + a);
		}
	}

	public static synchronized void event(String m, int a, int b) {
		hit(m);
		if (out != null) {
			out.println(m + " " + a + " " + b);
		}
	}

	public static synchronized void event(String m, String s) {
		hit(m);
		if (out != null) {
			out.println(m + " " + quote(s));
		}
	}

	public static synchronized void event(String m, String s, int a) {
		hit(m);
		if (out != null) {
			out.println(m + " " + quote(s) + " " + a);
		}
	}

	// ------------------------------------------- drawing calls (per frame)

	public static synchronized void call(String m) {
		hit(m);
		if (detail && out != null) {
			out.println(m);
		}
	}

	public static synchronized void call(String m, int a) {
		hit(m);
		if (detail && out != null) {
			out.println(m + " " + a);
		}
	}

	public static synchronized void call(String m, int a, int b) {
		hit(m);
		if (detail && out != null) {
			out.println(m + " " + a + " " + b);
		}
	}

	public static synchronized void call(String m, int a, int b, int c) {
		hit(m);
		if (detail && out != null) {
			out.println(m + " " + a + " " + b + " " + c);
		}
	}

	public static synchronized void call(String m, int a, int b, int c, int d) {
		hit(m);
		if (detail && out != null) {
			out.println(m + " " + a + " " + b + " " + c + " " + d);
		}
	}

	public static synchronized void call(String m, String s) {
		hit(m);
		if (detail && out != null) {
			out.println(m + " " + quote(s));
		}
	}

	public static synchronized void call(String m, String s, int a) {
		hit(m);
		if (detail && out != null) {
			out.println(m + " " + quote(s) + " " + a);
		}
	}

	public static synchronized void call(String m, String s, int a, int b, int c) {
		hit(m);
		if (detail && out != null) {
			out.println(m + " " + quote(s) + " " + a + " " + b + " " + c);
		}
	}

	/** A call with an image object as the first argument (drawImage/drawRegion). */
	public static synchronized void callImage(String m, Object image, int[] args) {
		hit(m);
		if (detail && out != null) {
			StringBuffer sb = new StringBuffer(m);
			sb.append(" #").append(objId(image));
			for (int i = 0; i < args.length; i++) {
				sb.append(' ').append(args[i]);
			}
			out.println(sb.toString());
		}
	}

	/** A call with a byte payload (RMS writes): a hex dump.
	 *  ALWAYS written, regardless of the detailed-frames limit — there are few writes,
	 *  and the save format is needed in full. */
	public static synchronized void bytes(String m, String name, byte[] data,
			int off, int len) {
		hit(m);
		if (out == null) {
			return;
		}
		StringBuffer sb = new StringBuffer(m);
		sb.append(' ').append(quote(name));
		if (data == null) {
			sb.append(" null");
		} else {
			sb.append(" len=").append(len).append(" hex=");
			for (int i = 0; i < len; i++) {
				int v = data[off + i] & 0xff;
				sb.append(HEX[v >> 4]).append(HEX[v & 0xf]);
			}
		}
		out.println(sb.toString());
		out.flush();
	}

	private static final char[] HEX = "0123456789abcdef".toCharArray();

	/** Registration of a created Player: a stable id + the media type and size
	 *  (the size identifies the specific /mid/*.mid). */
	public static synchronized void player(Object pl, String type, int nbytes) {
		hit("Manager.createPlayer");
		int id = objId(pl);
		if (out != null) {
			out.println("P " + id + " " + quote(type) + " " + nbytes + "b");
		}
	}

	/** A player event referring to its id: "Player.start #4". */
	public static synchronized void playerEvent(String m, Object pl) {
		hit(m);
		if (out != null) {
			out.println(m + " #" + objId(pl));
		}
	}

	/** Registration of a created Image: a stable id + the source. */
	public static synchronized void image(Object image, String src, int w, int h) {
		hit("Image.create");
		int id = objId(image);
		if (out != null) {
			out.println("I " + id + " " + w + "x" + h + " " + quote(src));
		}
	}

	// ---------------------------------------------------------------- internals

	/** A stable sequence number of an object (image, player). */
	private static int objId(Object o) {
		if (o == null) {
			return 0;
		}
		Integer id = (Integer) imageIds.get(o);
		if (id == null) {
			id = new Integer(nextImageId++);
			imageIds.put(o, id);
		}
		return id.intValue();
	}

	private static void hit(String m) {
		int[] c = (int[]) counts.get(m);
		if (c == null) {
			counts.put(m, new int[] { 1 });
		} else {
			c[0]++;
		}
	}

	private static String quote(String s) {
		if (s == null) {
			return "null";
		}
		StringBuffer sb = new StringBuffer(s.length() + 2);
		sb.append('"');
		for (int i = 0; i < s.length(); i++) {
			char ch = s.charAt(i);
			if (ch == '"' || ch == '\\') {
				sb.append('\\').append(ch);
			} else if (ch == '\n') {
				sb.append("\\n");
			} else if (ch < 0x20) {
				sb.append(' ');
			} else {
				sb.append(ch);
			}
		}
		return sb.append('"').toString();
	}

	static synchronized void finish() {
		if (out == null) {
			return;
		}
		out.println("# histogram (frames total: " + (frame + 1) + ")");
		for (Iterator it = counts.entrySet().iterator(); it.hasNext();) {
			Map.Entry e = (Map.Entry) it.next();
			out.println("# " + e.getKey() + " " + ((int[]) e.getValue())[0]);
		}
		out.flush();
		if (!"-".equals(TARGET)) {
			out.close();
		}
		out = null;
	}
}
