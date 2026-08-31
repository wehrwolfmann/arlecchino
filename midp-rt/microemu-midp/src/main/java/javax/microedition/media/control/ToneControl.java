/*
 *  arlecchino-midp-rt: минимальный API-заглушечный слой JSR-135 (MMAPI),
 *  написан по публичной спецификации JSR-135 для форка arlecchino-midp-rt
 *  (2026), LGPL 2.1 — см. LICENSE/NOTICE в корне репозитория.
 */
package javax.microedition.media.control;

import javax.microedition.media.Control;

public interface ToneControl extends Control {

	byte VERSION = -2;

	byte TEMPO = -3;

	byte RESOLUTION = -4;

	byte BLOCK_START = -5;

	byte BLOCK_END = -6;

	byte PLAY_BLOCK = -7;

	byte SET_VOLUME = -8;

	byte REPEAT = -9;

	byte SILENCE = -1;

	byte C4 = 60;

	void setSequence(byte[] sequence);
}
