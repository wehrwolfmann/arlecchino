/*
 *  arlecchino-midp-rt: реальный Player JSR-135 для сэмплированного звука
 *  (WAV/AU/AIFF — всё, что читает javax.sound.sampled) поверх Clip.
 *  Громкость VolumeControl — через FloatControl MASTER_GAIN (перевод
 *  уровня 0..100 в децибелы), mute — через BooleanControl MUTE с
 *  fallback'ом на gain.
 *
 *  Написан по публичной спецификации JSR-135 для форка arlecchino-midp-rt
 *  (2026), LGPL 2.1 — см. LICENSE/NOTICE в корне репозитория.
 */
package org.microemu.media;

import java.io.ByteArrayInputStream;
import java.io.IOException;

import javax.microedition.media.MediaException;
import javax.sound.sampled.AudioInputStream;
import javax.sound.sampled.AudioSystem;
import javax.sound.sampled.BooleanControl;
import javax.sound.sampled.Clip;
import javax.sound.sampled.FloatControl;
import javax.sound.sampled.LineEvent;
import javax.sound.sampled.LineListener;
import javax.sound.sampled.LineUnavailableException;
import javax.sound.sampled.UnsupportedAudioFileException;

import org.microemu.log.Logger;

public class SampledPlayer extends AbstractPlayer {

	private final byte[] data;

	private Clip clip;

	/** start() вызван и конец носителя ещё не достигнут. */
	private volatile boolean playing = false;

	public SampledPlayer(byte[] data, String contentType) {
		super(contentType);
		this.data = data;
	}

	protected void doRealize() throws MediaException {
		// формат проверяется здесь, само устройство берётся в prefetch
		try {
			AudioInputStream probe = AudioSystem.getAudioInputStream(new ByteArrayInputStream(data));
			probe.close();
		} catch (UnsupportedAudioFileException e) {
			throw new MediaException("Unsupported audio format: " + e.getMessage());
		} catch (IOException e) {
			throw new MediaException("Cannot read audio data: " + e.getMessage());
		}
	}

	protected void doPrefetch() throws MediaException {
		try {
			AudioInputStream stream = AudioSystem.getAudioInputStream(new ByteArrayInputStream(data));
			clip = AudioSystem.getClip();
			clip.open(stream);
			clip.addLineListener(new LineListener() {
				public void update(LineEvent event) {
					if (event.getType() == LineEvent.Type.STOP && playing
							&& clip != null && !clip.isRunning()
							&& clip.getFramePosition() >= clip.getFrameLength()) {
						playing = false;
						mediaEnded();
					}
				}
			});
			applyVolume();
		} catch (UnsupportedAudioFileException e) {
			releaseClip();
			throw new MediaException("Unsupported audio format: " + e.getMessage());
		} catch (LineUnavailableException e) {
			releaseClip();
			throw new MediaException("Audio device unavailable: " + e.getMessage());
		} catch (IOException e) {
			releaseClip();
			throw new MediaException("Cannot read audio data: " + e.getMessage());
		}
	}

	protected void doStart() throws MediaException {
		// после конца носителя — заново с начала
		if (clip.getFramePosition() >= clip.getFrameLength()) {
			clip.setFramePosition(0);
		}
		playing = true;
		if (loopCount < 0) {
			clip.loop(Clip.LOOP_CONTINUOUSLY);
		} else if (loopCount > 1) {
			// MMAPI: count воспроизведений = (count - 1) повторов Clip'а
			clip.loop(loopCount - 1);
		} else {
			clip.start();
		}
	}

	protected void doStop() throws MediaException {
		playing = false;
		clip.stop();
	}

	protected void doDeallocate() {
		releaseClip();
	}

	protected void doClose() {
		releaseClip();
	}

	private void releaseClip() {
		playing = false;
		if (clip != null) {
			try {
				clip.close();
			} catch (Exception e) {
				Logger.warn("Audio clip close failed" + ": " + e);
			}
			clip = null;
		}
	}

	protected void applyVolume() {
		if (clip == null) {
			return;
		}
		try {
			if (clip.isControlSupported(BooleanControl.Type.MUTE)) {
				BooleanControl mute = (BooleanControl) clip.getControl(BooleanControl.Type.MUTE);
				mute.setValue(volumeMuted);
			}
			if (clip.isControlSupported(FloatControl.Type.MASTER_GAIN)) {
				FloatControl gain = (FloatControl) clip.getControl(FloatControl.Type.MASTER_GAIN);
				float db;
				int effective = volumeMuted ? 0 : volumeLevel;
				if (effective <= 0) {
					db = gain.getMinimum();
				} else {
					db = (float) (20.0 * Math.log10(effective / 100.0));
					if (db < gain.getMinimum()) {
						db = gain.getMinimum();
					} else if (db > gain.getMaximum()) {
						db = gain.getMaximum();
					}
				}
				gain.setValue(db);
			}
		} catch (Exception e) {
			Logger.warn("Audio volume apply failed" + ": " + e);
		}
	}

	public long setMediaTime(long now) throws MediaException {
		checkNotClosed();
		if (clip == null) {
			throw new MediaException("Player must be prefetched");
		}
		if (now < 0) {
			now = 0;
		}
		long len = clip.getMicrosecondLength();
		if (len >= 0 && now > len) {
			now = len;
		}
		clip.setMicrosecondPosition(now);
		return clip.getMicrosecondPosition();
	}

	public long getMediaTime() {
		if (clip == null) {
			return TIME_UNKNOWN;
		}
		return clip.getMicrosecondPosition();
	}

	public long getDuration() {
		if (clip == null) {
			return TIME_UNKNOWN;
		}
		return clip.getMicrosecondLength();
	}
}
