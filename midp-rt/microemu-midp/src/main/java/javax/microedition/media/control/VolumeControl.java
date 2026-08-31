/*
 *  arlecchino-midp-rt: минимальный API-заглушечный слой JSR-135 (MMAPI),
 *  написан по публичной спецификации JSR-135 для форка arlecchino-midp-rt
 *  (2026), LGPL 2.1 — см. LICENSE/NOTICE в корне репозитория.
 */
package javax.microedition.media.control;

import javax.microedition.media.Control;

public interface VolumeControl extends Control {

	void setMute(boolean mute);

	boolean isMuted();

	int setLevel(int level);

	int getLevel();
}
