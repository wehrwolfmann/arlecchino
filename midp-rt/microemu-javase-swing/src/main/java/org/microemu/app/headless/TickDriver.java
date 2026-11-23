/*
 *  arlecchino-midp-rt: the tick mode harness (headless --ticks N).
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
 *
 *  A deterministic run: neither input, nor frame snapshots, nor the end of the run
 *  depend on the wall clock. Everything happens at the tick boundary (TickInput) in
 *  the thread that draws the frame:
 *
 *    tick n, before "F n": snapshot of frame n-1 (frame_{n-1}.png); when n == N —
 *                       index.json, "TICKS_DONE N" and JVM exit;
 *    tick n, after "F n": scenario events with tick == n in file order,
 *                       synchronously (the game's keyPressed/keyReleased are called
 *                       right here, before frame n is painted).
 *
 *  The only thing that looks at the wall clock is the watchdog: if no new tick
 *  arrives within --tick-timeout-ms, the run is considered hung (exit 4).
 */
package org.microemu.app.headless;

import java.util.List;

import org.microemu.TickInput;

public class TickDriver implements TickInput.Driver {

	private final CaptureOptions opts;

	private final FrameRecorder recorder; // null — without --frames-dir

	private final List<InputScript.Event> events;

	private int next = 0;

	private volatile int lastTick = -1;

	private volatile long lastProgress = System.currentTimeMillis();

	public TickDriver(CaptureOptions opts, FrameRecorder recorder, List<InputScript.Event> events) {
		this.opts = opts;
		this.recorder = recorder;
		this.events = events;
		for (int i = 0; i < events.size(); i++) {
			InputScript.Event e = events.get(i);
			if (e.tick >= opts.ticks) {
				System.err.println("inputs: an event at tick " + e.tick + " is beyond --ticks "
						+ opts.ticks + " — it will not be delivered");
			}
		}
	}

	public void beforeTick(int tick) {
		lastTick = tick;
		lastProgress = System.currentTimeMillis();
		try {
			if (recorder != null && tick > 0) {
				recorder.snapshot(tick - 1);
			}
			if (tick >= opts.ticks) {
				if (recorder != null) {
					recorder.writeIndex(opts);
				}
				System.out.println("TICKS_DONE " + tick);
				System.out.flush();
				System.exit(0);
			}
		} catch (Exception e) {
			e.printStackTrace();
			System.exit(3);
		}
	}

	public void deliver(int tick) {
		while (next < events.size() && events.get(next).tick < tick) {
			next++; // cannot happen: ticks are consecutive, but do not hang on a stray one
		}
		while (next < events.size() && events.get(next).tick == tick) {
			InputScript.inject(events.get(next++));
		}
	}

	/** Main thread: wait for the end of the run (exit happens in beforeTick) or a hang. */
	public int await() {
		while (true) {
			try {
				Thread.sleep(200);
			} catch (InterruptedException e) {
				return 3;
			}
			if (System.currentTimeMillis() - lastProgress > opts.tickTimeoutMs) {
				System.err.println("TICKS_STALLED: no new tick within " + opts.tickTimeoutMs
						+ " ms (last " + lastTick + " of " + opts.ticks
						+ "); has the game stopped drawing frames?");
				return 4;
			}
		}
	}
}
