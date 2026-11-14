//! A thin Java ME platform: just enough MIDP/CLDC to run a translated game headless
//! and write a call trace in midp-rt's `MidpTrace` text format, so that the trace of
//! the translation and the trace of the original jar can be diffed frame by frame.
//!
//! Pixels are not produced here.  Every drawing call is also forwarded to the
//! installed [`crate::gfx::Backend`] (none by default), which is where a real
//! renderer plugs in.
//!
//! Semantics kept identical to `arlecchino difftest --fixed-clock`:
//! `currentTimeMillis` is a virtual clock that starts at 1_000_000 ms and
//! advances 1 ms per call; every `new Random(..)` uses FIXED_SEED.

use crate::{gfx, throw, throw_new, JArr, JRef, JStr, Peer};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;

/// Panic payload that ends the run once enough frames were traced.
#[derive(Debug)]
pub struct Stop;

pub const FIXED_SEED: i64 = 0x5DEECE66D ^ 0xA110D5;

struct Host {
    res_dir: PathBuf,
    trace: Vec<String>,
    frame: i32,
    max_frames: i32,
    vt_nanos: i64,
    current: Peer,
    threads: Vec<Peer>,
    repaint_pending: bool,
    width: i32,
    height: i32,
    next_obj: i32,
    stores: HashMap<String, Vec<Option<Vec<i8>>>>,
    keys: Vec<(i32, i32, bool)>, // (after distinct frame n, key, pressed)
    in_paint: bool,
    distinct: i32,
    last_body: Vec<String>,
    body_start: usize,
}

thread_local! {
    static HOST: RefCell<Host> = RefCell::new(Host {
        res_dir: PathBuf::from("."),
        trace: vec!["# jrt trace".into()],
        frame: -1,
        max_frames: 50,
        vt_nanos: 1_000_000_000_000,
        current: Peer::default(),
        threads: vec![],
        repaint_pending: false,
        width: 240,
        height: 320,
        next_obj: 1,
        stores: HashMap::new(),
        keys: vec![],
        in_paint: false,
        distinct: 0,
        last_body: vec![],
        body_start: 0,
    });
}

fn host<R>(f: impl FnOnce(&mut Host) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

fn tr(line: String) {
    host(|h| {
        if h.frame < h.max_frames {
            h.trace.push(line)
        }
    });
}

fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' | '\\' => {
                o.push('\\');
                o.push(ch)
            }
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push(' '),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn next_id() -> i32 {
    host(|h| {
        let i = h.next_obj;
        h.next_obj += 1;
        i
    })
}

/// Runs a MIDlet: static init, startApp, then the first started thread on this
/// OS thread (cooperative: `Thread.sleep`/`serviceRepaints` are the yield points).
/// Returns the trace lines.
pub fn run_midlet(res_dir: &str, w: i32, h: i32, frames: i32, keys: Vec<(i32, i32, bool)>, clinit: fn(), midlet: fn() -> Peer) -> Vec<String> {
    crate::install_hook();
    host(|x| {
        x.res_dir = PathBuf::from(res_dir);
        x.width = w;
        x.height = h;
        x.max_frames = frames;
        x.keys = keys;
    });
    let r = crate::catch(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            clinit();
            let m = midlet();
            m.get().start_app();
            loop {
                let t = host(|x| if x.threads.is_empty() { None } else { Some(x.threads.remove(0)) });
                match t {
                    Some(t) => t.get().run(),
                    None => {
                        // No game thread: the "event thread" keeps delivering input
                        // and painting.  A MIDlet that only reacts to events (no
                        // thread, no pending repaint) is idle: hand it the next
                        // scheduled key at once, or end the run when none is left.
                        if !pump() && !deliver_next_key_now() {
                            std::panic::panic_any(Stop);
                        }
                    }
                }
            }
        }))
    });
    match r {
        Ok(Err(p)) if p.downcast_ref::<Stop>().is_some() => {}
        Ok(Err(p)) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            host(|x| x.trace.push(format!("# STOPPED by panic: {msg}")));
        }
        Ok(Ok(_)) => {}
        Err(t) => host(|x| x.trace.push(format!("# STOPPED by uncaught {}", t.class))),
    }
    host(|x| std::mem::take(&mut x.trace))
}

/// Parses a key schedule `frame:code:d|u,…` (`105:-5:d,106:-5:u`): press (`d`) or
/// release (`u`) key `code` after the `frame`-th distinct frame.
pub fn parse_keys(spec: &str) -> Vec<(i32, i32, bool)> {
    spec.split(',')
        .filter(|x| !x.trim().is_empty())
        .filter_map(|x| {
            let p: Vec<&str> = x.trim().split(':').collect();
            match p.as_slice() {
                [frame, code, dir] => Some((frame.parse().ok()?, code.parse().ok()?, *dir == "d")),
                _ => None,
            }
        })
        .collect()
}

pub fn write_trace(path: &str, lines: &[String]) {
    let mut f = std::fs::File::create(path).expect("trace file");
    for l in lines {
        let _ = writeln!(f, "{l}");
    }
}

/// Idle event thread: delivers the next scheduled key regardless of its frame.
/// Returns false when no key is left.
fn deliver_next_key_now() -> bool {
    let next = host(|h| if h.keys.is_empty() { None } else { Some(h.keys.remove(0)) });
    match next {
        Some((_, k, down)) => {
            deliver_key(k, down);
            true
        }
        None => false,
    }
}

fn deliver_key(k: i32, down: bool) {
    let cur = host(|h| h.current.clone());
    if cur.is_null() {
        return;
    }
    if down {
        trace_event(format!("keyPressed {k}"));
        cur.get().key_pressed(k)
    } else {
        trace_event(format!("keyReleased {k}"));
        cur.get().key_released(k)
    }
}

/// One "event thread" step: deliver due keys, paint if a repaint is pending.
/// Returns whether anything happened.
fn pump() -> bool {
    let keys = host(|h| {
        // keys are scheduled "after the n-th distinct frame" (see scripts/frames.sh);
        // the open frame is judged tentatively, it is committed at the next paint
        let n = h.distinct + if h.body_start != 0 && frame_body(h) != h.last_body { 1 } else { 0 };
        let mut due = vec![];
        while let Some(k) = h.keys.first() {
            if k.0 <= n {
                due.push((k.1, k.2));
                h.keys.remove(0);
            } else {
                break;
            }
        }
        due
    });
    let delivered = !keys.is_empty();
    for (k, down) in keys {
        deliver_key(k, down);
    }
    let pending = host(|h| std::mem::replace(&mut h.repaint_pending, false));
    if pending {
        paint_now();
    }
    delivered || pending
}

fn trace_event(l: String) {
    host(|h| h.trace.push(l));
}

/// Counts distinct frames the same way scripts/frames.sh does.
fn frame_body(h: &Host) -> Vec<String> {
    h.trace[h.body_start..]
        .iter()
        .filter(|l| {
            !(l.as_str() == "repaint" || l.as_str() == "serviceRepaints" || l.as_str() == "translate 0 0" || l.starts_with("Font.getFont") || l.starts_with("key") || l.starts_with("getGameAction") || l.starts_with("#") || l.starts_with("Player.realize") || l.starts_with("Player.prefetch"))
        })
        .cloned()
        .collect()
}

fn close_frame(h: &mut Host) {
    if h.body_start == 0 {
        return;
    }
    let body = frame_body(h);
    h.body_start = 0;
    if body != h.last_body {
        h.distinct += 1;
        h.last_body = body;
    }
}

fn paint_now() {
    let (cur, stop) = host(|h| {
        close_frame(h);
        h.frame += 1;
        let stop = h.frame >= h.max_frames;
        if !stop {
            let f = h.frame;
            h.trace.push(format!("F {f} t=0"));
            h.body_start = h.trace.len();
        }
        (h.current.clone(), stop)
    });
    if stop {
        std::panic::panic_any(Stop);
    }
    if cur.is_null() {
        return;
    }
    let (w, h) = host(|x| (x.width, x.height));
    let frame = host(|x| x.frame);
    let g = JRef::new(GraphicsObj { target: Some(gfx::Target::Screen), clip: (0, 0, w, h), ..Default::default() });
    gfx::with(|b| b.begin_frame(frame, w, h));
    host(|x| x.in_paint = true);
    cur.get().paint(g);
    host(|x| x.in_paint = false);
    gfx::with(|b| b.end_frame(frame));
}

// ---- java.lang ---------------------------------------------------------------

pub fn current_time_millis() -> i64 {
    host(|h| {
        let v = h.vt_nanos;
        h.vt_nanos += 1_000_000;
        v / 1_000_000
    })
}

pub fn gc() {}

pub fn sleep(_ms: i64) {
    pump();
}

pub fn yield_now() {
    pump();
}

pub type Thread = JRef<ThreadObj>;
#[derive(Default)]
pub struct ThreadObj {
    target: Peer,
}
pub fn thread_new(target: Peer) -> Thread {
    JRef::new(ThreadObj { target })
}
pub fn thread_start(t: &Thread) {
    let p = t.rd(|t| t.target.clone());
    host(|h| h.threads.push(p));
}

pub type Random = JRef<j2me_jvm::Random>;
pub fn random_new() -> Random {
    JRef::new(j2me_jvm::Random::new(FIXED_SEED))
}
pub fn random_new_seed(_seed: i64) -> Random {
    JRef::new(j2me_jvm::Random::new(FIXED_SEED))
}
pub fn random_next_int(r: &Random) -> i32 {
    r.wr(|r| r.next_int())
}
pub fn random_next_int_bound(r: &Random, n: i32) -> i32 {
    if n <= 0 {
        throw_new("java.lang.IllegalArgumentException");
    }
    r.wr(|r| r.next_int_bound(n))
}
pub fn random_next_long(r: &Random) -> i64 {
    r.wr(|r| r.next_long())
}
pub fn random_set_seed(r: &Random, _s: i64) {
    r.wr(|r| r.set_seed(FIXED_SEED))
}

pub fn integer_to_string(i: i32) -> JStr {
    JStr::lit(&i.to_string())
}
pub fn string_value_of_int(i: i32) -> JStr {
    JStr::lit(&i.to_string())
}
pub fn integer_parse_int(s: &JStr) -> i32 {
    match j2me_jvm::parse_int(s.units()) {
        Ok(v) => v,
        Err(_) => throw_new("java.lang.NumberFormatException"),
    }
}

pub type StringBuffer = JRef<Vec<u16>>;
pub mod string_buffer {
    use super::*;
    pub fn new() -> StringBuffer {
        JRef::new(vec![])
    }
    pub fn new_str(s: JStr) -> StringBuffer {
        JRef::new(s.units().to_vec())
    }
    pub fn append_str(b: &StringBuffer, s: JStr) -> StringBuffer {
        let u: Vec<u16> = if s.is_null() { "null".encode_utf16().collect() } else { s.units().to_vec() };
        b.wr(|v| v.extend(u));
        b.clone()
    }
    pub fn append_int(b: &StringBuffer, i: i32) -> StringBuffer {
        b.wr(|v| v.extend(i.to_string().encode_utf16()));
        b.clone()
    }
    pub fn append_char(b: &StringBuffer, c: u16) -> StringBuffer {
        b.wr(|v| v.push(c));
        b.clone()
    }
    pub fn to_string(b: &StringBuffer) -> JStr {
        JStr::from_units(b.rd(|v| v.clone()))
    }
    pub fn length(b: &StringBuffer) -> i32 {
        b.rd(|v| v.len() as i32)
    }
    pub fn char_at(b: &StringBuffer, i: i32) -> u16 {
        match b.rd(|v| v.get(i as usize).copied()) {
            Some(c) if i >= 0 => c,
            _ => throw_new("java.lang.StringIndexOutOfBoundsException"),
        }
    }
    pub fn delete_char_at(b: &StringBuffer, i: i32) -> StringBuffer {
        let n = length(b);
        if i < 0 || i >= n {
            throw_new("java.lang.StringIndexOutOfBoundsException");
        }
        b.wr(|v| {
            v.remove(i as usize);
        });
        b.clone()
    }
    pub fn set_length(b: &StringBuffer, n: i32) {
        if n < 0 {
            throw_new("java.lang.StringIndexOutOfBoundsException");
        }
        b.wr(|v| v.resize(n as usize, 0));
    }
    pub fn insert_str(b: &StringBuffer, at: i32, s: JStr) -> StringBuffer {
        let n = length(b);
        if at < 0 || at > n {
            throw_new("java.lang.StringIndexOutOfBoundsException");
        }
        let u: Vec<u16> = if s.is_null() { "null".encode_utf16().collect() } else { s.units().to_vec() };
        b.wr(|v| {
            let tail = v.split_off(at as usize);
            v.extend(u);
            v.extend(tail);
        });
        b.clone()
    }
}

// ---- resources / io ----------------------------------------------------------

pub type InputStream = JRef<j2me_jvm::ByteArrayInputStream>;
pub type DataInputStream = JRef<DataIn>;
pub struct DataIn(pub InputStream);
impl Default for DataIn {
    fn default() -> Self {
        DataIn(JRef::null())
    }
}

fn resource_bytes(name: &str) -> Option<Vec<u8>> {
    let p = host(|h| h.res_dir.join(name.trim_start_matches('/')));
    std::fs::read(p).ok()
}

pub fn get_resource_as_stream(name: &JStr) -> InputStream {
    match resource_bytes(&name.to_rust()) {
        Some(b) => JRef::new(j2me_jvm::ByteArrayInputStream::new(b)),
        None => JRef::null(),
    }
}
fn io<T>(r: j2me_jvm::JavaResult<T>) -> T {
    match r {
        Ok(v) => v,
        Err(j2me_jvm::JavaError::Io(m)) if m.contains("EOF") => throw_new("java.io.EOFException"),
        Err(j2me_jvm::JavaError::Io(_)) => throw_new("java.io.IOException"),
        Err(e) => throw(e),
    }
}
pub fn is_read(s: &InputStream) -> i32 {
    s.wr(|s| io(s.read()))
}
pub fn is_read_arr(s: &InputStream, b: &JArr<i8>) -> i32 {
    let n = b.len();
    is_read_range(s, b, 0, n)
}
pub fn is_read_range(s: &InputStream, b: &JArr<i8>, off: i32, len: i32) -> i32 {
    let mut tmp = b.to_vec();
    let r = s.wr(|s| io(s.read_range(&mut tmp, off, len)));
    b.with_mut(|d| d.clone_from_slice(&tmp));
    r
}
pub fn is_skip(s: &InputStream, n: i64) -> i64 {
    s.wr(|s| s.skip(n))
}
pub fn is_available(s: &InputStream) -> i32 {
    s.rd(|s| s.available())
}
pub fn is_close(_s: &InputStream) {}

pub fn dis_new(s: InputStream) -> DataInputStream {
    if s.is_null() {
        throw(j2me_jvm::JavaError::NullPointer);
    }
    JRef::new(DataIn(s))
}
fn with_dis<R>(d: &DataInputStream, f: impl FnOnce(&mut j2me_jvm::DataInputStream) -> j2me_jvm::JavaResult<R>) -> R {
    // DataInputStream over the shared ByteArrayInputStream: rebuild around the current position
    let s = d.rd(|d| d.0.clone());
    s.wr(|bis| {
        let taken = std::mem::replace(bis, j2me_jvm::ByteArrayInputStream::new(vec![]));
        let mut dis = j2me_jvm::DataInputStream::from_stream(taken);
        let r = f(&mut dis);
        *bis = dis.into_inner();
        io(r)
    })
}
pub fn dis_read_byte(d: &DataInputStream) -> i8 {
    with_dis(d, |x| x.read_byte())
}
pub fn dis_read_unsigned_byte(d: &DataInputStream) -> i32 {
    with_dis(d, |x| x.read_unsigned_byte())
}
pub fn dis_read_short(d: &DataInputStream) -> i16 {
    with_dis(d, |x| x.read_short())
}
pub fn dis_read_unsigned_short(d: &DataInputStream) -> i32 {
    with_dis(d, |x| x.read_unsigned_short())
}
pub fn dis_read_int(d: &DataInputStream) -> i32 {
    with_dis(d, |x| x.read_int())
}
pub fn dis_read_long(d: &DataInputStream) -> i64 {
    with_dis(d, |x| x.read_long())
}
pub fn dis_read_boolean(d: &DataInputStream) -> bool {
    with_dis(d, |x| x.read_boolean())
}
pub fn dis_read_char(d: &DataInputStream) -> u16 {
    with_dis(d, |x| x.read_char())
}
pub fn dis_read_utf(d: &DataInputStream) -> JStr {
    JStr::from_units(with_dis(d, |x| x.read_utf()))
}
pub fn dis_read_fully(d: &DataInputStream, b: &JArr<i8>) {
    let mut tmp = b.to_vec();
    with_dis(d, |x| x.read_fully(&mut tmp));
    b.with_mut(|dst| dst.clone_from_slice(&tmp));
}
pub fn dis_read(d: &DataInputStream, b: &JArr<i8>) -> i32 {
    let mut tmp = b.to_vec();
    let r = with_dis(d, |x| x.read(&mut tmp));
    b.with_mut(|dst| dst.clone_from_slice(&tmp));
    r
}
pub fn dis_skip_bytes(d: &DataInputStream, n: i32) -> i32 {
    with_dis(d, |x| x.skip(n as i64)) as i32
}
pub fn dis_close(_d: &DataInputStream) {}

// ---- lcdui -------------------------------------------------------------------

pub type Graphics = JRef<GraphicsObj>;
#[derive(Default)]
pub struct GraphicsObj {
    /// The screen or a mutable image (`None` only for a default-constructed value).
    target: Option<gfx::Target>,
    color: i32,
    clip: (i32, i32, i32, i32),
    tx: i32,
    ty: i32,
    font: Font,
}
impl GraphicsObj {
    fn state(&self) -> gfx::State {
        let font = if self.font.is_null() { (0, 0, 0) } else { self.font.rd(|f| (f.face, f.style, f.size)) };
        gfx::State { color: self.color, clip: self.clip, translate: (self.tx, self.ty), font }
    }
}
/// Writes the trace line (screen only, like midp-rt) and forwards the call to the backend.
fn draw(g: &Graphics, line: impl FnOnce() -> String, call: impl FnOnce(&mut dyn gfx::Backend, gfx::Target, &gfx::State)) {
    let (target, st) = g.rd(|g| (g.target, g.state()));
    let Some(target) = target else { return };
    if target == gfx::Target::Screen {
        tr(line());
    }
    gfx::with(|b| call(b, target, &st));
}
fn gtr(g: &Graphics, line: String) {
    if g.rd(|g| g.target == Some(gfx::Target::Screen)) {
        tr(line);
    }
}
pub fn g_set_color(g: &Graphics, rgb: i32) {
    g.wr(|g| g.color = rgb & 0xffffff);
    gtr(g, format!("setColor {}", rgb & 0xffffff));
}
pub fn g_set_color_rgb(g: &Graphics, r: i32, gr: i32, b: i32) {
    let rgb = ((r & 255) << 16) | ((gr & 255) << 8) | (b & 255);
    g_set_color(g, rgb)
}
pub fn g_get_color(g: &Graphics) -> i32 {
    g.rd(|g| g.color)
}
pub fn g_set_clip(g: &Graphics, x: i32, y: i32, w: i32, h: i32) {
    g.wr(|g| g.clip = (x, y, w, h));
    gtr(g, format!("setClip {x} {y} {w} {h}"));
}
pub fn g_clip_rect(g: &Graphics, x: i32, y: i32, w: i32, h: i32) {
    gtr(g, format!("clipRect {x} {y} {w} {h}"));
    g.wr(|g| {
        let (cx, cy, cw, ch) = g.clip;
        let nx = cx.max(x);
        let ny = cy.max(y);
        let nr = (cx + cw).min(x + w);
        let nb = (cy + ch).min(y + h);
        g.clip = (nx, ny, (nr - nx).max(0), (nb - ny).max(0));
    });
}
pub fn g_get_clip_x(g: &Graphics) -> i32 {
    g.rd(|g| g.clip.0)
}
pub fn g_get_clip_y(g: &Graphics) -> i32 {
    g.rd(|g| g.clip.1)
}
pub fn g_get_clip_width(g: &Graphics) -> i32 {
    g.rd(|g| g.clip.2)
}
pub fn g_get_clip_height(g: &Graphics) -> i32 {
    g.rd(|g| g.clip.3)
}
pub fn g_translate(g: &Graphics, x: i32, y: i32) {
    g.wr(|g| {
        g.tx += x;
        g.ty += y
    });
    gtr(g, format!("translate {x} {y}"));
}
pub fn g_get_translate_x(g: &Graphics) -> i32 {
    g.rd(|g| g.tx)
}
pub fn g_get_translate_y(g: &Graphics) -> i32 {
    g.rd(|g| g.ty)
}
pub fn g_fill_rect(g: &Graphics, x: i32, y: i32, w: i32, h: i32) {
    draw(g, || format!("fillRect {x} {y} {w} {h}"), |b, t, s| b.fill_rect(t, s, x, y, w, h));
}
pub fn g_draw_rect(g: &Graphics, x: i32, y: i32, w: i32, h: i32) {
    draw(g, || format!("drawRect {x} {y} {w} {h}"), |b, t, s| b.draw_rect(t, s, x, y, w, h));
}
pub fn g_draw_line(g: &Graphics, x: i32, y: i32, x2: i32, y2: i32) {
    draw(g, || format!("drawLine {x} {y} {x2} {y2}"), |b, t, s| b.draw_line(t, s, x, y, x2, y2));
}
pub fn g_draw_image(g: &Graphics, img: &Image, x: i32, y: i32, a: i32) {
    if img.is_null() {
        throw(j2me_jvm::JavaError::NullPointer);
    }
    let id = img.rd(|i| i.id);
    draw(g, || format!("drawImage #{id} {x} {y} {a}"), |b, t, s| b.draw_image(t, s, id, x, y, a));
}
#[allow(clippy::too_many_arguments)]
pub fn g_draw_region(g: &Graphics, img: &Image, sx: i32, sy: i32, w: i32, h: i32, tf: i32, x: i32, y: i32, a: i32) {
    if img.is_null() {
        throw(j2me_jvm::JavaError::NullPointer);
    }
    let id = img.rd(|i| i.id);
    draw(
        g,
        || format!("drawRegion #{id} {sx} {sy} {w} {h} {tf} {x} {y} {a}"),
        |b, t, s| b.draw_region(t, s, id, sx, sy, w, h, tf, x, y, a),
    );
}
pub fn g_draw_string(g: &Graphics, text: &JStr, x: i32, y: i32, a: i32) {
    let text = text.to_rust();
    draw(g, || format!("drawString {} {x} {y} {a}", quote(&text)), |b, t, s| b.draw_string(t, s, &text, x, y, a));
}
pub fn g_draw_char(g: &Graphics, c: u16, x: i32, y: i32, a: i32) {
    let text = String::from_utf16_lossy(&[c]);
    draw(g, || format!("drawChar {c} {x} {y} {a}"), |b, t, s| b.draw_string(t, s, &text, x, y, a));
}
pub fn g_set_font(g: &Graphics, f: &Font) {
    let (face, style, size) = f.rd(|f| (f.face, f.style, f.size));
    g.wr(|g| g.font = f.clone());
    gtr(g, format!("setFont {face} {style} {size}"));
}
pub fn g_fill_arc(g: &Graphics, x: i32, y: i32, w: i32, h: i32, s0: i32, e: i32) {
    draw(g, || format!("fillArc {x} {y} {w} {h} {s0} {e}"), |b, t, s| b.fill_arc(t, s, x, y, w, h, s0, e));
}
pub fn g_draw_arc(g: &Graphics, x: i32, y: i32, w: i32, h: i32, s0: i32, e: i32) {
    draw(g, || format!("drawArc {x} {y} {w} {h} {s0} {e}"), |b, t, s| b.draw_arc(t, s, x, y, w, h, s0, e));
}
pub fn g_fill_round_rect(g: &Graphics, x: i32, y: i32, w: i32, h: i32, aw: i32, ah: i32) {
    draw(g, || format!("fillRoundRect {x} {y} {w} {h} {aw} {ah}"), |b, t, s| b.fill_round_rect(t, s, x, y, w, h, aw, ah));
}
pub fn g_draw_round_rect(g: &Graphics, x: i32, y: i32, w: i32, h: i32, aw: i32, ah: i32) {
    draw(g, || format!("drawRoundRect {x} {y} {w} {h} {aw} {ah}"), |b, t, s| b.draw_round_rect(t, s, x, y, w, h, aw, ah));
}
pub fn g_fill_triangle(g: &Graphics, x1: i32, y1: i32, x2: i32, y2: i32, x3: i32, y3: i32) {
    draw(g, || format!("fillTriangle {x1} {y1} {x2} {y2} {x3} {y3}"), |b, t, s| b.fill_triangle(t, s, x1, y1, x2, y2, x3, y3));
}

pub type Image = JRef<ImageObj>;
#[derive(Default)]
pub struct ImageObj {
    pub id: i32,
    pub w: i32,
    pub h: i32,
}
fn png_size(b: &[u8]) -> Option<(i32, i32)> {
    if b.len() >= 24 && &b[1..4] == b"PNG" {
        let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]) as i32;
        let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]) as i32;
        Some((w, h))
    } else {
        None
    }
}
fn new_image(w: i32, h: i32, src: &str, png: Option<&[u8]>) -> Image {
    let id = next_id();
    trace_event(format!("I {id} {w}x{h} {}", quote(src)));
    gfx::with(|b| b.create_image(id, w, h, png));
    JRef::new(ImageObj { id, w, h })
}
pub fn image_create(name: &JStr) -> Image {
    let n = name.to_rust();
    let bytes = resource_bytes(&n);
    match bytes.as_deref().and_then(png_size) {
        Some((w, h)) => new_image(w, h, &n, bytes.as_deref()),
        None => throw_new("java.io.IOException"),
    }
}
pub fn image_create_bytes(data: &JArr<i8>, off: i32, len: i32) -> Image {
    let v: Vec<u8> = data.with(|d| d[off as usize..(off + len) as usize].iter().map(|&x| x as u8).collect());
    match png_size(&v) {
        Some((w, h)) => new_image(w, h, &format!("byte[{len}]"), Some(&v)),
        None => throw_new("java.lang.IllegalArgumentException"),
    }
}
pub fn image_create_mutable(w: i32, h: i32) -> Image {
    new_image(w, h, "mutable", None)
}
pub fn image_get_width(i: &Image) -> i32 {
    i.rd(|i| i.w)
}
pub fn image_get_height(i: &Image) -> i32 {
    i.rd(|i| i.h)
}
pub fn image_get_graphics(i: &Image) -> Graphics {
    let (id, w, h) = i.rd(|i| (i.id, i.w, i.h));
    JRef::new(GraphicsObj { target: Some(gfx::Target::Image(id)), clip: (0, 0, w, h), ..Default::default() })
}

pub type Font = JRef<FontObj>;
#[derive(Default, Clone)]
pub struct FontObj {
    face: i32,
    style: i32,
    size: i32,
}
pub fn font_get(face: i32, style: i32, size: i32) -> Font {
    tr(format!("Font.getFont {face} {style} {size}"));
    JRef::new(FontObj { face, style, size })
}
pub fn font_default() -> Font {
    JRef::new(FontObj::default())
}
/// Metrics of AWT `SansSerif 10` as measured under midp-rt (machine-dependent!).
pub fn font_height(_f: &Font) -> i32 {
    14
}
pub fn font_baseline(_f: &Font) -> i32 {
    11
}
pub fn font_string_width(_f: &Font, s: &JStr) -> i32 {
    s.length() * 6
}
pub fn font_char_width(_f: &Font, _c: u16) -> i32 {
    6
}

pub fn display_get() {}
pub fn display_set_current(p: Peer) {
    trace_event("Display.setCurrent".into());
    host(|h| h.current = p.clone());
    if !p.is_null() {
        p.get().show_notify();
    }
    host(|h| h.repaint_pending = true);
}
pub fn canvas_repaint() {
    tr("repaint".into());
    host(|h| h.repaint_pending = true);
}
pub fn canvas_service_repaints() {
    tr("serviceRepaints".into());
    if host(|h| h.in_paint) {
        return;
    }
    // the event thread also delivers input between paints (pump paints if needed)
    pump();
}
pub fn canvas_width() -> i32 {
    host(|h| h.width)
}
pub fn canvas_height() -> i32 {
    host(|h| h.height)
}
pub fn canvas_set_full_screen(m: bool) {
    trace_event(format!("setFullScreenMode {}", m as i32));
}
/// midp-rt's default key map (standard ITU-T keys and the game actions).
pub fn canvas_get_game_action(k: i32) -> i32 {
    let a = match k {
        -1 | 50 => 1,  // UP
        -2 | 56 => 6,  // DOWN
        -3 | 52 => 2,  // LEFT
        -4 | 54 => 5,  // RIGHT
        -5 | 53 => 8,  // FIRE
        49 => 9,
        51 => 10,
        55 => 11,
        57 => 12,
        _ => 0,
    };
    trace_event(format!("getGameAction {k} {a}"));
    a
}
pub fn midlet_notify_destroyed() {
    std::panic::panic_any(Stop);
}

// ---- rms ---------------------------------------------------------------------

pub type RecordStore = JRef<String>;
pub fn rs_open(name: &JStr, create: bool) -> RecordStore {
    let n = name.to_rust();
    trace_event(format!("RecordStore.open {}", quote(&n)));
    let exists = host(|h| h.stores.contains_key(&n));
    if !exists {
        if !create {
            throw_new("javax.microedition.rms.RecordStoreNotFoundException");
        }
        host(|h| h.stores.insert(n.clone(), vec![]));
    }
    JRef::new(n)
}
pub fn rs_close(r: &RecordStore) {
    let n = r.rd(|n| n.clone());
    trace_event(format!("RecordStore.close {}", quote(&n)));
}
pub fn rs_num_records(r: &RecordStore) -> i32 {
    let n = r.rd(|n| n.clone());
    let c = host(|h| h.stores[&n].iter().filter(|x| x.is_some()).count() as i32);
    trace_event(format!("RecordStore.getNumRecords {} {c}", quote(&n)));
    c
}
pub fn rs_get_record(r: &RecordStore, id: i32) -> JArr<i8> {
    let n = r.rd(|n| n.clone());
    match host(|h| h.stores[&n].get((id - 1) as usize).cloned().flatten()) {
        Some(v) => JArr::from_vec(v),
        None => throw_new("javax.microedition.rms.InvalidRecordIDException"),
    }
}
pub fn rs_add_record(r: &RecordStore, d: &JArr<i8>, off: i32, len: i32) -> i32 {
    let n = r.rd(|n| n.clone());
    let v: Vec<i8> = d.with(|d| d[off as usize..(off + len) as usize].to_vec());
    let hex: String = v.iter().map(|b| format!("{:02x}", *b as u8)).collect();
    trace_event(format!("RecordStore.addRecord {} len={len} hex={hex}", quote(&n)));
    host(|h| {
        let s = h.stores.get_mut(&n).unwrap();
        s.push(Some(v));
        s.len() as i32
    })
}
pub fn rs_set_record(r: &RecordStore, id: i32, d: &JArr<i8>, off: i32, len: i32) {
    let n = r.rd(|n| n.clone());
    let v: Vec<i8> = d.with(|d| d[off as usize..(off + len) as usize].to_vec());
    host(|h| {
        let s = h.stores.get_mut(&n).unwrap();
        if id >= 1 && (id as usize) <= s.len() {
            s[(id - 1) as usize] = Some(v);
        }
    })
}
pub fn rs_delete(name: &JStr) {
    let n = name.to_rust();
    trace_event(format!("RecordStore.delete {}", quote(&n)));
    if host(|h| h.stores.remove(&n)).is_none() {
        throw_new("javax.microedition.rms.RecordStoreNotFoundException");
    }
}

// ---- media -------------------------------------------------------------------

pub type Player = JRef<i32>;
pub fn manager_create_player(s: &InputStream, ty: &JStr) -> Player {
    let n = s.rd(|s| s.count());
    let id = next_id();
    trace_event(format!("P {id} {} {n}b", quote(&ty.to_rust())));
    JRef::new(id)
}
pub fn player_event(p: &Player, m: &str) {
    let id = p.rd(|i| *i);
    trace_event(format!("Player.{m} #{id}"));
}
pub fn player_set_loop_count(_p: &Player, c: i32) {
    trace_event(format!("Player.setLoopCount {c}"));
}
pub fn player_get_state(_p: &Player) -> i32 {
    300
}

// ---- added game by game ------------------------------------------------------

pub fn is_shown() -> bool {
    true
}
pub fn char_to_upper(c: u16) -> u16 {
    match c {
        0x61..=0x7a => c - 32,
        0xe0..=0xfe if c != 0xf7 => c - 32,
        _ => c,
    }
}
pub fn string_from_bytes(b: &JArr<i8>, off: i32, len: i32) -> JStr {
    let v: Vec<u8> = b.with(|d| d[off as usize..(off + len) as usize].iter().map(|&x| x as u8).collect());
    JStr::lit(&String::from_utf8_lossy(&v))
}
pub fn bis_new(b: &JArr<i8>) -> InputStream {
    let v: Vec<u8> = b.with(|d| d.iter().map(|&x| x as u8).collect());
    JRef::new(j2me_jvm::ByteArrayInputStream::new(v))
}
/// `read()` on a DataInputStream: one byte or -1 at the end.
pub fn dis_read_raw(d: &DataInputStream) -> i32 {
    let s = d.rd(|d| d.0.clone());
    is_read(&s)
}
pub fn dis_read_fully_range(d: &DataInputStream, b: &JArr<i8>, off: i32, len: i32) {
    let mut tmp = vec![0i8; len.max(0) as usize];
    with_dis(d, |x| x.read_fully(&mut tmp));
    b.with_mut(|dst| dst[off as usize..(off + len) as usize].copy_from_slice(&tmp));
}
pub type ByteOut = JRef<Vec<i8>>;
pub type DataOutputStream = JRef<ByteOut>;
pub fn bos_new() -> ByteOut {
    JRef::new(vec![])
}
pub fn bos_to_byte_array(b: &ByteOut) -> JArr<i8> {
    JArr::from_vec(b.rd(|v| v.clone()))
}
pub fn dos_new(o: ByteOut) -> DataOutputStream {
    JRef::new(o)
}
fn dos_put(d: &DataOutputStream, bytes: &[u8]) {
    let o = d.rd(|o| o.clone());
    o.wr(|v| v.extend(bytes.iter().map(|&b| b as i8)));
}
pub fn dos_write_byte(d: &DataOutputStream, x: i32) {
    dos_put(d, &[x as u8]);
}
pub fn dos_write_short(d: &DataOutputStream, x: i32) {
    dos_put(d, &(x as i16).to_be_bytes());
}
pub fn dos_write_int(d: &DataOutputStream, x: i32) {
    dos_put(d, &x.to_be_bytes());
}
pub fn dos_write_utf(d: &DataOutputStream, s: &JStr) {
    let mut o = j2me_jvm::DataOutputStream::new();
    if o.write_utf(s.units()).is_err() {
        throw_new("java.io.UTFDataFormatException");
    }
    dos_put(d, o.as_bytes());
}
/// Key names as midp-rt reports them for the standard keys.
pub fn canvas_key_name(k: i32) -> JStr {
    let n = match k {
        48..=57 => ((k as u8) as char).to_string(),
        42 => "*".into(),
        35 => "#".into(),
        -1 => "UP".into(),
        -2 => "DOWN".into(),
        -3 => "LEFT".into(),
        -4 => "RIGHT".into(),
        -5 => "SELECT".into(),
        _ => format!("{k}"),
    };
    JStr::lit(&n)
}
pub type Control = JRef<i32>;
pub fn player_get_control(_p: &Player, _name: &JStr) -> Control {
    JRef::new(100)
}
pub fn volume_set_level(c: &Control, l: i32) -> i32 {
    let l = l.clamp(0, 100);
    c.wr(|v| *v = l);
    // midp-rt does not trace VolumeControl
    l
}
