/*
 *  arlecchino-midp-rt smoke-launcher: headless-запуск MIDlet + дамп кадра в PNG.
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 *
 *  Использование: java SmokeLaunch <out.png> <game.jar | MIDletClass> [аргументы Common...]
 *
 *  ASM-инструментация классов MIDlet ВКЛЮЧЕНА (с ASM 9 работает и на современном
 *  байткоде, major 52+); отключить можно аргументом --no-instrument.
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
		// Любой сбой -> немедленный exit: иначе non-daemon потоки MIDlet'а
		// оставляют JVM висеть после исключения в main
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

		// Даём MIDlet'у отработать стартовую отрисовку
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
