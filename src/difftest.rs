//! Этап A5 (PLAN_runtime.md, блок A) — стенд поведенческого сравнения.
//!
//! Порт `arlecchino/difftest.py`.
//!
//! `arlecchino difftest <orig.jar> <rebuilt.jar>` гоняет ОБА jar безголово в
//! FreeJ2ME-plus (этап A2, `diffrun`) с одним и тем же скриптом ввода (формат
//! этапа A3) и сравнивает поведение.
//!
//! Разведка A2 задала модель сравнения, вокруг которой построен модуль:
//!
//!   * СОДЕРЖИМОЕ кадра детерминировано — каждое различимое состояние экрана даёт
//!     один и тот же sha256 от прогона к прогону;
//!   * ВРЕМЯ кадра — нет: переходы состояний плывут на ±1 кадр (живые часы).
//!     Необязательный режим A7 `--fixed-clock` (`fixedclock`) виртуализирует
//!     часы/ГСЧ MIDlet-а и срезает почти весь этот дрейф.
//!
//! Поэтому мы никогда не сравниваем кадр i с кадром i: сравнивается **структура
//! состояний экрана** — множество уникальных хэшей, порядок их первого появления
//! и индексы первого появления (с допуском на сдвиг), плюс побочные эффекты RMS
//! (этап A4) и признаки падений из stderr эмулятора.
//!
//! Калибровка шума (`--calibrate`, и по умолчанию внутри difftest): ОДИН И ТОТ ЖЕ
//! jar гоняется дважды и сравнивается сам с собой; эта база говорит, сколько
//! расхождения — чистый недетерминизм эмулятора, и вердикт «оригинал против
//! пересборки» падает только на сигналах СИЛЬНЕЕ этого шума.
//!
//! Чистая логика сравнения (всё, чему не нужен эмулятор) живёт в верхней половине
//! модуля и покрыта тестами без java; оркестрация прогонов — ниже.

use crate::diffrun::{self, HeadlessResult, InputEvent, RunOpts};
use crate::midprun;
use crate::util;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Умолчания. `shift_tol` — В КАДРАХ: разведка намерила ±1, ставим 3 с запасом;
/// эффективный допуск дополнительно расширяется тем, что намерила калибровка.
pub const DEFAULT_FRAMES: i64 = 30;
pub const DEFAULT_SHIFT_TOL: i64 = 3;
pub const DEFAULT_JACCARD_MARGIN: f64 = 0.15;

/// Безголовые движки, которыми умеет править difftest (B4': midp-rt — наш
/// LGPL-форк MicroEmulator в отдельном процессе — вторая независимая опора рядом
/// с FreeJ2ME-plus).
pub const ENGINES: [&str; 2] = ["freej2me", "midp-rt"];
/// Проблемы окружения (не установлен jar движка / java) — пробрасываем наверх,
/// чтобы вызывающий сделал SKIP, а не записал поведенческий провал.
const ENV_ERROR_MARKERS: [&str; 3] =
    ["freej2me-lr.jar not found", "java executable", "midp-rt.jar not found"];

/// Файлы внутри каталога RMS, которые НЕ являются хранилищами игры
/// (FreeJ2ME-plus кладёт рядом свой конфиг на игру).
const RMS_ENGINE_FILES: [&str; 1] = ["game.conf"];

// ───────────────────────── чистая логика сравнения ──────────────────────────
/// Свернуть пер-кадровые хэши в (состояние, число_подряд_идущих_кадров).
pub fn collapse_states(hashes: &[String]) -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = Vec::new();
    for h in hashes {
        match out.last_mut() {
            Some(last) if last.0 == *h => last.1 += 1,
            _ => out.push((h.clone(), 1)),
        }
    }
    out
}

/// Различные состояния в порядке ПЕРВОГО появления (устойчиво к дрейфу ±1 кадр,
/// в отличие от сырой RLE-последовательности, которая может потерять или добыть
/// хвостовой цикл мигания).
pub fn unique_in_order(hashes: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    let mut out = Vec::new();
    for h in hashes {
        if seen.insert(h) {
            out.push(h.clone());
        }
    }
    out
}

/// хэш состояния -> индекс кадра его первого появления
pub fn first_seen(hashes: &[String]) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for (i, h) in hashes.iter().enumerate() {
        out.entry(h.clone()).or_insert(i as i64);
    }
    out
}

/// Структурное сравнение последовательностей состояний экрана двух прогонов.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqComparison {
    /// |уникальные_a ∩ уникальные_b| / |уникальные_a ∪ уникальные_b|
    pub jaccard: f64,
    /// состояния, виденные только в прогоне a
    pub only_a: Vec<String>,
    pub only_b: Vec<String>,
    /// общие состояния впервые появляются в одном и том же порядке
    pub order_ok: bool,
    /// max |first_seen_a - first_seen_b| по общим состояниям
    pub max_shift: i64,
    /// совпадение по индексам (только для сведения — чувствительно ко времени)
    pub frame_match_rate: f64,
}

impl SeqComparison {
    pub fn sets_equal(&self) -> bool {
        self.only_a.is_empty() && self.only_b.is_empty()
    }
}

pub fn compare_sequences(hashes_a: &[String], hashes_b: &[String]) -> SeqComparison {
    let ua = unique_in_order(hashes_a);
    let ub = unique_in_order(hashes_b);
    let sa: BTreeSet<String> = ua.iter().cloned().collect();
    let sb: BTreeSet<String> = ub.iter().cloned().collect();
    let shared: BTreeSet<String> = sa.intersection(&sb).cloned().collect();
    let union: BTreeSet<String> = sa.union(&sb).cloned().collect();
    let fa = first_seen(hashes_a);
    let fb = first_seen(hashes_b);
    let max_shift = shared
        .iter()
        .map(|h| (fa[h] - fb[h]).abs())
        .max()
        .unwrap_or(0);
    let order_a: Vec<&String> = ua.iter().filter(|h| shared.contains(*h)).collect();
    let order_b: Vec<&String> = ub.iter().filter(|h| shared.contains(*h)).collect();
    let n = hashes_a.len().min(hashes_b.len());
    let matches = hashes_a.iter().zip(hashes_b.iter()).filter(|(x, y)| x == y).count();
    SeqComparison {
        jaccard: if union.is_empty() {
            1.0
        } else {
            shared.len() as f64 / union.len() as f64
        },
        only_a: sa.difference(&sb).cloned().collect(),
        only_b: sb.difference(&sa).cloned().collect(),
        order_ok: order_a == order_b,
        max_shift,
        frame_match_rate: if n == 0 { 1.0 } else { matches as f64 / n as f64 },
    }
}

/// Всё, что вердикту надо знать про один безголовый прогон.
#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub label: String,
    pub out_dir: String,
    pub hashes: Vec<String>,
    pub nonblank: bool,
    /// относительный путь -> sha256 (после прогона)
    pub rms: BTreeMap<String, String>,
    /// имена классов исключений, найденные в stderr
    pub exceptions: BTreeSet<String>,
    /// сам прогон провалился
    pub error: Option<String>,
}

pub fn rms_paths(s: &RunSummary) -> BTreeSet<String> {
    s.rms.keys().cloned().collect()
}

#[derive(Debug, Clone)]
pub struct Verdict {
    pub passed: bool,
    /// почему ПРОВАЛ (пусто при успехе)
    pub reasons: Vec<String>,
    /// информационные наблюдения
    pub notes: Vec<String>,
    /// оригинал против оригинала (шум); None, если калибровка пропущена
    pub baseline: Option<SeqComparison>,
    /// оригинал против пересборки
    pub cross: SeqComparison,
}

fn empty_seq() -> SeqComparison {
    SeqComparison {
        jaccard: 0.0,
        only_a: vec![],
        only_b: vec![],
        order_ok: false,
        max_shift: 0,
        frame_match_rate: 0.0,
    }
}

fn pct0(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

/// Вердикт A5: ПРОВАЛ только на сигналах сильнее откалиброванного шума.
///
/// `orig_b` — второй прогон оригинала (база шума); None — судить по фиксированным
/// порогам без калибровки. Чистая функция над RunSummary.
pub fn decide(
    orig_a: &RunSummary,
    orig_b: Option<&RunSummary>,
    rebuilt: &RunSummary,
    shift_tol: i64,
    jaccard_margin: f64,
) -> Verdict {
    let mut reasons: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // 0) прогоны вообще должны были состояться
    if let Some(e) = &orig_a.error {
        return Verdict {
            passed: false,
            reasons: vec![format!("original run failed: {e}")],
            notes,
            baseline: None,
            cross: empty_seq(),
        };
    }
    if let Some(e) = &rebuilt.error {
        return Verdict {
            passed: false,
            reasons: vec![format!("rebuilt run failed: {e}")],
            notes,
            baseline: None,
            cross: empty_seq(),
        };
    }

    let baseline = match orig_b {
        Some(b) if b.error.is_none() => Some(compare_sequences(&orig_a.hashes, &b.hashes)),
        _ => None,
    };
    let cross = compare_sequences(&orig_a.hashes, &rebuilt.hashes);

    // 1) признаки падения, уникальные для пересобранного jar
    let mut orig_exc = orig_a.exceptions.clone();
    if let Some(b) = orig_b {
        orig_exc.extend(b.exceptions.iter().cloned());
    }
    let new_exc: Vec<String> = rebuilt.exceptions.difference(&orig_exc).cloned().collect();
    if !new_exc.is_empty() {
        reasons.push(format!("new exceptions in rebuilt stderr: {}", new_exc.join(", ")));
    }
    if !orig_a.exceptions.is_empty() {
        notes.push(format!(
            "original itself logs exceptions (ignored as baseline): {}",
            orig_a.exceptions.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    // 2) пустой экран
    if orig_a.nonblank && !rebuilt.nonblank {
        reasons.push("rebuilt rendered ONLY blank frames (original did not)".into());
    }

    // 3) структура состояний экрана, судимая относительно базы шума
    let base_j = baseline.as_ref().map(|b| b.jaccard).unwrap_or(1.0);
    let min_jaccard = (base_j - jaccard_margin).max(0.0);
    if cross.jaccard < min_jaccard {
        reasons.push(match &baseline {
            Some(b) => format!(
                "state-set similarity {:.2} below noise floor {:.2} (baseline {:.2} - margin {:.2})",
                cross.jaccard, min_jaccard, b.jaccard, jaccard_margin
            ),
            None => format!(
                "state-set similarity {:.2} below threshold {:.2}",
                cross.jaccard, min_jaccard
            ),
        });
    }
    // порядок: требуем только то, что выдерживает сама база
    if !cross.order_ok {
        if baseline.as_ref().map(|b| b.order_ok).unwrap_or(true) {
            reasons.push("shared screen states appear in a DIFFERENT order".into());
        } else {
            notes.push(
                "state order differs, but the noise baseline already shows order \
                 instability — not held against rebuilt"
                    .into(),
            );
        }
    }
    // сдвиг по времени: допуск расширен тем, что намерила база
    let eff_tol = shift_tol.max(baseline.as_ref().map(|b| b.max_shift).unwrap_or(0) + 1);
    if cross.max_shift > eff_tol {
        reasons.push(format!(
            "state timing shifted by {} frames (tolerance ±{eff_tol})",
            cross.max_shift
        ));
    }

    // 4) побочные эффекты RMS
    if rms_paths(orig_a) != rms_paths(rebuilt) {
        let pa = rms_paths(orig_a);
        let pr = rms_paths(rebuilt);
        let only_o: Vec<String> = pa.difference(&pr).cloned().collect();
        let only_r: Vec<String> = pr.difference(&pa).cloned().collect();
        let paths_stable = match (&baseline, orig_b) {
            (None, _) => true,
            (Some(_), Some(b)) => pa == rms_paths(b),
            (Some(_), None) => true,
        };
        if paths_stable {
            reasons.push(format!(
                "RMS stores differ (only-orig: {}, only-rebuilt: {})",
                fmt_list(&only_o),
                fmt_list(&only_r)
            ));
        } else {
            notes.push(
                "RMS store sets differ, but they differ between the two original runs \
                 too (noise)"
                    .into(),
            );
        }
    } else {
        let content_noise = orig_b.map(|b| orig_a.rms != b.rms).unwrap_or(false);
        if orig_a.rms != rebuilt.rms {
            if content_noise {
                notes.push(
                    "RMS contents differ byte-wise (within noise — original runs differ too)"
                        .into(),
                );
            } else if orig_b.is_none() {
                notes.push("RMS contents differ byte-wise (no baseline to judge by)".into());
            } else {
                reasons.push(
                    "RMS contents differ byte-wise while two original runs were \
                     byte-identical"
                        .into(),
                );
            }
        }
    }

    notes.push(format!(
        "frame-by-frame index match (timing-sensitive, info only): orig-vs-rebuilt {}{}",
        pct0(cross.frame_match_rate),
        match &baseline {
            Some(b) => format!(", baseline {}", pct0(b.frame_match_rate)),
            None => String::new(),
        }
    ));
    Verdict {
        passed: reasons.is_empty(),
        reasons,
        notes,
        baseline,
        cross,
    }
}

/// Питоновский `[...] or '-'`: пустой список печатается как `-`, иначе как repr.
fn fmt_list(v: &[String]) -> String {
    if v.is_empty() {
        "-".to_string()
    } else {
        format!("['{}']", v.join("', '"))
    }
}

// ───────────────────── межмоторное сравнение (связка A↔B) ───────────────────
// PLAN_runtime.md «Связка A↔B»: ОДИН И ТОТ ЖЕ jar в FreeJ2ME-plus против midp-rt
// с одним скриптом ввода. Движки рисуют разными скинами/шрифтами, поэтому хэши
// пикселей НИКОГДА не совпадут между движками — сравнение намеренно МЯГЧЕ
// вердикта A5: сравнивается СТРУКТУРА смены состояний, поведение RMS (ИМЕНА
// хранилищ) и признаки падений. Пиксельный Jaccard даётся только для сведения.

/// Индексы кадров, где экран МЕНЯЕТ состояние (хэш отличается от предыдущего) —
/// «форма» прогона, не зависящая от движка.
pub fn transition_frames(hashes: &[String]) -> Vec<i64> {
    (1..hashes.len())
        .filter(|i| hashes[*i] != hashes[*i - 1])
        .map(|i| i as i64)
        .collect()
}

/// Имена хранилищ RMS, нормализованные для показа: базовое имя, срезаны `.rs`/`.rms`,
/// служебные файлы движка (`game.conf`) исключены. ВНИМАНИЕ: имена НЕ сравнимы
/// МЕЖДУ движками — FreeJ2ME-plus коверкает их на диске, — поэтому межмоторный
/// вердикт сравнивает лишь АКТИВНОСТЬ RMS.
pub fn rms_store_names(s: &RunSummary) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for rel in s.rms.keys() {
        let name = Path::new(rel)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel.clone());
        if RMS_ENGINE_FILES.contains(&name.as_str()) {
            continue;
        }
        let mut name = name;
        for suf in [".rs", ".rms"] {
            if let Some(stripped) = name.strip_suffix(suf) {
                name = stripped.to_string();
                break;
            }
        }
        names.insert(name);
    }
    names
}

/// Мягкое структурное сравнение одного jar на двух движках.
#[derive(Debug, Clone)]
pub struct CrossEngineComparison {
    pub agree: bool,
    /// признаки расхождения (пусто при согласии)
    pub reasons: Vec<String>,
    /// информационное (включая пиксельный Jaccard)
    pub notes: Vec<String>,
    /// уникальных состояний экрана на движок
    pub states: (i64, i64),
    /// индексы кадров смены состояния
    pub transitions: (Vec<i64>, Vec<i64>),
    /// ТОЛЬКО для сведения — между движками ожидается ~0
    pub pixel_jaccard: f64,
}

fn fmt_sorted(v: &BTreeSet<String>) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        format!("['{}']", v.iter().cloned().collect::<Vec<_>>().join("', '"))
    }
}

fn fmt_ints(v: &[i64]) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        format!("[{}]", v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", "))
    }
}

/// Чистая функция над двумя RunSummary.
///
/// Расхождение (reasons) при: провале прогона, «пусто против рисует», расхождении
/// счётчиков уникальных состояний / переходов больше `state_tol`, исключениях
/// только у одного движка, разных множествах имён хранилищ RMS.
pub fn compare_cross_engine(
    run_a: &RunSummary,
    run_b: &RunSummary,
    state_tol: i64,
) -> CrossEngineComparison {
    let mut reasons: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let ta = transition_frames(&run_a.hashes);
    let tb = transition_frames(&run_b.hashes);
    let na = run_a.hashes.iter().collect::<BTreeSet<_>>().len() as i64;
    let nb = run_b.hashes.iter().collect::<BTreeSet<_>>().len() as i64;

    for r in [run_a, run_b] {
        if let Some(e) = &r.error {
            reasons.push(format!("run {} failed: {e}", r.label));
        }
    }
    if reasons.is_empty() {
        if run_a.nonblank != run_b.nonblank {
            let blank = if !run_a.nonblank { run_a } else { run_b };
            reasons.push(format!(
                "{} rendered ONLY blank frames (the other engine did not)",
                blank.label
            ));
        }
        if (na - nb).abs() > state_tol {
            reasons.push(format!(
                "unique screen-state counts diverge: {}={na} vs {}={nb} (tolerance ±{state_tol})",
                run_a.label, run_b.label
            ));
        }
        if (ta.len() as i64 - tb.len() as i64).abs() > state_tol {
            reasons.push(format!(
                "state-change counts diverge: {}={} vs {}={} (tolerance ±{state_tol})",
                run_a.label,
                ta.len(),
                run_b.label,
                tb.len()
            ));
        }
        let only_a: BTreeSet<String> =
            run_a.exceptions.difference(&run_b.exceptions).cloned().collect();
        let only_b: BTreeSet<String> =
            run_b.exceptions.difference(&run_a.exceptions).cloned().collect();
        if !only_a.is_empty() || !only_b.is_empty() {
            reasons.push(format!(
                "exceptions logged by only one engine: {}: {}, {}: {}",
                run_a.label,
                fmt_sorted(&only_a),
                run_b.label,
                fmt_sorted(&only_b)
            ));
        }
        // RMS: между движками одинаково значима только АКТИВНОСТЬ
        let sa = rms_store_names(run_a);
        let sb = rms_store_names(run_b);
        if sa.is_empty() != sb.is_empty() {
            let (wrote, silent) = if !sa.is_empty() { (run_a, run_b) } else { (run_b, run_a) };
            reasons.push(format!(
                "RMS activity differs: {} wrote record stores, {} wrote none",
                wrote.label, silent.label
            ));
        } else if !sa.is_empty() {
            notes.push(format!(
                "both engines wrote RMS (store names per engine, NOT comparable across): \
                 {}: {}; {}: {}",
                run_a.label,
                fmt_sorted(&sa),
                run_b.label,
                fmt_sorted(&sb)
            ));
        }
    }

    let pixel = compare_sequences(&run_a.hashes, &run_b.hashes).jaccard;
    notes.push(format!(
        "pixel-level state Jaccard (info ONLY — different skins/fonts make ~0.00 \
         expected): {pixel:.2}"
    ));
    notes.push(format!(
        "state-change frames {}: {}; {}: {}",
        run_a.label,
        fmt_ints(&ta),
        run_b.label,
        fmt_ints(&tb)
    ));
    CrossEngineComparison {
        agree: reasons.is_empty(),
        reasons,
        notes,
        states: (na, nb),
        transitions: (ta, tb),
        pixel_jaccard: pixel,
    }
}

// ───────────────────────── оркестрация прогонов ─────────────────────────────
/// Имена классов исключений/ошибок Java, упомянутые в stderr эмулятора.
///
/// Сравниваем МНОЖЕСТВА, так что шум, который печатает и оригинал (например,
/// MIDlet, штатно логирующий IOException), никогда не валит вердикт.
pub fn scan_stderr(path: &Path) -> BTreeSet<String> {
    let text = match util::read_text_lossy(path) {
        Ok(t) => t,
        Err(_) => return BTreeSet::new(),
    };
    let re = Regex::new(
        r"\b((?:[A-Za-z_$][\w$]*\.)*[A-Za-z_$][\w$]*(?:Exception|Error))\b",
    )
    .unwrap();
    re.captures_iter(&text)
        .map(|c| c[1].rsplit('.').next().unwrap_or("").to_string())
        .collect()
}

/// Один безголовый прогон -> RunSummary. Провалившийся прогон записывает `error`
/// вместо паники, чтобы вердикт мог о нём сообщить (отсутствие эмулятора всё же
/// возвращается как Err — это проблема окружения, вызывающий делает skip).
pub fn summarize_run(
    jar: &Path,
    out_dir: &Path,
    label: &str,
    inputs: Option<&[InputEvent]>,
    engine: &str,
    opts: &RunOpts,
) -> Result<RunSummary, String> {
    if !ENGINES.contains(&engine) {
        return Err(format!(
            "unknown difftest engine {engine:?} (choose from {ENGINES:?})"
        ));
    }
    let mut summary = RunSummary {
        label: label.to_string(),
        out_dir: out_dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let res: Result<HeadlessResult, String> = if engine == "midp-rt" {
        // midp-rt тактирует себя сам — он уже детерминирован и такого флага не берёт
        let mut o = opts.clone();
        o.deterministic_capture = false;
        midprun::run_headless_midp_rt(jar, out_dir, inputs, &o, None, true)
    } else {
        diffrun::run_headless(jar, out_dir, inputs, opts, None)
    };
    match res {
        Err(e) => {
            if ENV_ERROR_MARKERS.iter().any(|m| e.contains(m)) {
                return Err(e); // проблема окружения, не поведенческий сигнал
            }
            summary.error = Some(e);
            return Ok(summary);
        }
        Ok(r) => {
            summary.hashes = r.hashes();
            summary.nonblank = r.any_nonblank();
            summary.rms = diffrun::snapshot_rms(Path::new(&r.rms_dir));
            summary.exceptions = scan_stderr(Path::new(&r.stderr_log));
        }
    }
    Ok(summary)
}

/// A7: при `enabled` вернуть A7-инструментированную копию `jar` под
/// `out_root/_fixedclock`, иначе `jar` без изменений. При неудаче инструментации
/// откатываемся на исходный jar (прогон всё равно возможен, просто недетерминированный).
fn fixed_clock_jar(jar: &Path, out_root: &Path, enabled: bool) -> PathBuf {
    if !enabled {
        return jar.to_path_buf();
    }
    crate::fixedclock::maybe_transform(jar, &out_root.join("_fixedclock"), None, true)
}

/// `--calibrate`: прогнать ОДИН jar дважды с одним скриптом ввода и намерить
/// базовое расхождение = чистый недетерминизм эмулятора (методика A2).
pub fn calibrate(
    jar: &Path,
    out_root: &Path,
    inputs: Option<&[InputEvent]>,
    engine: &str,
    fixed_clock: bool,
    opts: &RunOpts,
) -> Result<(SeqComparison, RunSummary, RunSummary), String> {
    std::fs::create_dir_all(out_root).map_err(|e| e.to_string())?;
    let jar = fixed_clock_jar(jar, out_root, fixed_clock);
    let a = summarize_run(&jar, &out_root.join("calib_a"), "calib_a", inputs, engine, opts)?;
    let b = summarize_run(&jar, &out_root.join("calib_b"), "calib_b", inputs, engine, opts)?;
    let cmp = compare_sequences(&a.hashes, &b.hashes);
    Ok((cmp, a, b))
}

/// Полный стенд A5: (по желанию) калибровка на оригинале, прогон пересобранного
/// jar, вердикт. Пишет `difftest_report.{md,json}` в out_root.
#[allow(clippy::too_many_arguments)]
pub fn difftest(
    orig_jar: &Path,
    rebuilt_jar: &Path,
    out_root: &Path,
    inputs: Option<&[InputEvent]>,
    shift_tol: i64,
    jaccard_margin: f64,
    with_baseline: bool,
    engine: &str,
    fixed_clock: bool,
    opts: &RunOpts,
) -> Result<Verdict, String> {
    std::fs::create_dir_all(out_root).map_err(|e| e.to_string())?;
    let orig_jar = fixed_clock_jar(orig_jar, &out_root.join("_fc_orig"), fixed_clock);
    let rebuilt_jar = fixed_clock_jar(rebuilt_jar, &out_root.join("_fc_rebuilt"), fixed_clock);
    let orig_a = summarize_run(&orig_jar, &out_root.join("orig_a"), "orig_a", inputs, engine, opts)?;
    let orig_b = if with_baseline {
        Some(summarize_run(&orig_jar, &out_root.join("orig_b"), "orig_b", inputs, engine, opts)?)
    } else {
        None
    };
    let rebuilt =
        summarize_run(&rebuilt_jar, &out_root.join("rebuilt"), "rebuilt", inputs, engine, opts)?;
    let verdict = decide(&orig_a, orig_b.as_ref(), &rebuilt, shift_tol, jaccard_margin);
    write_reports(
        out_root,
        &orig_jar,
        &rebuilt_jar,
        &orig_a,
        orig_b.as_ref(),
        &rebuilt,
        &verdict,
        engine,
    );
    Ok(verdict)
}

/// Прогнать ОДИН jar на обоих движках с одним скриптом ввода и сравнить мягко.
/// Пишет `cross_engine_report.{md,json}` в out_root.
pub fn cross_engine(
    jar: &Path,
    out_root: &Path,
    inputs: Option<&[InputEvent]>,
    engines: (&str, &str),
    state_tol: i64,
    fixed_clock: bool,
    opts: &RunOpts,
) -> Result<(CrossEngineComparison, RunSummary, RunSummary), String> {
    std::fs::create_dir_all(out_root).map_err(|e| e.to_string())?;
    let jar = fixed_clock_jar(jar, out_root, fixed_clock);
    let a = summarize_run(&jar, &out_root.join(engines.0), engines.0, inputs, engines.0, opts)?;
    let b = summarize_run(&jar, &out_root.join(engines.1), engines.1, inputs, engines.1, opts)?;
    let cmp = compare_cross_engine(&a, &b, state_tol);
    write_cross_reports(out_root, &jar, &a, &b, &cmp);
    Ok((cmp, a, b))
}

// ───────────────────────────── отчёты ────────────────────────────────────────
fn seq_json(c: Option<&SeqComparison>) -> serde_json::Value {
    match c {
        None => serde_json::Value::Null,
        Some(c) => serde_json::json!({
            "jaccard": round4(c.jaccard),
            "sets_equal": c.sets_equal(),
            "only_a": c.only_a,
            "only_b": c.only_b,
            "order_ok": c.order_ok,
            "max_shift_frames": c.max_shift,
            "frame_match_rate": round4(c.frame_match_rate),
        }),
    }
}

/// Округление до 4 знаков, как `round(x, 4)` в Python.
fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

fn run_json(r: &RunSummary) -> serde_json::Value {
    serde_json::json!({
        "label": r.label,
        "out_dir": r.out_dir,
        "frames": r.hashes.len(),
        "unique_states": r.hashes.iter().collect::<BTreeSet<_>>().len(),
        "nonblank": r.nonblank,
        "rms_files": r.rms.keys().cloned().collect::<Vec<_>>(),
        "exceptions": r.exceptions.iter().cloned().collect::<Vec<_>>(),
        "error": r.error,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn render_report(
    orig_jar: &Path,
    rebuilt_jar: &Path,
    orig_a: &RunSummary,
    orig_b: Option<&RunSummary>,
    rebuilt: &RunSummary,
    verdict: &Verdict,
    engine: &str,
) -> String {
    let mut lines: Vec<String> = vec![
        "# difftest report".into(),
        String::new(),
        format!("- original: `{}`", orig_jar.display()),
        format!("- rebuilt:  `{}`", rebuilt_jar.display()),
        format!("- engine:   `{engine}`"),
        String::new(),
        format!("## VERDICT: {}", if verdict.passed { "PASS" } else { "FAIL" }),
        String::new(),
    ];
    for r in &verdict.reasons {
        lines.push(format!("- ✗ {r}"));
    }
    for n in &verdict.notes {
        lines.push(format!("- ℹ {n}"));
    }
    lines.extend([
        String::new(),
        "## Runs".into(),
        String::new(),
        "| run | frames | unique states | non-blank | RMS files | exceptions |".into(),
        "|---|---|---|---|---|---|".into(),
    ]);
    let mut runs: Vec<&RunSummary> = vec![orig_a];
    if let Some(b) = orig_b {
        runs.push(b);
    }
    runs.push(rebuilt);
    for r in runs {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} |",
            r.label,
            r.hashes.len(),
            r.hashes.iter().collect::<BTreeSet<_>>().len(),
            if r.nonblank { "yes" } else { "NO" },
            r.rms.len(),
            if r.exceptions.is_empty() {
                "-".to_string()
            } else {
                r.exceptions.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        ));
    }
    lines.extend(seq_block("Noise baseline (orig vs orig)", verdict.baseline.as_ref()));
    lines.extend(seq_block("Orig vs rebuilt", Some(&verdict.cross)));
    lines.join("\n") + "\n"
}

fn seq_block(title: &str, c: Option<&SeqComparison>) -> Vec<String> {
    match c {
        None => vec![String::new(), format!("## {title}"), String::new(), "(skipped)".into()],
        Some(c) => vec![
            String::new(),
            format!("## {title}"),
            String::new(),
            format!(
                "- state-set similarity (Jaccard): {:.2}{}",
                c.jaccard,
                if c.sets_equal() {
                    " (sets EQUAL)".to_string()
                } else {
                    format!(" (only-a: {}, only-b: {})", c.only_a.len(), c.only_b.len())
                }
            ),
            format!(
                "- first-appearance order: {}",
                if c.order_ok { "same" } else { "DIFFERS" }
            ),
            format!("- max timing shift: {} frame(s)", c.max_shift),
            format!("- frame-by-frame match (info): {}", pct0(c.frame_match_rate)),
        ],
    }
}

#[allow(clippy::too_many_arguments)]
fn write_reports(
    out_root: &Path,
    orig_jar: &Path,
    rebuilt_jar: &Path,
    orig_a: &RunSummary,
    orig_b: Option<&RunSummary>,
    rebuilt: &RunSummary,
    verdict: &Verdict,
    engine: &str,
) {
    let _ = std::fs::write(
        out_root.join("difftest_report.md"),
        render_report(orig_jar, rebuilt_jar, orig_a, orig_b, rebuilt, verdict, engine),
    );
    let mut runs = vec![run_json(orig_a)];
    if let Some(b) = orig_b {
        runs.push(run_json(b));
    }
    runs.push(run_json(rebuilt));
    let doc = serde_json::json!({
        "orig_jar": orig_jar.to_string_lossy(),
        "rebuilt_jar": rebuilt_jar.to_string_lossy(),
        "engine": engine,
        "passed": verdict.passed,
        "reasons": verdict.reasons,
        "notes": verdict.notes,
        "baseline": seq_json(verdict.baseline.as_ref()),
        "cross": seq_json(Some(&verdict.cross)),
        "runs": runs,
    });
    let _ = std::fs::write(
        out_root.join("difftest_report.json"),
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    );
}

pub fn render_cross_report(
    jar: &Path,
    run_a: &RunSummary,
    run_b: &RunSummary,
    cmp: &CrossEngineComparison,
) -> String {
    let mut lines: Vec<String> = vec![
        "# cross-engine report (A↔B link)".into(),
        String::new(),
        format!("- jar: `{}`", jar.display()),
        format!("- engines: `{}` vs `{}`", run_a.label, run_b.label),
        String::new(),
        format!(
            "## VERDICT: {} (soft, structural — pixels are NOT compared across engines)",
            if cmp.agree { "AGREE" } else { "DIVERGE" }
        ),
        String::new(),
    ];
    for r in &cmp.reasons {
        lines.push(format!("- ✗ {r}"));
    }
    for n in &cmp.notes {
        lines.push(format!("- ℹ {n}"));
    }
    lines.extend([
        String::new(),
        "## Runs".into(),
        String::new(),
        "| engine | frames | unique states | state changes | non-blank | RMS stores | exceptions |"
            .into(),
        "|---|---|---|---|---|---|---|".into(),
    ]);
    for (r, t) in [(run_a, &cmp.transitions.0), (run_b, &cmp.transitions.1)] {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.label,
            r.hashes.len(),
            r.hashes.iter().collect::<BTreeSet<_>>().len(),
            t.len(),
            if r.nonblank { "yes" } else { "NO" },
            {
                let n = rms_store_names(r);
                if n.is_empty() {
                    "-".to_string()
                } else {
                    n.iter().cloned().collect::<Vec<_>>().join(", ")
                }
            },
            if r.exceptions.is_empty() {
                "-".to_string()
            } else {
                r.exceptions.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        ));
    }
    lines.join("\n") + "\n"
}

fn write_cross_reports(
    out_root: &Path,
    jar: &Path,
    run_a: &RunSummary,
    run_b: &RunSummary,
    cmp: &CrossEngineComparison,
) {
    let _ = std::fs::write(
        out_root.join("cross_engine_report.md"),
        render_cross_report(jar, run_a, run_b, cmp),
    );
    let doc = serde_json::json!({
        "jar": jar.to_string_lossy(),
        "engines": [run_a.label.clone(), run_b.label.clone()],
        "agree": cmp.agree,
        "reasons": cmp.reasons,
        "notes": cmp.notes,
        "unique_states": [cmp.states.0, cmp.states.1],
        "transition_frames": [cmp.transitions.0.clone(), cmp.transitions.1.clone()],
        "pixel_jaccard_info": round4(cmp.pixel_jaccard),
        "runs": [run_json(run_a), run_json(run_b)],
    });
    let _ = std::fs::write(
        out_root.join("cross_engine_report.json"),
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    );
}

// ───────────────────── упаковка пересобранного jar ──────────────────────────
/// Собрать ЗАПУСКАЕМЫЙ пересобранный jar: манифест и ресурсы из `src_jar`,
/// `.class` — из `classes_dir` (round-trip `merged/_classes`, который полон:
/// всё ещё сломанные классы засеяны там оригинальным байткодом).
pub fn pack_rebuilt_jar(src_jar: &Path, classes_dir: &Path, out_jar: &Path) -> Result<PathBuf, String> {
    if let Some(p) = out_jar.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::open(src_jar).map_err(|e| e.to_string())?;
    let mut zin = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let outf = std::fs::File::create(out_jar).map_err(|e| e.to_string())?;
    let mut zout = zip::ZipWriter::new(outf);
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for i in 0..zin.len() {
        let mut e = zin.by_index(i).map_err(|e| e.to_string())?;
        let name = e.name().to_string();
        if name.ends_with('/') || name.ends_with(".class") {
            continue; // только ресурсы и манифест; классы приходят пересобранными
        }
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut e, &mut buf).map_err(|e| e.to_string())?;
        zout.start_file(name, opts).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut zout, &buf).map_err(|e| e.to_string())?;
    }
    let mut classes: Vec<PathBuf> = util::walk_files(classes_dir)
        .into_iter()
        .filter(|p| p.extension().map(|e| e == "class").unwrap_or(false))
        .collect();
    classes.sort();
    for cls in classes {
        let rel = cls.strip_prefix(classes_dir).map_err(|e| e.to_string())?;
        let data = std::fs::read(&cls).map_err(|e| e.to_string())?;
        zout.start_file(rel.to_string_lossy().into_owned(), opts)
            .map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut zout, &data).map_err(|e| e.to_string())?;
    }
    zout.finish().map_err(|e| e.to_string())?;
    Ok(out_jar.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn collapse_and_unique() {
        assert_eq!(
            collapse_states(&h(&["a", "a", "b", "b", "b", "a"])),
            vec![("a".into(), 2), ("b".into(), 3), ("a".into(), 1)]
        );
        assert!(collapse_states(&[]).is_empty());
        let hs = h(&["splash", "splash", "menu", "blinkA", "blinkB", "blinkA"]);
        assert_eq!(unique_in_order(&hs), h(&["splash", "menu", "blinkA", "blinkB"]));
        assert_eq!(first_seen(&hs).get("blinkB"), Some(&4));
    }

    #[test]
    fn timing_shift_tolerated() {
        let a = h(&["s", "s", "s", "m", "m", "a", "b", "a"]);
        let b = h(&["s", "s", "s", "s", "m", "a", "b", "a"]);
        let c = compare_sequences(&a, &b);
        assert!(c.sets_equal() && c.order_ok);
        assert_eq!(c.max_shift, 1);
        assert!(c.frame_match_rate < 1.0);
    }
}
