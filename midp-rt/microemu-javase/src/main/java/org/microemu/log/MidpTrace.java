/*
 *  arlecchino-midp-rt: трассировка MIDP-вызовов игры (LGPL 2.1 — см. LICENSE
 *  и NOTICE в корне репозитория).
 *
 *  Зачем: чтобы перенести игру на другой (нативный) движок, нужно точно знать,
 *  какие методы MIDP она реально дёргает и с какими аргументами. Трасса,
 *  снятая под шимом, служит эталоном: последовательность вызовов рисования
 *  на кадр + гистограмма всей сессии.
 *
 *  Включение: -Dmidprt.trace=<файл> (или =- на stdout). Выключено по
 *  умолчанию: единственная проверка статического финального флага ON, поэтому
 *  на обычных прогонах накладных расходов нет.
 *
 *  Формат — построчный текст (легко diff'ать и грепать):
 *      F <номер кадра> t=<мс от старта трассы>   — граница кадра (paint)
 *      <метод> <арг> <арг> ...                   — вызов
 *      I <id> <w>x<h> <источник>                 — создание Image
 *      P <id> <тип> <N>b                         — создание Player (JSR-135)
 *      <метод> "<стор>" len=N hex=…              — запись RMS целиком
 *  В конце (shutdown hook) — блок "# histogram" с числом вызовов по методам.
 *
 *  Ограничение объёма: подробности рисования пишутся только для первых
 *  -Dmidprt.trace.frames кадров (по умолчанию 200), дальше идёт один счётчик.
 *  Редкие и важные вещи — ввод, RMS, звук, смена Displayable, создание
 *  Image — пишутся ВСЕГДА (event/bytes), лимит на них не распространяется.
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

	/** Путь файла трассы или null. Читается один раз при загрузке класса. */
	private static final String TARGET = System.getProperty("midprt.trace");

	/** Трасса включена. Финальный флаг — JIT выкидывает выключённые ветки. */
	public static final boolean ON = TARGET != null && TARGET.length() > 0;

	/** Ограничение объёма: после N кадров пишем только гистограмму. */
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
			Logger.warn("midprt.trace: не удалось открыть " + TARGET + ": " + e);
			return;
		}
		t0 = System.currentTimeMillis();
		out.println("# midp-rt trace, кадров с подробностями: " + MAX_FRAMES);
		Runtime.getRuntime().addShutdownHook(new Thread(new Runnable() {
			public void run() {
				finish();
			}
		}, "midprt-trace-close"));
	}

	/** Граница кадра: вызывается перед Displayable.paint(). */
	public static synchronized void frame() {
		if (out == null) {
			return;
		}
		frame++;
		detail = frame < MAX_FRAMES;
		if (detail) {
			out.println("F " + frame + " t=" + (System.currentTimeMillis() - t0));
		} else if (frame == MAX_FRAMES) {
			out.println("# ... подробности отключены после кадра " + MAX_FRAMES
					+ ", гистограмма продолжает считаться");
			out.flush();
		}
	}

	// ------------------------------------------------------- события (всегда)
	//
	// Ввод, RMS, звук, смена Displayable — редкие и важные: пишутся вне
	// зависимости от лимита подробных кадров.

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

	// ------------------------------------------- вызовы рисования (по кадрам)

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

	/** Вызов с объектом-изображением первым аргументом (drawImage/drawRegion). */
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

	/** Вызов с байтовой полезной нагрузкой (записи RMS): дамп в hex.
	 *  Пишется ВСЕГДА, независимо от лимита подробных кадров — записей мало,
	 *  а формат сейва нужен целиком. */
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

	/** Регистрация созданного Player: стабильный id + тип и размер носителя
	 *  (по размеру опознаётся конкретный /mid/*.mid). */
	public static synchronized void player(Object pl, String type, int nbytes) {
		hit("Manager.createPlayer");
		int id = objId(pl);
		if (out != null) {
			out.println("P " + id + " " + quote(type) + " " + nbytes + "b");
		}
	}

	/** Событие плеера со ссылкой на его id: "Player.start #4". */
	public static synchronized void playerEvent(String m, Object pl) {
		hit(m);
		if (out != null) {
			out.println(m + " #" + objId(pl));
		}
	}

	/** Регистрация созданного Image: стабильный id + источник. */
	public static synchronized void image(Object image, String src, int w, int h) {
		hit("Image.create");
		int id = objId(image);
		if (out != null) {
			out.println("I " + id + " " + w + "x" + h + " " + quote(src));
		}
	}

	// ---------------------------------------------------------------- служебное

	/** Стабильный порядковый номер объекта (изображения, плеера). */
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
		out.println("# histogram (кадров всего: " + (frame + 1) + ")");
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
