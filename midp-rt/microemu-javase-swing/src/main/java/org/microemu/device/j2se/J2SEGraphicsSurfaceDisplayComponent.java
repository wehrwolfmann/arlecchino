/*
 *  arlecchino-midp-rt: the common interface of DisplayComponents with access to
 *  J2SEGraphicsSurface (the Swing window and headless NoUi). Introduced so that
 *  GameCanvas.flushGraphics works in headless mode: upstream
 *  J2SEDeviceDisplay.flushGraphics hard-cast DisplayComponent to
 *  SwingDisplayComponent and in headless failed with a ClassCastException
 *  (silently, in the game thread) — the frames of GameCanvas games never reached the listener.
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
 */
package org.microemu.device.j2se;

import org.microemu.DisplayComponent;
import org.microemu.app.ui.DisplayRepaintListener;

public interface J2SEGraphicsSurfaceDisplayComponent extends DisplayComponent {

	/**
	 * The device screen surface (may be created lazily).
	 */
	J2SEGraphicsSurface getGraphicsSurface();

	/**
	 * Notify the repaint listeners about new surface contents.
	 */
	void fireDisplayRepaint(J2SEGraphicsSurface surface, int x, int y, int width, int height);

	/**
	 * Subscribe a listener to surface repaints. Both components
	 * (the Swing window and headless NoUi) already implement this method; it is lifted
	 * into the common interface so that external subscribers (e.g. ShellFrameBridge) can
	 * attach uniformly without casting to a concrete class.
	 */
	void addDisplayRepaintListener(DisplayRepaintListener l);
}
