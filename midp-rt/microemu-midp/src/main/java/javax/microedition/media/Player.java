/*
 *  arlecchino-midp-rt: a minimal API stub layer for JSR-135 (MMAPI),
 *  written from the public JSR-135 specification for the arlecchino-midp-rt fork
 *  (2026), LGPL 2.1 — see LICENSE/NOTICE at the repository root.
 */
package javax.microedition.media;

public interface Player extends Controllable {

	int UNREALIZED = 100;

	int REALIZED = 200;

	int PREFETCHED = 300;

	int STARTED = 400;

	int CLOSED = 0;

	long TIME_UNKNOWN = -1;

	void realize() throws MediaException;

	void prefetch() throws MediaException;

	void start() throws MediaException;

	void stop() throws MediaException;

	void deallocate();

	void close();

	void setTimeBase(TimeBase master) throws MediaException;

	TimeBase getTimeBase();

	long setMediaTime(long now) throws MediaException;

	long getMediaTime();

	int getState();

	long getDuration();

	String getContentType();

	void setLoopCount(int count);

	void addPlayerListener(PlayerListener playerListener);

	void removePlayerListener(PlayerListener playerListener);
}
