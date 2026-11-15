/*
 *  arlecchino-midp-rt: детерминированный ввод по номеру тика.
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 *
 *  Тик = граница кадра: каждый вызов Displayable.paint() через DisplayAccess
 *  и каждый GameCanvas.flushGraphics(). Номер тика совпадает с номером строки
 *  "F <n>" трассы midprt.trace (тик 0 = первый кадр).
 *
 *  Без драйвера (обычный режим) boundary() только пишет "F <n>" в трассу, как
 *  раньше. С драйвером (headless --ticks, см. org.microemu.app.headless.TickDriver)
 *  на каждой границе по порядку и в одном потоке:
 *
 *    1. driver.beforeTick(n) — снимок кадра n-1, остановка прогона на лимите;
 *    2. "F <n>" в трассу;
 *    3. driver.deliver(n)    — события сценария этого тика; клавиши уходят
 *                               в Displayable СИНХРОННО (isDelivering()),
 *                               а не через очередь EventDispatcher;
 *    4. возврат — дальше рисуется кадр n.
 *
 *  Поток игры в этот момент стоит в serviceRepaints() (Canvas) или сам
 *  выполняет flushGraphics() (GameCanvas), поэтому игра видит нажатие строго
 *  между логикой тика n-1 и отрисовкой тика n — независимо от планировщика.
 */
package org.microemu;

import org.microemu.log.MidpTrace;

public final class TickInput {

	/** Сторона стенда: что делать на границе тика. */
	public interface Driver {

		/** До строки "F n": снимок предыдущего кадра, выход по лимиту. */
		void beforeTick(int tick);

		/** После строки "F n", до отрисовки: доставить события тика n. */
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
	 * Идёт доставка событий тика в ЭТОМ потоке: клавиши и команды надо
	 * выполнять сразу, а repaint() не ставить в очередь (кадр тика всё равно
	 * сейчас будет нарисован).
	 */
	public static boolean isDelivering() {
		return deliveringThread == Thread.currentThread();
	}

	/** Номер следующего тика (сколько границ уже пройдено). */
	public static synchronized int current() {
		return tick;
	}

	/** Граница кадра. Вызывается перед отрисовкой кадра. */
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
