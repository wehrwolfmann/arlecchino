/*
 *  arlecchino-midp-rt: живой smoke-тест звука JSR-135 (MMAPI) на реальном
 *  MIDI-файле (сценарий Аллодов: createPlayer/realize/prefetch/
 *  setLoopCount(-1)/start + VolumeControl.setLevel/setMute, затем playTone).
 *  Запуск: java -cp ../build/midp-rt.jar:. MmapiSmoke <file.mid>
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE.
 */
import java.io.ByteArrayInputStream;
import java.nio.file.Files;
import java.nio.file.Paths;

import javax.microedition.media.Manager;
import javax.microedition.media.Player;
import javax.microedition.media.PlayerListener;
import javax.microedition.media.control.VolumeControl;

public class MmapiSmoke {
	public static void main(String[] args) throws Exception {
		byte[] mid = Files.readAllBytes(Paths.get(args[0]));
		Player p = Manager.createPlayer(new ByteArrayInputStream(mid), "audio/midi");
		p.addPlayerListener(new PlayerListener() {
			public void playerUpdate(Player pl, String event, Object data) {
				System.out.println("EVENT " + event + " " + data);
			}
		});
		p.realize();
		p.prefetch();
		p.setLoopCount(-1);
		System.out.println("PREFETCHED state=" + p.getState() + " duration_us=" + p.getDuration());
		VolumeControl vc = (VolumeControl) p.getControl("VolumeControl");
		p.start();
		vc.setLevel(100);
		vc.setMute(false);
		System.out.println("STARTED state=" + p.getState());
		Thread.sleep(5000);
		System.out.println("mediaTime_us=" + p.getMediaTime());
		System.out.println("MUTE ON");
		vc.setMute(true);
		Thread.sleep(2000);
		System.out.println("MUTE OFF level=30");
		vc.setMute(false);
		vc.setLevel(30);
		Thread.sleep(2000);
		p.stop();
		System.out.println("STOPPED state=" + p.getState());
		p.close();
		System.out.println("CLOSED state=" + p.getState());
		System.out.println("TONE test");
		Manager.playTone(72, 400, 100);
		Thread.sleep(600);
		Manager.playTone(60, 400, 100);
		Thread.sleep(800);
		System.out.println("DONE");
		System.exit(0);
	}
}
