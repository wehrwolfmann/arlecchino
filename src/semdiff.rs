//! Семантическая round-trip-сверка (этап Э3) — слой ВЕРИФИКАЦИИ / ФЛАГОВ.
//!
//! Порт `arlecchino/semdiff.py`.
//!
//! Обычная round-trip-проверка (`roundtrip`) доказывает только ПОКРЫТИЕ (каждый
//! исходный класс дал скомпилированный) и ГРУБЫЕ СЧЁТЧИКИ (число членов/методов
//! по `javap -p`). Автофикс может изменить СЕМАНТИКУ, не тронув эти счётчики —
//! прежде всего три автофикса, переписывающие конструкции рядом с поведением:
//!
//!   * `scratch_temp_dead_array_type` — расширяет объявление локальной до `Object`
//!     (может перевернуть тип элемента `new int[]` против `new char[]`),
//!   * `fold_interface_static_init`   — переносит блок `static {}` в инициализаторы
//!     полей (переупорядочивает/перекодирует `<clinit>`),
//!   * `const_interface_to_class`     — превращает `interface` в `class`
//!     (перещёлкивает места вызова с `invokeinterface` на `invokevirtual`).
//!
//! Модуль дизассемблирует поMETОДНО исходный `.class` (из переименованного jar —
//! канонический API, которым и так пользуется весь конвейер) и пересобранный
//! `.class`, НОРМАЛИЗУЕТ косметические различия, законно возникающие при
//! round-trip декомпилятор+компилятор, и ПОМЕЧАЕТ методы, чьё нормализованное
//! множество символьных ссылок различается по существу. Правила слияния он не
//! меняет никогда.
//!
//! Почему МНОЖЕСТВО символьных ссылок, а не точный поток опкодов?
//! -------------------------------------------------------------
//! Строгое сравнение потока `javap -c` слишком жёстко: javac перекодирует
//! декомпилированный исходник с другой полярностью ветвлений
//! (`if_icmpge` ↔ `if_icmplt; return`), другими номерами слотов локальных,
//! другими идиомами `dup`/`x++` и опускает перезагрузки — ничего из этого не
//! меняет поведение. Калибровка на заведомо хорошем слиянии Аллодов 712→2
//! (30+ чистых классов) показала сотни ложных построчных различий.
//!
//! Устойчивый сигнал БЕЗ ложных срабатываний (откалиброван на Аллодах) — это
//! множество различных символьных операндов каждого метода, с нормализацией:
//!
//!   N1. Сравниваем метод к методу по нормализованной сигнатуре (модификаторы
//!       доступа выброшены); метод, который есть только с одной стороны, помечается.
//!   N2. Ссылки на поля/методы держим как `(вид, имя, дескриптор)`, но ВЫБРАСЫВАЕМ
//!       класс-владелец — javac переразрешает член на тот супертип, который его
//!       ОБЪЯВЛЯЕТ (`Field aa.H:I` против `Field j.H:I`); тот же член, косметика.
//!   N3. Вид вызова оставляем различимым (`Method` против `InterfaceMethod`), чтобы
//!       переворот interface→class всё ещё ловился, а владелец при этом выброшен.
//!   N4. Сводим `StringBuffer` ↔ `StringBuilder` — декомпилятор выдаёт современный
//!       `StringBuilder` для конкатенации `+`, которую исходный обфусцированный
//!       байткод собирал через `StringBuffer`; поведение то же.
//!   N5. Ловим создание массива примитивов `newarray <тип>` — ровно то, что может
//!       изменить `scratch_temp_dead_array_type`.
//!   N6. ИСКЛЮЧАЕМ `checkcast` / `instanceof` — декомпиляторы вставляют избыточные
//!       приведения, которые оригинал проверял статически.
//!   N7. ИСКЛЮЧАЕМ синтетический `getClass():Class` — javac выдаёт его как
//!       проверку на null, которую декомпилированный исходник не воспроизводит.
//!   N8. Примиряем переименованные члены через `mapping.txt` (обратная карта
//!       новоеИмя→староеИмя), чтобы переименованный член не помечался как
//!       семантическое изменение.
//!
//! Всё деградирует мягко: нет `javap` — модуль ничего не делает (`available=false`);
//! класс, который не пересобрался, пропускается (сообщается, а не помечается).

use crate::util;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Ссылка на член/тип в нормализованном виде.
pub type Ref = (String, String, String);

fn re_offset() -> Regex {
    Regex::new(r"^\s*\d+:\s").unwrap()
}
fn re_lead() -> Regex {
    Regex::new(r"^\d+:\s*").unwrap()
}
fn re_ws() -> Regex {
    Regex::new(r"\s+").unwrap()
}
fn re_access() -> Regex {
    Regex::new(r"\b(public|private|protected|final|static|abstract|synchronized|native|strictfp)\b")
        .unwrap()
}

/// опкоды, чей операнд называет поле/метод/тип (семантическое ядро, которое храним)
const REF_OPS: &[&str] = &[
    "getfield",
    "putfield",
    "getstatic",
    "putstatic",
    "invokevirtual",
    "invokespecial",
    "invokestatic",
    "invokeinterface",
    "invokedynamic",
    "new",
    "anewarray",
    "multianewarray",
];

/// синтетическая проверка на null, которую javac выдаёт, а декомпилированный
/// исходник не воспроизводит (N7)
fn is_getclass(r: &Ref) -> bool {
    r.0 == "Method" && r.1 == "getClass" && r.2 == "()Ljava/lang/Class;"
}

// ───────────────────────────── mapping (N8) ─────────────────────────────────
/// Построить {новоеИмя: староеИмя} из `mapping.txt` переименователя.
///
/// Строки выглядят как `owner.oldName desc -> newName` и могут нести хвостовую
/// пометку `  (type-shadow)`. Ключуем ТОЛЬКО по имени (владелец при нормализации
/// всё равно выбрасывается); коллизии при хэшированных суффиксах невероятны.
pub fn load_reverse_map(mapping: Option<&Path>) -> BTreeMap<String, String> {
    let mut rev = BTreeMap::new();
    let path = match mapping {
        Some(p) if p.exists() => p,
        _ => return rev,
    };
    let text = util::read_text_lossy(path).unwrap_or_default();
    let re = Regex::new(r"^(\S+?)\.(\S+)\s+(\S+)\s+->\s+(\S+)\s*$").unwrap();
    for raw in text.split('\n') {
        let line = raw.split("  (").next().unwrap_or(raw).trim();
        if let Some(m) = re.captures(line) {
            rev.insert(m[4].to_string(), m[2].to_string());
        }
    }
    rev
}

fn apply_rev(name: &str, rev: &BTreeMap<String, String>) -> String {
    rev.get(name).cloned().unwrap_or_else(|| name.to_string())
}

// ───────────────────────────── нормализация ──────────────────────────────────
/// Превратить символьный операнд-комментарий `javap` (`// ...`) в нормализованный
/// кортеж ссылки. Применяет N2 (выбросить владельца), N3 (сохранить вид),
/// N4 (StringBuffer→StringBuilder), N8 (обратное переименование).
pub fn canon_ref(comment: &str, rev: &BTreeMap<String, String>) -> Ref {
    let c = comment.trim().replace("StringBuffer", "StringBuilder");
    let re_member = Regex::new(r"^(Field|Method|InterfaceMethod)\s+(.+)$").unwrap();
    if let Some(m) = re_member.captures(&c) {
        let kind = m[1].to_string(); // N3: Method и InterfaceMethod различимы
        let rest = &m[2];
        // ⭐ ПОЧИНЕН БАГ ОРИГИНАЛА: в Python `ci = rest.rfind(":")` при отсутствии
        // двоеточия даёт −1, и дальше `member = rest[:-1]`, `desc = rest[0:]` —
        // то есть у члена молча отгрызается последний символ, а дескриптором
        // становится вся строка. Здесь «двоеточия нет» = дескриптор пустой.
        let ci = rest.rfind(':').unwrap_or(rest.len());
        let member = &rest[..ci];
        let desc = if ci < rest.len() { &rest[ci + 1..] } else { "" };
        let name = member.rsplit('.').next().unwrap_or(member); // N2: владельца долой
        return (kind, apply_rev(name, rev), desc.to_string()); // N8
    }
    let re_class = Regex::new(r"^class\s+(.+)$").unwrap();
    if let Some(m) = re_class.captures(&c) {
        let t = m[1].trim().trim_matches('"').replace("StringBuffer", "StringBuilder");
        let base = Regex::new(r"^\[+L?").unwrap().replace(&t, "").to_string();
        let base = base.trim_end_matches(';').to_string();
        let base = base.rsplit('/').next().unwrap_or(&base).to_string();
        let base = base.rsplit('.').next().unwrap_or(&base).to_string();
        return ("Type".into(), base, String::new());
    }
    ("?".into(), c, String::new())
}

/// Нормализовать строку сигнатуры метода в устойчивый ключ (N1, N8).
///
/// ⭐ ПОЧИНЕН БАГ ОРИГИНАЛА (Python `semdiff._normsig`): там обратная карта
/// применялась ЦИКЛОМ по словарю, каждая замена поверх результата предыдущей.
/// Из-за этого замены СЦЕПЛЯЛИСЬ (`{"a_x": "a", "a": "b"}` давало `a_x → a → b`)
/// и результат зависел от порядка строк в `mapping.txt` — то есть сигнатура
/// нормализовалась не так, как ссылки (`_apply_rev` там делает ОДИН просмотр
/// словаря и не сцепляется). Здесь — один одновременный проход: каждое имя
/// заменяется ровно один раз, порядок `mapping.txt` больше ни на что не влияет.
pub fn normsig(sig: &str, rev: &BTreeMap<String, String>) -> String {
    let s = if rev.is_empty() {
        sig.to_string()
    } else {
        let mut keys: Vec<&String> = rev.keys().collect();
        // длинные имена первыми — на случай движка без \b-семантики
        keys.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        let alt = keys.iter().map(|k| regex::escape(k)).collect::<Vec<_>>().join("|");
        match Regex::new(&format!(r"\b({alt})\b")) {
            Ok(re) => re
                .replace_all(sig, |c: &regex::Captures| {
                    rev.get(&c[1]).cloned().unwrap_or_else(|| c[1].to_string())
                })
                .into_owned(),
            Err(_) => sig.to_string(),
        }
    };
    let s = re_access().replace_all(&s, "").into_owned();
    let s = re_ws().replace_all(&s, " ").into_owned();
    s.trim().trim_end_matches(';').trim().to_string()
}

/// Разобрать вывод `javap -c -p` в {нормализованная_сигнатура: множество_ссылок}.
///
/// Вынесено отдельно от запуска процесса — это ядро, покрываемое голден-тестом
/// против Python на одном и том же тексте `javap`.
pub fn parse_javap(stdout: &str, rev: &BTreeMap<String, String>) -> BTreeMap<String, BTreeSet<Ref>> {
    let mut res: BTreeMap<String, BTreeSet<Ref>> = BTreeMap::new();
    let mut cur: Option<String> = None;
    let mut refs: BTreeSet<Ref> = BTreeSet::new();
    let off = re_offset();
    let lead = re_lead();
    for line in stdout.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)) {
        let st = line.trim();
        if st.is_empty() || st.starts_with("Compiled from") || st == "{" || st == "}" {
            continue;
        }
        if st.ends_with(");") && !off.is_match(line) && !st.contains("Code:") {
            if let Some(c) = cur.take() {
                res.insert(c, std::mem::take(&mut refs));
            }
            cur = Some(normsig(st, rev));
            refs = BTreeSet::new();
            continue;
        }
        if st == "Code:" {
            continue;
        }
        if off.is_match(line) {
            let body = lead.replace(st, "").into_owned();
            let (code, comment) = match body.find("//") {
                Some(i) => (body[..i].to_string(), body[i + 2..].to_string()),
                None => (body.clone(), String::new()),
            };
            let parts: Vec<&str> = code.split_whitespace().collect();
            let op = parts.first().copied().unwrap_or("");
            if op == "newarray" && parts.len() >= 2 {
                // N5
                refs.insert(("NewArray".into(), parts[1].to_string(), String::new()));
            } else if REF_OPS.contains(&op) && !comment.trim().is_empty() {
                // N6 исключает checkcast/instanceof
                let cr = canon_ref(&comment, rev);
                if is_getclass(&cr) {
                    continue; // N7
                }
                refs.insert(cr);
            }
        }
    }
    if let Some(c) = cur {
        res.insert(c, refs);
    }
    res
}

/// `javap -c -p` над одним классом. None, если javap не отработал.
pub fn disassemble(
    classpath: &str,
    binary_name: &str,
    rev: &BTreeMap<String, String>,
) -> Option<BTreeMap<String, BTreeSet<Ref>>> {
    let cls = binary_name.replace('/', ".");
    let argv = vec![
        "javap".to_string(),
        "-c".into(),
        "-p".into(),
        "-cp".into(),
        classpath.to_string(),
        cls,
    ];
    let r = util::run_capture(&argv, None);
    if r.spawn_err.is_some() || !r.success() {
        return None;
    }
    Some(parse_javap(&r.stdout, rev))
}

/// Поле `only_orig` у пометки двулико, как в Python: булево «метод только в
/// оригинале» для одностороннего метода и список ссылок для расхождения множеств.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(untagged)]
pub enum OnlyOrig {
    Side(bool),
    Refs(Vec<String>),
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Flagged {
    pub method: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_orig: Option<OnlyOrig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_recompiled: Option<Vec<String>>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ClassResult {
    pub class: String,
    pub flagged: Vec<Flagged>,
}

/// Питоновский `repr` кортежа — голден-совместимое представление ссылки.
pub fn ref_repr(r: &Ref) -> String {
    if r.0 == "Type" || r.0 == "NewArray" || r.0 == "?" {
        format!("('{}', '{}')", r.0, r.1)
    } else {
        format!("('{}', '{}', '{}')", r.0, r.1, r.2)
    }
}

/// Сравнить методы одного класса. None, если хоть одна сторона не
/// дизассемблировалась (вызывающая сторона считает это «пропущено»).
pub fn compare_class(
    orig_cp: &str,
    recompiled_cp: &str,
    binary_name: &str,
    rev: &BTreeMap<String, String>,
) -> Option<ClassResult> {
    let a = disassemble(orig_cp, binary_name, rev)?;
    let b = disassemble(recompiled_cp, binary_name, rev)?;
    Some(compare_maps(binary_name, &a, &b))
}

/// Чистое сравнение двух разобранных дизассемблеров (ядро для голден-тестов).
pub fn compare_maps(
    binary_name: &str,
    a: &BTreeMap<String, BTreeSet<Ref>>,
    b: &BTreeMap<String, BTreeSet<Ref>>,
) -> ClassResult {
    let mut sigs: BTreeSet<&String> = BTreeSet::new();
    sigs.extend(a.keys());
    sigs.extend(b.keys());
    let mut flagged = Vec::new();
    for sig in sigs {
        match (a.get(sig), b.get(sig)) {
            (Some(x), Some(y)) => {
                if x != y {
                    let mut only_o: Vec<String> = x.difference(y).map(ref_repr).collect();
                    let mut only_r: Vec<String> = y.difference(x).map(ref_repr).collect();
                    only_o.sort();
                    only_r.sort();
                    flagged.push(Flagged {
                        method: sig.clone(),
                        reason: "symbolic reference set differs".into(),
                        only_orig: Some(OnlyOrig::Refs(only_o)),
                        only_recompiled: Some(only_r),
                    });
                }
            }
            _ => flagged.push(Flagged {
                method: sig.clone(),
                reason: "method present on one side only".into(),
                // Python: "only_orig": sig not in b
                only_orig: Some(OnlyOrig::Side(!b.contains_key(sig))),
                only_recompiled: None,
            }),
        }
    }
    ClassResult { class: binary_name.to_string(), flagged }
}

// ───────────────────────────── оркестрация ──────────────────────────────────
fn jar_class_names(jar: &Path) -> Vec<String> {
    util::zip_namelist(jar)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.ends_with(".class"))
        .map(|n| n[..n.len() - 6].to_string())
        .collect()
}

pub fn javap_available() -> bool {
    crate::diffrun::which("javap").is_some()
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct SemdiffReport {
    pub available: bool,
    pub checked: i64,
    pub skipped: Vec<String>,
    pub flagged_classes: Vec<ClassResult>,
    pub total_flagged_methods: i64,
}

/// Прогнать семантический round-trip по каждому классу `orig_jar`, который пересобрался.
///
/// `orig_jar` — ПЕРЕИМЕНОВАННЫЙ jar (канонический API), сторона «оригинального» байткода.
/// `merged_classes_dir` — каталог `_classes` от пересборки best-effort.
/// `mapping` — `mapping.txt` переименователя (для примирения N8), необязателен.
/// `skip` — бинарные имена, которые пропустить (например, не пересобравшиеся классы).
pub fn check(
    orig_jar: &Path,
    merged_classes_dir: &Path,
    mapping: Option<&Path>,
    skip: &BTreeSet<String>,
) -> SemdiffReport {
    if !javap_available() {
        return SemdiffReport {
            available: false,
            checked: 0,
            skipped: Vec::new(),
            flagged_classes: Vec::new(),
            total_flagged_methods: 0,
        };
    }
    let rev = load_reverse_map(mapping);
    let orig_cp = orig_jar.to_string_lossy().into_owned();
    let recompiled_cp = merged_classes_dir.to_string_lossy().into_owned();
    let mut flagged_classes = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut checked = 0i64;
    let mut total_flagged = 0i64;
    let mut names = jar_class_names(orig_jar);
    names.sort();
    for bn in names {
        if skip.contains(&bn) || !merged_classes_dir.join(format!("{bn}.class")).exists() {
            skipped.push(bn);
            continue;
        }
        match compare_class(&orig_cp, &recompiled_cp, &bn, &rev) {
            None => skipped.push(bn),
            Some(res) => {
                checked += 1;
                if !res.flagged.is_empty() {
                    total_flagged += res.flagged.len() as i64;
                    flagged_classes.push(res);
                }
            }
        }
    }
    skipped.sort();
    SemdiffReport {
        available: true,
        checked,
        skipped,
        flagged_classes,
        total_flagged_methods: total_flagged,
    }
}

// ─────────────── дрейф на уровне варианта (--reject-semantic-drift) ─────────
/// Даёт ли компиляция `after_text` (автофикснутого варианта класса `rel`) байткод,
/// чьё нормализованное множество ссылок ДРЕЙФУЕТ от байткода исходного jar, тогда
/// как `before_text` НЕ дрейфовал?
///
/// Консервативно: возвращает false («дрейфа нет») всякий раз, когда решить нельзя —
/// нет javap, вариант не компилируется, класса нет в исходном jar, — так что решение
/// о приёме может стать только СТРОЖЕ, но не мягче.
pub fn variant_drifts(
    orig_jar: &Path,
    before_text: &str,
    after_text: &str,
    rel: &str,
    api_classpath: &str,
    workdir: &Path,
    mapping: Option<&Path>,
) -> bool {
    if !javap_available() {
        return false;
    }
    let binary = rel.strip_suffix(".java").unwrap_or(rel).to_string();
    let names = util::zip_namelist(orig_jar).unwrap_or_default();
    if !names.iter().any(|n| n == &format!("{binary}.class")) {
        return false;
    }
    let rev = load_reverse_map(mapping);

    let refs = |text: &str, tag: &str| -> Option<BTreeMap<String, BTreeSet<Ref>>> {
        let d = workdir.join(tag);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).ok()?;
        let src = d.join(format!("{binary}.java"));
        if let Some(p) = src.parent() {
            std::fs::create_dir_all(p).ok()?;
        }
        std::fs::write(&src, text).ok()?;
        let argv = vec![
            util::tool("ARLECCHINO_JAVAC", "javac"),
            "-d".into(),
            d.to_string_lossy().into_owned(),
            "--release".into(),
            "8".into(),
            "-encoding".into(),
            "utf-8".into(),
            "-nowarn".into(),
            "-proc:none".into(),
            "-cp".into(),
            api_classpath.to_string(),
            src.to_string_lossy().into_owned(),
        ];
        let r = util::run_capture(&argv, None);
        if !r.success() {
            return None;
        }
        disassemble(&d.to_string_lossy(), &binary, &rev)
    };

    let orig = match disassemble(&orig_jar.to_string_lossy(), &binary, &rev) {
        Some(o) => o,
        None => return false,
    };
    let after = match refs(after_text, "_after") {
        Some(a) => a,
        None => return false,
    };
    let before = refs(before_text, "_before");

    let drift = |cand: &BTreeMap<String, BTreeSet<Ref>>| -> bool {
        let mut sigs: BTreeSet<&String> = BTreeSet::new();
        sigs.extend(orig.keys());
        sigs.extend(cand.keys());
        for sig in sigs {
            match (orig.get(sig), cand.get(sig)) {
                (Some(x), Some(y)) if x == y => {}
                _ => return true,
            }
        }
        false
    };
    let after_drift = drift(&after);
    let before_drift = before.map(|b| drift(&b)).unwrap_or(false);
    // регрессией считаем только «ПОСЛЕ дрейфует, а ДО не дрейфовал»
    after_drift && !before_drift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canon_ref_drops_owner_keeps_kind() {
        let rev = BTreeMap::new();
        assert_eq!(
            canon_ref(" Field aa.H:I", &rev),
            ("Field".into(), "H".into(), "I".into())
        );
        assert_eq!(
            canon_ref(" InterfaceMethod javax/microedition/media/Player.start:()V", &rev),
            ("InterfaceMethod".into(), "start".into(), "()V".into())
        );
        assert_eq!(
            canon_ref(" class java/lang/StringBuffer", &rev),
            ("Type".into(), "StringBuilder".into(), String::new())
        );
    }
}
