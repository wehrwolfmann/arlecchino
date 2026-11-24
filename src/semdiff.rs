//! Semantic round-trip check (stage E3) — the VERIFICATION / FLAGGING layer.
//!
//! Port of `arlecchino/semdiff.py`.
//!
//! The ordinary round-trip check (`roundtrip`) proves only COVERAGE (every
//! source class produced a compiled one) and COARSE COUNTS (number of members/methods
//! per `javap -p`). An autofix can change SEMANTICS without touching these counts —
//! above all the three autofixes that rewrite constructs close to behaviour:
//!
//!   * `scratch_temp_dead_array_type` — widens a local's declaration to `Object`
//!     (may flip the element type of `new int[]` versus `new char[]`),
//!   * `fold_interface_static_init`   — moves a `static {}` block into field
//!     initialisers (reorders/re-encodes `<clinit>`),
//!   * `const_interface_to_class`     — turns an `interface` into a `class`
//!     (flips call sites from `invokeinterface` to `invokevirtual`).
//!
//! The module disassembles, METHOD BY METHOD, the original `.class` (from the renamed jar —
//! the canonical API the whole pipeline uses anyway) and the rebuilt
//! `.class`, NORMALISES cosmetic differences that legitimately arise in a
//! decompiler+compiler round trip, and FLAGS methods whose normalised
//! set of symbolic references differs in substance. It never changes the
//! merge rules.
//!
//! Why a SET of symbolic references rather than the exact opcode stream?
//! ---------------------------------------------------------------------
//! A strict comparison of the `javap -c` stream is too rigid: javac re-encodes
//! decompiled source with a different branch polarity
//! (`if_icmpge` ↔ `if_icmplt; return`), different local slot numbers,
//! different `dup`/`x++` idioms, and drops reloads — none of which
//! changes behaviour. Calibration on the known-good Allods 712→2 merge
//! (30+ clean classes) showed hundreds of false line-by-line differences.
//!
//! The stable signal WITHOUT false positives (calibrated on Allods) is the
//! set of distinct symbolic operands of each method, with normalisation:
//!
//!   N1. Compare method to method by normalised signature (access modifiers
//!       dropped); a method present on only one side is flagged.
//!   N2. Field/method references are kept as `(kind, name, descriptor)`, but the
//!       owner class is DROPPED — javac re-resolves a member to the supertype that
//!       DECLARES it (`Field aa.H:I` versus `Field j.H:I`); same member, cosmetic.
//!   N3. The call kind stays distinguishable (`Method` versus `InterfaceMethod`), so
//!       an interface→class flip is still caught while the owner is dropped.
//!   N4. Fold `StringBuffer` ↔ `StringBuilder` — the decompiler emits the modern
//!       `StringBuilder` for `+` concatenation that the original obfuscated
//!       bytecode built with `StringBuffer`; same behaviour.
//!   N5. Catch primitive array creation `newarray <type>` — exactly what
//!       `scratch_temp_dead_array_type` can change.
//!   N6. EXCLUDE `checkcast` / `instanceof` — decompilers insert redundant
//!       casts that the original checked statically.
//!   N7. EXCLUDE the synthetic `getClass():Class` — javac emits it as a
//!       null check that decompiled source does not reproduce.
//!   N8. Reconcile renamed members via `mapping.txt` (the reverse map
//!       newName→oldName), so that a renamed member is not flagged as a
//!       semantic change.
//!
//! Everything degrades gracefully: no `javap` — the module does nothing (`available=false`);
//! a class that did not recompile is skipped (reported, not flagged).

use crate::util;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A member/type reference in normalised form.
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

/// opcodes whose operand names a field/method/type (the semantic core we keep)
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

/// a synthetic null check that javac emits and decompiled
/// source does not reproduce (N7)
fn is_getclass(r: &Ref) -> bool {
    r.0 == "Method" && r.1 == "getClass" && r.2 == "()Ljava/lang/Class;"
}

// ───────────────────────────── mapping (N8) ─────────────────────────────────
/// Build {newName: oldName} from the renamer's `mapping.txt`.
///
/// Lines look like `owner.oldName desc -> newName` and may carry a trailing
/// `  (type-shadow)` mark. We key by NAME ONLY (the owner is dropped during
/// normalisation anyway); collisions with hashed suffixes are implausible.
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

// ───────────────────────────── normalisation ─────────────────────────────────
/// Turn a `javap` symbolic operand comment (`// ...`) into a normalised
/// reference tuple. Applies N2 (drop the owner), N3 (keep the kind),
/// N4 (StringBuffer→StringBuilder), N8 (reverse renaming).
pub fn canon_ref(comment: &str, rev: &BTreeMap<String, String>) -> Ref {
    let c = comment.trim().replace("StringBuffer", "StringBuilder");
    let re_member = Regex::new(r"^(Field|Method|InterfaceMethod)\s+(.+)$").unwrap();
    if let Some(m) = re_member.captures(&c) {
        let kind = m[1].to_string(); // N3: Method and InterfaceMethod are distinguishable
        let rest = &m[2];
        // ⭐ FIXED A BUG OF THE ORIGINAL: in Python, `ci = rest.rfind(":")` with no
        // colon gives −1, and then `member = rest[:-1]`, `desc = rest[0:]` —
        // i.e. the member silently loses its last character, and the whole string
        // becomes the descriptor. Here "no colon" = an empty descriptor.
        let ci = rest.rfind(':').unwrap_or(rest.len());
        let member = &rest[..ci];
        let desc = if ci < rest.len() { &rest[ci + 1..] } else { "" };
        let name = member.rsplit('.').next().unwrap_or(member); // N2: drop the owner
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

/// Normalise a method signature string into a stable key (N1, N8).
///
/// ⭐ FIXED A BUG OF THE ORIGINAL (Python `semdiff._normsig`): there the reverse map
/// was applied in a LOOP over the dictionary, each replacement on top of the previous result.
/// Because of that, replacements CHAINED (`{"a_x": "a", "a": "b"}` gave `a_x → a → b`)
/// and the result depended on the order of lines in `mapping.txt` — i.e. the signature
/// was normalised differently from the references (`_apply_rev` there does ONE pass over
/// the dictionary and does not chain). Here it is a single simultaneous pass: each name
/// is replaced exactly once, and the order of `mapping.txt` no longer affects anything.
pub fn normsig(sig: &str, rev: &BTreeMap<String, String>) -> String {
    let s = if rev.is_empty() {
        sig.to_string()
    } else {
        let mut keys: Vec<&String> = rev.keys().collect();
        // longest names first — in case of an engine without \b semantics
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

/// Parse `javap -c -p` output into {normalised_signature: set_of_references}.
///
/// Kept separate from spawning the process — this is the core covered by a golden test
/// against Python on the same `javap` text.
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
        // javap prints the static initializer as `static {};` — without this its
        // references were credited to whichever method preceded it.
        let is_clinit = st == "static {};";
        if is_clinit || (st.ends_with(");") && !off.is_match(line) && !st.contains("Code:")) {
            if let Some(c) = cur.take() {
                res.insert(c, std::mem::take(&mut refs));
            }
            cur = Some(if is_clinit { "<clinit>".to_string() } else { normsig(st, rev) });
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
                // N6 excludes checkcast/instanceof
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

/// `javap -c -p` over one class. None if javap failed.
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

/// The `only_orig` field of a flag is two-faced, as in Python: a boolean "method only in
/// the original" for a one-sided method, and a list of references for a set mismatch.
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

/// Python's `repr` of a tuple — the golden-compatible representation of a reference.
pub fn ref_repr(r: &Ref) -> String {
    if r.0 == "Type" || r.0 == "NewArray" || r.0 == "?" {
        format!("('{}', '{}')", r.0, r.1)
    } else {
        format!("('{}', '{}', '{}')", r.0, r.1, r.2)
    }
}

/// Compare the methods of one class. None if either side failed to
/// disassemble (the caller treats it as "skipped").
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

/// Pure comparison of two parsed disassemblies (the core for golden tests).
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

// ───────────────────────────── orchestration ─────────────────────────────────
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

/// Run the semantic round trip over every class of `orig_jar` that was rebuilt.
///
/// `orig_jar` is the RENAMED jar (canonical API), the "original" bytecode side.
/// `merged_classes_dir` is the `_classes` directory from the best-effort rebuild.
/// `mapping` is the renamer's `mapping.txt` (for N8 reconciliation), optional.
/// `skip` — binary names to skip (e.g. classes that did not rebuild).
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

// ─────────────── variant-level drift (--reject-semantic-drift) ──────────────
/// Does compiling `after_text` (an autofixed variant of class `rel`) give bytecode
/// whose normalised reference set DRIFTS from the original jar's bytecode, while
/// `before_text` did NOT drift?
///
/// Conservative: returns false ("no drift") whenever it cannot decide —
/// no javap, the variant does not compile, the class is missing from the original jar — so the
/// acceptance decision can only become STRICTER, never looser.
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
    // count as a regression only "AFTER drifts while BEFORE did not"
    after_drift && !before_drift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_initializer_is_its_own_method() {
        let rev = BTreeMap::new();
        let javap = "class e {\n  private final boolean a(boolean);\n    Code:\n       0: getfield      #2                  // Field dh:Z\n\n  static {};\n    Code:\n       0: iconst_0\n       1: putstatic     #3                  // Field D:Z\n}\n";
        let m = parse_javap(javap, &rev);
        let a = m
            .iter()
            .find(|(k, _)| k.contains("a(boolean)"))
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("no a(boolean) in {:?}", m.keys().collect::<Vec<_>>()));
        assert!(a.contains(&("Field".into(), "dh".into(), "Z".into())));
        assert!(!a.contains(&("Field".into(), "D".into(), "Z".into())));
        assert!(m["<clinit>"].contains(&("Field".into(), "D".into(), "Z".into())));
    }

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
