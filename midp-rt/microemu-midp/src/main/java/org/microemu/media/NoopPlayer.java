/*
 *  arlecchino-midp-rt: a no-op Player implementation for the JSR-135 stub layer
 *  (sound is deferred to Arlecchino Phase 2). It drives the state machine
 *  UNREALIZED -> REALIZED -> PREFETCHED -> STARTED -> ... -> CLOSED correctly, so
 *  games that check getState() do not break; it plays nothing.
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root.
 */
package org.microemu.media;

import javax.microedition.media.Control;
import javax.microedition.media.MediaException;
import javax.microedition.media.Manager;
import javax.microedition.media.Player;
import javax.microedition.media.PlayerListener;
import javax.microedition.media.TimeBase;
import javax.microedition.media.control.ToneControl;
import javax.microedition.media.control.VolumeControl;

public class NoopPlayer implements Player {

	private final String contentType;

	private int state = UNREALIZED;

	private final VolumeControl volumeControl = new VolumeControl() {

		private int level = 100;

		private boolean muted = false;

		public void setMute(boolean mute) {
			this.muted = mute;
		}

		public boolean isMuted() {
			return muted;
		}

		public int setLevel(int level) {
			if (level < 0) {
				level = 0;
			} else if (level > 100) {
				level = 100;
			}
			this.level = level;
			return this.level;
		}

		public int getLevel() {
			return level;
		}
	};

	private final ToneControl toneControl = new ToneControl() {
		public void setSequence(byte[] sequence) {
			// no-op: tone sequences are not played
		}
	};

	public NoopPlayer(String contentType) {
		this.contentType = contentType;
	}

	public synchronized void realize() throws MediaException {
		checkNotClosed();
		if (state == UNREALIZED) {
			state = REALIZED;
		}
	}

	public synchronized void prefetch() throws MediaException {
		checkNotClosed();
		if (state == UNREALIZED) {
			state = REALIZED;
		}
		if (state == REALIZED) {
			state = PREFETCHED;
		}
	}

	public synchronized void start() throws MediaException {
		checkNotClosed();
		prefetch();
		state = STARTED;
	}

	public synchronized void stop() throws MediaException {
		checkNotClosed();
		if (state == STARTED) {
			state = PREFETCHED;
		}
	}

	public synchronized void deallocate() {
		if (state == CLOSED) {
			return;
		}
		if (state == STARTED || state == PREFETCHED) {
			state = REALIZED;
		}
	}

	public synchronized void close() {
		state = CLOSED;
	}

	public void setTimeBase(TimeBase master) throws MediaException {
		checkNotClosed();
	}

	public TimeBase getTimeBase() {
		return Manager.getSystemTimeBase();
	}

	public long setMediaTime(long now) throws MediaException {
		checkNotClosed();
		return TIME_UNKNOWN;
	}

	public long getMediaTime() {
		return TIME_UNKNOWN;
	}

	public synchronized int getState() {
		return state;
	}

	public long getDuration() {
		return TIME_UNKNOWN;
	}

	public String getContentType() {
		return contentType;
	}

	public void setLoopCount(int count) {
		// no-op
	}

	public void addPlayerListener(PlayerListener playerListener) {
		// no-op: no events are generated (no sound is played)
	}

	public void removePlayerListener(PlayerListener playerListener) {
		// no-op
	}

	public Control getControl(String controlType) {
		if (controlType == null) {
			throw new IllegalArgumentException("controlType is null");
		}
		if (controlType.equals("VolumeControl")
				|| controlType.equals("javax.microedition.media.control.VolumeControl")) {
			return volumeControl;
		}
		if (controlType.equals("ToneControl")
				|| controlType.equals("javax.microedition.media.control.ToneControl")) {
			return toneControl;
		}
		return null;
	}

	public Control[] getControls() {
		return new Control[] { volumeControl, toneControl };
	}

	private void checkNotClosed() {
		if (state == CLOSED) {
			throw new IllegalStateException("Player is closed");
		}
	}
}
