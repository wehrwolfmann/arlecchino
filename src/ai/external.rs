//! Builds the [`ExternalModel`]: what the rename planner knows about supertypes that
//! live OUTSIDE the jar.
//!
//! Sources, in order: the built-in `Object`/`Runnable` table; the generated
//! clean-room MIDP/CLDC stub (`javax.microedition.*`, see `midp_stub`); and `javap`
//! on the local JDK for any remaining `java.*`/`javax.*` supertype.  The JDK is a
//! SUPERSET of CLDC, so it can only add locks and collisions, never remove them —
//! the safe direction.  Anything still unknown (vendor APIs such as
//! `com.nokia.mid.ui.FullCanvas`) stays "opaque", and the planner then refuses to
//! rename the inheritable members of its subclasses.

use super::classfile::{self, ClassInfo, Member, ACC_PUBLIC, ACC_STATIC};
use super::names::ExternalModel;
use crate::{midp_stub, util};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

pub fn load(classes: &BTreeMap<String, ClassInfo>, use_stub: bool, use_javap: bool) -> ExternalModel {
    let mut m = ExternalModel::builtin();
    if use_stub {
        if let Some(stub) = midp_stub::ensure_stub() {
            if let Ok(all) = classfile::read_jar(&stub) {
                for (_, ci) in all {
                    m.add(ci);
                }
            }
        }
    }
    if use_javap {
        let mut tried: BTreeSet<String> = BTreeSet::new();
        for _round in 0..8 {
            let unknown = unknown_supertypes(classes, &m)
                .into_iter()
                .filter(|n| (n.starts_with("java/") || n.starts_with("javax/")) && !tried.contains(n))
                .collect::<Vec<_>>();
            if unknown.is_empty() {
                break;
            }
            tried.extend(unknown.iter().cloned());
            for ci in javap_models(&unknown) {
                m.add(ci);
            }
        }
    }
    m
}

/// External supertypes (transitively) that the model does not know yet.
pub fn unknown_supertypes(classes: &BTreeMap<String, ClassInfo>, m: &ExternalModel) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut work: Vec<String> = classes.values().flat_map(|c| c.supertypes().cloned().collect::<Vec<_>>()).collect();
    let mut seen = BTreeSet::new();
    while let Some(s) = work.pop() {
        if !seen.insert(s.clone()) {
            continue;
        }
        if let Some(ci) = classes.get(&s).or_else(|| m.known.get(&s)) {
            work.extend(ci.supertypes().cloned());
        } else {
            out.insert(s);
        }
    }
    out
}

fn javap_models(internal: &[String]) -> Vec<ClassInfo> {
    let javap = util::tool("ARLECCHINO_JAVAP", "javap");
    let mut argv = vec![javap, "-protected".to_string(), "-s".to_string()];
    argv.extend(internal.iter().map(|n| n.replace('/', ".")));
    let r = util::run_capture(&argv, Some(Duration::from_secs(120)));
    if r.spawn_err.is_some() {
        return Vec::new();
    }
    parse_javap(&r.stdout)
}

fn strip_generics(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Parse `javap -protected -s` output into class models (pure; unit-tested).
pub fn parse_javap(text: &str) -> Vec<ClassInfo> {
    let mut out = Vec::new();
    let mut cur: Option<ClassInfo> = None;
    let mut pending: Option<(String, bool, bool)> = None; // (name, is_method, is_static)
    for raw in text.lines() {
        let line = raw.trim();
        if line.ends_with('{') && (line.contains(" class ") || line.contains(" interface ") || line.starts_with("class ") || line.starts_with("interface ")) {
            if let Some(c) = cur.take() {
                out.push(c);
            }
            let head = strip_generics(line.trim_end_matches('{'));
            let words: Vec<&str> = head.split_whitespace().collect();
            let kw = words.iter().position(|w| *w == "class" || *w == "interface");
            let Some(kw) = kw else { continue };
            let is_iface = words[kw] == "interface";
            let name = words.get(kw + 1).map(|s| s.replace('.', "/")).unwrap_or_default();
            let mut super_name = if is_iface { Some("java/lang/Object".to_string()) } else { None };
            let mut interfaces = Vec::new();
            let mut mode = "";
            for w in &words[kw + 2..] {
                match *w {
                    "extends" => mode = if is_iface { "implements" } else { "extends" },
                    "implements" => mode = "implements",
                    t => {
                        let t = t.trim_end_matches(',').replace('.', "/");
                        if t.is_empty() {
                            continue;
                        }
                        if mode == "extends" {
                            super_name = Some(t);
                        } else if mode == "implements" {
                            interfaces.push(t);
                        }
                    }
                }
            }
            if super_name.is_none() && name != "java/lang/Object" {
                super_name = Some("java/lang/Object".into());
            }
            cur = Some(ClassInfo {
                access: if is_iface { 0x0601 } else { ACC_PUBLIC },
                name,
                super_name,
                interfaces,
                fields: vec![],
                methods: vec![],
                strings: vec![],
                class_refs: vec![],
            });
            continue;
        }
        if let Some(d) = line.strip_prefix("descriptor:") {
            if let (Some((name, is_method, is_static)), Some(c)) = (pending.take(), cur.as_mut()) {
                let access = ACC_PUBLIC | if is_static { ACC_STATIC } else { 0 };
                let mm = Member { access, name, desc: d.trim().to_string() };
                if is_method {
                    c.methods.push(mm);
                } else {
                    c.fields.push(mm);
                }
            }
            continue;
        }
        if line.ends_with(';') && cur.is_some() {
            let decl = strip_generics(line.trim_end_matches(';'));
            let decl = decl.split(" throws ").next().unwrap_or("").to_string();
            let is_static = decl.split_whitespace().any(|w| w == "static");
            if let Some(p) = decl.find('(') {
                let name = decl[..p].split_whitespace().last().unwrap_or("").to_string();
                let cls = cur.as_ref().map(|c| c.name.replace('/', ".")).unwrap_or_default();
                if name == cls || name.is_empty() || decl.trim() == "static {}" {
                    pending = None; // constructor
                } else {
                    pending = Some((name, true, is_static));
                }
            } else {
                let name = decl.split_whitespace().last().unwrap_or("").to_string();
                pending = Some((name, false, is_static));
            }
        }
    }
    if let Some(c) = cur.take() {
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_javap_listing() {
        let t = "Compiled from \"Thread.java\"
public class java.lang.Thread implements java.lang.Runnable {
  public static final int MIN_PRIORITY;
    descriptor: I
  public java.lang.Thread();
    descriptor: ()V
  public synchronized void start();
    descriptor: ()V
  public static void sleep(long) throws java.lang.InterruptedException;
    descriptor: (J)V
}
public interface java.util.Comparator<T> {
  public abstract int compare(T, T);
    descriptor: (Ljava/lang/Object;Ljava/lang/Object;)I
}
";
        let v = parse_javap(t);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "java/lang/Thread");
        assert_eq!(v[0].interfaces, vec!["java/lang/Runnable".to_string()]);
        assert_eq!(v[0].super_name.as_deref(), Some("java/lang/Object"));
        assert_eq!(v[0].fields[0].name, "MIN_PRIORITY");
        let names: Vec<&str> = v[0].methods.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["start", "sleep"]);
        assert!(v[0].methods[1].is_static());
        assert_eq!(v[1].name, "java/util/Comparator");
        assert!(v[1].is_interface());
        assert_eq!(v[1].methods[0].desc, "(Ljava/lang/Object;Ljava/lang/Object;)I");
    }
}
