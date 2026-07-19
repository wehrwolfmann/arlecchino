//! Shared helpers: home/path expansion, external-tool resolution, lossy file reads.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Classpath / path separator (POSIX). Mirrors Python `os.pathsep`.
pub const PATHSEP: &str = ":";

/// Captured result of running an external process.
pub struct RunResult {
    /// Exit code (None if killed/signalled or spawn failed).
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// Set when the process could not be launched at all.
    pub spawn_err: Option<String>,
}

impl RunResult {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

/// Run `argv`, capturing stdout+stderr, with an optional timeout (kills on expiry).
/// Drains both pipes on background threads to avoid buffer deadlock.
pub fn run_capture(argv: &[String], timeout: Option<Duration>) -> RunResult {
    if argv.is_empty() {
        return RunResult {
            status: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            spawn_err: Some("empty argv".into()),
        };
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return RunResult {
                status: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                spawn_err: Some(e.to_string()),
            }
        }
    };
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t1 = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t2 = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let mut timed_out = false;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) => {
                if let Some(to) = timeout {
                    if start.elapsed() >= to {
                        let _ = child.kill();
                        let _ = child.wait();
                        timed_out = true;
                        break None;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break None,
        }
    };
    let out = t1.join().unwrap_or_default();
    let err = t2.join().unwrap_or_default();
    RunResult {
        status,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        timed_out,
        spawn_err: None,
    }
}

/// Run `argv` inheriting the parent's stdio (interactive), returning the exit code.
pub fn run_inherit(argv: &[String]) -> i32 {
    if argv.is_empty() {
        return 1;
    }
    match Command::new(&argv[0]).args(&argv[1..]).status() {
        Ok(st) => st.code().unwrap_or(1),
        Err(_) => 1,
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Expand a leading `~` to the home directory (like `os.path.expanduser`).
pub fn expanduser(p: &str) -> PathBuf {
    if p == "~" {
        return home_dir();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(p)
}

/// Make a path absolute against the current working dir without requiring it to
/// exist (unlike `canonicalize`). Approximates Python `Path(...).resolve()` enough
/// for our display/relative-path uses.
pub fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        normalize_dots(p)
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        normalize_dots(&cwd.join(p))
    }
}

fn normalize_dots(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve an external tool name honouring an environment override.
/// e.g. `tool("ARLECCHINO_JAVAC", "javac")`.
pub fn tool(env: &str, default: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| default.to_string())
}

/// Read a file as UTF-8, replacing invalid sequences (like Python `errors="replace"`).
pub fn read_text_lossy(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ── zip / jar helpers ────────────────────────────────────────────────────────
/// All entry names in a zip/jar (like Python `ZipFile.namelist()`).
pub fn zip_namelist(path: &Path) -> std::io::Result<Vec<String>> {
    let f = std::fs::File::open(path)?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut out = Vec::with_capacity(ar.len());
    for i in 0..ar.len() {
        let e = ar.by_index(i).map_err(|e| std::io::Error::other(e.to_string()))?;
        out.push(e.name().to_string());
    }
    Ok(out)
}

/// Read one entry's bytes from a zip/jar.
pub fn zip_read_entry(path: &Path, name: &str) -> std::io::Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut e = ar
        .by_name(name)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut buf = Vec::new();
    e.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Extract all entries of a zip/jar into `dest` (like `ZipFile.extractall`).
pub fn zip_extract_all(path: &Path, dest: &Path) -> std::io::Result<()> {
    let f = std::fs::File::open(path)?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::create_dir_all(dest)?;
    ar.extract(dest).map_err(|e| std::io::Error::other(e.to_string()))
}

/// Create a fresh unique temp directory with the given prefix.
pub fn make_temp_dir(prefix: &str) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = std::env::temp_dir().join(format!("{prefix}{}_{}_{}", std::process::id(), n, nanos));
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

/// Recursively collect every regular file under `dir` (like `Path.rglob("*")`).
pub fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let p = entry.path();
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(p),
                Ok(ft) if ft.is_file() => out.push(p),
                // follow into symlinked dirs / treat symlinked files as files
                _ => {
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.is_file() {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

/// Recursively collect files under `dir` whose name ends with `.<ext>`.
pub fn rglob_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let suffix = format!(".{ext}");
    walk_files(dir)
        .into_iter()
        .filter(|p| p.file_name().map(|n| n.to_string_lossy().ends_with(&suffix)).unwrap_or(false))
        .collect()
}

/// Split on `\n`, keeping the trailing newline on each line (like Python
/// `str.splitlines(keepends=True)` for `\n`-terminated text).
pub fn split_keepends(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    for i in 0..bytes.len() {
        if bytes[i] == b'\n' {
            out.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Recursively copy `src` tree into `dst`, skipping any path component whose name is
/// in `ignore` (like `shutil.copytree(..., ignore=ignore_patterns(...))` for names).
pub fn copy_tree(src: &Path, dst: &Path, ignore: &[&str]) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if ignore.iter().any(|ig| *ig == name_s.as_ref()) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to, ignore)?;
        } else {
            if let Some(p) = to.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// `shutil.move`-style rename that falls back to copy+remove across filesystems.
pub fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst)?;
            std::fs::remove_file(src)
        }
    }
}
