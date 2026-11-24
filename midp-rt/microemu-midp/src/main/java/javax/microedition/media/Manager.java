/*
 *  arlecchino-midp-rt: the JSR-135 (MMAPI) layer, written from the public
 *  JSR-135 specification for the arlecchino-midp-rt fork (2026), LGPL 2.1 —
 *  see LICENSE/NOTICE at the repository root.
 *
 *  Real sound: MIDI -> org.microemu.media.MidiPlayer (javax.sound.midi),
 *  WAV/AU/AIFF -> org.microemu.media.SampledPlayer (javax.sound.sampled),
 *  playTone -> org.microemu.media.ToneGenerator. The type is determined by the
 *  given content-type, or, when it is missing, by the data signature
 *  (MThd / RIFF...WAVE / .snd / FORM). Unsupported types and the system
 *  property org.microemu.media.noop=true give a safe no-op stub
 *  (org.microemu.media.NoopPlayer) — the game runs without sound.
 */
package javax.microedition.media;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;

import org.microemu.log.Logger;
import org.microemu.log.MidpTrace;
import org.microemu.media.MidiPlayer;
import org.microemu.media.NoopPlayer;
import org.microemu.media.SampledPlayer;
import org.microemu.media.ToneGenerator;

public final class Manager {

	public static final String TONE_DEVICE_LOCATOR = "device://tone";

	/** System property: true = always no-op players (headless/diff tests). */
	private static final String NOOP_PROPERTY = "org.microemu.media.noop";

	private static final String[] CONTENT_TYPES = { "audio/midi", "audio/x-mid",
			"audio/x-midi", "audio/sp-midi", "audio/wav", "audio/x-wav",
			"audio/basic", "audio/x-aiff" };

	private static final TimeBase SYSTEM_TIME_BASE = new TimeBase() {
		public long getTime() {
			return System.currentTimeMillis() * 1000L;
		}
	};

	private Manager() {
	}

	private static boolean noop() {
		try {
			return Boolean.getBoolean(NOOP_PROPERTY);
		} catch (SecurityException e) {
			return false;
		}
	}

	public static String[] getSupportedContentTypes(String protocol) {
		if (noop()) {
			return new String[0];
		}
		String[] copy = new String[CONTENT_TYPES.length];
		System.arraycopy(CONTENT_TYPES, 0, copy, 0, CONTENT_TYPES.length);
		return copy;
	}

	public static String[] getSupportedProtocols(String contentType) {
		if (noop()) {
			return new String[0];
		}
		return new String[] { "device" };
	}

	public static Player createPlayer(String locator) throws IOException, MediaException {
		if (locator == null) {
			throw new IllegalArgumentException("locator is null");
		}
		// device://tone (ToneControl sequences) is not played yet;
		// other protocols (http/file/capture) are not supported — we return a no-op
		// so that games with optional sound do not crash while resolving the player.
		return new NoopPlayer(null);
	}

	public static Player createPlayer(InputStream stream, String type)
			throws IOException, MediaException {
		if (stream == null) {
			throw new IllegalArgumentException("stream is null");
		}
		byte[] data = readFully(stream);
		Player player;
		String kind = noop() ? null : mediaKind(type, data);
		if (noop()) {
			player = new NoopPlayer(type);
		} else if ("midi".equals(kind)) {
			player = new MidiPlayer(data, type != null ? type : "audio/midi");
		} else if ("sampled".equals(kind)) {
			player = new SampledPlayer(data, type != null ? type : "audio/x-wav");
		} else {
			player = null;
		}
		if (player != null) {
			if (MidpTrace.ON) {
				MidpTrace.player(player, type == null ? "?" : type, data.length);
			}
			return player;
		}
		Logger.warn("MMAPI: unsupported media type '" + type + "' ("
				+ data.length + " bytes) - no-op player");
		return new NoopPlayer(type);
	}

	public static void playTone(int note, int duration, int volume) throws MediaException {
		if (note < 0 || note > 127) {
			throw new IllegalArgumentException("note out of range: " + note);
		}
		if (duration <= 0) {
			throw new IllegalArgumentException("duration must be positive: " + duration);
		}
		if (noop()) {
			return;
		}
		ToneGenerator.playTone(note, duration, volume);
	}

	public static TimeBase getSystemTimeBase() {
		return SYSTEM_TIME_BASE;
	}

	// ------------------------------------------------------------------ helpers

	private static byte[] readFully(InputStream stream) throws IOException {
		ByteArrayOutputStream out = new ByteArrayOutputStream();
		byte[] buf = new byte[8192];
		int n;
		while ((n = stream.read(buf)) != -1) {
			out.write(buf, 0, n);
		}
		return out.toByteArray();
	}

	/** Media classification: "midi" | "sampled" | null (unknown). */
	private static String mediaKind(String type, byte[] data) {
		if (type != null) {
			String t = type.toLowerCase();
			if (t.indexOf("midi") >= 0 || t.indexOf("mid") >= 0) {
				return "midi";
			}
			if (t.indexOf("wav") >= 0 || t.indexOf("aiff") >= 0
					|| t.equals("audio/basic")) {
				return "sampled";
			}
		}
		// data signatures (if the content-type is not given or unknown)
		if (startsWith(data, "MThd")) {
			return "midi";
		}
		if (startsWith(data, "RIFF") || startsWith(data, ".snd")
				|| startsWith(data, "FORM")) {
			return "sampled";
		}
		return null;
	}

	private static boolean startsWith(byte[] data, String magic) {
		if (data.length < magic.length()) {
			return false;
		}
		for (int i = 0; i < magic.length(); i++) {
			if (data[i] != (byte) magic.charAt(i)) {
				return false;
			}
		}
		return true;
	}
}
