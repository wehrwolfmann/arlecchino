/*
 *  arlecchino-midp-rt: общая база реальных Player'ов JSR-135 (MMAPI):
 *  state-machine UNREALIZED -> REALIZED -> PREFETCHED -> STARTED -> CLOSED,
 *  список PlayerListener'ов с асинхронной доставкой событий, счётчик loop,
 *  VolumeControl (уровень 0..100 + mute) с событием VOLUME_CHANGED.
 *
 *  Написан по публичной спецификации JSR-135 для форка arlecchino-midp-rt
 *  (2026), LGPL 2.1 — см. LICENSE/NOTICE в корне репозитория.
 */
package org.microemu.media;

import java.util.Vector;

import javax.microedition.media.Control;
import javax.microedition.media.Manager;
import javax.microedition.media.MediaException;
import javax.microedition.media.Player;
import javax.microedition.media.PlayerListener;
import javax.microedition.media.TimeBase;
import javax.microedition.media.control.VolumeControl;

import org.microemu.log.Logger;

public abstract class AbstractPlayer implements Player {

	private final String contentType;

	private int state = UNREALIZED;

	private final Vector listeners = new Vector();

	/** Счётчик повторов по семантике MMAPI: 1 = один раз, -1 = бесконечно. */
	protected int loopCount = 1;

	/** Громкость 0..100 (применяется наследником в applyVolume). */
	protected int volumeLevel = 100;

	protected boolean volumeMuted = false;

	private final VolumeControl volumeControl = new VolumeControl() {

		public void setMute(boolean mute) {
			synchronized (AbstractPlayer.this) {
				if (mute == volumeMuted) {
					return;
				}
				volumeMuted = mute;
				applyVolume();
			}
			postEvent(PlayerListener.VOLUME_CHANGED, this);
		}

		public boolean isMuted() {
			return volumeMuted;
		}

		public int setLevel(int level) {
			if (level < 0) {
				level = 0;
			} else if (level > 100) {
				level = 100;
			}
			synchronized (AbstractPlayer.this) {
				if (level != volumeLevel) {
					volumeLevel = level;
					applyVolume();
				} else {
					volumeLevel = level;
				}
			}
			postEvent(PlayerListener.VOLUME_CHANGED, this);
			return level;
		}

		public int getLevel() {
			return volumeLevel;
		}
	};

	protected AbstractPlayer(String contentType) {
		this.contentType = contentType;
	}

	// ------------------------------------------------------------ state machine

	public synchronized void realize() throws MediaException {
		checkNotClosed();
		if (state == UNREALIZED) {
			doRealize();
			state = REALIZED;
		}
	}

	public synchronized void prefetch() throws MediaException {
		checkNotClosed();
		realize();
		if (state == REALIZED) {
			doPrefetch();
			state = PREFETCHED;
		}
	}

	public synchronized void start() throws MediaException {
		checkNotClosed();
		prefetch();
		if (state == PREFETCHED) {
			doStart();
			state = STARTED;
			postEvent(PlayerListener.STARTED, new Long(getMediaTime()));
		}
	}

	public synchronized void stop() throws MediaException {
		checkNotClosed();
		if (state == STARTED) {
			doStop();
			state = PREFETCHED;
			postEvent(PlayerListener.STOPPED, new Long(getMediaTime()));
		}
	}

	public synchronized void deallocate() {
		if (state == CLOSED) {
			return;
		}
		if (state == STARTED) {
			try {
				doStop();
			} catch (MediaException e) {
				Logger.warn("MMAPI deallocate: stop failed" + ": " + e);
			}
			state = PREFETCHED;
		}
		if (state == PREFETCHED) {
			doDeallocate();
			state = REALIZED;
		}
	}

	public synchronized void close() {
		if (state == CLOSED) {
			return;
		}
		deallocate();
		doClose();
		state = CLOSED;
		postEvent(PlayerListener.CLOSED, null);
	}

	public synchronized int getState() {
		return state;
	}

	/** Перевод STARTED -> PREFETCHED по естественному концу носителя
	 * (вызывается наследником из callback'а аудиосистемы). */
	protected synchronized void mediaEnded() {
		if (state == STARTED) {
			state = PREFETCHED;
			postEvent(PlayerListener.END_OF_MEDIA, new Long(getMediaTime()));
		}
	}

	protected void checkNotClosed() {
		if (state == CLOSED) {
			throw new IllegalStateException("Player is closed");
		}
	}

	private void checkNotStartedNotClosed() {
		checkNotClosed();
		if (state == STARTED) {
			throw new IllegalStateException("Player is started");
		}
	}

	// ------------------------------------------------- хуки реализаций носителя

	/** UNREALIZED -> REALIZED: разбор данных носителя. */
	protected abstract void doRealize() throws MediaException;

	/** REALIZED -> PREFETCHED: захват аудиоустройства. */
	protected abstract void doPrefetch() throws MediaException;

	/** PREFETCHED -> STARTED. */
	protected abstract void doStart() throws MediaException;

	/** STARTED -> PREFETCHED. */
	protected abstract void doStop() throws MediaException;

	/** PREFETCHED -> REALIZED: освобождение аудиоустройства. */
	protected abstract void doDeallocate();

	/** -> CLOSED: окончательное освобождение ресурсов. */
	protected abstract void doClose();

	/** Применить volumeLevel/volumeMuted к работающему носителю
	 * (вызывается под монитором плеера). */
	protected abstract void applyVolume();

	// ------------------------------------------------------------------- общее

	public void setTimeBase(TimeBase master) throws MediaException {
		checkNotStartedNotClosed();
		// поддерживается только системная TimeBase
	}

	public TimeBase getTimeBase() {
		checkNotClosed();
		return Manager.getSystemTimeBase();
	}

	public long setMediaTime(long now) throws MediaException {
		checkNotStartedNotClosed();
		return TIME_UNKNOWN;
	}

	public long getMediaTime() {
		return TIME_UNKNOWN;
	}

	public long getDuration() {
		return TIME_UNKNOWN;
	}

	public String getContentType() {
		checkNotClosed();
		return contentType;
	}

	public synchronized void setLoopCount(int count) {
		if (count == 0) {
			throw new IllegalArgumentException("loop count is 0");
		}
		if (state == STARTED) {
			throw new IllegalStateException("Player is started");
		}
		loopCount = count;
	}

	public void addPlayerListener(PlayerListener playerListener) {
		checkNotClosed();
		if (playerListener != null && !listeners.contains(playerListener)) {
			listeners.addElement(playerListener);
		}
	}

	public void removePlayerListener(PlayerListener playerListener) {
		checkNotClosed();
		listeners.removeElement(playerListener);
	}

	public Control getControl(String controlType) {
		if (controlType == null) {
			throw new IllegalArgumentException("controlType is null");
		}
		checkNotClosed();
		if (controlType.equals("VolumeControl")
				|| controlType.equals("javax.microedition.media.control.VolumeControl")) {
			return volumeControl;
		}
		return null;
	}

	public Control[] getControls() {
		checkNotClosed();
		return new Control[] { volumeControl };
	}

	// ----------------------------------------------------------------- события

	/** Асинхронная доставка события всем слушателям (фоновая нить,
	 * как требует спецификация: callback не из нити вызвавшего метода). */
	protected void postEvent(final String event, final Object eventData) {
		if (listeners.isEmpty()) {
			return;
		}
		final Object[] snapshot;
		synchronized (listeners) {
			snapshot = new Object[listeners.size()];
			listeners.copyInto(snapshot);
		}
		final Player self = this;
		Thread t = new Thread(new Runnable() {
			public void run() {
				for (int i = 0; i < snapshot.length; i++) {
					try {
						((PlayerListener) snapshot[i]).playerUpdate(self, event, eventData);
					} catch (Throwable e) {
						Logger.warn("PlayerListener threw" + ": " + e);
					}
				}
			}
		}, "MMAPI-event");
		t.setDaemon(true);
		t.start();
	}
}
