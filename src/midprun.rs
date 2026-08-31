//! `arlecchino run <game.jar|jad>` (этапы B3'/B4' PLAN_runtime.md, сторона Apache):
//! запуск MIDlet В ОКНЕ через **midp-rt** — наш LGPL-форк MicroEmulator
//! (артефакт `build/midp-rt.jar`).
//!
//! Порт `arlecchino/midprun.py`.
//!
//! Исходники форка живут в этом же дереве — subtree `midp-rt/` (LGPL 2.1). Готовый
//! `midp-rt.jar` в git НЕ хранится (у форка свой `.gitignore` на `build/`), поэтому
//! путь к нему ищется по порядку: `--midp-rt` → `$ARLECCHINO_MIDP_RT` → корень
//! движков → рядом с бинарём → `build/` любого найденного чекаута форка.
//!
//! ⚖️ Граница LGPL = граница ПРОЦЕССА (решение B0): этот модуль только СОБИРАЕТ
//! командную строку java и порождает отдельный процесс — ни строчки LGPL-кода не
//! импортируется, не копируется и не линкуется в Arlecchino.
//!
//! Два поведенческих урока, добытые кровью (живые Аллоды, 2026-06-10):
//!
//! * **Автостарт.** Когда рантайму дают ПУТЬ К JAR, он лишь открывает набор в
//!   списке лаунчера — пользователь должен нажать Start. Вызов по имени класса
//!   (`--appclasspath game.jar <MIDletClass>`) создаёт и запускает MIDlet сразу,
//!   поэтому мы сами разбираем `MIDlet-1` из манифеста (или .jad) и предпочитаем
//!   этот режим; путь-к-jar остаётся запасным, когда `MIDlet-1` не нашёлся.
//!
//! * **Инструментация ВКЛЮЧЕНА по умолчанию.** ASM форка переведён с 3.1 на 9.x,
//!   так что инструментация классов MIDlet работает и на современном байткоде
//!   (major 52+ — наши пересобранные jar), и на исходных телефонных jar. Важнее
//!   другое: НЕинструментированный загрузчик MicroEmulator режет класс на 16 КБ,
//!   а его `ClassNotFoundException` молча съедается в `MIDletClassLoader.loadClass`
//!   → JVM показывает голый `NoClassDefFoundError` (Аллоды: класс `i` = 96 КБ).
//!   `--no-instrument` (родной флаг форка) оставлен лишь как запасной выход.

use crate::config::engines_root_default;
use crate::diffrun::{FrameRecord, HeadlessResult, RunOpts, which, InputEvent};
use crate::util;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Каталог запущенного бинаря (для поиска «рядом с собой»).
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

/// Имя подкаталога с исходниками форка (subtree `midp-rt/`, LGPL 2.1).
pub const REPO_DIRNAME: &str = "midp-rt";
/// Имя собранного артефакта рантайма.
pub const JAR_NAME: &str = "midp-rt.jar";
/// Сколько уровней вверх от каталога бинаря просматривать (хватает на
/// `target/release/arlecchino` → корень дерева).
const EXE_ANCESTORS: usize = 4;

/// Все места, где может лежать ЧЕКАУТ форка, в порядке приоритета:
///
/// 1. `$ARLECCHINO_MIDP_RT_SRC` — явное указание;
/// 2. `<корень движков>/midp-rt` — куда его может положить `arlecchino setup`;
/// 3. рядом с бинарём: `<каталог бинаря>/midp-rt` и тот же подкаталог у каждого
///    предка (так собранный в `target/release/` бинарь находит subtree в дереве);
/// 4. `~/Projects/arlecchino-midp-rt` — отдельный чекаут, запасной путь.
///
/// ⚖️ Ни один из них не втягивается в сборку: это ИСХОДНИКИ отдельного
/// артефакта, который собирает свой `build.sh` и запускает отдельный процесс.
pub fn midp_rt_repos(root: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Ok(v) = std::env::var("ARLECCHINO_MIDP_RT_SRC") {
        if !v.is_empty() {
            push(util::expanduser(&v));
        }
    }
    let engines = root.map(|r| r.to_path_buf()).unwrap_or_else(engines_root_default);
    push(engines.join(REPO_DIRNAME));
    if let Some(dir) = exe_dir() {
        // сам каталог бинаря плюс несколько предков: `target/release/arlecchino`
        // должен доставать до корня дерева, а дальше подниматься незачем.
        let mut cur: Option<&Path> = Some(dir.as_path());
        for _ in 0..EXE_ANCESTORS {
            match cur {
                Some(d) => {
                    push(d.join(REPO_DIRNAME));
                    cur = d.parent();
                }
                None => break,
            }
        }
    }
    push(util::home_dir().join("Projects").join("arlecchino-midp-rt"));
    out
}

/// Первый чекаут форка, в котором действительно есть `build.sh`.
pub fn midp_rt_repo(root: Option<&Path>) -> Option<PathBuf> {
    midp_rt_repos(root).into_iter().find(|r| r.join("build.sh").is_file())
}

/// Все места, где ищется ГОТОВЫЙ `midp-rt.jar` (тем же порядком, что и чекауты:
/// переменная окружения → корень движков → рядом с бинарём → чекауты).
pub fn midp_rt_jar_candidates(root: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Ok(v) = std::env::var("ARLECCHINO_MIDP_RT") {
        if !v.is_empty() {
            push(util::expanduser(&v));
        }
    }
    let engines = root.map(|r| r.to_path_buf()).unwrap_or_else(engines_root_default);
    push(engines.join(JAR_NAME));
    if let Some(dir) = exe_dir() {
        push(dir.join(JAR_NAME));
    }
    for repo in midp_rt_repos(root) {
        push(repo.join("build").join(JAR_NAME));
    }
    out
}

/// Главный класс окна (отдельный процесс) рантайма.
pub const MAIN_WINDOW: &str = "org.microemu.app.Main";
/// Главный класс безголового режима рантайма (там же захват кадров).
pub const MAIN_HEADLESS: &str = "org.microemu.app.Headless";

/// Сообщение «как это починить» на случай отсутствующего midp-rt.jar.
/// `checked` пуст → перечисляем стандартные места сами.
pub fn missing_hint(checked: &[PathBuf], root: Option<&Path>) -> String {
    let mut lines = vec![
        "midp-rt.jar not found — the MIDP runtime (LGPL fork of MicroEmulator) \
         is a separate artifact."
            .to_string(),
    ];
    match midp_rt_repo(root) {
        Some(repo) => lines.push(format!("Build it:  cd {} && ./build.sh", repo.display())),
        None => {
            lines.push(
                "No source checkout of the fork was found either. Put one at \
                 <engines root>/midp-rt, next to the arlecchino binary, or at \
                 ~/Projects/arlecchino-midp-rt — or set $ARLECCHINO_MIDP_RT_SRC."
                    .to_string(),
            );
            lines.push(
                "searched: ".to_string()
                    + &midp_rt_repos(root)
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
            );
        }
    }
    lines.push("or point at an existing jar with --midp-rt PATH / $ARLECCHINO_MIDP_RT.".to_string());
    let checked: Vec<PathBuf> = if checked.is_empty() {
        midp_rt_jar_candidates(root)
    } else {
        checked.to_vec()
    };
    lines.push(
        "checked: ".to_string()
            + &checked.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
    );
    lines.join("\n")
}

/// Найти midp-rt.jar (порядок — в шапке модуля); None, когда его нет.
pub fn find_midp_rt(explicit: Option<&str>, root: Option<&Path>) -> Option<PathBuf> {
    if let Some(e) = explicit {
        let p = util::expanduser(e);
        return if p.exists() { Some(p) } else { None };
    }
    if let Ok(env) = std::env::var("ARLECCHINO_MIDP_RT") {
        let p = util::expanduser(&env);
        if !env.is_empty() && p.exists() {
            return Some(p);
        }
    }
    midp_rt_jar_candidates(root).into_iter().find(|cand| cand.exists())
}

/// midp-rt.jar плюс ASM-jar(ы), нужные его инструментации MIDlet-ов
/// (`lib/asm*.jar` в раскладке форка, либо рядом с самим jar).
pub fn runtime_classpath(midp_rt: &Path) -> Vec<PathBuf> {
    let mut cp = vec![midp_rt.to_path_buf()];
    let dirs = [
        // раскладка чекаута: build/midp-rt.jar рядом с ../lib/asm-*.jar
        midp_rt.parent().and_then(|p| p.parent()).map(|p| p.join("lib")),
        // раскладка установки: midp-rt.jar рядом с lib/asm-*.jar
        midp_rt.parent().map(|p| p.join("lib")),
        // всё вповалку в одном каталоге
        midp_rt.parent().map(|p| p.to_path_buf()),
    ];
    for d in dirs.into_iter().flatten() {
        if !d.is_dir() {
            continue;
        }
        let mut jars: Vec<PathBuf> = std::fs::read_dir(&d)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .map(|n| {
                                let n = n.to_string_lossy();
                                n.starts_with("asm") && n.ends_with(".jar")
                            })
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        jars.sort();
        if !jars.is_empty() {
            cp.extend(jars);
            break;
        }
    }
    cp
}

// ───────────────────── разбор MIDlet-1 (автостарт) ──────────────────────────
/// Минимальный разборщик манифеста JAR / .jad: `Ключ: значение` с продолжениями
/// в стиле RFC (строки манифеста, начинающиеся с одного пробела).
pub fn fold_manifest_lines(text: &str) -> BTreeMap<String, String> {
    let mut attrs: BTreeMap<String, String> = BTreeMap::new();
    let mut key: Option<String> = None;
    for raw in text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)) {
        if raw.trim().is_empty() {
            key = None;
            continue;
        }
        if let Some(tail) = raw.strip_prefix(' ') {
            if let Some(k) = &key {
                let cont = tail.trim_end_matches('\r').to_string();
                attrs.entry(k.clone()).and_modify(|v| v.push_str(&cont));
            }
            continue;
        }
        match raw.find(':') {
            None => {
                key = None;
            }
            Some(i) => {
                let k = raw[..i].trim().to_string();
                let v = raw[i + 1..].trim().to_string();
                attrs.insert(k.clone(), v);
                key = Some(k);
            }
        }
    }
    attrs
}

/// `MIDlet-1 = 'Имя, /icon.png, the.Class'` → `the.Class` (или None).
pub fn midlet1_class(attrs: &BTreeMap<String, String>) -> Option<String> {
    let entry = attrs.get("MIDlet-1")?;
    if entry.is_empty() {
        return None;
    }
    let parts: Vec<&str> = entry.split(',').collect();
    if parts.len() < 3 || parts[2].trim().is_empty() {
        return None;
    }
    Some(parts[2].trim().to_string())
}

/// Вызов «по имени класса» для .jar или .jad:
/// `(midlet_class, app_jar, properties_jad)`.
///
/// `(None, None, None)` = «MIDlet-1 не найден, передаём путь рантайму как есть»
/// (его список лаунчера, Start вручную). На кривых файлах не падает.
pub fn autostart_target(game: &Path) -> (Option<String>, Option<PathBuf>, Option<PathBuf>) {
    let none = (None, None, None);
    let ext = game
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "jad" {
        let text = match util::read_text_lossy(game) {
            Ok(t) => t,
            Err(_) => return none,
        };
        let attrs = fold_manifest_lines(&text);
        let cls = match midlet1_class(&attrs) {
            Some(c) => c,
            None => return none,
        };
        let jar_ref = attrs.get("MIDlet-Jar-URL").cloned().unwrap_or_default();
        if jar_ref.is_empty() || jar_ref.contains("://") {
            return none;
        }
        let jar = util::absolutize(&game.parent().unwrap_or(Path::new(".")).join(&jar_ref));
        if !jar.exists() {
            return none;
        }
        return (Some(cls), Some(jar), Some(game.to_path_buf()));
    }
    let manifest = match util::zip_read_entry(game, "META-INF/MANIFEST.MF") {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => return none,
    };
    match midlet1_class(&fold_manifest_lines(&manifest)) {
        Some(cls) => (Some(cls), Some(game.to_path_buf()), None),
        None => none,
    }
}

// ───────────────────────── командная строка / запуск ─────────────────────────
#[derive(Debug, Clone, Default)]
pub struct WindowOpts {
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub rms_dir: Option<PathBuf>,
    pub instrument: bool,
    pub trace: Option<PathBuf>,
}

fn pathsep_join(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(":")
}

/// Собрать (НЕ запускать) командную строку java для оконной сессии midp-rt.
///
/// `instrument = true` (умолчание) оставляет ASM-9 инструментацию MIDlet
/// включённой — на практике это обязательно: сырой загрузчик режет классы на
/// 16 КБ и превращает отказ в голый `NoClassDefFoundError`.
pub fn build_run_argv(game: &Path, midp_rt: &Path, o: &WindowOpts) -> Vec<String> {
    let java = util::tool("ARLECCHINO_JAVA", "java");
    let mut argv = vec![java];
    if let Some(t) = &o.trace {
        argv.push(format!("-Dmidprt.trace={}", t.display()));
    }
    if let Some(r) = &o.rms_dir {
        // MicroEmulator держит конфиг и файловый RMS под <user.home>/.microemulator/
        // (suite-<name>/<store>.rs). Переопределить user.home — единственный
        // неинвазивный способ перенести это, заодно изолирует прогон от живого
        // ~/.microemulator пользователя.
        argv.push(format!("-Duser.home={}", r.display()));
    }
    argv.push("-cp".into());
    argv.push(pathsep_join(&runtime_classpath(midp_rt)));
    argv.push(MAIN_WINDOW.into());
    // файловый RMS, чтобы сохранения пережили процесс (FileRecordStoreManager)
    argv.push("--rms".into());
    argv.push("file".into());
    if !o.instrument {
        argv.push("--no-instrument".into());
    }
    if let (Some(w), Some(h)) = (o.width, o.height) {
        argv.push("--resizableDevice".into());
        argv.push(w.to_string());
        argv.push(h.to_string());
    }
    // Автостарт (режим имени класса), когда MIDlet-1 восстановим, иначе путь как есть.
    let (cls, app_jar, props_jad) = autostart_target(game);
    match (cls, app_jar) {
        (Some(c), Some(j)) => {
            argv.push("--appclasspath".into());
            argv.push(j.to_string_lossy().into_owned());
            if let Some(p) = props_jad {
                argv.push("--propertiesjad".into());
                argv.push(p.to_string_lossy().into_owned());
            }
            argv.push(c);
        }
        _ => argv.push(game.to_string_lossy().into_owned()),
    }
    argv
}

#[derive(Debug, Clone)]
pub struct HeadlessArgvOpts {
    pub frames_dir: PathBuf,
    pub frames: i64,
    pub period_ms: i64,
    pub boot_wait_ms: i64,
    pub inputs_file: Option<PathBuf>,
    pub rms_dir: Option<PathBuf>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub instrument: bool,
    pub home_dir: Option<PathBuf>,
}

/// Собрать (НЕ запускать) командную строку java для безголовой сессии захвата
/// (режим форка `--frames-dir`: PNG-кадры + index.json).
///
/// Переписывание на автостарт MIDlet-1 происходит ВНУТРИ рантайма (форк сам
/// читает манифест/jad — безголовый список лаунчера никогда не перерисовывается,
/// автостарт там обязателен); путь к игре передаём как есть.
pub fn build_headless_argv(game: &Path, midp_rt: &Path, o: &HeadlessArgvOpts) -> Vec<String> {
    let java = util::tool("ARLECCHINO_JAVA", "java");
    let mut argv = vec![java, "-Djava.awt.headless=true".to_string()];
    if let Some(h) = &o.home_dir {
        argv.push(format!("-Duser.home={}", h.display()));
    }
    argv.push("-cp".into());
    argv.push(pathsep_join(&runtime_classpath(midp_rt)));
    argv.push(MAIN_HEADLESS.into());
    argv.push("--frames-dir".into());
    argv.push(o.frames_dir.to_string_lossy().into_owned());
    argv.push("--frames".into());
    argv.push(o.frames.to_string());
    argv.push("--period-ms".into());
    argv.push(o.period_ms.to_string());
    argv.push("--boot-wait-ms".into());
    argv.push(o.boot_wait_ms.to_string());
    if let Some(f) = &o.inputs_file {
        argv.push("--inputs".into());
        argv.push(f.to_string_lossy().into_owned());
    }
    if let Some(r) = &o.rms_dir {
        argv.push("--rms".into()); // значение-ПУТЬ = плоский файловый каталог RMS
        argv.push(r.to_string_lossy().into_owned());
    }
    if !o.instrument {
        argv.push("--no-instrument".into());
    }
    if let (Some(w), Some(h)) = (o.width, o.height) {
        argv.push("--resizableDevice".into());
        argv.push(w.to_string());
        argv.push(h.to_string());
    }
    argv.push(game.to_string_lossy().into_owned());
    argv
}

/// Безголовый прогон `jar` в midp-rt с захватом кадров — близнец
/// `diffrun::run_headless`, та же форма результата, чтобы difftest мог
/// обращаться с обоими движками одинаково.
///
/// Граница LGPL = граница процесса: отдельный процесс java пишет
/// frame_NNNN.png + index.json, мы лишь порождаем его и читаем файлы обратно.
pub fn run_headless_midp_rt(
    jar: &Path,
    out_dir: &Path,
    inputs: Option<&[InputEvent]>,
    opts: &RunOpts,
    midp_rt: Option<&str>,
    instrument: bool,
) -> Result<HeadlessResult, String> {
    let rt = match find_midp_rt(midp_rt, None) {
        Some(p) => p,
        None => return Err(missing_hint(&[], None)),
    };
    let java = util::tool("ARLECCHINO_JAVA", "java");
    if which(&java).is_none() {
        return Err(format!("java executable not found ({java:?})"));
    }
    let jar = util::absolutize(jar);
    if !jar.exists() {
        return Err(format!("midlet jar not found: {}", jar.display()));
    }

    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let rms = opts.rms_dir.clone().unwrap_or_else(|| out_dir.join("rms"));
    std::fs::create_dir_all(&rms).map_err(|e| e.to_string())?;
    let home = out_dir.join("home");
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;

    let mut inputs_file = None;
    if let Some(ev) = inputs {
        if !ev.is_empty() {
            let f = out_dir.join("inputs.json");
            std::fs::write(&f, serde_json::to_string(ev).unwrap_or_else(|_| "[]".into()))
                .map_err(|e| e.to_string())?;
            inputs_file = Some(f);
        }
    }

    let argv = build_headless_argv(
        &jar,
        &rt,
        &HeadlessArgvOpts {
            frames_dir: out_dir.to_path_buf(),
            frames: opts.max_frames,
            period_ms: opts.frame_period_ms,
            boot_wait_ms: (opts.boot_wait_s * 1000.0) as i64,
            inputs_file,
            rms_dir: Some(rms.clone()),
            width: Some(opts.width),
            height: Some(opts.height),
            instrument,
            home_dir: Some(home),
        },
    );

    let stderr_path = out_dir.join("emulator_stderr.log");
    let timeout =
        opts.boot_wait_s + opts.max_frames as f64 * opts.frame_period_ms as f64 / 1000.0 + 120.0;
    let res = util::run_capture(&argv, Some(Duration::from_secs_f64(timeout)));
    let combined = format!("{}{}", res.stdout, res.stderr);
    let _ = std::fs::write(&stderr_path, &combined);
    if res.timed_out {
        return Err(format!(
            "midp-rt headless run timed out after {timeout:.0}s: {}",
            jar.display()
        ));
    }

    let index = out_dir.join("index.json");
    if !res.success() || !index.exists() {
        let tail: String = combined.chars().rev().take(800).collect::<String>().chars().rev().collect();
        return Err(format!(
            "midp-rt headless run failed (exit {}, index.json {}): {}\n{}",
            res.status.unwrap_or(-1),
            if index.exists() { "present" } else { "MISSING" },
            jar.display(),
            tail
        ));
    }

    let data: serde_json::Value = serde_json::from_str(&util::read_text_lossy(&index).unwrap_or_default())
        .map_err(|e| format!("bad index.json: {e}"))?;
    let mut result = HeadlessResult {
        frames: Vec::new(),
        stderr_log: stderr_path.to_string_lossy().into_owned(),
        rms_dir: rms.to_string_lossy().into_owned(),
    };
    if let Some(arr) = data.get("frames").and_then(|f| f.as_array()) {
        for f in arr {
            result.frames.push(FrameRecord {
                index: f.get("index").and_then(|v| v.as_i64()).unwrap_or(0),
                file: f.get("file").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                sha256: f.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                width: f.get("width").and_then(|v| v.as_i64()).unwrap_or(0),
                height: f.get("height").and_then(|v| v.as_i64()).unwrap_or(0),
                blank: f.get("blank").and_then(|v| v.as_bool()).unwrap_or(false),
            });
        }
    }
    Ok(result)
}

/// Породить отдельный процесс java (граница LGPL) и дождаться его.
///
/// Мост кадров/ввода (allods-shell): когда задан `$ALLODS_SHELL_SOCKET`,
/// `ShellFrameBridge` форка midp-rt гонит экран MIDP во внешний Rust-рендер по
/// этому Unix-сокету и читает оттуда события клавиатуры. Мост сам читает
/// переменную окружения (наследует наше окружение), мы лишь показываем её
/// пользователю; `allods-renderer` надо запустить заранее.
pub fn run_midlet(game: &Path, midp_rt: &Path, o: &WindowOpts) -> i32 {
    let argv = build_run_argv(game, midp_rt, o);
    if argv.last().map(|s| s == &game.to_string_lossy()).unwrap_or(false) {
        println!("  (no MIDlet-1 in manifest/jad — opening the runtime Launcher; press Start there)");
    }
    if let Ok(sock) = std::env::var("ALLODS_SHELL_SOCKET") {
        if !sock.is_empty() {
            println!("  allods-shell мост включён -> {sock} (запусти allods-renderer заранее)");
        }
    }
    println!("  launching: {}", argv.join(" "));
    util::run_inherit(&argv)
}

/// Прогнать `./build.sh` форка (его собственный репозиторий — сюда ничего не
/// вкладывается). None при успехе, человекочитаемая строка ошибки иначе.
pub fn build_midp_rt(repo: Option<&Path>, root: Option<&Path>) -> Option<String> {
    let repo = match repo.map(|p| p.to_path_buf()).or_else(|| midp_rt_repo(root)) {
        Some(r) => r,
        None => {
            return Some(format!(
                "no fork checkout found (searched: {})",
                midp_rt_repos(root)
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    };
    let script = repo.join("build.sh");
    if !script.exists() {
        return Some(format!("no fork checkout at {}", repo.display()));
    }
    let out = std::process::Command::new("bash")
        .arg(&script)
        .current_dir(&repo)
        .output();
    match out {
        Err(e) => Some(format!("build.sh did not run: {e}")),
        Ok(o) if !o.status.success() => {
            let text = if o.stderr.is_empty() { &o.stdout } else { &o.stderr };
            let text = String::from_utf8_lossy(text);
            let tail: Vec<&str> = text.trim().lines().rev().take(3).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            Some(format!(
                "build.sh exit {}: {}",
                o.status.code().unwrap_or(-1),
                tail.join(" | ")
            ))
        }
        Ok(_) => None,
    }
}

/// Шаг `arlecchino setup`: найти midp-rt.jar; если его нет, но чекаут форка
/// присутствует — СОБРАТЬ через его ./build.sh (отдельный репозиторий/процесс).
/// Мягко: отказ — это строка статуса с командой-починкой, а не паника.
pub fn setup_midp_rt(root: Option<&Path>) -> String {
    let mut jar = find_midp_rt(None, root);
    let mut built = String::new();
    if jar.is_none() {
        let err = build_midp_rt(None, root);
        jar = if err.is_none() { find_midp_rt(None, root) } else { None };
        if jar.is_none() {
            return format!(
                "not found ({})\n{}",
                err.unwrap_or_else(|| "build.sh produced no jar".into()),
                missing_hint(&[], root)
            );
        }
        built = "built via build.sh; ".into();
    }
    let jar = jar.unwrap();
    // Поставить рантайм в корень движков: `<root>/midp-rt.jar` + `<root>/lib/asm-*.jar`.
    // После этого чекаут исходников больше не нужен ни для чего, кроме пересборки.
    let mut installed = String::new();
    let root = root.map(|r| r.to_path_buf()).unwrap_or_else(engines_root_default);
    let target = root.join(JAR_NAME);
    let jar = if jar != target {
        let asm_src = runtime_classpath(&jar);
        let libdir = root.join("lib");
        let ok = std::fs::create_dir_all(&libdir).is_ok()
            && std::fs::copy(&jar, &target).is_ok()
            && asm_src.iter().skip(1).all(|a| {
                a.file_name()
                    .map(|n| std::fs::copy(a, libdir.join(n)).is_ok())
                    .unwrap_or(false)
            });
        if ok {
            installed = "installed -> ".to_string();
            target
        } else {
            jar
        }
    } else {
        jar
    };
    let asm: Vec<String> = runtime_classpath(&jar)
        .into_iter()
        .skip(1)
        .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    format!(
        "ok ({}{}{}; asm: {})",
        built,
        installed,
        jar.display(),
        if asm.is_empty() {
            "NOT FOUND — instrumentation needs lib/asm*.jar".to_string()
        } else {
            asm.join(", ")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_folding_and_midlet1() {
        let m = "Manifest-Version: 1.0\nMIDlet-1: Game, /i.png, com.x.\n Main\nMIDlet-Name: G\n";
        let a = fold_manifest_lines(m);
        assert_eq!(a.get("MIDlet-1").map(String::as_str), Some("Game, /i.png, com.x.Main"));
        assert_eq!(midlet1_class(&a).as_deref(), Some("com.x.Main"));
        let bad = fold_manifest_lines("MIDlet-1: Game, /i.png\n");
        assert_eq!(midlet1_class(&bad), None);
    }
}
