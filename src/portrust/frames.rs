//! Pixel comparison of two frame directories: midp-rt's `--frames-dir` capture of
//! the original and the `--frames-dir` output of a port-rust crate (both write
//! `frame_NNNN.png`, frame i = the screen after tick/paint i).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An RGB24 frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Reads a PNG as RGB24 (alpha dropped, palette expanded).
pub fn read_png(p: &Path) -> Result<Frame, String> {
    let f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| format!("{}: {e}", p.display()))?;
    let mut buf = vec![0u8; r.output_buffer_size().ok_or_else(|| format!("{}: image too large", p.display()))?];
    let info = r.next_frame(&mut buf).map_err(|e| format!("{}: {e}", p.display()))?;
    let data = &buf[..info.buffer_size()];
    let n = (info.width * info.height) as usize;
    let rgb = match info.color_type {
        png::ColorType::Rgb => data.to_vec(),
        png::ColorType::Rgba => (0..n).flat_map(|i| data[i * 4..i * 4 + 3].to_vec()).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => (0..n).flat_map(|i| [data[i * 2]; 3]).collect(),
        png::ColorType::Indexed => return Err(format!("{}: palette left after expansion", p.display())),
    };
    Ok(Frame { width: info.width, height: info.height, rgb })
}

/// `frame_NNNN.png` files of a directory by index.
pub fn list_frames(dir: &Path) -> Result<BTreeMap<i64, PathBuf>, String> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix("frame_").and_then(|r| r.strip_suffix(".png")).and_then(|n| n.parse::<i64>().ok()) {
            out.insert(n, e.path());
        }
    }
    Ok(out)
}

/// One compared pair.
#[derive(Clone, Debug, Default)]
pub struct FrameResult {
    pub index: i64,
    /// pixels whose RGB differs
    pub differing: u64,
    /// bounding box of the differing pixels `(x0, y0, x1, y1)`, inclusive
    pub bbox: Option<(u32, u32, u32, u32)>,
    /// the two frames have different sizes (every pixel counts as differing)
    pub size_mismatch: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FrameDiff {
    pub reference_frames: usize,
    pub port_frames: usize,
    /// reference frames with no port frame (or the other way round)
    pub unmatched: usize,
    pub frames: Vec<FrameResult>,
    pub pixels_total: u64,
}

impl FrameDiff {
    pub fn identical(&self) -> usize {
        self.frames.iter().filter(|f| f.differing == 0).count()
    }
    pub fn pixels_differing(&self) -> u64 {
        self.frames.iter().map(|f| f.differing).sum()
    }
    /// Share of identical frames among the compared ones, in percent.
    pub fn frame_match_percent(&self) -> f64 {
        if self.frames.is_empty() {
            return 0.0;
        }
        100.0 * self.identical() as f64 / self.frames.len() as f64
    }
    /// Share of identical pixels over all compared frames, in percent.
    pub fn pixel_match_percent(&self) -> f64 {
        if self.pixels_total == 0 {
            return 0.0;
        }
        100.0 * (self.pixels_total - self.pixels_differing()) as f64 / self.pixels_total as f64
    }
}

/// Compares two frames pixel by pixel.
pub fn compare_frames(index: i64, a: &Frame, b: &Frame) -> FrameResult {
    if a.width != b.width || a.height != b.height {
        return FrameResult { index, differing: (a.width as u64 * a.height as u64).max(b.width as u64 * b.height as u64), bbox: None, size_mismatch: true };
    }
    let mut r = FrameResult { index, ..Default::default() };
    for (i, (pa, pb)) in a.rgb.chunks_exact(3).zip(b.rgb.chunks_exact(3)).enumerate() {
        if pa != pb {
            r.differing += 1;
            let (x, y) = ((i as u32) % a.width, (i as u32) / a.width);
            r.bbox = Some(match r.bbox {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }
    r
}

/// Writes a diff picture: the reference dimmed, differing pixels in red.
fn write_diff(path: &Path, a: &Frame, b: &Frame) -> Result<(), String> {
    let mut out = Vec::with_capacity(a.rgb.len());
    for (pa, pb) in a.rgb.chunks_exact(3).zip(b.rgb.chunks_exact(3)) {
        if pa != pb {
            out.extend_from_slice(&[255, 0, 0]);
        } else {
            out.extend(pa.iter().map(|c| c / 3));
        }
    }
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), a.width, a.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&out).map_err(|e| e.to_string())
}

/// Compares reference frame `i` with port frame `i + offset`, for every `i` both have.
pub fn compare_dirs(reference: &Path, port: &Path, offset: i64, diff_dir: Option<&Path>) -> Result<FrameDiff, String> {
    let rf = list_frames(reference)?;
    let pf = list_frames(port)?;
    let mut d = FrameDiff { reference_frames: rf.len(), port_frames: pf.len(), ..Default::default() };
    if let Some(dd) = diff_dir {
        std::fs::create_dir_all(dd).map_err(|e| format!("{}: {e}", dd.display()))?;
    }
    for (i, rp) in &rf {
        let Some(pp) = pf.get(&(i + offset)) else {
            d.unmatched += 1;
            continue;
        };
        let (a, b) = (read_png(rp)?, read_png(pp)?);
        let r = compare_frames(*i, &a, &b);
        d.pixels_total += a.width as u64 * a.height as u64;
        if r.differing != 0 && !r.size_mismatch {
            if let Some(dd) = diff_dir {
                write_diff(&dd.join(format!("diff_{i:04}.png")), &a, &b)?;
            }
        }
        d.frames.push(r);
    }
    d.unmatched += pf.keys().filter(|k| !rf.contains_key(&(*k - offset))).count();
    Ok(d)
}

#[derive(clap::Args, Debug, Clone)]
pub struct FrameDiffArgs {
    /// frames midp-rt captured for the original (`Headless --frames-dir`)
    pub reference: String,
    /// frames the translated crate wrote (`trace … --frames-dir`)
    pub port: String,
    /// compare reference frame i with port frame i+OFFSET
    #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
    pub offset: i64,
    /// write diff_NNNN.png (differing pixels in red) for every differing frame
    #[arg(long = "diff-dir")]
    pub diff_dir: Option<String>,
    /// how many differing frames to list
    #[arg(long, default_value_t = 10)]
    pub show: usize,
}

pub fn cmd_frame_diff(a: &FrameDiffArgs) -> i32 {
    let ex = |p: &str| crate::util::expanduser(p);
    let dd = a.diff_dir.as_deref().map(ex);
    let d = match compare_dirs(&ex(&a.reference), &ex(&a.port), a.offset, dd.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            println!("frame-diff: {e}");
            return 2;
        }
    };
    println!(
        "frame-diff: reference {} frames, port {}, compared {}, unmatched {}",
        d.reference_frames,
        d.port_frames,
        d.frames.len(),
        d.unmatched
    );
    println!(
        "frame-diff: identical frames {} of {} ({:.2} %), identical pixels {:.4} % ({} of {} differ)",
        d.identical(),
        d.frames.len(),
        d.frame_match_percent(),
        d.pixel_match_percent(),
        d.pixels_differing(),
        d.pixels_total
    );
    let mut worst: Vec<&FrameResult> = d.frames.iter().filter(|f| f.differing != 0).collect();
    if let Some(first) = worst.first() {
        println!("frame-diff: first differing frame {}", first.index);
    }
    worst.sort_by(|x, y| y.differing.cmp(&x.differing).then(x.index.cmp(&y.index)));
    for f in worst.iter().take(a.show) {
        match (f.size_mismatch, f.bbox) {
            (true, _) => println!("   frame {}: size differs", f.index),
            (false, Some((x0, y0, x1, y1))) => println!("   frame {}: {} px differ in [{x0},{y0}]-[{x1},{y1}]", f.index, f.differing),
            _ => {}
        }
    }
    i32::from(d.identical() != d.frames.len() || d.unmatched != 0 || d.frames.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_pixels_and_box() {
        let a = Frame { width: 3, height: 2, rgb: vec![0; 18] };
        let mut b = a.clone();
        b.rgb[3 * 4] = 1; // (1,1)
        b.rgb[3 * 2 + 2] = 9; // (2,0)
        let r = compare_frames(0, &a, &b);
        assert_eq!(r.differing, 2);
        assert_eq!(r.bbox, Some((1, 0, 2, 1)));
        let same = compare_frames(1, &a, &a);
        assert_eq!(same.differing, 0);
    }
}
