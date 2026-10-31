//! A small, read-only `.class` reader: just enough structure for the AI rename
//! planner — the class's own name, its supertypes, its fields and methods with
//! access flags, and its `CONSTANT_String` values (to spot names used reflectively,
//! e.g. `Class.forName("a.b")`).
//!
//! It never rewrites bytecode; the actual rewrite is done by ASM (`NameApplier`).

use crate::util;
use std::collections::BTreeMap;
use std::path::Path;

pub const ACC_PUBLIC: u16 = 0x0001;
pub const ACC_PRIVATE: u16 = 0x0002;
pub const ACC_STATIC: u16 = 0x0008;
pub const ACC_SYNTHETIC: u16 = 0x1000;
pub const ACC_INTERFACE: u16 = 0x0200;
pub const ACC_BRIDGE: u16 = 0x0040;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub access: u16,
    pub name: String,
    pub desc: String,
}

impl Member {
    pub fn is_static(&self) -> bool {
        self.access & ACC_STATIC != 0
    }
    pub fn is_private(&self) -> bool {
        self.access & ACC_PRIVATE != 0
    }
    pub fn is_synthetic(&self) -> bool {
        self.access & (ACC_SYNTHETIC | ACC_BRIDGE) != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassInfo {
    pub access: u16,
    /// Internal name, e.g. `a/b/c`.
    pub name: String,
    pub super_name: Option<String>,
    pub interfaces: Vec<String>,
    pub fields: Vec<Member>,
    pub methods: Vec<Member>,
    pub strings: Vec<String>,
    /// Every class named by a `CONSTANT_Class` entry (array types excluded).
    pub class_refs: Vec<String>,
}

impl ClassInfo {
    pub fn is_interface(&self) -> bool {
        self.access & ACC_INTERFACE != 0
    }
    pub fn supertypes(&self) -> impl Iterator<Item = &String> {
        self.super_name.iter().chain(self.interfaces.iter())
    }
}

struct R<'a> {
    d: &'a [u8],
    off: usize,
}

impl R<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.off)?;
        self.off += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.d.get(self.off..self.off + 2)?;
        self.off += 2;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.d.get(self.off..self.off + 4)?;
        self.off += 4;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        if self.off + n > self.d.len() {
            return None;
        }
        self.off += n;
        Some(())
    }
}

/// Decode JVM "modified UTF-8" well enough for identifiers and string constants.
fn mutf8(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap_or_else(|_| String::from_utf8_lossy(b).into_owned())
}

#[derive(Clone)]
enum Cp {
    None,
    Utf8(String),
    Class(u16),
    Str(u16),
}

pub fn parse_class(data: &[u8]) -> Option<ClassInfo> {
    let mut r = R { d: data, off: 0 };
    if r.u32()? != 0xCAFE_BABE {
        return None;
    }
    r.skip(4)?;
    let count = r.u16()? as usize;
    let mut cp = vec![Cp::None; count.max(1)];
    let mut i = 1;
    while i < count {
        let tag = r.u8()?;
        match tag {
            1 => {
                let ln = r.u16()? as usize;
                let s = mutf8(data.get(r.off..r.off + ln)?);
                r.skip(ln)?;
                cp[i] = Cp::Utf8(s);
            }
            7 => cp[i] = Cp::Class(r.u16()?),
            8 => cp[i] = Cp::Str(r.u16()?),
            5 | 6 => {
                r.skip(8)?;
                i += 1;
            }
            16 | 19 | 20 => {
                r.skip(2)?;
            }
            15 => r.skip(3)?,
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => r.skip(4)?,
            _ => return None,
        }
        i += 1;
    }
    let utf = |idx: u16| -> Option<String> {
        match cp.get(idx as usize)? {
            Cp::Utf8(s) => Some(s.clone()),
            _ => None,
        }
    };
    let class_name = |idx: u16| -> Option<String> {
        match cp.get(idx as usize)? {
            Cp::Class(n) => utf(*n),
            _ => None,
        }
    };
    let access = r.u16()?;
    let name = class_name(r.u16()?)?;
    let sup = r.u16()?;
    let super_name = if sup == 0 { None } else { class_name(sup) };
    let nif = r.u16()?;
    let mut interfaces = Vec::new();
    for _ in 0..nif {
        interfaces.push(class_name(r.u16()?)?);
    }
    let members = |r: &mut R| -> Option<Vec<Member>> {
        let n = r.u16()?;
        let mut out = Vec::new();
        for _ in 0..n {
            let access = r.u16()?;
            let name = utf(r.u16()?)?;
            let desc = utf(r.u16()?)?;
            let na = r.u16()?;
            for _ in 0..na {
                r.skip(2)?;
                let ln = r.u32()? as usize;
                r.skip(ln)?;
            }
            out.push(Member { access, name, desc });
        }
        Some(out)
    };
    let fields = members(&mut r)?;
    let methods = members(&mut r)?;
    let strings = cp
        .iter()
        .filter_map(|c| match c {
            Cp::Str(u) => utf(*u),
            _ => None,
        })
        .collect();
    let mut class_refs: Vec<String> = cp
        .iter()
        .filter_map(|c| match c {
            Cp::Class(u) => utf(*u).filter(|n| !n.starts_with('[')),
            _ => None,
        })
        .collect();
    class_refs.sort();
    class_refs.dedup();
    Some(ClassInfo { access, name, super_name, interfaces, fields, methods, strings, class_refs })
}

/// Every parseable class of a jar, keyed by internal name.
pub fn read_jar(jar: &Path) -> Result<BTreeMap<String, ClassInfo>, String> {
    let names = util::zip_namelist(jar).map_err(|e| format!("{}: {e}", jar.display()))?;
    let mut out = BTreeMap::new();
    for n in names {
        if !n.ends_with(".class") || n.starts_with("META-INF/") {
            continue;
        }
        let bytes = util::zip_read_entry(jar, &n).map_err(|e| format!("{n}: {e}"))?;
        if let Some(ci) = parse_class(&bytes) {
            out.insert(ci.name.clone(), ci);
        }
    }
    Ok(out)
}

/// Class names (internal form) named by the jar manifest (`MIDlet-N`, `Main-Class`,
/// …) — the launcher finds those by name, so they must never be renamed.
pub fn manifest_classes(jar: &Path) -> Vec<String> {
    let Ok(bytes) = util::zip_read_entry(jar, "META-INF/MANIFEST.MF") else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes).replace("\r\n ", "").replace("\n ", "");
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim();
        let v = v.trim();
        if k.starts_with("MIDlet-") && k[7..].chars().all(|c| c.is_ascii_digit()) && !k[7..].is_empty() {
            // MIDlet-1: Name, icon, class
            if let Some(cls) = v.rsplit(',').next() {
                out.push(cls.trim().replace('.', "/"));
            }
        } else if k == "Main-Class" {
            out.push(v.replace('.', "/"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage() {
        assert!(parse_class(b"not a class").is_none());
        assert!(parse_class(&[0xCA, 0xFE, 0xBA, 0xBE]).is_none());
    }
}
