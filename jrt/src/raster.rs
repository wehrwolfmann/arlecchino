//! A software renderer for [`crate::gfx::Backend`]: the screen and every image are
//! ARGB pixel buffers, and every drawing call is rasterised the way midp-rt (the
//! reference runtime, AWT underneath) puts it on its `TYPE_INT_RGB` screen, so a
//! frame of a translated game can be compared with a frame of the original pixel
//! by pixel.
//!
//! What is exact and what is not (measured against midp-rt, see
//! `arlecchino frame-diff`):
//!
//! * exact: `fillRect`, `drawRect` (four lines, like midp-rt), horizontal and
//!   vertical `drawLine`, `drawImage` and `drawRegion` (all eight transforms) of
//!   PNG and mutable images, with clip, translation and alpha blending
//!   (Java2D's 8-bit multiplication table);
//! * approximate: slanted `drawLine` (plain Bresenham), `fillTriangle`
//!   (pixel-centre rule);
//! * not drawn, only counted ([`unsupported`]): arcs, round rectangles and text —
//!   AWT rasterises them with its own curve and font engines.
//!
//! The screen starts black (midp-rt's surface fill), mutable images start fully
//! transparent (midp-rt's `createImage(w, h)`; the MIDP specification says white).

use crate::gfx::{Backend, State, Target};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// A pixel buffer, `0xAARRGGBB` per pixel, row by row.
#[derive(Clone, Debug, Default)]
pub struct Pixels {
    pub width: i32,
    pub height: i32,
    pub argb: Vec<u32>,
}

impl Pixels {
    pub fn new(width: i32, height: i32, fill: u32) -> Self {
        let n = (width.max(0) as usize) * (height.max(0) as usize);
        Pixels { width, height, argb: vec![fill; n] }
    }
    /// The buffer as RGB24 bytes (alpha dropped, like a `TYPE_INT_RGB` copy).
    pub fn rgb(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.argb.len() * 3);
        for p in &self.argb {
            o.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        o
    }
}

/// Java2D's `mul8table[a][b]`: `a * b / 255`, rounded the way AWT rounds.
fn mul8(a: u32, b: u32) -> u32 {
    if a == 0 || b == 0 {
        return 0;
    }
    (a * 0x010101 * b + (1 << 23)) >> 24
}

/// Source-over of a non-premultiplied ARGB pixel on an opaque or translucent one.
fn blend(dst: u32, src: u32) -> u32 {
    let sa = src >> 24;
    if sa == 0 {
        return dst;
    }
    if sa == 0xff {
        return src;
    }
    let df = 0xff - sa;
    let da = mul8(df, dst >> 24);
    let ra = sa + da;
    let ch = |shift: u32| {
        let s = (src >> shift) & 0xff;
        let d = (dst >> shift) & 0xff;
        let mut r = mul8(sa, s) + mul8(da, d);
        if ra != 0 && ra < 0xff {
            // back to non-premultiplied (Java2D's div8table)
            r = ((r * 0xff + ra / 2) / ra).min(0xff);
        }
        r
    };
    (ra << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Decodes a PNG the way an AWT toolkit image comes out of it: 8 bits per
/// channel, palette and `tRNS` expanded into ARGB.
pub fn decode_png(bytes: &[u8]) -> Option<Pixels> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as i32, info.height as i32);
    let data = &buf[..info.buffer_size()];
    let mut px = Pixels::new(w, h, 0);
    let n = (w * h) as usize;
    match info.color_type {
        png::ColorType::Rgba => (0..n).for_each(|i| {
            let p = &data[i * 4..i * 4 + 4];
            px.argb[i] = u32::from_be_bytes([p[3], p[0], p[1], p[2]]);
        }),
        png::ColorType::Rgb => (0..n).for_each(|i| {
            let p = &data[i * 3..i * 3 + 3];
            px.argb[i] = u32::from_be_bytes([0xff, p[0], p[1], p[2]]);
        }),
        png::ColorType::GrayscaleAlpha => (0..n).for_each(|i| {
            let (g, a) = (data[i * 2], data[i * 2 + 1]);
            px.argb[i] = u32::from_be_bytes([a, g, g, g]);
        }),
        png::ColorType::Grayscale => (0..n).for_each(|i| {
            let g = data[i];
            px.argb[i] = u32::from_be_bytes([0xff, g, g, g]);
        }),
        png::ColorType::Indexed => return None, // EXPAND never leaves a palette
    }
    Some(px)
}

/// Clip rectangle in device coordinates, already cut to the buffer.
fn clip_box(px: &Pixels, st: &State) -> (i32, i32, i32, i32) {
    let (cx, cy, cw, ch) = st.clip;
    let x0 = cx.max(0);
    let y0 = cy.max(0);
    let x1 = cx.saturating_add(cw.max(0)).min(px.width);
    let y1 = cy.saturating_add(ch.max(0)).min(px.height);
    (x0, y0, x1, y1)
}

fn opaque(st: &State) -> u32 {
    0xff00_0000 | (st.color as u32 & 0xff_ffff)
}

fn fill(px: &mut Pixels, st: &State, x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let (x0, y0, x1, y1) = clip_box(px, st);
    let (tx, ty) = st.translate;
    let ax = x.saturating_add(tx).max(x0);
    let ay = y.saturating_add(ty).max(y0);
    let bx = x.saturating_add(tx).saturating_add(w).min(x1);
    let by = y.saturating_add(ty).saturating_add(h).min(y1);
    let c = opaque(st);
    for yy in ay..by {
        let row = (yy * px.width) as usize;
        px.argb[row + ax as usize..row + bx.max(ax) as usize].fill(c);
    }
}

fn plot(px: &mut Pixels, clip: (i32, i32, i32, i32), x: i32, y: i32, c: u32) {
    let (x0, y0, x1, y1) = clip;
    if x >= x0 && x < x1 && y >= y0 && y < y1 {
        px.argb[(y * px.width + x) as usize] = c;
    }
}

fn line(px: &mut Pixels, st: &State, x1: i32, y1: i32, x2: i32, y2: i32) {
    let (tx, ty) = st.translate;
    let (mut x, mut y, ex, ey) = (x1 + tx, y1 + ty, x2 + tx, y2 + ty);
    let clip = clip_box(px, st);
    let c = opaque(st);
    let dx = (ex - x).abs();
    let dy = -(ey - y).abs();
    let sx = if x < ex { 1 } else { -1 };
    let sy = if y < ey { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        plot(px, clip, x, y, c);
        if x == ex && y == ey {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn triangle(px: &mut Pixels, st: &State, p: [(i32, i32); 3]) {
    let (tx, ty) = st.translate;
    let p: Vec<(f64, f64)> = p.iter().map(|&(x, y)| ((x + tx) as f64, (y + ty) as f64)).collect();
    let (x0, y0, x1, y1) = clip_box(px, st);
    let c = opaque(st);
    let min_y = p.iter().map(|q| q.1).fold(f64::MAX, f64::min).floor().max(y0 as f64) as i32;
    let max_y = p.iter().map(|q| q.1).fold(f64::MIN, f64::max).ceil().min(y1 as f64) as i32;
    for yy in min_y..max_y {
        let cy = yy as f64 + 0.5;
        let mut xs = vec![];
        for i in 0..3 {
            let (a, b) = (p[i], p[(i + 1) % 3]);
            if (a.1 <= cy && b.1 > cy) || (b.1 <= cy && a.1 > cy) {
                xs.push(a.0 + (cy - a.1) * (b.0 - a.0) / (b.1 - a.1));
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let from = ((xs[0] - 0.5).ceil() as i32).max(x0);
        let to = ((xs[xs.len() - 1] - 0.5).ceil() as i32).min(x1);
        for xx in from..to {
            px.argb[(yy * px.width + xx) as usize] = c;
        }
    }
}

/// Where source pixel `(u, v)` of the destination box comes from, for a MIDP
/// `Sprite` transform of a `w`×`h` region (the same affine maps midp-rt builds).
fn source_of(transform: i32, w: i32, h: i32, u: i32, v: i32) -> (i32, i32) {
    match transform {
        0 => (u, v),                 // TRANS_NONE
        5 => (v, h - 1 - u),         // TRANS_ROT90
        3 => (w - 1 - u, h - 1 - v), // TRANS_ROT180
        6 => (w - 1 - v, u),         // TRANS_ROT270
        2 => (w - 1 - u, v),         // TRANS_MIRROR
        7 => (w - 1 - v, h - 1 - u), // TRANS_MIRROR_ROT90
        1 => (u, h - 1 - v),         // TRANS_MIRROR_ROT180
        4 => (v, u),                 // TRANS_MIRROR_ROT270
        _ => (u, v),
    }
}

/// Copies a `w`×`h` region of `src` to device position `(dx, dy)`.
#[allow(clippy::too_many_arguments)]
fn blit(dst: &mut Pixels, st: &State, src: &Pixels, sx: i32, sy: i32, w: i32, h: i32, transform: i32, dx: i32, dy: i32) {
    let swap = matches!(transform, 4..=7);
    let (dw, dh) = if swap { (h, w) } else { (w, h) };
    let (x0, y0, x1, y1) = clip_box(dst, st);
    let (tx, ty) = st.translate;
    let (ox, oy) = (dx + tx, dy + ty);
    for v in (y0 - oy).max(0)..dh.min(y1 - oy) {
        for u in (x0 - ox).max(0)..dw.min(x1 - ox) {
            let (su, sv) = source_of(transform, w, h, u, v);
            let (qx, qy) = (sx + su, sy + sv);
            if qx < 0 || qy < 0 || qx >= src.width || qy >= src.height {
                continue;
            }
            let s = src.argb[(qy * src.width + qx) as usize];
            let i = ((oy + v) * dst.width + ox + u) as usize;
            dst.argb[i] = blend(dst.argb[i], s);
        }
    }
}

/// The renderer.  Install it with [`install`]; it then owns the screen of the run.
#[derive(Default)]
pub struct Raster {
    screen: Pixels,
    images: HashMap<i32, Pixels>,
    frames_dir: Option<PathBuf>,
    frames: i32,
    unsupported: BTreeMap<&'static str, usize>,
}

thread_local! {
    static LAST: RefCell<Option<(Pixels, BTreeMap<&'static str, usize>, i32)>> = const { RefCell::new(None) };
}

impl Raster {
    /// A renderer that writes `frame_NNNN.png` (the screen after each paint) into
    /// `frames_dir` when given.
    pub fn new(frames_dir: Option<PathBuf>) -> Self {
        if let Some(d) = &frames_dir {
            let _ = std::fs::create_dir_all(d);
        }
        Raster { frames_dir, ..Default::default() }
    }

    fn target(&mut self, to: Target) -> Option<&mut Pixels> {
        match to {
            Target::Screen => Some(&mut self.screen),
            Target::Image(id) => self.images.get_mut(&id),
        }
    }

    fn skip(&mut self, what: &'static str) {
        *self.unsupported.entry(what).or_default() += 1;
    }

    fn publish(&self) {
        LAST.with(|l| *l.borrow_mut() = Some((self.screen.clone(), self.unsupported.clone(), self.frames)));
    }
}

/// Installs a [`Raster`] for this thread.
pub fn install(frames_dir: Option<PathBuf>) {
    crate::gfx::install(Box::new(Raster::new(frames_dir)));
}

/// The screen after the last finished frame of this thread's renderer.
pub fn last_screen() -> Option<Pixels> {
    LAST.with(|l| l.borrow().as_ref().map(|x| x.0.clone()))
}

/// Drawing calls the renderer skipped (name → count) and how many frames it drew.
pub fn unsupported() -> (BTreeMap<&'static str, usize>, i32) {
    LAST.with(|l| l.borrow().as_ref().map(|x| (x.1.clone(), x.2)).unwrap_or_default())
}

/// Writes RGB24 bytes as a PNG.
pub fn write_png(path: &std::path::Path, w: i32, h: i32, rgb: &[u8]) -> std::io::Result<()> {
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(f, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(rgb).map_err(std::io::Error::other)?;
    Ok(())
}

impl Backend for Raster {
    fn begin_frame(&mut self, _frame: i32, width: i32, height: i32) {
        if self.screen.width != width || self.screen.height != height {
            self.screen = Pixels::new(width, height, 0xff00_0000);
        }
    }
    fn end_frame(&mut self, frame: i32) {
        self.frames = frame + 1;
        if let Some(d) = &self.frames_dir {
            let p = d.join(format!("frame_{frame:04}.png"));
            if let Err(e) = write_png(&p, self.screen.width, self.screen.height, &self.screen.rgb()) {
                eprintln!("raster: {}: {e}", p.display());
            }
        }
        self.publish();
    }
    fn create_image(&mut self, id: i32, width: i32, height: i32, png: Option<&[u8]>) {
        let px = match png {
            Some(b) => decode_png(b).unwrap_or_else(|| Pixels::new(width, height, 0)),
            None => Pixels::new(width, height, 0),
        };
        self.images.insert(id, px);
    }
    fn fill_rect(&mut self, to: Target, st: &State, x: i32, y: i32, w: i32, h: i32) {
        if let Some(px) = self.target(to) {
            fill(px, st, x, y, w, h)
        }
    }
    fn draw_rect(&mut self, to: Target, st: &State, x: i32, y: i32, w: i32, h: i32) {
        // midp-rt draws a rectangle as four lines
        if let Some(px) = self.target(to) {
            line(px, st, x, y, x + w, y);
            line(px, st, x + w, y, x + w, y + h);
            line(px, st, x + w, y + h, x, y + h);
            line(px, st, x, y + h, x, y);
        }
    }
    fn draw_line(&mut self, to: Target, st: &State, x1: i32, y1: i32, x2: i32, y2: i32) {
        if let Some(px) = self.target(to) {
            line(px, st, x1, y1, x2, y2)
        }
    }
    fn fill_triangle(&mut self, to: Target, st: &State, x1: i32, y1: i32, x2: i32, y2: i32, x3: i32, y3: i32) {
        if let Some(px) = self.target(to) {
            triangle(px, st, [(x1, y1), (x2, y2), (x3, y3)])
        }
    }
    fn fill_round_rect(&mut self, _: Target, _: &State, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32) {
        self.skip("fillRoundRect")
    }
    fn draw_round_rect(&mut self, _: Target, _: &State, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32) {
        self.skip("drawRoundRect")
    }
    fn fill_arc(&mut self, _: Target, _: &State, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32) {
        self.skip("fillArc")
    }
    fn draw_arc(&mut self, _: Target, _: &State, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32) {
        self.skip("drawArc")
    }
    fn draw_string(&mut self, _: Target, _: &State, _: &str, _: i32, _: i32, _: i32) {
        self.skip("drawString")
    }
    fn draw_image(&mut self, to: Target, st: &State, image: i32, x: i32, y: i32, anchor: i32) {
        let Some(src) = self.images.get(&image).cloned() else { return };
        let anchor = if anchor == 0 { 0x10 | 0x04 } else { anchor };
        // midp-rt's drawImage anchor arithmetic (RIGHT/HCENTER, BOTTOM/VCENTER)
        let mut nx = x;
        let mut ny = y;
        if (anchor & 0x08) != 0 {
            nx -= src.width;
        } else if (anchor & 0x01) != 0 {
            nx -= src.width / 2;
        }
        if (anchor & 0x20) != 0 {
            ny -= src.height;
        } else if (anchor & 0x02) != 0 {
            ny -= src.height / 2;
        }
        if let Some(dst) = self.target(to) {
            blit(dst, st, &src, 0, 0, src.width, src.height, 0, nx, ny)
        }
    }
    fn draw_region(&mut self, to: Target, st: &State, image: i32, sx: i32, sy: i32, w: i32, h: i32, transform: i32, x: i32, y: i32, anchor: i32) {
        let Some(src) = self.images.get(&image).cloned() else { return };
        let (dw, dh) = if matches!(transform, 4..=7) { (h, w) } else { (w, h) };
        let anchor = if anchor == 0 { 0x10 | 0x04 } else { anchor };
        // midp-rt's drawRegion anchor arithmetic: note the `- 1`
        let mut nx = x;
        let mut ny = y;
        if (anchor & 0x20) != 0 {
            ny -= dh - 1;
        } else if (anchor & 0x02) != 0 && (anchor & 0x10) == 0 {
            ny -= ((dh - 1) as u32 >> 1) as i32;
        }
        if (anchor & 0x08) != 0 {
            nx -= dw - 1;
        } else if (anchor & 0x01) != 0 && (anchor & 0x04) == 0 {
            nx -= ((dw - 1) as u32 >> 1) as i32;
        }
        if let Some(dst) = self.target(to) {
            blit(dst, st, &src, sx, sy, w, h, transform, nx, ny)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(clip: (i32, i32, i32, i32), translate: (i32, i32), color: i32) -> State {
        State { color, clip, translate, font: (0, 0, 0) }
    }

    #[test]
    fn mul8_matches_the_awt_table_corners() {
        assert_eq!(mul8(255, 255), 255);
        assert_eq!(mul8(255, 7), 7);
        assert_eq!(mul8(128, 255), 128);
        assert_eq!(mul8(128, 128), 64);
        assert_eq!(mul8(1, 1), 0);
    }

    #[test]
    fn fill_is_cut_by_clip_and_translation() {
        let mut p = Pixels::new(8, 8, 0xff00_0000);
        fill(&mut p, &st((2, 2, 3, 3), (1, 1), 0xffffff), 0, 0, 10, 10);
        let set: Vec<usize> = (0..64).filter(|&i| p.argb[i] == 0xffff_ffff).collect();
        assert_eq!(set, vec![18, 19, 20, 26, 27, 28, 34, 35, 36]);
    }

    #[test]
    fn region_transforms_put_the_corner_where_midp_says() {
        // 2x1 source: left pixel red, right pixel green
        let src = Pixels { width: 2, height: 1, argb: vec![0xffff_0000, 0xff00_ff00] };
        let cases = [(0, vec![0xffff_0000, 0xff00_ff00]), (2, vec![0xff00_ff00, 0xffff_0000]), (5, vec![0xffff_0000, 0xff00_ff00]), (6, vec![0xff00_ff00, 0xffff_0000])];
        for (t, want) in cases {
            let mut d = Pixels::new(2, 2, 0xff00_0000);
            blit(&mut d, &st((0, 0, 2, 2), (0, 0), 0), &src, 0, 0, 2, 1, t, 0, 0);
            let got: Vec<u32> = d.argb.iter().copied().filter(|&p| p != 0xff00_0000).collect();
            assert_eq!(got, want, "transform {t}");
        }
    }

    #[test]
    fn half_alpha_blends_like_java2d() {
        assert_eq!(blend(0xff00_0000, 0x80ff_ffff), 0xff80_8080);
        assert_eq!(blend(0xff12_3456, 0x0000_0000), 0xff12_3456);
    }
}
