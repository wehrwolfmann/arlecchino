/*
 *  arlecchino-midp-rt: тестовый MIDlet для smoke-теста headless-рендера
 *  (Canvas: fillRect + drawString; System.out проверяет Injected-редирект
 *  ASM-инструментации).
 *
 *  Написан для форка arlecchino-midp-rt (2026), LGPL 2.1 — см. LICENSE/NOTICE
 *  в корне репозитория. База форка — MicroEmulator (barteo/microemu).
 */
import javax.microedition.midlet.MIDlet;
import javax.microedition.lcdui.*;

public class TestMidlet extends MIDlet {
    static class C extends Canvas {
        // последний keyCode — фикстура проверки --inputs (нижняя полоса
        // меняет цвет от нажатой клавиши: кадр после ввода != кадру до)
        private int lastKey = 0;

        protected void paint(Graphics g) {
            g.setColor(0xFF0000);
            g.fillRect(0, 0, getWidth(), getHeight());
            g.setColor(0xFFFFFF);
            g.drawString("HELLO", 10, 10, Graphics.TOP | Graphics.LEFT);
            g.setColor((lastKey * 0x010305) & 0xFFFFFF);
            g.fillRect(0, getHeight() - 12, getWidth(), 12);
            System.out.println("PAINT_CALLED " + getWidth() + "x" + getHeight());
        }

        protected void keyPressed(int keyCode) {
            lastKey = keyCode;
            System.out.println("KEY_PRESSED " + keyCode);
            repaint();
        }
    }
    protected void startApp() {
        System.out.println("START_APP");
        C c = new C();
        Display.getDisplay(this).setCurrent(c);
        c.repaint();
        // Без авто-destroy: кадр дампит SmokeLaunch и сам завершает процесс
    }
    protected void pauseApp() {}
    protected void destroyApp(boolean u) {}
}
