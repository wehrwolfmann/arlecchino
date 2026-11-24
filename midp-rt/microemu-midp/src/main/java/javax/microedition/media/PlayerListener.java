/*
 *  arlecchino-midp-rt: a minimal API stub layer for JSR-135 (MMAPI),
 *  written from the public JSR-135 specification for the arlecchino-midp-rt fork
 *  (2026), LGPL 2.1 — see LICENSE/NOTICE at the repository root.
 */
package javax.microedition.media;

public interface PlayerListener {

	String STARTED = "started";

	String STOPPED = "stopped";

	String STOPPED_AT_TIME = "stoppedAtTime";

	String END_OF_MEDIA = "endOfMedia";

	String DURATION_UPDATED = "durationUpdated";

	String DEVICE_UNAVAILABLE = "deviceUnavailable";

	String DEVICE_AVAILABLE = "deviceAvailable";

	String VOLUME_CHANGED = "volumeChanged";

	String SIZE_CHANGED = "sizeChanged";

	String ERROR = "error";

	String CLOSED = "closed";

	String RECORD_STARTED = "recordStarted";

	String RECORD_STOPPED = "recordStopped";

	String RECORD_ERROR = "recordError";

	String BUFFERING_STARTED = "bufferingStarted";

	String BUFFERING_STOPPED = "bufferingStopped";

	void playerUpdate(Player player, String event, Object eventData);
}
