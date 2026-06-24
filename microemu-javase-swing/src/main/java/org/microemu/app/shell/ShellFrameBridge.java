/*
 *  arlecchino-midp-rt: мост кадров MIDP-экрана в Rust-рендер (allods-shell)
 *  через Unix-domain-сокет.
 *
 *  Назначение: при заданной переменной -Dallods.shell.socket=PATH (или env
 *  ALLODS_SHELL_SOCKET=PATH) подписаться на repaint'ы DisplayComponent'а и
 *  гнать готовые RGBA8-кадры экрана в внешний процесс-рендер. Если переменной
 *  нет — мост НЕ создаётся (обычный режим midp-rt не меняется).
 *
 *  Протокол провода (зеркалит rust-shell/src/ipc.rs, FrameSink — КЛИЕНТ):
 *    хендшейк один раз: MAGIC("ALDS",4) | width:u32 LE | height:u32 LE
 *    затем поток кадров: подряд по width*height*4 байт RGBA8.
 *  Источник = клиент (connect), рендер = сервер (listen/accept) — рендер можно
 *  держать живым, переподключаемся при разрывах.
 *
 *  Конверсия XRGB(0x00RRGGBB) -> RGBA8 (alpha форсируется 0xFF) дословно
 *  повторяет rust-shell/src/frame.rs::upload_xrgb (геймплей-буфер withAlpha=false
 *  держит 0 в старшем байте — иначе текстура была бы прозрачной).
 *
 *  Поток репейнта НЕ блокируется: repaintInvoked только копирует последний кадр
 *  в буфер обмена (last-frame-wins) и будит фоновый поток-отправитель; сеть и
 *  переподключения живут в этом фоновом потоке.
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 */
package org.microemu.app.shell;

import java.io.IOException;
import java.net.StandardProtocolFamily;
import java.net.UnixDomainSocketAddress;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.channels.SocketChannel;
import java.nio.file.Path;

import org.microemu.DisplayComponent;
import org.microemu.app.ui.DisplayRepaintListener;
import org.microemu.device.j2se.J2SEGraphicsSurface;
import org.microemu.device.j2se.J2SEGraphicsSurfaceDisplayComponent;

public final class ShellFrameBridge implements DisplayRepaintListener {

	/** System property и env-имя для пути сокета рендера. */
	public static final String PROP = "allods.shell.socket";
	public static final String ENV = "ALLODS_SHELL_SOCKET";

	private static final byte[] MAGIC = { 'A', 'L', 'D', 'S' };

	/**
	 * Если задан путь сокета — создать мост, подписать его на repaint'ы данного
	 * компонента и запустить фоновый поток-отправитель. Иначе вернуть null
	 * (обычный режим midp-rt). Может опционально чейниться к уже стоящему
	 * слушателю (windowed-режим вешает свой) — здесь компоненты MicroEmulator'а
	 * держат лишь один слушатель, поэтому в windowed режиме мост подменяет его;
	 * для основного окна это безвредно (оно перерисовывается само, listener
	 * нужен лишь zoom-окну/GIF-рекордеру, по умолчанию выключенным).
	 */
	public static ShellFrameBridge attachIfEnabled(DisplayComponent component) {
		String path = System.getProperty(PROP);
		if (path == null || path.isEmpty()) {
			String env = System.getenv(ENV);
			if (env != null && !env.isEmpty()) {
				path = env;
			}
		}
		if (path == null || path.isEmpty()) {
			return null;
		}
		if (!(component instanceof J2SEGraphicsSurfaceDisplayComponent)) {
			System.err.println("[allods-shell] DisplayComponent не поддерживает "
					+ "поверхность кадра — мост не активирован");
			return null;
		}
		ShellFrameBridge bridge = new ShellFrameBridge(path);
		((J2SEGraphicsSurfaceDisplayComponent) component).addDisplayRepaintListener(bridge);
		bridge.start();
		System.err.println("[allods-shell] мост кадров включён -> " + path);
		return bridge;
	}

	private final Path socketPath;
	private final Thread sender;
	private volatile boolean running = true;

	// last-frame-wins обмен между потоком репейнта и потоком-отправителем.
	private final Object lock = new Object();
	private int[] pendingXrgb; // последний кадр (копия imageData); null = нет нового
	private int frameW;
	private int frameH;

	private ShellFrameBridge(String path) {
		this.socketPath = Path.of(path);
		this.sender = new Thread(this::sendLoop, "allods-shell-sender");
		this.sender.setDaemon(true);
	}

	private void start() {
		sender.start();
	}

	public void stop() {
		running = false;
		synchronized (lock) {
			lock.notifyAll();
		}
	}

	// ─────────────── поток репейнта (быстрый, без сети) ───────────────
	public void repaintInvoked(Object repaintObject) {
		if (!(repaintObject instanceof J2SEGraphicsSurface)) {
			return;
		}
		J2SEGraphicsSurface surface = (J2SEGraphicsSurface) repaintObject;
		int w, h;
		int[] copy;
		synchronized (surface) {
			int[] data = surface.getImageData();
			if (data == null) {
				return;
			}
			w = surface.getImage().getWidth();
			h = surface.getImage().getHeight();
			if (data.length != w * h) {
				return; // несогласованный кадр — пропускаем
			}
			copy = data.clone();
		}
		synchronized (lock) {
			pendingXrgb = copy;
			frameW = w;
			frameH = h;
			lock.notifyAll();
		}
	}

	// ─────────────── фоновый поток-отправитель (сеть) ───────────────
	private void sendLoop() {
		while (running) {
			SocketChannel ch = null;
			Thread reader = null;
			try {
				ch = connect();
				if (ch == null) {
					sleep(200);
					continue;
				}
				// Обратный канал ВВОДА на том же сокете: отдельный поток читает
				// события [kind][code] и инжектит их в midp-rt. Запись кадров и
				// чтение ввода на одном блокирующем SocketChannel из двух потоков
				// — штатный сценарий NIO.
				final SocketChannel rch = ch;
				reader = new Thread(() -> inputLoop(rch), "allods-shell-input");
				reader.setDaemon(true);
				reader.start();

				streamFrames(ch);
			} catch (IOException e) {
				// разрыв/нет рендера — переподключаемся
			} finally {
				closeQuietly(ch); // закрытие канала разбудит reader из read()
				if (reader != null) {
					try {
						reader.join(500);
					} catch (InterruptedException ie) {
						Thread.currentThread().interrupt();
					}
				}
			}
			if (running) {
				sleep(200);
			}
		}
	}

	// ─────────────── обратный канал ввода (renderer -> Java) ───────────────
	// Коды клавиш — стабильный контракт с rust-shell/src/ipc.rs::input_code.
	// Значения МЕНЯТЬ синхронно в обоих местах.
	private static final String[] CODE_NAMES = new String[32];
	static {
		CODE_NAMES[1] = "UP";
		CODE_NAMES[2] = "DOWN";
		CODE_NAMES[3] = "LEFT";
		CODE_NAMES[4] = "RIGHT";
		CODE_NAMES[5] = "SELECT"; // FIRE / центр
		CODE_NAMES[6] = "SOFT_LEFT";
		CODE_NAMES[7] = "SOFT_RIGHT";
		CODE_NAMES[10] = "NUM0";
		CODE_NAMES[11] = "NUM1";
		CODE_NAMES[12] = "NUM2";
		CODE_NAMES[13] = "NUM3";
		CODE_NAMES[14] = "NUM4";
		CODE_NAMES[15] = "NUM5";
		CODE_NAMES[16] = "NUM6";
		CODE_NAMES[17] = "NUM7";
		CODE_NAMES[18] = "NUM8";
		CODE_NAMES[19] = "NUM9";
		CODE_NAMES[20] = "STAR";
		CODE_NAMES[21] = "POUND";
		CODE_NAMES[22] = "CLR";
	}

	private void inputLoop(SocketChannel ch) {
		ByteBuffer buf = ByteBuffer.allocate(2);
		try {
			while (running) {
				buf.clear();
				// читаем ровно 2 байта (kind, code)
				while (buf.hasRemaining()) {
					int n = ch.read(buf);
					if (n < 0) {
						return; // канал закрыт
					}
				}
				buf.flip();
				int kind = buf.get() & 0xFF;
				int code = buf.get() & 0xFF;
				String name = (code >= 0 && code < CODE_NAMES.length) ? CODE_NAMES[code] : null;
				if (name == null) {
					System.err.println("[allods-shell] неизвестный код ввода " + code);
					continue;
				}
				boolean press = (kind == 1);
				try {
					org.microemu.app.headless.InputScript.injectKey(name, press);
				} catch (RuntimeException re) {
					System.err.println("[allods-shell] инжекция ввода " + name
							+ " не удалась: " + re);
				}
			}
		} catch (IOException e) {
			// канал закрыт/разорван — поток-отправитель переподключится
		}
	}

	private SocketChannel connect() throws IOException {
		UnixDomainSocketAddress addr = UnixDomainSocketAddress.of(socketPath);
		SocketChannel ch = SocketChannel.open(StandardProtocolFamily.UNIX);
		try {
			ch.connect(addr);
		} catch (IOException e) {
			closeQuietly(ch);
			return null; // рендер ещё не слушает — повтор позже
		}
		return ch;
	}

	private void streamFrames(SocketChannel ch) throws IOException {
		boolean handshakeSent = false;
		ByteBuffer rgba = null;
		System.err.println("[allods-shell] подключился к рендеру " + socketPath);
		while (running) {
			int[] xrgb;
			int w, h;
			synchronized (lock) {
				while (running && pendingXrgb == null) {
					try {
						lock.wait();
					} catch (InterruptedException ie) {
						Thread.currentThread().interrupt();
						return;
					}
				}
				if (!running) {
					return;
				}
				xrgb = pendingXrgb;
				w = frameW;
				h = frameH;
				pendingXrgb = null;
			}

			if (!handshakeSent) {
				ByteBuffer hdr = ByteBuffer.allocate(12).order(ByteOrder.LITTLE_ENDIAN);
				hdr.put(MAGIC);
				hdr.putInt(w);
				hdr.putInt(h);
				hdr.flip();
				writeFully(ch, hdr);
				handshakeSent = true;
				rgba = ByteBuffer.allocate(w * h * 4);
			}

			rgba.clear();
			for (int i = 0; i < xrgb.length; i++) {
				int px = xrgb[i];
				rgba.put((byte) (px >> 16)); // R
				rgba.put((byte) (px >> 8));  // G
				rgba.put((byte) px);         // B
				rgba.put((byte) 0xFF);       // A форсирован
			}
			rgba.flip();
			writeFully(ch, rgba);
		}
	}

	private static void writeFully(SocketChannel ch, ByteBuffer buf) throws IOException {
		while (buf.hasRemaining()) {
			if (ch.write(buf) < 0) {
				throw new IOException("сокет закрыт при записи");
			}
		}
	}

	private static void sleep(long ms) {
		try {
			Thread.sleep(ms);
		} catch (InterruptedException e) {
			Thread.currentThread().interrupt();
		}
	}

	private static void closeQuietly(SocketChannel ch) {
		if (ch != null) {
			try {
				ch.close();
			} catch (IOException ignore) {
				// no-op
			}
		}
	}
}
