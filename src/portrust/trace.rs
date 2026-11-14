//! Trace comparison: the call trace of a translated crate (`jrt`) against the
//! trace midp-rt writes for the original jar (`-Dmidprt.trace`).
//!
//! Both traces are first brought to one canonical form:
//!
//! * frame timestamps, `repaint`/`serviceRepaints`, comments, font metrics,
//!   `getGameAction` and `Player.realize/prefetch` are dropped — they depend on
//!   the host, not on the game;
//! * `Font.getFont` and `translate 0 0` are dropped (midp-rt issues them itself), and
//!   so is `RecordStore.getNumRecords`: midp-rt's file store calls it on its own
//!   while it saves a record (RMS writes are still compared byte for byte);
//! * images and players are renumbered in order of appearance; ids listed in
//!   `drop_ids` (objects midp-rt creates on its own) and, optionally, objects that
//!   are never drawn or played, are left out;
//! * consecutive identical frames are folded: midp-rt repaints on a wall-clock
//!   timer, so the same frame can be captured several times.
//!
//! Key events are kept as `K …` lines between frames.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct CanonOptions {
    /// Object ids (`I <id>`, `P <id>`) to leave out entirely.
    pub drop_ids: Vec<i64>,
    /// Also leave out objects that no line ever references (`#<id>`).
    pub drop_unreferenced: bool,
}

/// The canonical form of one trace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Canon {
    pub lines: Vec<String>,
    /// Distinct frames after folding.
    pub frames: usize,
}

fn is_noise(l: &str) -> bool {
    l.starts_with('#')
        || l == "repaint"
        || l == "serviceRepaints"
        || l.starts_with("Font.getHeight")
        || l.starts_with("Font.stringWidth")
        || l.starts_with("Font.getFont")
        || l.starts_with("getGameAction")
        || l.starts_with("Player.realize")
        || l.starts_with("Player.prefetch")
        || l.starts_with("RecordStore.getNumRecords")
        || l == "translate 0 0"
}

fn object_ref(tok: &str) -> Option<i64> {
    tok.strip_prefix('#').filter(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit())).and_then(|d| d.parse().ok())
}

fn declared_id(l: &str) -> Option<i64> {
    let mut it = l.split_whitespace();
    match (it.next(), it.next()) {
        (Some("I" | "P"), Some(id)) => id.parse().ok(),
        _ => None,
    }
}

/// Brings a raw trace to canonical form.
pub fn canonical(text: &str, opts: &CanonOptions) -> Canon {
    let mut kept: Vec<String> = Vec::new();
    for raw in text.lines() {
        let l = raw.trim_end();
        if l.is_empty() || is_noise(l) {
            continue;
        }
        let l = if l.starts_with("F ") {
            "F".to_string()
        } else if l.starts_with("Display.setCurrent") {
            "Display.setCurrent".to_string()
        } else {
            l.to_string()
        };
        kept.push(l);
    }
    let mut dropped: HashSet<i64> = opts.drop_ids.iter().copied().collect();
    if opts.drop_unreferenced {
        let referenced: HashSet<i64> = kept.iter().flat_map(|l| l.split_whitespace().filter_map(object_ref).collect::<Vec<_>>()).collect();
        dropped.extend(kept.iter().filter_map(|l| declared_id(l)).filter(|id| !referenced.contains(id)));
    }
    let mut renumber: HashMap<i64, i64> = HashMap::new();
    let mut out = Vec::new();
    let mut body: Vec<String> = Vec::new();
    let mut prev: Option<Vec<String>> = None;
    let mut keys: Vec<String> = Vec::new();
    let mut in_frame = false;
    let mut frames = 0;
    let mut flush = |body: &mut Vec<String>, keys: &mut Vec<String>, out: &mut Vec<String>, in_frame: bool| {
        if in_frame && prev.as_ref() != Some(body) {
            out.push("F".to_string());
            out.extend(body.iter().cloned());
            prev = Some(std::mem::take(body));
            frames += 1;
        }
        body.clear();
        out.append(keys);
    };
    for l in kept {
        if l == "F" {
            flush(&mut body, &mut keys, &mut out, in_frame);
            in_frame = true;
            continue;
        }
        if l.starts_with("keyPressed ") || l.starts_with("keyReleased ") {
            keys.push(format!("K {l}"));
            continue;
        }
        let line = if let Some(id) = declared_id(&l) {
            if dropped.contains(&id) {
                continue;
            }
            let next = renumber.len() as i64 + 1;
            let new = *renumber.entry(id).or_insert(next);
            let mut toks: Vec<String> = l.split_whitespace().map(str::to_string).collect();
            toks[1] = new.to_string();
            toks.join(" ")
        } else if l.split_whitespace().any(|t| object_ref(t).is_some()) {
            l.split_whitespace()
                .map(|t| match object_ref(t) {
                    Some(id) => match renumber.get(&id) {
                        Some(n) => format!("#{n}"),
                        None => format!("#?{id}"),
                    },
                    None => t.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            l
        };
        if in_frame {
            body.push(line);
        } else {
            out.push(line);
        }
    }
    flush(&mut body, &mut keys, &mut out, in_frame);
    Canon { lines: out, frames }
}

/// Number of lines that differ between two line lists (as `diff` counts `<` and `>`).
pub fn differing_lines(a: &[String], b: &[String]) -> usize {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[prefix..], &b[prefix..]);
    let suffix = a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[..a.len() - suffix], &b[..b.len() - suffix]);
    if a.is_empty() || b.is_empty() {
        return a.len() + b.len();
    }
    if a.len().saturating_mul(b.len()) > 25_000_000 {
        // too large for an exact LCS: count positional mismatches instead (an upper bound)
        let common = a.iter().zip(b).filter(|(x, y)| x == y).count();
        return a.len() + b.len() - 2 * common;
    }
    let mut row = vec![0u32; b.len() + 1];
    for x in a {
        let mut diag = 0;
        for (j, y) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = if x == y { diag + 1 } else { up.max(row[j]) };
            diag = up;
        }
    }
    a.len() + b.len() - 2 * row[b.len()] as usize
}

/// Result of [`compare`].
#[derive(Debug, Clone)]
pub struct TraceDiff {
    pub reference: Canon,
    pub port: Canon,
    pub differing: usize,
}

/// Compares a midp-rt reference trace with the trace of a translation.
/// `reference_drop` are ids midp-rt created itself (applied to the reference only).
pub fn compare(reference: &str, port: &str, reference_drop: &[i64], drop_unreferenced: bool) -> TraceDiff {
    let r = canonical(reference, &CanonOptions { drop_ids: reference_drop.to_vec(), drop_unreferenced });
    let p = canonical(port, &CanonOptions { drop_ids: vec![], drop_unreferenced });
    let differing = differing_lines(&r.lines, &p.lines);
    TraceDiff { reference: r, port: p, differing }
}

/// Runs `jar` headless under midp-rt with `-Dmidprt.trace` and returns the trace
/// text.  `frames` is the number of frames traced in detail; `work` receives the
/// captured PNGs, the record stores and the trace file.
pub fn midp_rt_trace(
    jar: &std::path::Path,
    midp_rt: &std::path::Path,
    work: &std::path::Path,
    frames: i64,
    size: (i64, i64),
) -> Result<String, String> {
    use crate::midprun::{build_headless_argv, HeadlessArgvOpts};
    let trace_file = work.join("ref-trace.txt");
    let frames_dir = work.join("frames");
    std::fs::create_dir_all(&frames_dir).map_err(|e| e.to_string())?;
    let mut argv = build_headless_argv(
        jar,
        midp_rt,
        &HeadlessArgvOpts {
            frames_dir,
            frames: (frames / 2).max(4),
            period_ms: 100,
            boot_wait_ms: 1500,
            inputs_file: None,
            rms_dir: None,
            width: Some(size.0),
            height: Some(size.1),
            instrument: true,
            home_dir: Some(work.join("home")),
        },
    );
    let props = [
        "-Dorg.microemu.media.noop=1".to_string(),
        format!("-Dmidprt.trace={}", trace_file.display()),
        format!("-Dmidprt.trace.frames={frames}"),
    ];
    argv.splice(1..1, props);
    let r = crate::util::run_capture_in(&argv, Some(std::time::Duration::from_secs(300)), Some(work));
    if !trace_file.exists() {
        return Err(format!("midp-rt wrote no trace: {}{}", r.stderr.trim(), r.spawn_err.unwrap_or_default()));
    }
    std::fs::read_to_string(&trace_file).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_frames_and_renumbers_objects() {
        let reference = "# midp-rt trace\nI 7 16x16 \"/a.png\"\nDisplay.setCurrent SynthCanvas@1f\nF 0 t=12\nsetColor 0\nfillRect 0 0 240 320\nrepaint\nF 1 t=110\nsetColor 0\nfillRect 0 0 240 320\nkeyPressed 53\nF 2 t=230\ndrawImage #7 1 2 20\n";
        let port = "# jrt trace\nI 1 16x16 \"/a.png\"\nDisplay.setCurrent\nF 0 t=0\nsetColor 0\nfillRect 0 0 240 320\nkeyPressed 53\nF 1 t=0\ndrawImage #1 1 2 20\n";
        let d = compare(reference, port, &[], false);
        assert_eq!(d.reference.frames, 2);
        assert_eq!(d.reference.lines, d.port.lines, "{:#?}", d);
        assert_eq!(d.differing, 0);
        assert!(d.reference.lines.contains(&"K keyPressed 53".to_string()));
    }

    #[test]
    fn drops_objects_never_used() {
        let t = "I 3 1x1 \"byte[10]\"\nI 4 2x2 \"/b.png\"\nF 0 t=0\ndrawImage #4 0 0 20\n";
        let c = canonical(t, &CanonOptions { drop_ids: vec![], drop_unreferenced: true });
        assert_eq!(c.lines, vec!["I 1 2x2 \"/b.png\"", "F", "drawImage #1 0 0 20"]);
    }

    #[test]
    fn counts_differences_like_diff() {
        let a: Vec<String> = ["x", "a", "b", "c", "y"].iter().map(|s| s.to_string()).collect();
        let b: Vec<String> = ["x", "a", "c", "d", "y"].iter().map(|s| s.to_string()).collect();
        assert_eq!(differing_lines(&a, &b), 2);
        assert_eq!(differing_lines(&a, &a), 0);
    }
}
