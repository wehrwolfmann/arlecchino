//! Tiny RU/EN localisation layer for user-facing display text (GUI + CLI framing).
//!
//! Language selection:
//!   1. `ARLECCHINO_LANG=ru|en` override (for testing);
//!   2. otherwise system locale via `LC_ALL` -> `LC_MESSAGES` -> `LANG`
//!      (first non-empty; starts with `ru` -> Russian);
//!   3. English is the default/fallback.
//!
//! Only human-facing strings are routed here.  Technical output (report.json /
//! report.md content, merge/progress logs, file paths, CLI flag names) stays in a
//! stable form so external formats and Python-parity are preserved.

use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Ru,
}

/// Map a locale string to a UI language: `ru*` (case-insensitive) -> Russian, else English.
pub fn lang_from_locale(s: &str) -> Lang {
    if s.trim().to_lowercase().starts_with("ru") {
        Lang::Ru
    } else {
        Lang::En
    }
}

/// Resolve the UI language from the override env then the system locale.
pub fn detect_lang() -> Lang {
    if let Ok(v) = std::env::var("ARLECCHINO_LANG") {
        if !v.trim().is_empty() {
            return lang_from_locale(&v);
        }
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return lang_from_locale(&v);
            }
        }
    }
    Lang::En
}

/// Cached active language (resolved once at first use).
pub fn lang() -> Lang {
    static CACHE: OnceLock<Lang> = OnceLock::new();
    *CACHE.get_or_init(detect_lang)
}

// ── string tables (identical key sets — enforced by a test) ──────────────────
const EN_PAIRS: &[(&str, &str)] = &[
    // CLI framing / errors
    ("jar_not_found", "jar not found: {0}"),
    ("no_working_engines", "No working engines (run `arlecchino setup`)."),
    ("freej2me_not_found", "FreeJ2ME not found — run `arlecchino setup` or pass --freej2me PATH"),
    ("setup_tip", "Tip: copy the bundled engines.default.toml to ~/.arlecchino/engines.toml to customise."),
    ("done_merged", "Done. merged -> {0}"),
    ("reports_at", "Reports: {0}, {1}"),
    // GUI — window / tabs
    ("app_title", "Arlecchino — meta-decompiler & J2ME toolkit"),
    ("tab_decompile", "Decompile"),
    ("tab_emulate", "Emulate"),
    ("tab_setup", "Setup"),
    ("tab_about", "About"),
    // GUI — decompile controls
    ("input_jar", "Input .jar:"),
    ("output_dir", "Output dir:"),
    ("browse", "Browse…"),
    ("opt_deobf", "Deobfuscate (readability pass)"),
    ("opt_per_method", "Per-method merge (experimental)"),
    ("opt_no_autofix", "Disable auto-fix"),
    ("opt_no_roundtrip", "Disable round-trip check"),
    ("opt_no_rename", "Disable bytecode member-renamer"),
    ("run_decompile", "Run decompile"),
    ("normalize_only", "Normalize jadx tree"),
    ("pick_jadx_tree", "jadx source dir to normalize:"),
    // GUI — emulate
    ("emu_jar", "MIDlet .jar:"),
    ("emu_width", "Width"),
    ("emu_height", "Height"),
    ("emu_scale", "Scale"),
    ("run_emulate", "Launch in FreeJ2ME"),
    // GUI — setup
    ("setup_desc", "Download decompiler engines + FreeJ2ME into ~/.arlecchino/engines."),
    ("run_setup", "Run setup"),
    ("setup_no_freej2me", "Skip FreeJ2ME"),
    // GUI — log / status
    ("log_output", "Output"),
    ("clear_log", "Clear"),
    ("status_idle", "Idle"),
    ("status_running", "Running…"),
    ("status_done", "Done"),
    ("status_failed", "Failed"),
    ("busy_wait", "A task is already running."),
    ("pick_jar_first", "Select an input .jar first."),
    // GUI — launcher / uninstall
    ("menu_app", "Application"),
    ("add_launcher", "Add to application menu"),
    ("launcher_added", "Launcher installed: {0}"),
    ("uninstall", "Uninstall…"),
    ("uninstall_confirm_title", "Uninstall Arlecchino?"),
    ("uninstall_confirm_body", "Really delete the installed binary, launcher and icon? This cannot be undone."),
    ("confirm_yes", "Yes, delete"),
    ("confirm_no", "Cancel"),
    ("uninstall_done", "Uninstalled. Removed {0} item(s)."),
    // GUI — about
    ("about_body", "Meta-decompiler and J2ME port toolkit. Runs several Java decompilers, merges the best-of per class, auto-fixes artifacts and round-trips to bytecode. Apache-2.0."),
    // Tray
    ("tray_open", "Open panel"),
    ("tray_quit", "Quit"),
    ("tray_status", "Status: {0}"),
];

const RU_PAIRS: &[(&str, &str)] = &[
    // CLI framing / errors
    ("jar_not_found", "jar не найден: {0}"),
    ("no_working_engines", "Нет рабочих движков (запустите `arlecchino setup`)."),
    ("freej2me_not_found", "FreeJ2ME не найден — запустите `arlecchino setup` или укажите --freej2me PATH"),
    ("setup_tip", "Совет: скопируйте встроенный engines.default.toml в ~/.arlecchino/engines.toml для настройки."),
    ("done_merged", "Готово. merged -> {0}"),
    ("reports_at", "Отчёты: {0}, {1}"),
    // GUI — window / tabs
    ("app_title", "Arlecchino — мета-декомпилятор и набор для J2ME"),
    ("tab_decompile", "Декомпиляция"),
    ("tab_emulate", "Эмуляция"),
    ("tab_setup", "Установка"),
    ("tab_about", "О программе"),
    // GUI — decompile controls
    ("input_jar", "Входной .jar:"),
    ("output_dir", "Каталог вывода:"),
    ("browse", "Обзор…"),
    ("opt_deobf", "Деобфускация (для читаемости)"),
    ("opt_per_method", "Слияние по методам (экспериментально)"),
    ("opt_no_autofix", "Отключить авто-фикс"),
    ("opt_no_roundtrip", "Отключить round-trip проверку"),
    ("opt_no_rename", "Отключить байткод-переименователь членов"),
    ("run_decompile", "Запустить декомпиляцию"),
    ("normalize_only", "Нормализовать дерево jadx"),
    ("pick_jadx_tree", "Каталог исходников jadx для нормализации:"),
    // GUI — emulate
    ("emu_jar", "MIDlet .jar:"),
    ("emu_width", "Ширина"),
    ("emu_height", "Высота"),
    ("emu_scale", "Масштаб"),
    ("run_emulate", "Запустить в FreeJ2ME"),
    // GUI — setup
    ("setup_desc", "Скачать движки-декомпиляторы + FreeJ2ME в ~/.arlecchino/engines."),
    ("run_setup", "Запустить установку"),
    ("setup_no_freej2me", "Без FreeJ2ME"),
    // GUI — log / status
    ("log_output", "Вывод"),
    ("clear_log", "Очистить"),
    ("status_idle", "Готов"),
    ("status_running", "Выполняется…"),
    ("status_done", "Готово"),
    ("status_failed", "Ошибка"),
    ("busy_wait", "Задача уже выполняется."),
    ("pick_jar_first", "Сначала выберите входной .jar."),
    // GUI — launcher / uninstall
    ("menu_app", "Приложение"),
    ("add_launcher", "Добавить в меню приложений"),
    ("launcher_added", "Ярлык установлен: {0}"),
    ("uninstall", "Удалить…"),
    ("uninstall_confirm_title", "Удалить Arlecchino?"),
    ("uninstall_confirm_body", "Точно удалить установленный бинарь, ярлык и иконку? Действие необратимо."),
    ("confirm_yes", "Да, удалить"),
    ("confirm_no", "Отмена"),
    ("uninstall_done", "Удалено. Убрано элементов: {0}."),
    // GUI — about
    ("about_body", "Мета-декомпилятор и набор для порта J2ME. Запускает несколько Java-декомпиляторов, собирает лучшее по классам, чинит артефакты и делает round-trip в байткод. Apache-2.0."),
    // Tray
    ("tray_open", "Открыть панель"),
    ("tray_quit", "Выход"),
    ("tray_status", "Статус: {0}"),
];

static EN: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| EN_PAIRS.iter().copied().collect());
static RU: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| RU_PAIRS.iter().copied().collect());

/// Translate a key for the active language (falls back to English, then the key).
pub fn t(key: &str) -> &'static str {
    let table = match lang() {
        Lang::Ru => &*RU,
        Lang::En => &*EN,
    };
    table.get(key).or_else(|| EN.get(key)).copied().unwrap_or("??")
}

/// Translate + substitute positional `{0}`, `{1}`, … placeholders.
pub fn tf(key: &str, args: &[&str]) -> String {
    let mut s = t(key).to_string();
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), a);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_ru_variants() {
        assert_eq!(lang_from_locale("ru_RU.UTF-8"), Lang::Ru);
        assert_eq!(lang_from_locale("RU"), Lang::Ru);
        assert_eq!(lang_from_locale("ru"), Lang::Ru);
    }

    #[test]
    fn locale_non_ru_and_empty_default_english() {
        assert_eq!(lang_from_locale("en_US.UTF-8"), Lang::En);
        assert_eq!(lang_from_locale("de_DE"), Lang::En);
        assert_eq!(lang_from_locale(""), Lang::En);
        assert_eq!(lang_from_locale("   "), Lang::En);
    }

    #[test]
    fn tables_have_identical_keys() {
        let en: std::collections::BTreeSet<_> = EN_PAIRS.iter().map(|(k, _)| *k).collect();
        let ru: std::collections::BTreeSet<_> = RU_PAIRS.iter().map(|(k, _)| *k).collect();
        assert_eq!(en, ru, "EN and RU key sets differ — untranslated string(s)");
        // no duplicate keys
        assert_eq!(en.len(), EN_PAIRS.len(), "duplicate key in EN table");
        assert_eq!(ru.len(), RU_PAIRS.len(), "duplicate key in RU table");
    }

    #[test]
    fn tf_substitutes_positional() {
        // uses EN fallback regardless of host locale
        std::env::set_var("ARLECCHINO_LANG", "en");
        assert_eq!(tf("jar_not_found", &["/x/y.jar"]), "jar not found: /x/y.jar");
    }
}
