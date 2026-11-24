/*
 *  arlecchino-midp-rt: a minimal API stub layer for JSR-135 (MMAPI),
 *  written from the public JSR-135 specification for the arlecchino-midp-rt fork
 *  (2026), LGPL 2.1 — see LICENSE/NOTICE at the repository root.
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
