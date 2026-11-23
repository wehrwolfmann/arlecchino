/*
 *  arlecchino-midp-rt: deterministic input by tick number.
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
 *
 *  Tick = frame boundary: every Displayable.paint() call via DisplayAccess
 *  and every GameCanvas.flushGraphics(). The tick number matches the number of the
 *  "F <n>" line of the midprt.trace (tick 0 = the first frame).
 *
 *  Without a driver (normal mode) boundary() only writes "F <n>" into the trace, as
 *  before. With a driver (headless --ticks, see org.microemu.app.headless.TickDriver)
 *  at every boundary, in order and in one thread:
 *
 *    1. driver.beforeTick(n) — snapshot of frame n-1, stopping the run at the limit;
 *    2. "F <n>" into the trace;
 *    3. driver.deliver(n)    — the scenario events of this tick; keys go
 *                               to the Displayable SYNCHRONOUSLY (isDelivering()),
 *                               not through the EventDispatcher queue;
 *    4. return — frame n is drawn next.
 *
 *  At this moment the game thread is waiting in serviceRepaints() (Canvas) or is itself
 *  running flushGraphics() (GameCanvas), so the game sees a key press strictly
 *  between the logic of tick n-1 and the drawing of tick n — regardless of the scheduler.
 */
package org.microemu;

import org.microemu.log.MidpTrace;

public final class TickInput {

	/** The harness side: what to do at a tick boundary. */
	public interface Driver {

		/** Before the "F n" line: snapshot of the previous frame, exit at the limit. */
		void beforeTick(int tick);

		/** After the "F n" line, before drawing: deliver the events of tick n. */
		void deliver(int tick);
	}

	private static volatile Driver driver;

	private static volatile Thread deliveringThread;

	private static int tick = 0;

	private TickInput() {
	}

	public static void install(Driver d) {
		driver = d;
	}

	public static boolean active() {
		return driver != null;
	}

	/**
	 * Tick events are being delivered in THIS thread: keys and commands must
	 * run immediately, and repaint() must not be queued (the tick's frame is
	 * about to be drawn anyway).
	 */
	public static boolean isDelivering() {
		return deliveringThread == Thread.currentThread();
	}

	/** Number of the next tick (how many boundaries have been passed). */
	public static synchronized int current() {
		return tick;
	}

	/** Frame boundary. Called before a frame is drawn. */
	public static void boundary() {
		Driver d = driver;
		if (d == null) {
			if (MidpTrace.ON) {
				MidpTrace.frame();
			}
			return;
		}
		synchronized (TickInput.class) {
			int n = tick;
			d.beforeTick(n);
			if (MidpTrace.ON) {
				MidpTrace.frame();
			}
			tick = n + 1;
			Thread prev = deliveringThread;
			deliveringThread = Thread.currentThread();
			try {
				d.deliver(n);
			} finally {
				deliveringThread = prev;
			}
		}
	}
}
