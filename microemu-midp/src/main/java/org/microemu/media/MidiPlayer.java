/*
 *  arlecchino-midp-rt: реальный MIDI-Player JSR-135 поверх javax.sound.midi
 *  (Sequencer -> масштабирующий Receiver -> Synthesizer). Громкость
 *  VolumeControl реализована перехватом CC7 (channel volume): значения из
 *  самого файла запоминаются как база и умножаются на level/100, поэтому
 *  setLevel работает и когда секвенция сама шлёт CC7 (в т.ч. на каждом
 *  повторе при setLoopCount(-1)).
 *
 *  Написан по публичной спецификации JSR-135 для форка arlecchino-midp-rt
 *  (2026), LGPL 2.1 — см. LICENSE/NOTICE в корне репозитория.
 */
package org.microemu.media;

import java.io.ByteArrayInputStream;
import java.io.IOException;

import javax.microedition.media.MediaException;
import javax.sound.midi.InvalidMidiDataException;
import javax.sound.midi.MetaEventListener;
import javax.sound.midi.MetaMessage;
import javax.sound.midi.MidiMessage;
import javax.sound.midi.MidiSystem;
import javax.sound.midi.MidiUnavailableException;
import javax.sound.midi.Receiver;
import javax.sound.midi.Sequence;
import javax.sound.midi.Sequencer;
import javax.sound.midi.ShortMessage;
import javax.sound.midi.Synthesizer;

import org.microemu.log.Logger;

public class MidiPlayer extends AbstractPlayer {

	/** Meta-событие 0x2F = End Of Track. */
	private static final int META_END_OF_TRACK = 0x2F;

	private static final int CC_CHANNEL_VOLUME = 7;

	/** Громкость канала по умолчанию (General MIDI). */
	private static final int DEFAULT_CHANNEL_VOLUME = 100;

	private final byte[] data;

	private Sequence sequence;

	private Sequencer sequencer;

	private Synthesizer synthesizer;

	private VolumeScalingReceiver volumeReceiver;

	public MidiPlayer(byte[] data, String contentType) {
		super(contentType);
		this.data = data;
	}

	protected void doRealize() throws MediaException {
		try {
			sequence = MidiSystem.getSequence(new ByteArrayInputStream(data));
		} catch (InvalidMidiDataException e) {
			throw new MediaException("Invalid MIDI data: " + e.getMessage());
		} catch (IOException e) {
			throw new MediaException("Cannot read MIDI data: " + e.getMessage());
		}
	}

	protected void doPrefetch() throws MediaException {
		try {
			sequencer = MidiSystem.getSequencer(false);
			sequencer.open();
			synthesizer = MidiSystem.getSynthesizer();
			synthesizer.open();
			volumeReceiver = new VolumeScalingReceiver(synthesizer.getReceiver());
			volumeReceiver.setVolume(volumeLevel, volumeMuted);
			sequencer.getTransmitter().setReceiver(volumeReceiver);
			sequencer.setSequence(sequence);
			sequencer.addMetaEventListener(new MetaEventListener() {
				public void meta(MetaMessage meta) {
					if (meta.getType() == META_END_OF_TRACK
							&& sequencer != null && !sequencer.isRunning()) {
						mediaEnded();
					}
				}
			});
		} catch (MidiUnavailableException e) {
			releaseDevices();
			throw new MediaException("MIDI device unavailable: " + e.getMessage());
		} catch (InvalidMidiDataException e) {
			releaseDevices();
			throw new MediaException("Invalid MIDI data: " + e.getMessage());
		}
	}

	protected void doStart() throws MediaException {
		if (loopCount < 0) {
			sequencer.setLoopCount(Sequencer.LOOP_CONTINUOUSLY);
		} else {
			// MMAPI: count = число воспроизведений; Sequencer: число ПОВТОРОВ
			sequencer.setLoopCount(loopCount - 1);
		}
		// после END_OF_MEDIA позиция остаётся в конце — начать заново
		if (sequencer.getMicrosecondPosition() >= sequencer.getMicrosecondLength()) {
			sequencer.setMicrosecondPosition(0);
		}
		sequencer.start();
	}

	protected void doStop() throws MediaException {
		sequencer.stop();
	}

	protected void doDeallocate() {
		releaseDevices();
	}

	protected void doClose() {
		releaseDevices();
		sequence = null;
	}

	private void releaseDevices() {
		if (sequencer != null) {
			try {
				sequencer.close();
			} catch (Exception e) {
				Logger.warn("MIDI sequencer close failed" + ": " + e);
			}
			sequencer = null;
		}
		if (synthesizer != null) {
			try {
				synthesizer.close();
			} catch (Exception e) {
				Logger.warn("MIDI synthesizer close failed" + ": " + e);
			}
			synthesizer = null;
		}
		volumeReceiver = null;
	}

	protected void applyVolume() {
		if (volumeReceiver != null) {
			volumeReceiver.setVolume(volumeLevel, volumeMuted);
		}
	}

	public long setMediaTime(long now) throws MediaException {
		checkNotClosed();
		if (sequencer == null) {
			throw new MediaException("Player must be prefetched");
		}
		if (now < 0) {
			now = 0;
		}
		long len = sequencer.getMicrosecondLength();
		if (len >= 0 && now > len) {
			now = len;
		}
		sequencer.setMicrosecondPosition(now);
		return sequencer.getMicrosecondPosition();
	}

	public long getMediaTime() {
		if (sequencer == null) {
			return TIME_UNKNOWN;
		}
		return sequencer.getMicrosecondPosition();
	}

	public long getDuration() {
		if (sequencer != null) {
			return sequencer.getMicrosecondLength();
		}
		if (sequence != null) {
			return sequence.getMicrosecondLength();
		}
		return TIME_UNKNOWN;
	}

	/**
	 * Receiver-обёртка между Sequencer и Synthesizer: масштабирует громкость
	 * каналов (CC7) множителем level/100 и помнит «базовую» громкость каждого
	 * канала из самой секвенции, чтобы пере-применять её при setLevel/setMute.
	 */
	private static final class VolumeScalingReceiver implements Receiver {

		private final Receiver delegate;

		private final int[] channelBase = new int[16];

		private int level = 100;

		private boolean muted = false;

		VolumeScalingReceiver(Receiver delegate) {
			this.delegate = delegate;
			for (int i = 0; i < channelBase.length; i++) {
				channelBase[i] = DEFAULT_CHANNEL_VOLUME;
			}
		}

		public synchronized void send(MidiMessage message, long timeStamp) {
			if (message instanceof ShortMessage) {
				ShortMessage sm = (ShortMessage) message;
				if (sm.getCommand() == ShortMessage.CONTROL_CHANGE
						&& sm.getData1() == CC_CHANNEL_VOLUME) {
					int channel = sm.getChannel();
					channelBase[channel] = sm.getData2();
					sendScaledVolume(channel, timeStamp);
					return;
				}
			}
			delegate.send(message, timeStamp);
		}

		synchronized void setVolume(int level, boolean muted) {
			this.level = level;
			this.muted = muted;
			for (int channel = 0; channel < channelBase.length; channel++) {
				sendScaledVolume(channel, -1);
			}
		}

		private void sendScaledVolume(int channel, long timeStamp) {
			int value = muted ? 0 : channelBase[channel] * level / 100;
			if (value > 127) {
				value = 127;
			}
			try {
				ShortMessage scaled = new ShortMessage();
				scaled.setMessage(ShortMessage.CONTROL_CHANGE, channel, CC_CHANNEL_VOLUME, value);
				delegate.send(scaled, timeStamp);
			} catch (InvalidMidiDataException e) {
				Logger.warn("MIDI volume scaling failed" + ": " + e);
			}
		}

		public void close() {
			// синтезатор закрывает MidiPlayer.releaseDevices
		}
	}
}
