/*
 *  arlecchino-midp-rt smoke launcher: headless MIDlet launch + a frame dump to PNG.
 *
 *  Written for the arlecchino-midp-rt fork (2026), LGPL 2.1 — see LICENSE/NOTICE
 *  at the repository root. The fork is based on MicroEmulator (barteo/microemu).
 *
 *  Usage: java SmokeLaunch <out.png> <game.jar | MIDletClass> [Common arguments...]
 *
 *  ASM instrumentation of MIDlet classes is ON (with ASM 9 it also works on modern
 *  bytecode, major 52+); it can be turned off with the --no-instrument argument.
 */

import java.io.File;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.List;

import javax.imageio.ImageIO;

import org.microemu.DisplayComponent;
import org.microemu.MIDletBridge;
import org.microemu.app.Common;
import org.microemu.app.ui.DisplayRepaintListener;
import org.microemu.app.ui.Message;
import org.microemu.app.ui.noui.NoUiDisplayComponent;
import org.microemu.app.util.DeviceEntry;
import org.microemu.device.DeviceDisplay;
import org.microemu.device.EmulatorContext;
import org.microemu.device.FontManager;
import org.microemu.device.InputMethod;
import org.microemu.device.impl.DeviceImpl;
import org.microemu.device.j2se.J2SEDevice;
import org.microemu.device.j2se.J2SEDeviceDisplay;
import org.microemu.device.j2se.J2SEFontManager;
import org.microemu.device.j2se.J2SEGraphicsSurface;
import org.microemu.device.j2se.J2SEInputMethod;

public class SmokeLaunch {

	private static volatile J2SEGraphicsSurface lastSurface;

	public static void main(String[] args) {
		// Any failure -> immediate exit: otherwise the MIDlet's non-daemon threads
		// leave the JVM hanging after an exception in main
		try {
			run(args);
		} catch (Throwable e) {
			e.printStackTrace();
			System.exit(3);
		}
	}

	private static void run(String[] args) throws Exception {
		if (args.length < 2) {
			System.err.println("usage: SmokeLaunch <out.png> <game.jar|MIDletClass> [args...]");
			System.exit(1);
		}
		final String outPng = args[0];

		final NoUiDisplayComponent displayComponent = new NoUiDisplayComponent();
		displayComponent.addDisplayRepaintListener(new DisplayRepaintListener() {
			public void repaintInvoked(Object repaintObject) {
				lastSurface = (J2SEGraphicsSurface) repaintObject;
			}
		});

		EmulatorContext context = new EmulatorContext() {

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

			public boolean platformRequest(String url) {
				Message.info("MIDlet requests that the device handle the following URL: " + url);
				return false;
			}
		};

		Common emulator = new Common(context);

		List params = new ArrayList();
		params.add("--rms");
		params.add("memory");
		for (int i = 1; i < args.length; i++) {
			params.add(args[i]);
		}

		DeviceEntry defaultDevice = new DeviceEntry("Default device", null, DeviceImpl.DEFAULT_LOCATION, true, false);
		emulator.initParams(params, defaultDevice, J2SEDevice.class);
		emulator.initMIDlet(true);

		// Let the MIDlet finish its initial drawing
		Thread.sleep(2500);

		J2SEGraphicsSurface surface = lastSurface;
		if (surface == null) {
			System.err.println("NO_FRAME (repaint не пришёл)");
			System.exit(2);
		}
		synchronized (surface) {
			ImageIO.write(surface.getImage(), "png", new File(outPng));
		}
		System.out.println("FRAME_WRITTEN " + outPng);
		System.exit(0);
	}
}
