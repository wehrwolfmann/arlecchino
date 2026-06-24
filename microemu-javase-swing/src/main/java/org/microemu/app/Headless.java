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

	// Форк arlecchino-midp-rt: компонент вынесен в поле, чтобы режим захвата
	// кадров (--frames-dir) мог подписаться на repaint'ы
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
		// Форк arlecchino-midp-rt: headless по умолчанию без звука (no-op
		// MMAPI-плееры) — diff-тесты кадров детерминированы и не зависят от
		// наличия аудиоустройства; переопределяется -Dorg.microemu.media.noop=false
		if (System.getProperty("org.microemu.media.noop") == null) {
			System.setProperty("org.microemu.media.noop", "true");
		}

		// Форк arlecchino-midp-rt: режим захвата кадров (--frames-dir DIR
		// --frames N --period-ms MS --boot-wait-ms MS --inputs FILE) +
		// расширение --rms DIR (file-backed RMS плоско в DIR); свои флаги
		// снимаются здесь, остаток уходит в Common как раньше
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

		// Форк arlecchino-midp-rt: мост кадров в Rust-рендер (allods-shell).
		// Активен только при -Dallods.shell.socket=PATH / env ALLODS_SHELL_SOCKET;
		// иначе no-op (обычный headless-режим не меняется). Может сосуществовать
		// с --frames-dir: компоненты держат один listener, поэтому если включён
		// захват, мост не подменяет рекордер (приоритет у явного --frames-dir).
		if (recorder == null) {
			org.microemu.app.shell.ShellFrameBridge.attachIfEnabled(app.displayComponent);
		}

		app.emulator.initMIDlet(true);

		if (recorder != null) {
			// non-daemon потоки MIDlet'а держат JVM — по концу захвата выходим сами
			System.exit(recorder.captureLoop(opts));
		}
	}

}
