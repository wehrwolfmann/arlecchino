/*
 *  arlecchino-midp-rt: a single-tone generator for Manager.playTone
 *  on top of javax.sound.midi (a shared lazy Synthesizer, the note on channel 16
 *  (index 15, not percussion), noteOff by a duration timer).
 *
 *  Written from the public JSR-135 specification for the arlecchino-midp-rt fork
 *  (2026), LGPL 2.1 — see LICENSE/NOTICE at the repository root.
 */
package org.microemu.media;

import java.util.Timer;
import java.util.TimerTask;

import javax.microedition.media.MediaException;
import javax.sound.midi.MidiChannel;
import javax.sound.midi.MidiSystem;
import javax.sound.midi.Synthesizer;

public final class ToneGenerator {

	/** Channel 15 is a free non-melodic one (9 = percussion, we avoid it). */
	private static final int TONE_CHANNEL = 15;

	private static Synthesizer synthesizer;

	private static Timer timer;

	private static int activeNote = -1;

	private ToneGenerator() {
	}

	public static synchronized void playTone(int note, int duration, int volume)
			throws MediaException {
		if (note < 0 || note > 127) {
			throw new IllegalArgumentException("note out of range: " + note);
		}
		if (duration <= 0) {
			throw new IllegalArgumentException("duration must be positive: " + duration);
		}
		if (volume < 0) {
			volume = 0;
		} else if (volume > 100) {
			volume = 100;
		}
		if (volume == 0) {
			return;
		}
		try {
			if (synthesizer == null) {
				synthesizer = MidiSystem.getSynthesizer();
				synthesizer.open();
				timer = new Timer("MMAPI-tone", true);
			}
			MidiChannel channel = synthesizer.getChannels()[TONE_CHANNEL];
			if (activeNote >= 0) {
				channel.noteOff(activeNote);
			}
			final int midiNote = note;
			activeNote = midiNote;
			channel.noteOn(midiNote, volume * 127 / 100);
			timer.schedule(new TimerTask() {
				public void run() {
					noteOff(midiNote);
				}
			}, duration);
		} catch (Exception e) {
			throw new MediaException("Tone playback failed: " + e.getMessage());
		}
	}

	private static synchronized void noteOff(int note) {
		if (synthesizer != null && activeNote == note) {
			synthesizer.getChannels()[TONE_CHANNEL].noteOff(note);
			activeNote = -1;
		}
	}
}
