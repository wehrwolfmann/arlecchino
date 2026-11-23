/**
 *  MicroEmulator
 *  Copyright (C) 2008 Bartek Teodorczyk <barteo@barteo.net>
 *
 *  It is licensed under the following two licenses as alternatives:
 *    1. GNU Lesser General Public License (the "LGPL") version 2.1 or any newer version
 *    2. Apache License (the "AL") Version 2.0
 *
 *  You may not use this file except in compliance with at least one of
 *  the above two licenses.
 *
 *  You may obtain a copy of the LGPL at
 *      http://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt
 *
 *  You may obtain a copy of the AL at
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the LGPL or the AL for the specific language governing permissions and
 *  limitations.
 */
package org.microemu.app;

import java.io.InputStream;
import java.util.ArrayList;

import org.microemu.DisplayComponent;
import org.microemu.MIDletBridge;
import org.microemu.app.headless.CaptureOptions;
import org.microemu.app.headless.FrameRecorder;
import org.microemu.app.headless.InputScript;
import org.microemu.app.headless.TickDriver;
import org.microemu.app.ui.Message;
import org.microemu.app.ui.noui.NoUiDisplayComponent;
import org.microemu.app.util.DeviceEntry;
import org.microemu.app.util.FileRecordStoreManager;
import org.microemu.device.DeviceDisplay;
import org.microemu.device.EmulatorContext;
import org.microemu.device.FontManager;
import org.microemu.device.InputMethod;
import org.microemu.device.impl.DeviceImpl;
import org.microemu.device.j2se.J2SEDevice;
import org.microemu.device.j2se.J2SEDeviceDisplay;
import org.microemu.device.j2se.J2SEFontManager;
import org.microemu.device.j2se.J2SEInputMethod;
import org.microemu.log.Logger;

public class Headless {

	private Common emulator;

	// arlecchino-midp-rt fork: the component is kept in a field so that the frame
	// capture mode (--frames-dir) can subscribe to its repaints
	private NoUiDisplayComponent displayComponent = new NoUiDisplayComponent();

	private EmulatorContext context = new EmulatorContext() {

		private InputMethod inputMethod = new J2SEInputMethod();

		private DeviceDisplay deviceDisplay = new J2SEDeviceDisplay(this);

		private FontManager fontManager = new J2SEFontManager();

		public DisplayComponent getDisplayComponent() {
			return displayComponent;
		}

		public InputMethod getDeviceInputMethod() {
			return inputMethod;
		}

		public DeviceDisplay getDeviceDisplay() {
			return deviceDisplay;
		}

		public FontManager getDeviceFontManager() {
			return fontManager;
		}

		public InputStream getResourceAsStream(Class origClass, String name) {
            return MIDletBridge.getCurrentMIDlet().getClass().getResourceAsStream(name);
		}
		
		public boolean platformRequest(final String URL) {
			new Thread(new Runnable() {
				public void run() {
					Message.info("MIDlet requests that the device handle the following URL: " + URL);
				}
			}).start();

			return false;
		}
	};

	public Headless() {
		emulator = new Common(context);
	}

	public static void main(String[] args) {
		// arlecchino-midp-rt fork: headless is silent by default (no-op
		// MMAPI players) — frame diff tests are deterministic and do not depend on
		// an audio device being present; overridden by -Dorg.microemu.media.noop=false
		if (System.getProperty("org.microemu.media.noop") == null) {
			System.setProperty("org.microemu.media.noop", "true");
		}

		// arlecchino-midp-rt fork: frame capture mode (--frames-dir DIR
		// --frames N --period-ms MS --boot-wait-ms MS --inputs FILE) +
		// the --rms DIR extension (file-backed RMS flat in DIR); our own flags
		// are stripped here, the rest goes to Common as before
		CaptureOptions opts;
		try {
			opts = CaptureOptions.parse(args);
		} catch (IllegalArgumentException e) {
			System.err.println(e.getMessage());
			System.exit(2);
			return;
		}

		StringBuffer debugArgs = new StringBuffer();
		ArrayList params = new ArrayList();

		// Allow to override in command line
		// Non-persistent RMS
		params.add("--rms");
		params.add("memory");

		if (opts.rmsDir != null) {
			opts.rmsDir.mkdirs();
			FileRecordStoreManager.setStorageRoot(opts.rmsDir);
			params.add("--rms");
			params.add("file");
		}

		for (int i = 0; i < opts.runtimeArgs.size(); i++) {
			String arg = (String) opts.runtimeArgs.get(i);
			params.add(arg);
			if (debugArgs.length() != 0) {
				debugArgs.append(", ");
			}
			debugArgs.append("[").append(arg).append("]");
		}

		if (args.length > 0) {
			Logger.debug("headless arguments", debugArgs.toString());
		}

		Headless app = new Headless();

		DeviceEntry defaultDevice = new DeviceEntry("Default device", null, DeviceImpl.DEFAULT_LOCATION, true, false);

		app.emulator.initParams(params, defaultDevice, J2SEDevice.class);

		FrameRecorder recorder = null;
		if (opts.enabled()) {
			recorder = new FrameRecorder(opts.framesDir);
			app.displayComponent.addDisplayRepaintListener(recorder);
		}

		// arlecchino-midp-rt fork: frame bridge to the Rust renderer (allods-shell).
		// Active only with -Dallods.shell.socket=PATH / env ALLODS_SHELL_SOCKET;
		// otherwise a no-op (the normal headless mode is unchanged). It can coexist
		// with --frames-dir: components hold a single listener, so if capture is
		// on, the bridge does not replace the recorder (an explicit --frames-dir wins).
		if (recorder == null) {
			org.microemu.app.shell.ShellFrameBridge.attachIfEnabled(app.displayComponent);
		}

		// arlecchino-midp-rt fork: deterministic --ticks N mode — input by
		// tick number, synchronously at the frame boundary (org.microemu.TickInput);
		// the driver is installed BEFORE the MIDlet starts so that tick 0 is not missed
		TickDriver ticks = null;
		if (opts.tickMode()) {
			java.util.List events;
			try {
				events = (opts.inputsFile != null) ? InputScript.parse(opts.inputsFile)
						: new java.util.ArrayList();
			} catch (java.io.IOException e) {
				System.err.println("inputs: " + e.getMessage());
				System.exit(2);
				return;
			}
			if (!events.isEmpty() && !InputScript.byTick(events)) {
				System.err.println("inputs: в режиме --ticks события задаются полем tick, а не t_ms");
				System.exit(2);
				return;
			}
			ticks = new TickDriver(opts, recorder, events);
			org.microemu.TickInput.install(ticks);
		}

		app.emulator.initMIDlet(true);

		if (ticks != null) {
			System.exit(ticks.await());
		}

		if (recorder != null) {
			// the MIDlet's non-daemon threads keep the JVM alive — exit ourselves when capture ends
			System.exit(recorder.captureLoop(opts));
		}
	}

}
