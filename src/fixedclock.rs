//! Этап A7 (PLAN_runtime.md, блок A) — детерминированное покадровое воспроизведение
//! через инструментацию байткода MIDlet (путь *(b)*: инструментируем MIDlet, GPL-эмулятор
//! НЕ форкаем).
//!
//! Порт `arlecchino/fixedclock.py` — оркестрация вокруг
//! `assets/DeterministicClockTransformer.java`. Сам трансформер остаётся на Java:
//! он работает через ASM, а по решению проекта граница Rust/Java = граница процесса.
//! Трансформер переписывает jar MIDlet-а так, что `System.currentTimeMillis()`/
//! `nanoTime()` становятся виртуальными часами (счётчик с фиксированным шагом), а
//! `new Random(...)` получает фиксированное зерно — отрисованное состояние MIDlet-а
//! становится чистой функцией от ПОСЛЕДОВАТЕЛЬНОСТИ его обращений к часам/ГСЧ и не
//! зависит от часов хоста. Два воспроизведения одного ввода тогда дают побайтово
//! одинаковые кадры даже на FreeJ2ME-plus (его недетерминизм давал базовый Jaccard
//! 0.86 на Аллодах).
//!
//! Используется ТА ЖЕ инфраструктура ASM, что и у переименователя членов
//! (`rename`): jar-ы ASM под `<engines>/asm` и каталог скомпилированных
//! помощников `_classes/`.
//!
//! Всё деградирует мягко: нет jar-ов ASM или скомпилированного помощника —
//! `transform` пишет об этом и возвращает None, вызывающий откатывается на
//! неинструментированный jar (обычный недетерминированный режим MVP).
//!
//! ТОЛЬКО ПО ЗАПРОСУ. Ничего здесь не выполняется без явного `difftest --fixed-clock`
//! или переменной `ARLECCHINO_FIXED_CLOCK`.

use crate::rename;
use crate::util;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const HELPER_CLASS: &str = "DeterministicClockTransformer";
pub const HELPER_SOURCE: &str = include_str!("../assets/DeterministicClockTransformer.java");

/// Переменная окружения: если истинна, difftest инструментирует оба jar перед
/// прогоном. CLI `--fixed-clock` включает то же самое явно.
pub const ENV_FLAG: &str = "ARLECCHINO_FIXED_CLOCK";

pub fn env_enabled() -> bool {
    matches!(
        std::env::var(ENV_FLAG).unwrap_or_default().trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn helper_class_file(root: Option<&Path>) -> PathBuf {
    rename::helper_classes_dir(root).join(format!("{HELPER_CLASS}.class"))
}

/// Скомпилирован ли A7-трансформер.
pub fn helper_compiled(root: Option<&Path>) -> bool {
    helper_class_file(root).exists()
}

/// true, только если есть и jar-ы ASM, и скомпилированный A7-трансформер.
pub fn available(root: Option<&Path>) -> bool {
    !rename::asm_jars(root).is_empty() && helper_compiled(root)
}

/// javac-компиляция `DeterministicClockTransformer.java` против jar-ов ASM
/// (идемпотентно). Используется командой `setup` рядом с переименователем членов.
pub fn compile_helper(root: Option<&Path>) -> String {
    let jars = rename::asm_jars(root);
    if jars.is_empty() {
        return "skip (no ASM jars; run setup with network access)".into();
    }
    let dir = rename::asm_dir(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return format!("FAILED (cannot create {})", dir.display());
    }
    let src = dir.join(format!("{HELPER_CLASS}.java"));
    if std::fs::write(&src, HELPER_SOURCE).is_err() {
        return format!("FAILED (cannot write helper source: {})", src.display());
    }
    let outdir = rename::helper_classes_dir(root);
    let _ = std::fs::create_dir_all(&outdir);
    let cp = jars.iter().map(|j| j.to_string_lossy().into_owned()).collect::<Vec<_>>().join(util::PATHSEP);
    let argv = vec![
        util::tool("ARLECCHINO_JAVAC", "javac"),
        "-nowarn".into(),
        "-proc:none".into(),
        "-cp".into(),
        cp,
        "-d".into(),
        outdir.to_string_lossy().into_owned(),
        src.to_string_lossy().into_owned(),
    ];
    let r = util::run_capture(&argv, Some(Duration::from_secs(300)));
    if r.success() && helper_compiled(root) {
        format!("ok ({})", helper_class_file(root).display())
    } else {
        let tail: String = r.stderr.trim().lines().rev().take(2).collect::<Vec<_>>().join(" | ");
        format!("FAILED ({tail})")
    }
}

/// Jar-ы, несущие API MIDP (`javax.microedition.*`) — отдаются трансформеру
/// как `--cp`, чтобы пересчёт стекмап-кадров мог разрешить супертипы MIDlet-а
/// (Canvas, MIDlet, RecordStore, …).
///
/// Их нужен только запасной путь ПЕРЕСЧЁТА кадров (класс, у которого кадры
/// вырезаны); обычный путь сохраняет собственные кадры jar и ничего не разрешает.
/// Поэтому отсутствие этих jar — не ошибка.
pub fn api_classpath(root: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(rt) = crate::midprun::find_midp_rt(None, root) {
        out.push(rt);
    }
    let roots: Vec<PathBuf> = match root {
        Some(r) => vec![r.to_path_buf()],
        None => vec![crate::config::engines_root_default()],
    };
    for r in roots {
        for name in ["freej2me-lr.jar", "freej2me.jar"] {
            let cand = r.join(name);
            if cand.exists() {
                out.push(cand);
                break;
            }
        }
    }
    out
}

/// Позвать ASM-трансформер: переписать `jar` -> `out_jar` с виртуальными часами и
/// ГСЧ с фиксированным зерном, внедрив самодостаточный рантайм-класс
/// `arlecchino/det/Det`. Возвращает out_jar при успехе, None при любой неудаче
/// (с печатью), чтобы вызывающий откатился на исходный jar.
pub fn transform(
    jar: &Path,
    out_jar: &Path,
    root: Option<&Path>,
    recompute_frames: bool,
) -> Option<PathBuf> {
    let jars = rename::asm_jars(root);
    if jars.is_empty() || !helper_compiled(root) {
        println!(
            "   fixed-clock: ASM/transformer unavailable — skipping \
             (run `arlecchino setup`). Using original jar (nondeterministic)."
        );
        return None;
    }
    let mut cp: Vec<String> = jars.iter().map(|j| j.to_string_lossy().into_owned()).collect();
    cp.push(rename::helper_classes_dir(root).to_string_lossy().into_owned());
    if let Some(p) = out_jar.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let mut argv = vec![
        util::tool("ARLECCHINO_JAVA", "java"),
        "-cp".into(),
        cp.join(util::PATHSEP),
        HELPER_CLASS.into(),
    ];
    let api = api_classpath(root);
    if !api.is_empty() {
        argv.push("--cp".into());
        argv.push(
            api.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(util::PATHSEP),
        );
    }
    if recompute_frames {
        argv.push("--recompute-frames".into());
    }
    argv.push(jar.to_string_lossy().into_owned());
    argv.push(out_jar.to_string_lossy().into_owned());

    let r = util::run_capture(&argv, Some(Duration::from_secs(600)));
    if let Some(e) = &r.spawn_err {
        println!("   fixed-clock: launch error ({e}); using original jar.");
        return None;
    }
    if !r.success() || !out_jar.exists() {
        let tail: String = r.stderr.trim().chars().rev().take(300).collect::<String>().chars().rev().collect();
        println!("   fixed-clock: FAILED ({tail}); using original jar.");
        return None;
    }
    println!("   fixed-clock: {}", r.stdout.trim());
    // Показываем предупреждения помощника (например, типы, которые он не смог
    // разрешить при пересчёте кадров), а не даём им сгинуть в трубе.
    for line in r.stderr.trim().lines() {
        if !line.trim().is_empty() {
            println!("   fixed-clock: {}", line.trim());
        }
    }
    Some(out_jar.to_path_buf())
}

/// Решить-и-выполнить инструментацию A7.
///
/// `enabled` (из `--fixed-clock` или `env_enabled`): при false вернуть исходный
/// `jar` без изменений. При true переписать в `<stem>.fixedclock.jar` внутри
/// `workdir`; при любой неудаче откатиться на исходный jar.
pub fn maybe_transform(jar: &Path, workdir: &Path, root: Option<&Path>, enabled: bool) -> PathBuf {
    if !enabled {
        return jar.to_path_buf();
    }
    let _ = std::fs::create_dir_all(workdir);
    let stem = jar.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let out_jar = workdir.join(format!("{stem}.fixedclock.jar"));
    transform(jar, &out_jar, root, false).unwrap_or_else(|| jar.to_path_buf())
}
