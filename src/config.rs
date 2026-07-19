//! Engine configuration.
//!
//! Engines are described in a TOML/JSON config file instead of being hard-coded.
//! Resolution order for the config:
//!
//!   1. explicit --config PATH
//!   2. ./arlecchino.toml  (cwd)
//!   3. ~/.arlecchino/engines.toml
//!   4. the built-in default (engines.default.toml, embedded in the binary)
//!
//! Each engine entry:
//!
//!   [[engine]]
//!   name   = "vineflower"
//!   type   = "names"          # "names" keeps original class names (mergeable)
//!                             # "renamed" renames classes (compare-only)
//!   kind   = "jar"            # "jar" -> `java -jar`, "tool" -> run argv[0]
//!   path   = "vineflower.jar" # absolute, or relative to the engine root
//!   args   = ["{jar}", "{out}"]
//!   sources_subdir = ""       # where the .java land under {out} ("sources" for jadx)
//!   normalize = "none"        # "none" | "jadx"
//!
//! Substitution tokens available in args: {jar} {out} {java} {path}.

use crate::util::{self, expanduser};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Bundled default engine configuration (materialised at build time).
pub const DEFAULT_CONFIG_TOML: &str = include_str!("../assets/engines.default.toml");

pub fn engines_root_default() -> PathBuf {
    util::home_dir().join(".arlecchino").join("engines")
}

#[derive(Clone, Debug)]
pub struct Engine {
    pub name: String,
    pub type_: String,   // names | renamed
    pub kind: String,    // jar | tool
    pub path: String,
    pub args: Vec<String>,
    pub sources_subdir: String,
    pub normalize: String, // none | jadx
    pub enabled: bool,
}

impl Engine {
    pub fn resolve_path(&self, root: &Path) -> PathBuf {
        let p = expanduser(&self.path);
        if p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    }

    pub fn argv(&self, jar: &Path, out: &Path, root: &Path) -> Result<Vec<String>, String> {
        let java = util::tool("ARLECCHINO_JAVA", "java");
        let binp = self.resolve_path(root);
        let jar_s = jar.to_string_lossy().to_string();
        let out_s = out.to_string_lossy().to_string();
        let path_s = binp.to_string_lossy().to_string();

        let mut argv: Vec<String> = match self.kind.as_str() {
            "jar" => vec![java.clone(), "-jar".to_string(), path_s.clone()],
            "tool" => vec![path_s.clone()],
            other => return Err(format!("unknown engine kind: {other}")),
        };
        for a in &self.args {
            argv.push(
                a.replace("{jar}", &jar_s)
                    .replace("{out}", &out_s)
                    .replace("{java}", &java)
                    .replace("{path}", &path_s),
            );
        }
        Ok(argv)
    }

    pub fn sources_dir(&self, out: &Path) -> PathBuf {
        if !self.sources_subdir.is_empty() && out.join(&self.sources_subdir).is_dir() {
            return out.join(&self.sources_subdir);
        }
        // jadx always emits to <out>/sources even if not declared
        if out.join("sources").is_dir() {
            return out.join("sources");
        }
        out.to_path_buf()
    }
}

// ── raw (deserialised) shapes ────────────────────────────────────────────────
fn d_names() -> String {
    "names".into()
}
fn d_jar() -> String {
    "jar".into()
}
fn d_none() -> String {
    "none".into()
}
fn d_true() -> bool {
    true
}

#[derive(Deserialize)]
struct RawEngine {
    name: String,
    #[serde(rename = "type", default = "d_names")]
    type_: String,
    #[serde(default = "d_jar")]
    kind: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    sources_subdir: String,
    #[serde(default = "d_none")]
    normalize: String,
    #[serde(default = "d_true")]
    enabled: bool,
}

#[derive(Deserialize)]
struct RawConfig {
    engines_root: Option<String>,
    #[serde(rename = "engine", default)]
    engine: Vec<RawEngine>,
}

/// Where the active config came from (for display).
#[derive(Clone, Debug)]
pub enum ConfigSource {
    File(PathBuf),
    BuiltinDefault,
}

impl std::fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigSource::File(p) => write!(f, "{}", p.display()),
            ConfigSource::BuiltinDefault => write!(f, "<bundled engines.default.toml>"),
        }
    }
}

pub fn find_config(explicit: Option<&str>) -> ConfigSource {
    if let Some(e) = explicit {
        return ConfigSource::File(util::absolutize(&expanduser(e)));
    }
    let candidates = [
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("arlecchino.toml"),
        util::home_dir().join(".arlecchino").join("engines.toml"),
    ];
    for c in candidates {
        if c.exists() {
            return ConfigSource::File(c);
        }
    }
    ConfigSource::BuiltinDefault
}

fn parse_raw(text: &str, is_json: bool) -> Result<RawConfig, String> {
    if is_json {
        serde_json::from_str(text).map_err(|e| format!("JSON config error: {e}"))
    } else {
        toml::from_str(text).map_err(|e| format!("TOML config error: {e}"))
    }
}

/// Return (engines, config_source, engines_root).
pub fn load_engines(explicit: Option<&str>) -> Result<(Vec<Engine>, ConfigSource, PathBuf), String> {
    let src = find_config(explicit);
    let raw: RawConfig = match &src {
        ConfigSource::BuiltinDefault => parse_raw(DEFAULT_CONFIG_TOML, false)?,
        ConfigSource::File(p) => {
            let text = std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read config {}: {e}", p.display()))?;
            let is_json = p.extension().map(|e| e == "json").unwrap_or(false);
            parse_raw(&text, is_json)?
        }
    };

    let root = raw
        .engines_root
        .map(|s| expanduser(&s))
        .unwrap_or_else(engines_root_default);

    let engines = raw
        .engine
        .into_iter()
        .map(|e| Engine {
            name: e.name,
            type_: e.type_,
            kind: e.kind,
            path: e.path,
            args: e.args,
            sources_subdir: e.sources_subdir,
            normalize: e.normalize,
            enabled: e.enabled,
        })
        .collect();

    Ok((engines, src, root))
}
