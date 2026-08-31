/*
 *  arlecchino-midp-rt: общий интерфейс DisplayComponent'ов с доступом к
 *  J2SEGraphicsSurface (Swing-окно и headless NoUi). Введён, чтобы
 *  GameCanvas.flushGraphics работал в headless-режиме: upstream
 *  J2SEDeviceDisplay.flushGraphics жёстко кастовал DisplayComponent к
 *  SwingDisplayComponent и в headless падал с ClassCastException
 *  (молча, в потоке игры) — кадры GameCanvas-игр не доходили до listener'а.
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 */
package org.microemu.device.j2se;

import org.microemu.DisplayComponent;
import org.microemu.app.ui.DisplayRepaintListener;

public interface J2SEGraphicsSurfaceDisplayComponent extends DisplayComponent {

	/**
	 * Поверхность экрана устройства (может лениво создаваться).
	 */
	J2SEGraphicsSurface getGraphicsSurface();

	/**
	 * Уведомить слушателей перерисовки о новом содержимом поверхности.
	 */
	void fireDisplayRepaint(J2SEGraphicsSurface surface, int x, int y, int width, int height);

	/**
	 * Подписать слушателя на перерисовки поверхности. Оба компонента
	 * (Swing-окно и headless NoUi) уже реализуют этот метод; вынесен в общий
	 * интерфейс, чтобы внешние подписчики (напр. ShellFrameBridge) могли
	 * вешаться единообразно, не кастуя к конкретному классу.
	 */
	void addDisplayRepaintListener(DisplayRepaintListener l);
}
