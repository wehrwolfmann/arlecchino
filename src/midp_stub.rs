//! Clean-room MIDP 2.0 / CLDC 1.1 API stub generator (Stage Э8).
//!
//! The bytecode member renamer (`assets/MemberRenamer.java`) decides whether an in-jar
//! method fulfils an EXTERNAL contract — i.e. overrides a framework callback such as
//! `Canvas.paint`, `MIDlet.startApp` or `CommandListener.commandAction` — and, if so,
//! LOCKS it (never renames it, preserving virtual dispatch to the runtime).
//!
//! Its PRECISE path resolves each external supertype by reflection on the renamer's
//! resolver classpath and checks whether that supertype really declares the method
//! (`jdkResolvable` / `jdkDeclaresMethod`).  This only works when the
//! `javax.microedition.*` types are *present* on that classpath.  The JDK does not ship
//! them, so without a stub the renamer falls back to the hardcoded `MIDP_CALLBACKS`
//! denylist.
//!
//! Stage Э8 makes the precise path ALWAYS available: we GENERATE — not vendor — a
//! minimal clean-room stub of the public MIDP 2.0 (JSR-118) / CLDC 1.1 (JSR-139) ABI
//! (public class/interface + method SIGNATURES only, empty/throwing bodies; no
//! implementation copied, no third-party jar downloaded) and `javac` it into
//! `~/.arlecchino/midp-stub.jar`.  [`stub_jar`] is then always added to the renamer's
//! resolver classpath.
//!
//! Clean-room note: the signatures below are derived from the published JSR-118 /
//! JSR-139 API signature lists.  They are public ABI surface (names + types) only — no
//! method bodies, constant values, or documentation text from any reference
//! implementation are reproduced.  Nothing is downloaded or vendored.
//!
//! Everything degrades gracefully: if `javac` is unavailable the build produces no jar
//! and the renamer falls back to the `MIDP_CALLBACKS` denylist exactly as before.

use crate::util;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the generated stub jar lives (next to `engines/`, under `~/.arlecchino`).
pub const STUB_JAR_NAME: &str = "midp-stub.jar";

/// Root of the tool's per-user state (`$ARLECCHINO_HOME`, else `~/.arlecchino`).
pub fn arlecchino_home() -> PathBuf {
    match std::env::var("ARLECCHINO_HOME") {
        Ok(v) if !v.is_empty() => util::expanduser(&v),
        _ => util::home_dir().join(".arlecchino"),
    }
}

/// Canonical location of the generated MIDP/CLDC stub jar.
pub fn stub_jar_path() -> PathBuf {
    arlecchino_home().join(STUB_JAR_NAME)
}

/// The stub jar iff it exists and is non-empty (else `None` — the caller falls back to
/// the `MIDP_CALLBACKS` denylist).
pub fn stub_jar() -> Option<PathBuf> {
    let p = stub_jar_path();
    match std::fs::metadata(&p) {
        Ok(md) if md.is_file() && md.len() > 0 => Some(p),
        _ => None,
    }
}

// ───────────────────────── clean-room signature set ─────────────────────────────
// Each entry: internal path -> (package, Java source containing ONLY public/protected
// ABI signatures with empty/throwing bodies).  Bodies are intentionally trivial; we
// never reproduce any reference implementation.  Methods the renamer might LOCK
// (framework callbacks the obfuscated jar overrides) are present with their exact
// descriptors so `jdkDeclaresMethod` resolves them precisely.
//
// Coverage rationale (what the renamer needs):
//   * MIDlet.startApp/pauseApp/destroyApp          — app entry callbacks
//   * Displayable / Canvas / GameCanvas callbacks  — paint/key/pointer/show/hide/size
//   * CommandListener / ItemCommandListener        — commandAction(...)
//   * PlayerListener                               — playerUpdate(...)
//   * Item / CustomItem                            — paint/getMin*/getPref*/keyPressed…
//   * Runnable (CLDC java.lang) is the JDK's own — already resolvable, not stubbed.
// We declare just enough of the type GRAPH (extends/implements) for these to load and
// for the callback descriptors to be discoverable via reflection.

const PKG_LCDUI: &str = "javax.microedition.lcdui";
const PKG_GAME: &str = "javax.microedition.lcdui.game";
const PKG_MEDIA: &str = "javax.microedition.media";
const PKG_MIDLET: &str = "javax.microedition.midlet";
const PKG_RMS: &str = "javax.microedition.rms";
const PKG_IO: &str = "javax.microedition.io";

/// Compose one stub compilation unit out of its package and its signature body.
fn src(pkg: &str, body: &str) -> String {
    format!("package {pkg};\n\n{body}\n")
}

// Bodies are empty for void, `throw new RuntimeException("stub")` only where a return
// value is needed at COMPILE time (these classes are never executed — they exist purely
// for reflection/type resolution on the renamer's classpath).
const SOURCES: &[(&str, &str, &str)] = &[
    // ── javax.microedition.midlet ───────────────────────────────────────
    (
        "javax/microedition/midlet/MIDlet.java",
        PKG_MIDLET,
        r#"public abstract class MIDlet {
    protected MIDlet() {}
    protected abstract void startApp();
    protected abstract void pauseApp();
    protected abstract void destroyApp(boolean unconditional);
    public final void notifyDestroyed() {}
    public final void notifyPaused() {}
    public final void resumeRequest() {}
    public final String getAppProperty(String key) { return null; }
    public final boolean platformRequest(String url) { return false; }
    public final int checkPermission(String permission) { return 0; }
}"#,
    ),
    (
        "javax/microedition/midlet/MIDletStateChangeException.java",
        PKG_MIDLET,
        r#"public class MIDletStateChangeException extends Exception {
    public MIDletStateChangeException() {}
    public MIDletStateChangeException(String s) { super(s); }
}"#,
    ),
    // ── javax.microedition.lcdui ────────────────────────────────────────
    (
        "javax/microedition/lcdui/Displayable.java",
        PKG_LCDUI,
        r#"public abstract class Displayable {
    Displayable() {}
    public int getWidth() { return 0; }
    public int getHeight() { return 0; }
    public boolean isShown() { return false; }
    public void addCommand(Command cmd) {}
    public void removeCommand(Command cmd) {}
    public void setCommandListener(CommandListener l) {}
    public void setTitle(String s) {}
    public String getTitle() { return null; }
    public Ticker getTicker() { return null; }
    public void setTicker(Ticker ticker) {}
    protected void sizeChanged(int w, int h) {}
}"#,
    ),
    (
        "javax/microedition/lcdui/Canvas.java",
        PKG_LCDUI,
        r#"public abstract class Canvas extends Displayable {
    public static final int UP = 1;
    public static final int DOWN = 6;
    public static final int LEFT = 2;
    public static final int RIGHT = 5;
    public static final int FIRE = 8;
    public static final int GAME_A = 9;
    public static final int GAME_B = 10;
    public static final int GAME_C = 11;
    public static final int GAME_D = 12;
    public static final int KEY_NUM0 = 48;
    public static final int KEY_NUM1 = 49;
    public static final int KEY_NUM2 = 50;
    public static final int KEY_NUM3 = 51;
    public static final int KEY_NUM4 = 52;
    public static final int KEY_NUM5 = 53;
    public static final int KEY_NUM6 = 54;
    public static final int KEY_NUM7 = 55;
    public static final int KEY_NUM8 = 56;
    public static final int KEY_NUM9 = 57;
    public static final int KEY_STAR = 42;
    public static final int KEY_POUND = 35;
    protected Canvas() {}
    public boolean isDoubleBuffered() { return false; }
    public boolean hasPointerEvents() { return false; }
    public boolean hasPointerMotionEvents() { return false; }
    public boolean hasRepeatEvents() { return false; }
    public int getGameAction(int keyCode) { return 0; }
    public int getKeyCode(int gameAction) { return 0; }
    public String getKeyName(int keyCode) { return null; }
    public final void repaint() {}
    public final void repaint(int x, int y, int w, int h) {}
    public final void serviceRepaints() {}
    public void setFullScreenMode(boolean mode) {}
    protected void showNotify() {}
    protected void hideNotify() {}
    protected abstract void paint(Graphics g);
    protected void keyPressed(int keyCode) {}
    protected void keyReleased(int keyCode) {}
    protected void keyRepeated(int keyCode) {}
    protected void pointerPressed(int x, int y) {}
    protected void pointerReleased(int x, int y) {}
    protected void pointerDragged(int x, int y) {}
}"#,
    ),
    (
        "javax/microedition/lcdui/Graphics.java",
        PKG_LCDUI,
        r#"public class Graphics {
    public static final int HCENTER = 1;
    public static final int VCENTER = 2;
    public static final int LEFT = 4;
    public static final int RIGHT = 8;
    public static final int TOP = 16;
    public static final int BOTTOM = 32;
    public static final int BASELINE = 64;
    public static final int SOLID = 0;
    public static final int DOTTED = 1;
    Graphics() {}
    public void setColor(int rgb) {}
    public void setColor(int r, int g, int b) {}
    public int getColor() { return 0; }
    public void setGrayScale(int value) {}
    public int getGrayScale() { return 0; }
    public Font getFont() { return null; }
    public void setFont(Font font) {}
    public void setStrokeStyle(int style) {}
    public int getStrokeStyle() { return 0; }
    public void setClip(int x, int y, int w, int h) {}
    public void clipRect(int x, int y, int w, int h) {}
    public int getClipX() { return 0; }
    public int getClipY() { return 0; }
    public int getClipWidth() { return 0; }
    public int getClipHeight() { return 0; }
    public void translate(int x, int y) {}
    public int getTranslateX() { return 0; }
    public int getTranslateY() { return 0; }
    public void drawLine(int x1, int y1, int x2, int y2) {}
    public void drawRect(int x, int y, int w, int h) {}
    public void fillRect(int x, int y, int w, int h) {}
    public void drawRoundRect(int x, int y, int w, int h, int aw, int ah) {}
    public void fillRoundRect(int x, int y, int w, int h, int aw, int ah) {}
    public void drawArc(int x, int y, int w, int h, int sa, int aa) {}
    public void fillArc(int x, int y, int w, int h, int sa, int aa) {}
    public void fillTriangle(int x1, int y1, int x2, int y2, int x3, int y3) {}
    public void drawString(String str, int x, int y, int anchor) {}
    public void drawSubstring(String str, int off, int len, int x, int y, int anchor) {}
    public void drawChar(char ch, int x, int y, int anchor) {}
    public void drawChars(char[] data, int off, int len, int x, int y, int anchor) {}
    public void drawImage(Image img, int x, int y, int anchor) {}
    public void drawRegion(Image src, int xsrc, int ysrc, int w, int h, int transform, int xdst, int ydst, int anchor) {}
    public void drawRGB(int[] rgbData, int off, int scanlen, int x, int y, int w, int h, boolean alpha) {}
    public void copyArea(int xsrc, int ysrc, int w, int h, int xdst, int ydst, int anchor) {}
}"#,
    ),
    (
        "javax/microedition/lcdui/Image.java",
        PKG_LCDUI,
        r#"public class Image {
    Image() {}
    public static Image createImage(int width, int height) { return new Image(); }
    public static Image createImage(String name) { throw new RuntimeException("stub"); }
    public static Image createImage(Image source) { return new Image(); }
    public static Image createImage(byte[] data, int off, int len) { return new Image(); }
    public static Image createImage(Image image, int x, int y, int w, int h, int transform) { return new Image(); }
    public Graphics getGraphics() { return null; }
    public int getWidth() { return 0; }
    public int getHeight() { return 0; }
    public boolean isMutable() { return false; }
    public void getRGB(int[] rgbData, int off, int scanlen, int x, int y, int w, int h) {}
}"#,
    ),
    (
        "javax/microedition/lcdui/Font.java",
        PKG_LCDUI,
        r#"public final class Font {
    public static final int STYLE_PLAIN = 0;
    public static final int STYLE_BOLD = 1;
    public static final int STYLE_ITALIC = 2;
    public static final int STYLE_UNDERLINED = 4;
    public static final int SIZE_SMALL = 8;
    public static final int SIZE_MEDIUM = 0;
    public static final int SIZE_LARGE = 16;
    public static final int FACE_SYSTEM = 0;
    public static final int FACE_MONOSPACE = 32;
    public static final int FACE_PROPORTIONAL = 64;
    public static final int FONT_STATIC_TEXT = 0;
    public static final int FONT_INPUT_TEXT = 1;
    Font() {}
    public static Font getDefaultFont() { return new Font(); }
    public static Font getFont(int face, int style, int size) { return new Font(); }
    public static Font getFont(int fontSpecifier) { return new Font(); }
    public int getStyle() { return 0; }
    public int getSize() { return 0; }
    public int getFace() { return 0; }
    public boolean isPlain() { return true; }
    public boolean isBold() { return false; }
    public boolean isItalic() { return false; }
    public boolean isUnderlined() { return false; }
    public int getHeight() { return 0; }
    public int getBaselinePosition() { return 0; }
    public int charWidth(char ch) { return 0; }
    public int charsWidth(char[] ch, int off, int len) { return 0; }
    public int stringWidth(String str) { return 0; }
    public int substringWidth(String str, int off, int len) { return 0; }
}"#,
    ),
    (
        "javax/microedition/lcdui/Command.java",
        PKG_LCDUI,
        r#"public class Command {
    public static final int SCREEN = 1;
    public static final int BACK = 2;
    public static final int CANCEL = 3;
    public static final int OK = 4;
    public static final int HELP = 5;
    public static final int STOP = 6;
    public static final int EXIT = 7;
    public static final int ITEM = 8;
    public Command(String label, int commandType, int priority) {}
    public Command(String shortLabel, String longLabel, int commandType, int priority) {}
    public String getLabel() { return null; }
    public String getLongLabel() { return null; }
    public int getCommandType() { return 0; }
    public int getPriority() { return 0; }
}"#,
    ),
    (
        "javax/microedition/lcdui/CommandListener.java",
        PKG_LCDUI,
        r#"public interface CommandListener {
    void commandAction(Command c, Displayable d);
}"#,
    ),
    (
        "javax/microedition/lcdui/Ticker.java",
        PKG_LCDUI,
        r#"public class Ticker {
    public Ticker(String str) {}
    public void setString(String str) {}
    public String getString() { return null; }
}"#,
    ),
    (
        "javax/microedition/lcdui/Display.java",
        PKG_LCDUI,
        r#"public class Display {
    public static final int LIST_ELEMENT = 1;
    public static final int CHOICE_GROUP_ELEMENT = 2;
    public static final int ALERT = 3;
    public static final int COLOR_BACKGROUND = 0;
    public static final int COLOR_FOREGROUND = 1;
    Display() {}
    public static Display getDisplay(javax.microedition.midlet.MIDlet m) { return new Display(); }
    public void setCurrent(Displayable next) {}
    public void setCurrent(Alert alert, Displayable next) {}
    public Displayable getCurrent() { return null; }
    public boolean isColor() { return true; }
    public int numColors() { return 0; }
    public int numAlphaLevels() { return 0; }
    public int getColor(int colorSpecifier) { return 0; }
    public boolean flashBacklight(int duration) { return false; }
    public boolean vibrate(int duration) { return false; }
    public boolean callSerially(Runnable r) { return false; }
    public int getBestImageWidth(int imageType) { return 0; }
    public int getBestImageHeight(int imageType) { return 0; }
}"#,
    ),
    (
        "javax/microedition/lcdui/Screen.java",
        PKG_LCDUI,
        r#"public abstract class Screen extends Displayable {
    Screen() {}
}"#,
    ),
    (
        "javax/microedition/lcdui/Alert.java",
        PKG_LCDUI,
        r#"public class Alert extends Screen {
    public Alert(String title) {}
    public Alert(String title, String alertText, Image image, AlertType type) {}
    public int getDefaultTimeout() { return 0; }
    public int getTimeout() { return 0; }
    public void setTimeout(int time) {}
    public String getString() { return null; }
    public void setString(String str) {}
    public Image getImage() { return null; }
    public void setImage(Image img) {}
}"#,
    ),
    (
        "javax/microedition/lcdui/AlertType.java",
        PKG_LCDUI,
        r#"public class AlertType {
    public static final AlertType INFO = new AlertType();
    public static final AlertType WARNING = new AlertType();
    public static final AlertType ERROR = new AlertType();
    public static final AlertType ALARM = new AlertType();
    public static final AlertType CONFIRMATION = new AlertType();
    protected AlertType() {}
    public boolean playSound(Display display) { return false; }
}"#,
    ),
    (
        "javax/microedition/lcdui/Form.java",
        PKG_LCDUI,
        r#"public class Form extends Screen {
    public Form(String title) {}
    public Form(String title, Item[] items) {}
    public int append(Item item) { return 0; }
    public int append(String str) { return 0; }
    public int append(Image img) { return 0; }
    public void delete(int itemNum) {}
    public void deleteAll() {}
    public Item get(int itemNum) { return null; }
    public void set(int itemNum, Item item) {}
    public void insert(int itemNum, Item item) {}
    public int size() { return 0; }
    public void setItemStateListener(ItemStateListener iListener) {}
}"#,
    ),
    (
        "javax/microedition/lcdui/Item.java",
        PKG_LCDUI,
        r#"public abstract class Item {
    public static final int PLAIN = 0;
    public static final int HYPERLINK = 1;
    public static final int BUTTON = 2;
    public static final int LAYOUT_DEFAULT = 0;
    public static final int LAYOUT_LEFT = 1;
    public static final int LAYOUT_RIGHT = 2;
    public static final int LAYOUT_CENTER = 3;
    Item() {}
    public String getLabel() { return null; }
    public void setLabel(String label) {}
    public int getLayout() { return 0; }
    public void setLayout(int layout) {}
    public void addCommand(Command cmd) {}
    public void removeCommand(Command cmd) {}
    public void setItemCommandListener(ItemCommandListener l) {}
    public void setDefaultCommand(Command cmd) {}
    public int getPreferredWidth() { return 0; }
    public int getPreferredHeight() { return 0; }
    public void setPreferredSize(int width, int height) {}
    public int getMinimumWidth() { return 0; }
    public int getMinimumHeight() { return 0; }
    public void notifyStateChanged() {}
}"#,
    ),
    (
        "javax/microedition/lcdui/ItemCommandListener.java",
        PKG_LCDUI,
        r#"public interface ItemCommandListener {
    void commandAction(Command c, Item item);
}"#,
    ),
    (
        "javax/microedition/lcdui/ItemStateListener.java",
        PKG_LCDUI,
        r#"public interface ItemStateListener {
    void itemStateChanged(Item item);
}"#,
    ),
    (
        "javax/microedition/lcdui/CustomItem.java",
        PKG_LCDUI,
        r#"public abstract class CustomItem extends Item {
    public static final int TRAVERSE_HORIZONTAL = 1;
    public static final int TRAVERSE_VERTICAL = 2;
    public static final int NONE = 0;
    public static final int KEY_PRESS = 1;
    public static final int KEY_RELEASE = 2;
    public static final int KEY_REPEAT = 4;
    public static final int POINTER_PRESS = 8;
    public static final int POINTER_RELEASE = 16;
    public static final int POINTER_DRAG = 32;
    public static final int TRAVERSE_IN = 64;
    protected CustomItem(String label) {}
    protected abstract int getMinContentWidth();
    protected abstract int getMinContentHeight();
    protected abstract int getPrefContentWidth(int height);
    protected abstract int getPrefContentHeight(int width);
    protected abstract void paint(Graphics g, int w, int h);
    protected final int getGameAction(int keyCode) { return 0; }
    protected final void repaint() {}
    protected final void repaint(int x, int y, int w, int h) {}
    protected final void invalidate() {}
    protected int getInteractionModes() { return 0; }
    protected void keyPressed(int keyCode) {}
    protected void keyReleased(int keyCode) {}
    protected void keyRepeated(int keyCode) {}
    protected void pointerPressed(int x, int y) {}
    protected void pointerReleased(int x, int y) {}
    protected void pointerDragged(int x, int y) {}
    protected void showNotify() {}
    protected void hideNotify() {}
    protected void sizeChanged(int w, int h) {}
    protected boolean traverse(int dir, int viewportWidth, int viewportHeight, int[] visRect_inout) { return false; }
    protected void traverseOut() {}
}"#,
    ),
    (
        "javax/microedition/lcdui/List.java",
        PKG_LCDUI,
        r#"public class List extends Screen {
    public static final Command SELECT_COMMAND = new Command("", Command.SCREEN, 0);
    public List(String title, int listType) {}
    public List(String title, int listType, String[] stringElements, Image[] imageElements) {}
    public int append(String str, Image img) { return 0; }
    public int size() { return 0; }
    public String getString(int elementNum) { return null; }
    public int getSelectedIndex() { return 0; }
    public void setSelectedIndex(int elementNum, boolean selected) {}
    public void delete(int elementNum) {}
    public void deleteAll() {}
}"#,
    ),
    (
        "javax/microedition/lcdui/TextBox.java",
        PKG_LCDUI,
        r#"public class TextBox extends Screen {
    public TextBox(String title, String text, int maxSize, int constraints) {}
    public String getString() { return null; }
    public void setString(String text) {}
    public int getMaxSize() { return 0; }
    public int setMaxSize(int maxSize) { return 0; }
    public int size() { return 0; }
}"#,
    ),
    // ── javax.microedition.lcdui.game ───────────────────────────────────
    (
        "javax/microedition/lcdui/game/GameCanvas.java",
        PKG_GAME,
        r#"import javax.microedition.lcdui.Canvas;
import javax.microedition.lcdui.Graphics;

public abstract class GameCanvas extends Canvas {
    public static final int UP_PRESSED = 0x0002;
    public static final int DOWN_PRESSED = 0x0040;
    public static final int LEFT_PRESSED = 0x0004;
    public static final int RIGHT_PRESSED = 0x0020;
    public static final int FIRE_PRESSED = 0x0100;
    public static final int GAME_A_PRESSED = 0x0200;
    public static final int GAME_B_PRESSED = 0x0400;
    public static final int GAME_C_PRESSED = 0x0800;
    public static final int GAME_D_PRESSED = 0x1000;
    protected GameCanvas(boolean suppressKeyEvents) {}
    protected Graphics getGraphics() { return null; }
    public int getKeyStates() { return 0; }
    public void paint(Graphics g) {}
    public void flushGraphics() {}
    public void flushGraphics(int x, int y, int width, int height) {}
}"#,
    ),
    (
        "javax/microedition/lcdui/game/Layer.java",
        PKG_GAME,
        r#"import javax.microedition.lcdui.Graphics;

public abstract class Layer {
    Layer() {}
    public void setPosition(int x, int y) {}
    public void move(int dx, int dy) {}
    public final int getX() { return 0; }
    public final int getY() { return 0; }
    public final int getWidth() { return 0; }
    public final int getHeight() { return 0; }
    public void setVisible(boolean visible) {}
    public final boolean isVisible() { return false; }
    public abstract void paint(Graphics g);
}"#,
    ),
    (
        "javax/microedition/lcdui/game/Sprite.java",
        PKG_GAME,
        r#"import javax.microedition.lcdui.Image;
import javax.microedition.lcdui.Graphics;

public class Sprite extends Layer {
    public static final int TRANS_NONE = 0;
    public static final int TRANS_ROT90 = 5;
    public static final int TRANS_ROT180 = 3;
    public static final int TRANS_ROT270 = 6;
    public static final int TRANS_MIRROR = 2;
    public static final int TRANS_MIRROR_ROT90 = 7;
    public static final int TRANS_MIRROR_ROT180 = 1;
    public static final int TRANS_MIRROR_ROT270 = 4;
    public Sprite(Image image) {}
    public Sprite(Image image, int frameWidth, int frameHeight) {}
    public Sprite(Sprite s) {}
    public void setFrame(int sequenceIndex) {}
    public final int getFrame() { return 0; }
    public int getRawFrameCount() { return 0; }
    public int getFrameSequenceLength() { return 0; }
    public void nextFrame() {}
    public void prevFrame() {}
    public final void paint(Graphics g) {}
    public void setFrameSequence(int[] sequence) {}
    public void setImage(Image img, int frameWidth, int frameHeight) {}
    public void defineReferencePixel(int x, int y) {}
    public void setRefPixelPosition(int x, int y) {}
    public int getRefPixelX() { return 0; }
    public int getRefPixelY() { return 0; }
    public void setTransform(int transform) {}
    public final boolean collidesWith(Sprite s, boolean pixelLevel) { return false; }
    public final boolean collidesWith(Image image, int x, int y, boolean pixelLevel) { return false; }
}"#,
    ),
    (
        "javax/microedition/lcdui/game/TiledLayer.java",
        PKG_GAME,
        r#"import javax.microedition.lcdui.Image;
import javax.microedition.lcdui.Graphics;

public class TiledLayer extends Layer {
    public TiledLayer(int columns, int rows, Image image, int tileWidth, int tileHeight) {}
    public int createAnimatedTile(int staticTileIndex) { return 0; }
    public void setAnimatedTile(int animatedTileIndex, int staticTileIndex) {}
    public int getAnimatedTile(int animatedTileIndex) { return 0; }
    public void setCell(int col, int row, int tileIndex) {}
    public int getCell(int col, int row) { return 0; }
    public void fillCells(int col, int row, int numCols, int numRows, int tileIndex) {}
    public final int getCellWidth() { return 0; }
    public final int getCellHeight() { return 0; }
    public final int getColumns() { return 0; }
    public final int getRows() { return 0; }
    public void setStaticTileSet(Image image, int tileWidth, int tileHeight) {}
    public final void paint(Graphics g) {}
}"#,
    ),
    (
        "javax/microedition/lcdui/game/LayerManager.java",
        PKG_GAME,
        r#"import javax.microedition.lcdui.Graphics;

public class LayerManager {
    public LayerManager() {}
    public void append(Layer l) {}
    public void insert(Layer l, int index) {}
    public Layer getLayerAt(int index) { return null; }
    public int getSize() { return 0; }
    public void remove(Layer l) {}
    public void paint(Graphics g, int x, int y) {}
    public void setViewWindow(int x, int y, int width, int height) {}
}"#,
    ),
    (
        "javax/microedition/media/Controllable.java",
        PKG_MEDIA,
        r#"public interface Controllable {
    javax.microedition.media.Control[] getControls();
    javax.microedition.media.Control getControl(String controlType);
}"#,
    ),
    (
        "javax/microedition/media/Control.java",
        PKG_MEDIA,
        r#"public interface Control {
}"#,
    ),
    // ── javax.microedition.media ────────────────────────────────────────
    (
        "javax/microedition/media/Player.java",
        PKG_MEDIA,
        r#"public interface Player extends Controllable {
    int UNREALIZED = 100;
    int REALIZED = 200;
    int PREFETCHED = 300;
    int STARTED = 400;
    int CLOSED = 0;
    long TIME_UNKNOWN = -1L;
    void realize() throws MediaException;
    void prefetch() throws MediaException;
    void start() throws MediaException;
    void stop() throws MediaException;
    void deallocate();
    void close();
    void setLoopCount(int count);
    long setMediaTime(long now) throws MediaException;
    long getMediaTime();
    int getState();
    long getDuration();
    String getContentType();
    void setTimeBase(TimeBase master) throws MediaException;
    TimeBase getTimeBase();
    void addPlayerListener(PlayerListener playerListener);
    void removePlayerListener(PlayerListener playerListener);
}"#,
    ),
    (
        "javax/microedition/media/PlayerListener.java",
        PKG_MEDIA,
        r#"public interface PlayerListener {
    String STARTED = "started";
    String STOPPED = "stopped";
    String END_OF_MEDIA = "endOfMedia";
    String DURATION_UPDATED = "durationUpdated";
    String DEVICE_UNAVAILABLE = "deviceUnavailable";
    String DEVICE_AVAILABLE = "deviceAvailable";
    String VOLUME_CHANGED = "volumeChanged";
    String SIZE_CHANGED = "sizeChanged";
    String ERROR = "error";
    String CLOSED = "closed";
    void playerUpdate(Player player, String event, Object eventData);
}"#,
    ),
    (
        "javax/microedition/media/Manager.java",
        PKG_MEDIA,
        r#"public final class Manager {
    public static final String TONE_DEVICE_LOCATOR = "device://tone";
    private Manager() {}
    public static Player createPlayer(String locator) throws java.io.IOException, MediaException { throw new MediaException("stub"); }
    public static Player createPlayer(java.io.InputStream stream, String type) throws java.io.IOException, MediaException { throw new MediaException("stub"); }
    public static void playTone(int note, int duration, int volume) throws MediaException {}
    public static String[] getSupportedContentTypes(String protocol) { return new String[0]; }
    public static String[] getSupportedProtocols(String contentType) { return new String[0]; }
    public static TimeBase getSystemTimeBase() { return null; }
}"#,
    ),
    (
        "javax/microedition/media/MediaException.java",
        PKG_MEDIA,
        r#"public class MediaException extends Exception {
    public MediaException() {}
    public MediaException(String reason) { super(reason); }
}"#,
    ),
    (
        "javax/microedition/media/TimeBase.java",
        PKG_MEDIA,
        r#"public interface TimeBase {
    long getTime();
}"#,
    ),
    // ── javax.microedition.rms ──────────────────────────────────────────
    (
        "javax/microedition/rms/RecordStore.java",
        PKG_RMS,
        r#"public class RecordStore {
    public static final int AUTHMODE_PRIVATE = 0;
    public static final int AUTHMODE_ANY = 1;
    RecordStore() {}
    public static RecordStore openRecordStore(String name, boolean createIfNecessary) throws RecordStoreException { throw new RecordStoreException("stub"); }
    public static void deleteRecordStore(String name) throws RecordStoreException {}
    public static String[] listRecordStores() { return null; }
    public void closeRecordStore() throws RecordStoreException {}
    public int addRecord(byte[] data, int offset, int numBytes) throws RecordStoreException { return 0; }
    public byte[] getRecord(int recordId) throws RecordStoreException { return null; }
    public int getRecord(int recordId, byte[] buffer, int offset) throws RecordStoreException { return 0; }
    public void setRecord(int recordId, byte[] newData, int offset, int numBytes) throws RecordStoreException {}
    public void deleteRecord(int recordId) throws RecordStoreException {}
    public int getNumRecords() throws RecordStoreException { return 0; }
    public int getRecordSize(int recordId) throws RecordStoreException { return 0; }
    public int getNextRecordID() throws RecordStoreException { return 0; }
    public int getSize() throws RecordStoreException { return 0; }
    public int getSizeAvailable() throws RecordStoreException { return 0; }
    public RecordEnumeration enumerateRecords(RecordFilter filter, RecordComparator comparator, boolean keepUpdated) throws RecordStoreException { return null; }
}"#,
    ),
    (
        "javax/microedition/rms/RecordStoreException.java",
        PKG_RMS,
        r#"public class RecordStoreException extends Exception {
    public RecordStoreException() {}
    public RecordStoreException(String message) { super(message); }
}"#,
    ),
    (
        "javax/microedition/rms/RecordStoreNotOpenException.java",
        PKG_RMS,
        r#"public class RecordStoreNotOpenException extends RecordStoreException {
    public RecordStoreNotOpenException() {}
    public RecordStoreNotOpenException(String message) { super(message); }
}"#,
    ),
    (
        "javax/microedition/rms/RecordStoreNotFoundException.java",
        PKG_RMS,
        r#"public class RecordStoreNotFoundException extends RecordStoreException {
    public RecordStoreNotFoundException() {}
    public RecordStoreNotFoundException(String message) { super(message); }
}"#,
    ),
    (
        "javax/microedition/rms/RecordStoreFullException.java",
        PKG_RMS,
        r#"public class RecordStoreFullException extends RecordStoreException {
    public RecordStoreFullException() {}
    public RecordStoreFullException(String message) { super(message); }
}"#,
    ),
    (
        "javax/microedition/rms/InvalidRecordIDException.java",
        PKG_RMS,
        r#"public class InvalidRecordIDException extends RecordStoreException {
    public InvalidRecordIDException() {}
    public InvalidRecordIDException(String message) { super(message); }
}"#,
    ),
    (
        "javax/microedition/rms/RecordEnumeration.java",
        PKG_RMS,
        r#"public interface RecordEnumeration {
    int numRecords();
    byte[] nextRecord() throws InvalidRecordIDException, RecordStoreNotOpenException, RecordStoreException;
    int nextRecordId() throws InvalidRecordIDException;
    byte[] previousRecord() throws InvalidRecordIDException, RecordStoreNotOpenException, RecordStoreException;
    int previousRecordId() throws InvalidRecordIDException;
    boolean hasNextElement();
    boolean hasPreviousElement();
    void reset();
    void rebuild();
    void keepUpdated(boolean keepUpdated);
    boolean isKeptUpdated();
    void destroy();
}"#,
    ),
    (
        "javax/microedition/rms/RecordFilter.java",
        PKG_RMS,
        r#"public interface RecordFilter {
    boolean matches(byte[] candidate);
}"#,
    ),
    (
        "javax/microedition/rms/RecordComparator.java",
        PKG_RMS,
        r#"public interface RecordComparator {
    int EQUIVALENT = 0;
    int FOLLOWS = 1;
    int PRECEDES = -1;
    int compare(byte[] rec1, byte[] rec2);
}"#,
    ),
    (
        "javax/microedition/rms/RecordListener.java",
        PKG_RMS,
        r#"public interface RecordListener {
    void recordAdded(RecordStore recordStore, int recordId);
    void recordChanged(RecordStore recordStore, int recordId);
    void recordDeleted(RecordStore recordStore, int recordId);
}"#,
    ),
    // ── javax.microedition.io ───────────────────────────────────────────
    (
        "javax/microedition/io/Connection.java",
        PKG_IO,
        r#"public interface Connection {
    void close() throws java.io.IOException;
}"#,
    ),
    (
        "javax/microedition/io/InputConnection.java",
        PKG_IO,
        r#"public interface InputConnection extends Connection {
    java.io.InputStream openInputStream() throws java.io.IOException;
    java.io.DataInputStream openDataInputStream() throws java.io.IOException;
}"#,
    ),
    (
        "javax/microedition/io/OutputConnection.java",
        PKG_IO,
        r#"public interface OutputConnection extends Connection {
    java.io.OutputStream openOutputStream() throws java.io.IOException;
    java.io.DataOutputStream openDataOutputStream() throws java.io.IOException;
}"#,
    ),
    (
        "javax/microedition/io/StreamConnection.java",
        PKG_IO,
        r#"public interface StreamConnection extends InputConnection, OutputConnection {
}"#,
    ),
    (
        "javax/microedition/io/ContentConnection.java",
        PKG_IO,
        r#"public interface ContentConnection extends StreamConnection {
    String getType();
    String getEncoding();
    long getLength();
}"#,
    ),
    (
        "javax/microedition/io/HttpConnection.java",
        PKG_IO,
        r#"public interface HttpConnection extends ContentConnection {
    String HEAD = "HEAD";
    String GET = "GET";
    String POST = "POST";
    int HTTP_OK = 200;
    int HTTP_NOT_FOUND = 404;
    String getURL();
    String getProtocol();
    String getHost();
    int getPort();
    String getRequestMethod();
    void setRequestMethod(String method) throws java.io.IOException;
    String getRequestProperty(String key);
    void setRequestProperty(String key, String value) throws java.io.IOException;
    int getResponseCode() throws java.io.IOException;
    String getResponseMessage() throws java.io.IOException;
    String getHeaderField(String name) throws java.io.IOException;
    int getHeaderFieldInt(String name, int def) throws java.io.IOException;
}"#,
    ),
    (
        "javax/microedition/io/Connector.java",
        PKG_IO,
        r#"public class Connector {
    public static final int READ = 1;
    public static final int WRITE = 2;
    public static final int READ_WRITE = 3;
    private Connector() {}
    public static Connection open(String name) throws java.io.IOException { throw new java.io.IOException("stub"); }
    public static Connection open(String name, int mode) throws java.io.IOException { throw new java.io.IOException("stub"); }
    public static Connection open(String name, int mode, boolean timeouts) throws java.io.IOException { throw new java.io.IOException("stub"); }
    public static java.io.InputStream openInputStream(String name) throws java.io.IOException { throw new java.io.IOException("stub"); }
    public static java.io.OutputStream openOutputStream(String name) throws java.io.IOException { throw new java.io.IOException("stub"); }
    public static java.io.DataInputStream openDataInputStream(String name) throws java.io.IOException { throw new java.io.IOException("stub"); }
    public static java.io.DataOutputStream openDataOutputStream(String name) throws java.io.IOException { throw new java.io.IOException("stub"); }
}"#,
    ),
];

/// Materialise the clean-room stub `.java` sources under `src_root`.  Returns the list
/// of written file paths, in table order.
pub fn generate_sources(src_root: &Path) -> Vec<PathBuf> {
    let mut written: Vec<PathBuf> = Vec::new();
    for (rel, pkg, body) in SOURCES {
        let p = src_root.join(rel);
        if let Some(parent) = p.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                continue;
            }
        }
        if std::fs::write(&p, src(pkg, body)).is_ok() {
            written.push(p);
        }
    }
    written
}

/// Generate + `javac`-compile the clean-room MIDP/CLDC stub into a jar.
///
/// Returns a human-readable status string.  Degrades gracefully: if `javac` is
/// unavailable (or compilation fails) the jar is NOT produced and the renamer keeps
/// using the `MIDP_CALLBACKS` denylist.
pub fn build_stub(dst: Option<&Path>, workdir: Option<&Path>) -> String {
    let dst: PathBuf = dst.map(|p| p.to_path_buf()).unwrap_or_else(stub_jar_path);
    let work: PathBuf =
        workdir.map(|p| p.to_path_buf()).unwrap_or_else(|| arlecchino_home().join("midp-stub-src"));
    let src_root = work.join("src");
    let cls_root = work.join("classes");
    if cls_root.exists() {
        let _ = std::fs::remove_dir_all(&cls_root);
    }
    if std::fs::create_dir_all(&cls_root).is_err() {
        return format!("FAILED (cannot create {})", cls_root.display());
    }

    let srcs = generate_sources(&src_root);
    if srcs.len() != SOURCES.len() {
        return format!(
            "FAILED (wrote {}/{} stub sources under {})",
            srcs.len(),
            SOURCES.len(),
            src_root.display()
        );
    }
    let javac = util::tool("ARLECCHINO_JAVAC", "javac");
    // CLDC 1.1 targets an old bytecode level, but we only need the classes to LOAD for
    // reflection on a modern JDK — compile with the running JDK's default level.
    let mut argv: Vec<String> = vec![
        javac,
        "-d".into(),
        cls_root.to_string_lossy().into_owned(),
        "-encoding".into(),
        "utf-8".into(),
        "-nowarn".into(),
        "-proc:none".into(),
    ];
    argv.extend(srcs.iter().map(|p| p.to_string_lossy().into_owned()));
    let r = util::run_capture(&argv, Some(Duration::from_secs(180)));
    if let Some(e) = r.spawn_err {
        return format!("skip (javac not runnable: {e}; denylist fallback in effect)");
    }
    let mut classes: Vec<PathBuf> = util::rglob_ext(&cls_root, "class");
    classes.sort();
    if r.status != Some(0) || classes.is_empty() {
        let tail: String =
            r.stderr.chars().rev().take(500).collect::<String>().chars().rev().collect();
        return format!(
            "FAILED javac ({} classes; denylist fallback in effect). tail:\n{tail}",
            classes.len()
        );
    }

    if let Some(p) = dst.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let tmp = dst.with_extension("jar.tmp");
    if let Err(e) = pack_classes(&classes, &cls_root, &tmp) {
        return format!("FAILED (cannot write {}: {e}; denylist fallback in effect)", tmp.display());
    }
    if let Err(e) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return format!("FAILED (cannot install {}: {e})", dst.display());
    }
    format!("ok ({} classes, {} sources -> {})", classes.len(), srcs.len(), dst.display())
}

/// Zip the compiled stub classes (paths relative to `cls_root`) into `out_jar`.
fn pack_classes(classes: &[PathBuf], cls_root: &Path, out_jar: &Path) -> Result<(), String> {
    let outf = std::fs::File::create(out_jar).map_err(|e| e.to_string())?;
    let mut zout = zip::ZipWriter::new(outf);
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for cls in classes {
        let rel = cls.strip_prefix(cls_root).map_err(|e| e.to_string())?;
        let data = std::fs::read(cls).map_err(|e| e.to_string())?;
        zout.start_file(rel.to_string_lossy().into_owned(), opts).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut zout, &data).map_err(|e| e.to_string())?;
    }
    zout.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// The stub jar, building it once on demand if it is not there yet.
///
/// `setup` generates the stub up front, but the renamer must not depend on that having
/// happened: a first run on a fresh machine still gets the PRECISE resolver path.
/// Returns `None` when the stub cannot be produced (no `javac`) — the renamer then uses
/// its `MIDP_CALLBACKS` denylist, exactly as before Stage Э8.
pub fn ensure_stub() -> Option<PathBuf> {
    if let Some(p) = stub_jar() {
        return Some(p);
    }
    let status = build_stub(None, None);
    let found = stub_jar();
    if found.is_none() {
        println!("   MIDP/CLDC stub: {status}");
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table entry names a `.java` file inside its own package directory.
    #[test]
    fn table_paths_match_packages() {
        assert_eq!(SOURCES.len(), 50);
        for (rel, pkg, _body) in SOURCES {
            assert!(rel.ends_with(".java"), "{rel}");
            let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            assert_eq!(dir, pkg.replace('.', "/"), "{rel}");
        }
    }

    /// The composed unit is `package …;` + the signature body + a trailing newline.
    #[test]
    fn composed_unit_shape() {
        let s = src("javax.microedition.midlet", "public abstract class MIDlet {\n}");
        assert_eq!(s, "package javax.microedition.midlet;\n\npublic abstract class MIDlet {\n}\n");
    }

    /// The stub really builds and carries the types whose callbacks the renamer probes.
    /// Skipped (not failed) where `javac` is unavailable — that is the denylist path.
    #[test]
    fn generated_stub_compiles() {
        let work = match util::make_temp_dir("arl_stub_test_") {
            Ok(w) => w,
            Err(_) => return,
        };
        let dst = work.join("home").join(STUB_JAR_NAME);
        let status = build_stub(Some(&dst), Some(&work.join("work")));
        if status.starts_with("skip") {
            let _ = std::fs::remove_dir_all(&work);
            return; // no javac on this machine
        }
        assert!(status.starts_with("ok"), "{status}");
        let names = util::zip_namelist(&dst).expect("stub jar unreadable");
        for want in [
            "javax/microedition/midlet/MIDlet.class",
            "javax/microedition/lcdui/Canvas.class",
            "javax/microedition/lcdui/Graphics.class",
            "javax/microedition/lcdui/CommandListener.class",
            "javax/microedition/lcdui/ItemStateListener.class",
            "javax/microedition/media/PlayerListener.class",
            "javax/microedition/rms/RecordStore.class",
            "javax/microedition/io/Connector.class",
        ] {
            assert!(names.iter().any(|n| n == want), "{want} missing from stub jar");
        }
        let _ = std::fs::remove_dir_all(&work);
    }
}
