//! Tiny EN/RU/DE localisation layer for user-facing display text (GUI + CLI framing).
//!
//! Language selection:
//!   1. `ARLECCHINO_LANG=en|ru|de` override (for testing and for CLI subprocesses);
//!   2. the stored user preference (`~/.config/arlecchino/settings.toml`), unless it
//!      says `system`;
//!   3. system locale via `LC_ALL` -> `LC_MESSAGES` -> `LANG` (first non-empty;
//!      `ru*` -> Russian, `de*` -> German);
//!   4. English is the default/fallback.
//!
//! The active language lives in an atomic slot rather than a write-once cache, so the
//! GUI can switch languages while running: every caller goes through `t()`/`tf()` on
//! each frame and simply picks up the new table.
//!
//! Only human-facing strings are routed here.  Technical output (report.json /
//! report.md content, merge/progress logs, file paths, CLI flag names) stays in a
//! stable form so external formats and Python-parity are preserved.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::LazyLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Ru,
    De,
}

impl Lang {
    /// Every language the UI can display, in menu order.
    pub const ALL: &'static [Lang] = &[Lang::En, Lang::Ru, Lang::De];

    /// Stable short code used in settings files and in `ARLECCHINO_LANG`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
            Lang::De => "de",
        }
    }

    /// The language's own name, never translated — so a user can find their language
    /// even while the UI is in a language they do not read.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
            Lang::De => "Deutsch",
        }
    }

    fn slot(self) -> u8 {
        match self {
            Lang::En => 1,
            Lang::Ru => 2,
            Lang::De => 3,
        }
    }

    fn from_slot(v: u8) -> Option<Lang> {
        match v {
            1 => Some(Lang::En),
            2 => Some(Lang::Ru),
            3 => Some(Lang::De),
            _ => None,
        }
    }
}

/// What the user chose in the settings: follow the system, or a fixed language.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LangPref {
    #[default]
    System,
    Fixed(Lang),
}

impl LangPref {
    pub fn code(self) -> &'static str {
        match self {
            LangPref::System => "system",
            LangPref::Fixed(l) => l.code(),
        }
    }

    /// Parse a settings value; anything unknown (or absent) means "follow the system".
    pub fn from_code(s: &str) -> LangPref {
        match s.trim().to_lowercase().as_str() {
            "en" => LangPref::Fixed(Lang::En),
            "ru" => LangPref::Fixed(Lang::Ru),
            "de" => LangPref::Fixed(Lang::De),
            _ => LangPref::System,
        }
    }
}

/// Map a locale string to a UI language: `ru*` -> Russian, `de*` -> German, else
/// English (case-insensitive).
pub fn lang_from_locale(s: &str) -> Lang {
    let s = s.trim().to_lowercase();
    if s.starts_with("ru") {
        Lang::Ru
    } else if s.starts_with("de") {
        Lang::De
    } else {
        Lang::En
    }
}

/// The language the system locale asks for (`LC_ALL` -> `LC_MESSAGES` -> `LANG`).
pub fn system_lang() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return lang_from_locale(&v);
            }
        }
    }
    Lang::En
}

/// Resolve the UI language: env override, then the stored preference, then the system.
pub fn detect_lang() -> Lang {
    if let Ok(v) = std::env::var("ARLECCHINO_LANG") {
        if !v.trim().is_empty() {
            return lang_from_locale(&v);
        }
    }
    match load_pref() {
        LangPref::Fixed(l) => l,
        LangPref::System => system_lang(),
    }
}

// ── stored preference ────────────────────────────────────────────────────────

/// Where the GUI keeps its settings (the directory the uninstaller already knows).
pub fn settings_path() -> PathBuf {
    crate::util::home_dir()
        .join(".config")
        .join("arlecchino")
        .join("settings.toml")
}

#[derive(serde::Deserialize, Default)]
struct UiSettings {
    #[serde(default)]
    language: Option<String>,
}

/// Read the language preference out of a settings file (missing/broken -> System).
pub fn load_pref_from(path: &Path) -> LangPref {
    let Ok(text) = std::fs::read_to_string(path) else {
        return LangPref::System;
    };
    let parsed: UiSettings = toml::from_str(&text).unwrap_or_default();
    parsed
        .language
        .map(|s| LangPref::from_code(&s))
        .unwrap_or_default()
}

/// Read the language preference from the default settings path.
pub fn load_pref() -> LangPref {
    load_pref_from(&settings_path())
}

/// Render a settings file with the given preference (whole file — it holds one key).
fn render_settings(pref: LangPref) -> String {
    format!(
        "# Arlecchino UI settings.\n\n# Interface language: system | en | ru | de\nlanguage = \"{}\"\n",
        pref.code()
    )
}

/// Write the language preference to a settings file, creating the directory.
pub fn save_pref_to(path: &Path, pref: LangPref) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, render_settings(pref)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Apply a preference to the running process and store it for the next start.
pub fn set_pref(pref: LangPref) -> Result<(), String> {
    apply_pref(pref);
    save_pref_to(&settings_path(), pref)
}

/// Switch the language of the running process without touching the settings file.
pub fn apply_pref(pref: LangPref) {
    let l = match pref {
        LangPref::System => system_lang(),
        LangPref::Fixed(l) => l,
    };
    ACTIVE.store(l.slot(), Ordering::Relaxed);
}

// ── active language ──────────────────────────────────────────────────────────

/// Active language slot; 0 = not resolved yet.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

/// The active language (resolved on first use, then switchable at runtime).
pub fn lang() -> Lang {
    match Lang::from_slot(ACTIVE.load(Ordering::Relaxed)) {
        Some(l) => l,
        None => {
            let l = detect_lang();
            ACTIVE.store(l.slot(), Ordering::Relaxed);
            l
        }
    }
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
    ("tab_settings", "Settings"),
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
    // GUI — settings
    ("settings_language", "Language"),
    ("settings_language_system", "System"),
    ("settings_language_hint", "Applied at once and kept for the next start."),
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
    ("tab_settings", "Настройки"),
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
    // GUI — settings
    ("settings_language", "Язык"),
    ("settings_language_system", "Системный"),
    ("settings_language_hint", "Применяется сразу и сохраняется до следующего запуска."),
    // GUI — about
    ("about_body", "Мета-декомпилятор и набор для порта J2ME. Запускает несколько Java-декомпиляторов, собирает лучшее по классам, чинит артефакты и делает round-trip в байткод. Apache-2.0."),
    // Tray
    ("tray_open", "Открыть панель"),
    ("tray_quit", "Выход"),
    ("tray_status", "Статус: {0}"),
];

const DE_PAIRS: &[(&str, &str)] = &[
    // CLI framing / errors
    ("jar_not_found", "JAR nicht gefunden: {0}"),
    ("no_working_engines", "Keine funktionsfähigen Engines (bitte `arlecchino setup` ausführen)."),
    ("freej2me_not_found", "FreeJ2ME nicht gefunden — führen Sie `arlecchino setup` aus oder geben Sie --freej2me PATH an"),
    ("setup_tip", "Tipp: Kopieren Sie die mitgelieferte engines.default.toml nach ~/.arlecchino/engines.toml, um sie anzupassen."),
    ("done_merged", "Fertig. merged -> {0}"),
    ("reports_at", "Berichte: {0}, {1}"),
    // GUI — window / tabs
    ("app_title", "Arlecchino — Meta-Decompiler und J2ME-Toolkit"),
    ("tab_decompile", "Dekompilieren"),
    ("tab_emulate", "Emulieren"),
    ("tab_setup", "Einrichten"),
    ("tab_settings", "Einstellungen"),
    ("tab_about", "Über"),
    // GUI — decompile controls
    ("input_jar", "Eingabe-JAR:"),
    ("output_dir", "Ausgabeordner:"),
    ("browse", "Durchsuchen…"),
    ("opt_deobf", "Deobfuskieren (bessere Lesbarkeit)"),
    ("opt_per_method", "Zusammenführen pro Methode (experimentell)"),
    ("opt_no_autofix", "Auto-Korrektur abschalten"),
    ("opt_no_roundtrip", "Round-trip-Prüfung abschalten"),
    ("opt_no_rename", "Umbenennen von Bytecode-Membern abschalten"),
    ("run_decompile", "Dekompilierung starten"),
    ("normalize_only", "jadx-Baum normalisieren"),
    ("pick_jadx_tree", "jadx-Quellordner zum Normalisieren:"),
    // GUI — emulate
    ("emu_jar", "MIDlet-JAR:"),
    ("emu_width", "Breite"),
    ("emu_height", "Höhe"),
    ("emu_scale", "Skalierung"),
    ("run_emulate", "In FreeJ2ME starten"),
    // GUI — setup
    ("setup_desc", "Decompiler-Engines und FreeJ2ME nach ~/.arlecchino/engines herunterladen."),
    ("run_setup", "Einrichtung starten"),
    ("setup_no_freej2me", "Ohne FreeJ2ME"),
    // GUI — log / status
    ("log_output", "Ausgabe"),
    ("clear_log", "Leeren"),
    ("status_idle", "Bereit"),
    ("status_running", "Läuft…"),
    ("status_done", "Fertig"),
    ("status_failed", "Fehlgeschlagen"),
    ("busy_wait", "Es läuft bereits eine Aufgabe."),
    ("pick_jar_first", "Wählen Sie zuerst eine Eingabe-JAR aus."),
    // GUI — launcher / uninstall
    ("menu_app", "Anwendung"),
    ("add_launcher", "Zum Anwendungsmenü hinzufügen"),
    ("launcher_added", "Verknüpfung erstellt: {0}"),
    ("uninstall", "Deinstallieren…"),
    ("uninstall_confirm_title", "Arlecchino deinstallieren?"),
    ("uninstall_confirm_body", "Programmdatei, Verknüpfung und Symbol wirklich löschen? Das lässt sich nicht rückgängig machen."),
    ("confirm_yes", "Ja, löschen"),
    ("confirm_no", "Abbrechen"),
    ("uninstall_done", "Deinstalliert. {0} Element(e) entfernt."),
    // GUI — settings
    ("settings_language", "Sprache"),
    ("settings_language_system", "System"),
    ("settings_language_hint", "Wird sofort übernommen und für den nächsten Start gespeichert."),
    // GUI — about
    ("about_body", "Meta-Decompiler und Toolkit für J2ME-Portierungen. Führt mehrere Java-Decompiler aus, übernimmt je Klasse das beste Ergebnis, korrigiert Artefakte automatisch und prüft das Ergebnis per Round-trip gegen den Bytecode. Apache-2.0."),
    // Tray
    ("tray_open", "Panel öffnen"),
    ("tray_quit", "Beenden"),
    ("tray_status", "Status: {0}"),
];

static EN: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| EN_PAIRS.iter().copied().collect());
static RU: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| RU_PAIRS.iter().copied().collect());
static DE: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| DE_PAIRS.iter().copied().collect());

/// Translate a key for the active language (falls back to English, then the key).
pub fn t(key: &str) -> &'static str {
    let table = match lang() {
        Lang::Ru => &*RU,
        Lang::De => &*DE,
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
    use std::sync::Mutex;

    /// Tests that flip the process-wide active language must not overlap.
    static ACTIVE_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        ACTIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn locale_ru_variants() {
        assert_eq!(lang_from_locale("ru_RU.UTF-8"), Lang::Ru);
        assert_eq!(lang_from_locale("RU"), Lang::Ru);
        assert_eq!(lang_from_locale("ru"), Lang::Ru);
    }

    #[test]
    fn locale_de_variants() {
        assert_eq!(lang_from_locale("de_DE.UTF-8"), Lang::De);
        assert_eq!(lang_from_locale("de_AT"), Lang::De);
        assert_eq!(lang_from_locale("DE"), Lang::De);
        assert_eq!(lang_from_locale("de"), Lang::De);
    }

    #[test]
    fn locale_other_and_empty_default_english() {
        assert_eq!(lang_from_locale("en_US.UTF-8"), Lang::En);
        assert_eq!(lang_from_locale("fr_FR.UTF-8"), Lang::En);
        assert_eq!(lang_from_locale(""), Lang::En);
        assert_eq!(lang_from_locale("   "), Lang::En);
    }

    #[test]
    fn tables_have_identical_keys() {
        let en: std::collections::BTreeSet<_> = EN_PAIRS.iter().map(|(k, _)| *k).collect();
        let ru: std::collections::BTreeSet<_> = RU_PAIRS.iter().map(|(k, _)| *k).collect();
        let de: std::collections::BTreeSet<_> = DE_PAIRS.iter().map(|(k, _)| *k).collect();
        assert_eq!(en, ru, "EN and RU key sets differ — untranslated string(s)");
        assert_eq!(en, de, "EN and DE key sets differ — untranslated string(s)");
        // no duplicate keys
        assert_eq!(en.len(), EN_PAIRS.len(), "duplicate key in EN table");
        assert_eq!(ru.len(), RU_PAIRS.len(), "duplicate key in RU table");
        assert_eq!(de.len(), DE_PAIRS.len(), "duplicate key in DE table");
    }

    #[test]
    fn no_table_entry_is_empty() {
        for (table, pairs) in [("EN", EN_PAIRS), ("RU", RU_PAIRS), ("DE", DE_PAIRS)] {
            for (k, v) in pairs {
                assert!(!v.trim().is_empty(), "{table}/{k} is empty");
            }
        }
    }

    #[test]
    fn tf_substitutes_positional() {
        let _g = guard();
        apply_pref(LangPref::Fixed(Lang::En));
        assert_eq!(tf("jar_not_found", &["/x/y.jar"]), "jar not found: /x/y.jar");
    }

    #[test]
    fn switching_language_is_live() {
        let _g = guard();
        apply_pref(LangPref::Fixed(Lang::De));
        assert_eq!(t("tab_settings"), "Einstellungen");
        assert_eq!(lang(), Lang::De);
        apply_pref(LangPref::Fixed(Lang::Ru));
        assert_eq!(t("tab_settings"), "Настройки");
        apply_pref(LangPref::Fixed(Lang::En));
        assert_eq!(t("tab_settings"), "Settings");
    }

    #[test]
    fn pref_codes_round_trip() {
        for p in [
            LangPref::System,
            LangPref::Fixed(Lang::En),
            LangPref::Fixed(Lang::Ru),
            LangPref::Fixed(Lang::De),
        ] {
            assert_eq!(LangPref::from_code(p.code()), p);
        }
        assert_eq!(LangPref::from_code("klingon"), LangPref::System);
        assert_eq!(LangPref::from_code(""), LangPref::System);
        assert_eq!(LangPref::from_code("DE"), LangPref::Fixed(Lang::De));
    }

    #[test]
    fn pref_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("arlecchino-i18n-{}", std::process::id()));
        let path = dir.join("settings.toml");
        let _ = std::fs::remove_file(&path);
        // absent file -> follow the system
        assert_eq!(load_pref_from(&path), LangPref::System);
        for p in [
            LangPref::Fixed(Lang::De),
            LangPref::Fixed(Lang::Ru),
            LangPref::System,
        ] {
            save_pref_to(&path, p).expect("write settings");
            assert_eq!(load_pref_from(&path), p);
        }
        // unreadable content must not panic and must fall back to the system
        std::fs::write(&path, "this is not toml =\n").unwrap();
        assert_eq!(load_pref_from(&path), LangPref::System);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_path_is_under_config_dir() {
        let p = settings_path();
        assert!(p.ends_with(".config/arlecchino/settings.toml"), "{}", p.display());
    }
}
