/*
 *  arlecchino-midp-rt: bridge of MIDP screen frames to the Rust renderer (allods-shell)
 *  over a Unix domain socket.
 *
 *  Purpose: when -Dallods.shell.socket=PATH (or env ALLODS_SHELL_SOCKET=PATH)
 *  is set, subscribe to the DisplayComponent's repaints and stream finished
 *  RGBA8 screen frames to an external renderer process. If the variable is not
 *  set, the bridge is NOT created (the normal midp-rt mode is unchanged).
 *
 *  Wire protocol (mirrors rust-shell/src/ipc.rs, FrameSink is the CLIENT):
 *    handshake once: MAGIC("ALDS",4) | width:u32 LE | height:u32 LE
 *    then a stream of frames: back to back, width*height*4 bytes of RGBA8 each.
 *  Source = client (connect), renderer = server (listen/accept) — the renderer can
 *  be kept alive, we reconnect on disconnects.
 *
 *  The XRGB(0x00RRGGBB) -> RGBA8 conversion (alpha forced to 0xFF) exactly
 *  repeats rust-shell/src/frame.rs::upload_xrgb (the gameplay buffer withAlpha=false
 *  holds 0 in the high byte — otherwise the texture would be transparent).
 *
 *  The repaint thread is NOT blocked: repaintInvoked only copies the latest frame
 *  into the exchange buffer (last-frame-wins) and wakes the background sender thread;
 *  networking and reconnects live in that background thread.
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
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

	/** System property and env name for the renderer socket path. */
	public static final String PROP = "allods.shell.socket";
	public static final String ENV = "ALLODS_SHELL_SOCKET";

	private static final byte[] MAGIC = { 'A', 'L', 'D', 'S' };

	/**
	 * If a socket path is set — create the bridge, subscribe it to the repaints of the
	 * given component and start the background sender thread. Otherwise return null
	 * (the normal midp-rt mode). It could optionally chain to an already installed
	 * listener (windowed mode installs its own) — but MicroEmulator components
	 * hold only one listener, so in windowed mode the bridge replaces it;
	 * this is harmless for the main window (it repaints itself, the listener
	 * is needed only by the zoom window/GIF recorder, both off by default).
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

	// last-frame-wins exchange between the repaint thread and the sender thread.
	private final Object lock = new Object();
	private int[] pendingXrgb; // the latest frame (a copy of imageData); null = nothing new
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

	// ─────────────── repaint thread (fast, no networking) ───────────────
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
				return; // inconsistent frame — skip it
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

	// ─────────────── background sender thread (networking) ───────────────
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
				// INPUT back channel on the same socket: a separate thread reads
				// [kind][code] events and injects them into midp-rt. Writing frames and
				// reading input on one blocking SocketChannel from two threads
				// is a standard NIO scenario.
				final SocketChannel rch = ch;
				reader = new Thread(() -> inputLoop(rch), "allods-shell-input");
				reader.setDaemon(true);
				reader.start();

				streamFrames(ch);
			} catch (IOException e) {
				// disconnected/no renderer — reconnect
			} finally {
				closeQuietly(ch); // closing the channel wakes the reader out of read()
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

	// ─────────────── input back channel (renderer -> Java) ───────────────
	// Key codes are a stable contract with rust-shell/src/ipc.rs::input_code.
	// CHANGE the values in both places in sync.
	private static final String[] CODE_NAMES = new String[32];
	static {
		CODE_NAMES[1] = "UP";
		CODE_NAMES[2] = "DOWN";
		CODE_NAMES[3] = "LEFT";
		CODE_NAMES[4] = "RIGHT";
		CODE_NAMES[5] = "SELECT"; // FIRE / centre
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
				// read exactly 2 bytes (kind, code)
				while (buf.hasRemaining()) {
					int n = ch.read(buf);
					if (n < 0) {
						return; // channel closed
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
			// channel closed/broken — the sender thread will reconnect
		}
	}

	private SocketChannel connect() throws IOException {
		UnixDomainSocketAddress addr = UnixDomainSocketAddress.of(socketPath);
		SocketChannel ch = SocketChannel.open(StandardProtocolFamily.UNIX);
		try {
			ch.connect(addr);
		} catch (IOException e) {
			closeQuietly(ch);
			return null; // the renderer is not listening yet — retry later
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
				rgba.put((byte) 0xFF);       // A forced
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
