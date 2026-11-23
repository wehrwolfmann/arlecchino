//! Headless MIDlet runs with frame capture (PLAN_runtime.md, block A, stage A2).
//!
//! Port of `arlecchino/diffrun.py`. We drive the libretro frontend of FreeJ2ME-plus
//! (`freej2me-lr.jar`, installed by `arlecchino setup`). Despite the name, there is
//! no native libretro core, no SDL and no X server here: the Main-Class of that jar
//! (`org.recompile.freej2me.Libretro`) talks over a plain stdin/stdout channel,
//! and we speak it directly. The emulator draws into an internal buffer and, on
//! request, writes raw RGB24 frames to stdout — a true headless capture.
//!
//! Channel protocol (clean-room notes on the behaviour of freej2me-plus 1.52):
//!
//!   child argv: 26 positional settings (width height rotate phone fps sound ...).
//!   stdin:  5-byte commands [op, b1, b2, b3, b4], code = b1<<24|b2<<16|b3<<8|b4
//!           op 11 = RMS/save directory (code = path length, path bytes follow)
//!           op 10 = load and start a jar (same length-prefix encoding)
//!           op 15 = frame request [15, mult_hi, mult_lo, processed_flag, ff_flag];
//!                   processed_flag=0 means "give me a fresh frame right now"
//!           op  3/2 = joypad down/up (code = key index) — stage A3
//!   stdout: b"+READY\n" once after start, then for each request a 16-byte
//!           header (magic 0xFE, w:2, h:2, rotation:1, vibration:8, restart:1,
//!           encoding:1) followed by w*h*3 bytes of RGB24 (big-endian).
//!
//! The MIDlet itself runs freely (live threads, live clock) — frames are taken
//! when we ask for them. This is the non-deterministic MVP mode documented in the
//! plan; determinism is stage A7 (`fixedclock`).
//!
//! Result of a run: `frame_NNNN.png` plus `frames.json` (frame index -> file,
//! sha256, width, height).

use crate::emulate::find_freej2me_lr;
use crate::util;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const FRAME_MAGIC: u8 = 0xFE;
/// Default screen size: 240x320 for most of the regression set.
pub const DEFAULT_W: i64 = 240;
pub const DEFAULT_H: i64 = 320;
const READY_TIMEOUT_S: f64 = 30.0;
/// tolerate up to 1 MiB of stray MIDlet output on stdout
const MAX_SKIP_BEFORE_MAGIC: u64 = 1 << 20;

/// `MobilePlatform.load()` checks the JVM `file.encoding` against the encoding the
/// MIDlet needs (`ISO_8859_1` by default) and asks for a restart on a mismatch — on
/// Linux with a UTF-8 locale this fires on EVERY jar unless java is started with
/// `-Dfile.encoding=<the needed one>`. The codes come as a byte in the frame header.
fn encoding_name(code: u8) -> &'static str {
    match code {
        1 => "Shift_JIS",
        2 => "EUC_KR",
        _ => "ISO_8859_1",
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct FrameRecord {
    pub index: i64,
    pub file: String,
    pub sha256: String,
    pub width: i64,
    pub height: i64,
    pub blank: bool,
}

#[derive(Debug, Clone, Default)]
pub struct HeadlessResult {
    pub frames: Vec<FrameRecord>,
    pub stderr_log: String,
    pub rms_dir: String,
}

impl HeadlessResult {
    pub fn hashes(&self) -> Vec<String> {
        self.frames.iter().map(|f| f.sha256.clone()).collect()
    }
    pub fn any_nonblank(&self) -> bool {
        self.frames.iter().any(|f| !f.blank)
    }
}

/// Headless run parameters (shared by both engines).
#[derive(Debug, Clone)]
pub struct RunOpts {
    pub max_frames: i64,
    pub frame_period_ms: i64,
    pub boot_wait_s: f64,
    pub width: i64,
    pub height: i64,
    pub rms_dir: Option<PathBuf>,
    /// A7: drive the capture loop by frame COUNT, without sleeping on the wall clock.
    pub deterministic_capture: bool,
}

impl Default for RunOpts {
    fn default() -> Self {
        RunOpts {
            max_frames: 30,
            frame_period_ms: 100,
            boot_wait_s: 2.0,
            width: DEFAULT_W,
            height: DEFAULT_H,
            rms_dir: None,
            deterministic_capture: false,
        }
    }
}

/// One input event: {"t_ms": 2000, "key": "NUM5", "action": "press"|"release"|"tap"}.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InputEvent {
    #[serde(default)]
    pub t_ms: i64,
    pub key: serde_json::Value,
    #[serde(default = "default_action")]
    pub action: String,
}

fn default_action() -> String {
    "press".into()
}

/// libretro joypad indices that the frontend maps to MIDP codes via
/// `Mobile.getMobileKey` (the plain phone branch, freej2me-plus 1.52):
/// 0..3 = D-pad, 7 = middle soft key, 8/9 = right/left soft key,
/// 18 = NUM5 (the classic keypad "fire").
pub fn libretro_key(name: &str) -> Option<i64> {
    Some(match name {
        "UP" => 0,
        "DOWN" => 1,
        "LEFT" => 2,
        "RIGHT" => 3,
        "NUM9" => 4,
        "NUM7" => 5,
        "NUM0" => 6,
        "FIRE" | "SOFT3" => 7,
        "SOFT_RIGHT" => 8,
        "SOFT_LEFT" => 9,
        "NUM1" => 10,
        "NUM3" => 11,
        "STAR" => 12,
        "POUND" => 13,
        "NUM2" => 14,
        "NUM4" => 15,
        "NUM6" => 16,
        "NUM8" => 17,
        "NUM5" => 18,
        "CLR" => 19,
        _ => return None,
    })
}

/// The 26 positional settings the libretro frontend expects (1.52 layout):
/// width height rotate phone fps sound custom_midi dump_audio loglevel
/// noalpha_hack backlight_mask fantasyzone_fix translate_origin custom_font
/// font_offset dump_gfx delete_kjx m3g_untextured m3g_wireframe fps_hack
/// immediate_repaints override_platform siemens_drawing halfres_m3g doja
/// ignore_volume. Everything off/neutral; sound OFF (headless mode), fps 60.
pub fn child_args(width: i64, height: i64) -> Vec<String> {
    let mut v = vec![width.to_string(), height.to_string()];
    for s in [
        "0", "0", "60", "0", "0", "0", "4", "0", "0", "0", "0", "0", "0", "0", "1", "0", "0", "0",
        "0", "0", "0", "0", "0", "0",
    ] {
        v.push(s.to_string());
    }
    v
}

fn read_exact(stream: &mut impl Read, n: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; n];
    let mut got = 0usize;
    while got < n {
        match stream.read(&mut buf[got..]) {
            Ok(0) => return Err(format!("emulator closed stdout ({got}/{n} bytes read)")),
            Ok(k) => got += k,
            Err(e) => return Err(format!("emulator stdout read error: {e}")),
        }
    }
    Ok(buf)
}

/// A 5-byte channel command: `[op, b1, b2, b3, b4]`, code = b1<<24|b2<<16|b3<<8|b4.
pub fn encode_cmd(op: u8, code: i64) -> [u8; 5] {
    let c = code as u32;
    [
        op,
        ((c >> 24) & 0xFF) as u8,
        ((c >> 16) & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        (c & 0xFF) as u8,
    ]
}

/// A command with a path: length prefix, followed by the path bytes themselves.
pub fn encode_path_cmd(op: u8, path: &str) -> Vec<u8> {
    let raw = path.as_bytes();
    let mut v = encode_cmd(op, raw.len() as i64).to_vec();
    v.extend_from_slice(raw);
    v
}

/// The bytes sent to the emulator for one input event (A3).
pub fn encode_input(ev: &InputEvent) -> Result<Vec<u8>, String> {
    let code = match &ev.key {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => libretro_key(&s.to_uppercase()),
        other => libretro_key(&other.to_string().to_uppercase()),
    };
    let code = match code {
        Some(c) => c,
        None => return Err(format!("unknown key in input script: {:?}", ev.key)),
    };
    let mut v = Vec::new();
    if ev.action == "press" || ev.action == "tap" {
        v.extend_from_slice(&encode_cmd(3, code)); // joypad: key pressed
    }
    if ev.action == "release" || ev.action == "tap" {
        v.extend_from_slice(&encode_cmd(2, code)); // joypad: key released
    }
    Ok(v)
}

fn send(stdin: &mut impl Write, op: u8, code: i64) -> Result<(), String> {
    stdin.write_all(&encode_cmd(op, code)).map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())
}

fn send_path(stdin: &mut impl Write, op: u8, path: &str) -> Result<(), String> {
    stdin.write_all(&encode_path_cmd(op, path)).map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// PNG frame `frame_NNNN.png` (in Python — via Pillow).
fn save_frame(out_dir: &Path, index: i64, w: i64, h: i64, rgb: &[u8]) -> String {
    let name = format!("frame_{index:04}.png");
    let file = std::fs::File::create(out_dir.join(&name));
    if let Ok(f) = file {
        let bw = std::io::BufWriter::new(f);
        let mut enc = png::Encoder::new(bw, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut wr) = enc.write_header() {
            let _ = wr.write_image_data(rgb);
        }
    }
    name
}

enum FrameOut {
    Frame(i64, i64, Vec<u8>),
    /// the emulator asks for a restart with a different `file.encoding`
    Restart(&'static str),
}

fn read_frame(stdout: &mut impl Read, lcd_w: i64, lcd_h: i64) -> Result<FrameOut, String> {
    let mut skipped: u64 = 0;
    loop {
        // look for the 0xFE magic — skip stray MIDlet output
        if read_exact(stdout, 1)?[0] == FRAME_MAGIC {
            break;
        }
        skipped += 1;
        if skipped > MAX_SKIP_BEFORE_MAGIC {
            return Err("no 0xFE frame marker within 1MiB of stdout".into());
        }
    }
    // 15 bytes after the magic: w:2 h:2 rotation:1 vib_dur:4 vib_str:4 restart:1 encoding:1
    let hdr = read_exact(stdout, 15)?;
    let w = ((hdr[0] as i64) << 8) | hdr[1] as i64;
    let h = ((hdr[2] as i64) << 8) | hdr[3] as i64;
    if hdr[13] != 0 {
        // a restart is requested — read the rest of the payload and bubble up
        read_exact(stdout, (lcd_w * lcd_h * 3) as usize)?;
        return Ok(FrameOut::Restart(encoding_name(hdr[14])));
    }
    if !(w > 0 && w <= 800 && h > 0 && h <= 800) {
        return Err(format!("implausible frame header {w}x{h} (desynced?)"));
    }
    let rgb = read_exact(stdout, (w * h * 3) as usize)?;
    Ok(FrameOut::Frame(w, h, rgb))
}

fn inject_input(stdin: &mut impl Write, ev: &InputEvent) -> Result<(), String> {
    let bytes = encode_input(ev)?;
    stdin.write_all(&bytes).map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())
}

/// Run `jar` headless in FreeJ2ME-plus and capture frames.
///
/// An environment error (no `freej2me-lr.jar` / no `java`) is an `Err` with a clear
/// message; the caller turns it into a skip.
pub fn run_headless(
    jar: &Path,
    out_dir: &Path,
    inputs: Option<&[InputEvent]>,
    opts: &RunOpts,
    lr_jar: Option<&Path>,
) -> Result<HeadlessResult, String> {
    let lr = match lr_jar.map(|p| p.to_path_buf()).or_else(|| find_freej2me_lr(None, None)) {
        Some(p) => p,
        None => {
            return Err("freej2me-lr.jar not found — run `arlecchino setup` \
                        (stage A1 installs the FreeJ2ME-plus libretro frontend)"
                .into())
        }
    };
    let java = util::tool("ARLECCHINO_JAVA", "java");
    if which(&java).is_none() {
        return Err(format!("java executable not found ({java:?})"));
    }
    let mut jar = util::absolutize(jar);
    if !jar.exists() {
        return Err(format!("midlet jar not found: {}", jar.display()));
    }

    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let rms = opts
        .rms_dir
        .clone()
        .unwrap_or_else(|| out_dir.join("rms"));
    std::fs::create_dir_all(&rms).map_err(|e| e.to_string())?;

    // The frontend decodes the jar path with the MIDlet's encoding (ISO_8859_1 by default) —
    // non-ASCII path components (e.g. ~/Téléchargements) get mangled and end up "not found".
    // We work around it with an ASCII symlink inside out_dir.
    if !jar.to_string_lossy().is_ascii() {
        let suffix = jar
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        let link = out_dir.join(format!("midlet{suffix}"));
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&jar, &link).map_err(|e| e.to_string())?;
        jar = link;
    }

    // The emulator itself asks for a restart when the MIDlet needs a different file.encoding
    // (a flag in the frame header) — we honour at most 2 restarts.
    let mut encoding = "ISO_8859_1";
    for _attempt in 0..3 {
        match run_once(&jar, out_dir, &lr, &java, encoding, inputs, opts, &rms)? {
            RunOnce::Done(res) => return Ok(res),
            RunOnce::Restart(enc) => encoding = enc,
        }
    }
    Err("emulator kept requesting restarts (encoding ping-pong)".into())
}

enum RunOnce {
    Done(HeadlessResult),
    Restart(&'static str),
}

#[allow(clippy::too_many_arguments)]
fn run_once(
    jar: &Path,
    out_dir: &Path,
    lr: &Path,
    java: &str,
    encoding: &str,
    inputs: Option<&[InputEvent]>,
    opts: &RunOpts,
    rms: &Path,
) -> Result<RunOnce, String> {
    let stderr_path = out_dir.join("emulator_stderr.log");
    let mut result = HeadlessResult {
        frames: Vec::new(),
        stderr_log: stderr_path.to_string_lossy().into_owned(),
        rms_dir: rms.to_string_lossy().into_owned(),
    };
    // A sorted copy of the input script; consumed as time goes on (A3).
    let mut pending: Vec<InputEvent> = inputs.map(|v| v.to_vec()).unwrap_or_default();
    pending.sort_by_key(|e| e.t_ms);
    let mut pending: std::collections::VecDeque<InputEvent> = pending.into();

    let err = std::fs::File::create(&stderr_path).map_err(|e| e.to_string())?;
    let mut proc: Child = Command::new(java)
        .arg(format!("-Dfile.encoding={encoding}"))
        .arg("-jar")
        .arg(lr)
        .args(child_args(opts.width, opts.height))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(err))
        .current_dir(out_dir)
        .spawn()
        .map_err(|e| format!("java executable not found ({java:?}): {e}"))?;

    // Load-stage watchdog: kills ONLY our own instance, by its handle.
    let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let fired = fired.clone();
        let ready = ready.clone();
        let killer = proc.id();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs_f64(READY_TIMEOUT_S);
            while Instant::now() < deadline {
                if ready.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if !ready.load(std::sync::atomic::Ordering::SeqCst) {
                fired.store(true, std::sync::atomic::Ordering::SeqCst);
                // kill only our own child process, by its pid
                unsafe { libc_kill(killer as i32) };
            }
        });
    }

    let outcome = (|| -> Result<Option<&'static str>, String> {
        let mut stdin = proc.stdin.take().ok_or("no stdin")?;
        let mut stdout = proc.stdout.take().ok_or("no stdout")?;
        let r = await_ready(&mut stdout, &fired);
        ready.store(true, std::sync::atomic::Ordering::SeqCst);
        r?;
        send_path(&mut stdin, 11, &format!("{}{}", rms.display(), std::path::MAIN_SEPARATOR))?;
        send_path(&mut stdin, 10, &jar.to_string_lossy())?;
        std::thread::sleep(Duration::from_secs_f64(opts.boot_wait_s));

        // period for mapping t_ms -> frame index in deterministic mode
        // (a neutral 100 ms if frame_period_ms == 0)
        let det_period = if opts.frame_period_ms != 0 { opts.frame_period_ms } else { 100 };
        let t0 = Instant::now();
        for i in 0..opts.max_frames {
            if opts.deterministic_capture {
                // count-based timeline: inject all events that fall into this frame's
                // virtual slot, regardless of the wall clock
                let slot_ms = (i + 1) * det_period;
                while pending.front().map(|e| e.t_ms < slot_ms).unwrap_or(false) {
                    inject_input(&mut stdin, &pending.pop_front().unwrap())?;
                }
            } else {
                let now_ms = t0.elapsed().as_secs_f64() * 1000.0;
                while pending.front().map(|e| e.t_ms as f64 <= now_ms).unwrap_or(false) {
                    inject_input(&mut stdin, &pending.pop_front().unwrap())?;
                }
            }
            send(&mut stdin, 15, 0)?; // ask for a fresh frame
            match read_frame(&mut stdout, opts.width, opts.height)? {
                FrameOut::Restart(enc) => return Ok(Some(enc)),
                FrameOut::Frame(w, h, rgb) => {
                    let name = save_frame(out_dir, i, w, h, &rgb);
                    result.frames.push(FrameRecord {
                        index: i,
                        file: name,
                        sha256: sha256_hex(&rgb),
                        width: w,
                        height: h,
                        blank: !rgb.iter().any(|b| *b != 0),
                    });
                }
            }
            if !opts.deterministic_capture {
                std::thread::sleep(Duration::from_millis(opts.frame_period_ms.max(0) as u64));
            }
        }
        Ok(None)
    })();
    let _ = proc.kill();
    let _ = proc.wait();
    let restart = outcome?;
    if let Some(enc) = restart {
        return Ok(RunOnce::Restart(enc));
    }

    let index = serde_json::json!({
        "jar": jar.to_string_lossy(),
        "width": opts.width,
        "height": opts.height,
        "frame_period_ms": opts.frame_period_ms,
        "boot_wait_s": opts.boot_wait_s,
        "rms_dir": rms.to_string_lossy(),
        "frames": result.frames,
    });
    std::fs::write(
        out_dir.join("frames.json"),
        serde_json::to_string_pretty(&index).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;
    Ok(RunOnce::Done(result))
}

/// Wait for `+READY` from the emulator.
///
/// ⭐ FIXED A BUG OF THE ORIGINAL (Python `diffrun._await_ready`): there the loop called
/// a blocking `proc.stdout.readline()`, and the `_READY_TIMEOUT_S = 30 s` deadline
/// was checked ONLY after a line had already arrived. So on a silent emulator hang
/// (exactly the case the timeout exists for) the watchdog never fired and Python
/// hung forever. Here the deadline is guarded by a separate thread: when it
/// expires it kills OUR OWN child process by its handle — the read hits EOF and
/// we return a clear error.
fn await_ready(stdout: &mut impl Read, watchdog: &std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let mut line: Vec<u8> = Vec::new();
    loop {
        let mut b = [0u8; 1];
        match stdout.read(&mut b) {
            Ok(0) => {
                return Err(if watchdog.load(Ordering::SeqCst) {
                    format!(
                        "emulator never printed +READY within {READY_TIMEOUT_S:.0}s (last: {:?})",
                        String::from_utf8_lossy(&line)
                    )
                } else {
                    format!(
                        "emulator never printed +READY (last: {:?})",
                        String::from_utf8_lossy(&line)
                    )
                })
            }
            Ok(_) => {
                line.push(b[0]);
                if line.windows(6).any(|w| w == b"+READY") {
                    return Ok(());
                }
                if b[0] == b'\n' {
                    line.clear();
                }
            }
            Err(e) => return Err(format!("emulator stdout read error: {e}")),
        }
    }
}

// ---- A4: RMS snapshots ------------------------------------------------------
/// Mapping relative-path -> sha256 for every file under the RMS directory the
/// emulator was pointed at (command 11). Take it before and after a run.
pub fn snapshot_rms(rms_dir: &Path) -> BTreeMap<String, String> {
    let mut snap = BTreeMap::new();
    if !rms_dir.exists() {
        return snap;
    }
    for p in util::walk_files(rms_dir) {
        if let Ok(rel) = p.strip_prefix(rms_dir) {
            if let Ok(data) = std::fs::read(&p) {
                snap.insert(rel.to_string_lossy().into_owned(), sha256_hex(&data));
            }
        }
    }
    snap
}

/// Which record stores appeared / disappeared / changed between snapshots.
pub fn diff_rms(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    out.insert(
        "added".to_string(),
        after.keys().filter(|k| !before.contains_key(*k)).cloned().collect(),
    );
    out.insert(
        "removed".to_string(),
        before.keys().filter(|k| !after.contains_key(*k)).cloned().collect(),
    );
    out.insert(
        "changed".to_string(),
        after
            .iter()
            .filter(|(k, v)| before.get(*k).map(|b| b != *v).unwrap_or(false))
            .map(|(k, _)| k.clone())
            .collect(),
    );
    out
}

/// `kill(pid, SIGKILL)` for OUR OWN child process. Strictly by the pid of our
/// own instance — no `pkill`/`killall` by name.
#[cfg(unix)]
unsafe fn libc_kill(pid: i32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(pid, 9);
}
#[cfg(not(unix))]
unsafe fn libc_kill(_pid: i32) {}

pub fn which(bin: &str) -> Option<PathBuf> {
    let p = Path::new(bin);
    if p.is_absolute() {
        return if p.exists() { Some(p.to_path_buf()) } else { None };
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(bin);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn child_args_has_26_entries() {
        let a = child_args(240, 320);
        assert_eq!(a.len(), 26);
        assert_eq!(a[0], "240");
        assert_eq!(a[1], "320");
        assert_eq!(a[4], "60");
    }
}
