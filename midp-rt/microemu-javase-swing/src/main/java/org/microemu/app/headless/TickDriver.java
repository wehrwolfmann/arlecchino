/*
 *  arlecchino-midp-rt: стенд режима тиков (headless --ticks N).
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 *
 *  Детерминированный прогон: ни ввод, ни снимки кадров, ни конец прогона не
 *  зависят от часов стены. Всё происходит на границе тика (TickInput) в
 *  потоке, который рисует кадр:
 *
 *    тик n, до "F n":  снимок кадра n-1 (frame_{n-1}.png), при n == N —
 *                       index.json, "TICKS_DONE N" и выход из JVM;
 *    тик n, после "F n": события сценария с tick == n по порядку файла,
 *                       синхронно (keyPressed/keyReleased игры вызываются
 *                       прямо здесь, до paint кадра n).
 *
 *  Единственное, что смотрит на часы стены, — сторож: если новый тик не
 *  наступил за --tick-timeout-ms, прогон считается зависшим (выход 4).
 */
package org.microemu.app.headless;

import java.util.List;

import org.microemu.TickInput;

public class TickDriver implements TickInput.Driver {

	private final CaptureOptions opts;

	private final FrameRecorder recorder; // null — без --frames-dir

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
				System.err.println("inputs: событие на тике " + e.tick + " за пределами --ticks "
						+ opts.ticks + " — не будет доставлено");
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
			next++; // не бывает: тики идут подряд, но не зависаем на чужом
		}
		while (next < events.size() && events.get(next).tick == tick) {
			InputScript.inject(events.get(next++));
		}
	}

	/** Поток main: ждать конца прогона (выход — из beforeTick) или зависания. */
	public int await() {
		while (true) {
			try {
				Thread.sleep(200);
			} catch (InterruptedException e) {
				return 3;
			}
			if (System.currentTimeMillis() - lastProgress > opts.tickTimeoutMs) {
				System.err.println("TICKS_STALLED: за " + opts.tickTimeoutMs
						+ " мс не наступил новый тик (последний " + lastTick + " из " + opts.ticks
						+ "); игра перестала рисовать кадры?");
				return 4;
			}
		}
	}
}
