/*
 *  arlecchino-midp-rt: a minimal API stub layer for JSR-135 (MMAPI),
 *  written from the public JSR-135 specification for the arlecchino-midp-rt fork
 *  (2026), LGPL 2.1 — see LICENSE/NOTICE at the repository root.
 */
package javax.microedition.media.control;

import javax.microedition.media.Control;

public interface VolumeControl extends Control {

	void setMute(boolean mute);

	boolean isMuted();

	int setLevel(int level);

	int getLevel();
}
