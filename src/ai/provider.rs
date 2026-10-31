//! Model providers for the optional AI stages: transport, retries, keys, caching.
//!
//! The contract every stage is written against is [`crate::ai_model::ChatModel`];
//! this module supplies the implementations:
//!
//! * [`AnthropicModel`] — the Anthropic Messages API (`POST /v1/messages`), key from
//!   `ANTHROPIC_API_KEY`;
//! * [`OpenAiCompatModel`] — any OpenAI-compatible `chat/completions` endpoint, e.g. a
//!   local Ollama or llama.cpp server, so code never has to leave the machine;
//! * [`CachedModel`] — wraps any model with an on-disk response cache keyed by a hash
//!   of the full request, so a re-run costs nothing and returns the same answers.
//!
//! Transient failures (HTTP 408/409/429/5xx/529, connection errors) are retried with
//! exponential back-off, honouring `retry-after`.  Permanent ones (bad request, bad
//! key, refusal) are returned at once.  Nothing here runs unless a stage explicitly
//! builds a provider, and every stage is off by default.

use crate::ai_model::{ChatModel, ChatRequest, Role};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-5-5";
pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_API_VERSION: &str = "2026-10-21";
/// Beta header for the scalar `fallbacks: "default"` form (server-side refusal
/// fallback, Claude API only).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
pub const DEFAULT_OPENAI_BASE_URL: &str = "http://localhost:11434/v1";
pub const DEFAULT_MAX_TOKENS: u32 = 16000;

/// Which wire protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Ok(ProviderKind::Anthropic),
            "openai" | "ollama" | "local" | "openai-compatible" => Ok(ProviderKind::OpenAi),
            other => Err(format!("unknown AI provider `{other}` (expected anthropic or openai)")),
        }
    }
}

/// Everything needed to build a provider.  Resolution order for each field:
/// explicit CLI flag > environment > `[ai]` table of a TOML config > built-in default.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub model: String,
    pub base_url: String,
    /// Name of the environment variable holding the key (never the key itself).
    pub api_key_env: String,
    pub max_tokens: u32,
    pub timeout: Duration,
    pub retries: u32,
    /// Anthropic only: `output_config.effort` (`low`..`max`); `None` = API default.
    pub effort: Option<String>,
    /// Anthropic only: send `fallbacks: "default"` so a classifier refusal is retried
    /// server-side on a fallback model instead of failing the batch.
    pub fallbacks: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            kind: ProviderKind::Anthropic,
            model: DEFAULT_ANTHROPIC_MODEL.into(),
            base_url: DEFAULT_ANTHROPIC_BASE_URL.into(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            max_tokens: DEFAULT_MAX_TOKENS,
            timeout: Duration::from_secs(600),
            retries: 4,
            effort: None,
            fallbacks: true,
        }
    }
}

/// CLI-level overrides; `None` means "not given on the command line".
#[derive(Debug, Clone, Default)]
pub struct ProviderOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub max_tokens: Option<u32>,
    pub effort: Option<String>,
    pub config: Option<PathBuf>,
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

impl ProviderConfig {
    /// Resolve flags > env (`ARLECCHINO_AI_PROVIDER`, `ARLECCHINO_AI_MODEL`,
    /// `ARLECCHINO_AI_BASE_URL`, `ARLECCHINO_AI_KEY_ENV`, `ARLECCHINO_AI_EFFORT`) >
    /// `[ai]` table in `--ai-config` > defaults.
    pub fn resolve(o: &ProviderOverrides) -> Result<Self, String> {
        let table: Option<toml::Table> = match &o.config {
            Some(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
                let t: toml::Table =
                    toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
                t.get("ai").and_then(|v| v.as_table()).cloned()
            }
            None => None,
        };
        let from_toml = |k: &str| -> Option<String> {
            table.as_ref().and_then(|t| t.get(k)).and_then(|v| match v {
                toml::Value::String(s) => Some(s.clone()),
                toml::Value::Integer(i) => Some(i.to_string()),
                toml::Value::Boolean(b) => Some(b.to_string()),
                _ => None,
            })
        };
        let pick = |flag: &Option<String>, env: &str, key: &str| -> Option<String> {
            flag.clone().or_else(|| env_nonempty(env)).or_else(|| from_toml(key))
        };

        let kind = match pick(&o.provider, "ARLECCHINO_AI_PROVIDER", "provider") {
            Some(s) => ProviderKind::parse(&s)?,
            None => ProviderKind::Anthropic,
        };
        let mut c = ProviderConfig { kind, ..Default::default() };
        if kind == ProviderKind::OpenAi {
            c.base_url = DEFAULT_OPENAI_BASE_URL.into();
            c.api_key_env = "OPENAI_API_KEY".into();
            c.model = "qwen2.5-coder".into();
            c.fallbacks = false;
        }
        if let Some(m) = pick(&o.model, "ARLECCHINO_AI_MODEL", "model") {
            c.model = m;
        }
        if let Some(u) = pick(&o.base_url, "ARLECCHINO_AI_BASE_URL", "base_url") {
            c.base_url = u.trim_end_matches('/').to_string();
        }
        if let Some(k) = pick(&o.api_key_env, "ARLECCHINO_AI_KEY_ENV", "api_key_env") {
            c.api_key_env = k;
        }
        if let Some(e) = pick(&o.effort, "ARLECCHINO_AI_EFFORT", "effort") {
            c.effort = Some(e);
        }
        if let Some(n) = o.max_tokens.or_else(|| from_toml("max_tokens").and_then(|s| s.parse().ok())) {
            c.max_tokens = n;
        }
        if let Some(r) = from_toml("retries").and_then(|s| s.parse().ok()) {
            c.retries = r;
        }
        if let Some(t) = from_toml("timeout_secs").and_then(|s| s.parse().ok()) {
            c.timeout = Duration::from_secs(t);
        }
        if let Some(f) = from_toml("fallbacks") {
            c.fallbacks = f == "true";
        }
        Ok(c)
    }

    /// True when requests leave this machine (anything but a loopback host).
    pub fn is_remote(&self) -> bool {
        is_remote_url(&self.base_url)
    }

    pub fn id(&self) -> String {
        let k = match self.kind {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
        };
        format!("{k}:{}", self.model)
    }

    /// Build the live provider.  Fails early (before any request) on a missing key
    /// for a remote endpoint.
    pub fn build(&self) -> Result<LiveModel, String> {
        let key = env_nonempty(&self.api_key_env);
        match self.kind {
            ProviderKind::Anthropic => {
                let key = key.ok_or_else(|| {
                    format!("{} is not set — the Anthropic provider needs an API key", self.api_key_env)
                })?;
                Ok(LiveModel::Anthropic(AnthropicModel { cfg: self.clone(), key, usage: Mutex::new(Usage::default()) }))
            }
            ProviderKind::OpenAi => {
                if key.is_none() && self.is_remote() {
                    return Err(format!(
                        "{} is not set and {} is not a local endpoint",
                        self.api_key_env, self.base_url
                    ));
                }
                Ok(LiveModel::OpenAi(OpenAiCompatModel { cfg: self.clone(), key, usage: Mutex::new(Usage::default()) }))
            }
        }
    }
}

/// A live provider built from a [`ProviderConfig`].
pub enum LiveModel {
    Anthropic(AnthropicModel),
    OpenAi(OpenAiCompatModel),
}

impl LiveModel {
    /// Tokens and calls actually spent so far (cache hits cost nothing).
    pub fn usage(&self) -> Usage {
        match self {
            LiveModel::Anthropic(m) => *m.usage.lock().unwrap(),
            LiveModel::OpenAi(m) => *m.usage.lock().unwrap(),
        }
    }
}

impl ChatModel for LiveModel {
    fn id(&self) -> String {
        match self {
            LiveModel::Anthropic(m) => m.id(),
            LiveModel::OpenAi(m) => m.id(),
        }
    }
    fn chat(&self, req: &ChatRequest) -> Result<String, String> {
        match self {
            LiveModel::Anthropic(m) => m.chat(req),
            LiveModel::OpenAi(m) => m.chat(req),
        }
    }
}

pub fn is_remote_url(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let hostport = rest.split('/').next().unwrap_or("");
    let host = if hostport.starts_with('[') {
        hostport.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    !matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") && !host.starts_with("127.")
}

/// Token accounting, summed over every call a provider made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Rough token estimate for budgeting batches (≈4 characters per token for code).
/// It is only used to SIZE batches, never to truncate silently.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

// ── error classification + retry ─────────────────────────────────────────────

#[derive(Debug)]
enum CallError {
    /// Worth retrying; optional server-suggested delay.
    Transient(String, Option<Duration>),
    Fatal(String),
}

fn classify_ureq(e: ureq::Error) -> CallError {
    match e {
        ureq::Error::Status(code, resp) => {
            let retry_after = resp
                .header("retry-after")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map(|s| Duration::from_secs_f64(s.clamp(0.0, 120.0)));
            let body = resp.into_string().unwrap_or_default();
            let msg = format!("HTTP {code}: {}", truncate(&body, 400));
            if matches!(code, 408 | 409 | 429 | 500..=599) {
                CallError::Transient(msg, retry_after)
            } else {
                CallError::Fatal(msg)
            }
        }
        ureq::Error::Transport(t) => CallError::Transient(format!("transport: {t}"), None),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn with_retries<F>(retries: u32, mut call: F) -> Result<String, String>
where
    F: FnMut() -> Result<String, CallError>,
{
    let mut attempt = 0u32;
    loop {
        match call() {
            Ok(s) => return Ok(s),
            Err(CallError::Fatal(m)) => return Err(m),
            Err(CallError::Transient(m, after)) => {
                if attempt >= retries {
                    return Err(format!("{m} (gave up after {} attempts)", attempt + 1));
                }
                let backoff = after.unwrap_or_else(|| Duration::from_secs(2u64.saturating_pow(attempt + 1).min(60)));
                eprintln!("   ai: transient error ({m}); retrying in {:.0}s", backoff.as_secs_f64());
                std::thread::sleep(backoff);
                attempt += 1;
            }
        }
    }
}

// ── Anthropic Messages API ───────────────────────────────────────────────────

pub struct AnthropicModel {
    cfg: ProviderConfig,
    key: String,
    usage: Mutex<Usage>,
}

/// The request body sent to `POST /v1/messages` (pure; unit-tested).
pub fn anthropic_body(cfg: &ProviderConfig, req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({"role": role_str(m.role), "content": m.content}))
        .collect();
    let mut body = json!({
        "model": cfg.model,
        "max_tokens": req.max_tokens.min(cfg.max_tokens).max(1),
        "messages": messages,
    });
    if !req.system.is_empty() {
        body["system"] = json!(req.system);
    }
    if let Some(e) = &cfg.effort {
        body["output_config"] = json!({"effort": e});
    }
    if cfg.fallbacks && cfg.base_url == DEFAULT_ANTHROPIC_BASE_URL {
        body["fallbacks"] = json!("default");
    }
    body
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Extract the reply text from a Messages API response (pure; unit-tested).
/// Thinking blocks are skipped; a refusal is an error, a `max_tokens` stop is an
/// error too (a cut-off JSON answer is worse than none).
pub fn anthropic_parse(v: &Value) -> Result<(String, Usage), String> {
    let stop = v.get("stop_reason").and_then(|s| s.as_str()).unwrap_or("");
    if stop == "refusal" {
        let cat = v
            .pointer("/stop_details/category")
            .and_then(|c| c.as_str())
            .unwrap_or("unspecified");
        return Err(format!("model declined the request (refusal, category {cat})"));
    }
    let mut text = String::new();
    if let Some(blocks) = v.get("content").and_then(|c| c.as_array()) {
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                text.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or(""));
            }
        }
    }
    if stop == "max_tokens" {
        return Err("reply cut off at max_tokens".into());
    }
    let usage = Usage {
        calls: 1,
        input_tokens: v.pointer("/usage/input_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
        output_tokens: v.pointer("/usage/output_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
    };
    Ok((text, usage))
}

impl ChatModel for AnthropicModel {
    fn id(&self) -> String {
        self.cfg.id()
    }
    fn chat(&self, req: &ChatRequest) -> Result<String, String> {
        let body = anthropic_body(&self.cfg, req);
        let url = format!("{}/v1/messages", self.cfg.base_url);
        with_retries(self.cfg.retries, || {
            let mut r = ureq::post(&url)
                .timeout(self.cfg.timeout)
                .set("x-api-key", &self.key)
                .set("anthropic-version", ANTHROPIC_API_VERSION)
                .set("content-type", "application/json");
            if body.get("fallbacks").is_some() {
                r = r.set("anthropic-beta", FALLBACK_BETA);
            }
            let resp = r.send_json(body.clone()).map_err(classify_ureq)?;
            let v: Value = resp
                .into_json()
                .map_err(|e| CallError::Transient(format!("bad JSON from API: {e}"), None))?;
            let (text, u) = anthropic_parse(&v).map_err(CallError::Fatal)?;
            let mut acc = self.usage.lock().unwrap();
            acc.calls += 1;
            acc.input_tokens += u.input_tokens;
            acc.output_tokens += u.output_tokens;
            Ok(text)
        })
    }
}

// ── OpenAI-compatible chat/completions (Ollama, llama.cpp, vLLM, …) ───────────

pub struct OpenAiCompatModel {
    cfg: ProviderConfig,
    key: Option<String>,
    usage: Mutex<Usage>,
}

pub fn openai_body(cfg: &ProviderConfig, req: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if !req.system.is_empty() {
        messages.push(json!({"role": "system", "content": req.system}));
    }
    for m in &req.messages {
        messages.push(json!({"role": role_str(m.role), "content": m.content}));
    }
    json!({
        "model": cfg.model,
        "max_tokens": req.max_tokens.min(cfg.max_tokens).max(1),
        "messages": messages,
        "stream": false,
    })
}

pub fn openai_parse(v: &Value) -> Result<(String, Usage), String> {
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| format!("no choices in reply: {}", truncate(&v.to_string(), 300)))?;
    if choice.get("finish_reason").and_then(|f| f.as_str()) == Some("length") {
        return Err("reply cut off at max_tokens".into());
    }
    let text = choice
        .pointer("/message/content")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let usage = Usage {
        calls: 1,
        input_tokens: v.pointer("/usage/prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
        output_tokens: v.pointer("/usage/completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
    };
    Ok((text, usage))
}

impl ChatModel for OpenAiCompatModel {
    fn id(&self) -> String {
        self.cfg.id()
    }
    fn chat(&self, req: &ChatRequest) -> Result<String, String> {
        let body = openai_body(&self.cfg, req);
        let url = format!("{}/chat/completions", self.cfg.base_url);
        with_retries(self.cfg.retries, || {
            let mut r = ureq::post(&url)
                .timeout(self.cfg.timeout)
                .set("content-type", "application/json");
            if let Some(k) = &self.key {
                r = r.set("Authorization", &format!("Bearer {k}"));
            }
            let resp = r.send_json(body.clone()).map_err(classify_ureq)?;
            let v: Value = resp
                .into_json()
                .map_err(|e| CallError::Transient(format!("bad JSON from server: {e}"), None))?;
            let (text, u) = openai_parse(&v).map_err(CallError::Fatal)?;
            let mut acc = self.usage.lock().unwrap();
            acc.calls += 1;
            acc.input_tokens += u.input_tokens;
            acc.output_tokens += u.output_tokens;
            Ok(text)
        })
    }
}

// ── on-disk response cache ───────────────────────────────────────────────────

/// Stable cache key: SHA-256 over the model id and the complete request.  The
/// request carries the code being asked about, so the key changes whenever the code,
/// the prompt or the model changes — and never otherwise.
pub fn cache_key(model_id: &str, req: &ChatRequest) -> String {
    let mut h = Sha256::new();
    h.update(model_id.as_bytes());
    h.update([0u8]);
    h.update(serde_json::to_vec(req).unwrap_or_default());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Wraps a model with a directory of `<key>.json` replies.  Errors are never cached.
/// In `offline` mode a miss is an error instead of a request — that is what
/// `--dry-run` uses, so a dry run can still show cached answers.
pub struct CachedModel<'a> {
    inner: Option<&'a dyn ChatModel>,
    id: String,
    dir: PathBuf,
    hits: Mutex<u64>,
    misses: Mutex<u64>,
}

pub const CACHE_MISS_OFFLINE: &str = "cache miss (offline)";

impl<'a> CachedModel<'a> {
    pub fn new(inner: &'a dyn ChatModel, dir: &Path) -> Self {
        CachedModel { id: inner.id(), inner: Some(inner), dir: dir.to_path_buf(), hits: Mutex::new(0), misses: Mutex::new(0) }
    }
    /// Cache-only: answers from disk, never calls a model.
    pub fn offline(model_id: &str, dir: &Path) -> Self {
        CachedModel { id: model_id.to_string(), inner: None, dir: dir.to_path_buf(), hits: Mutex::new(0), misses: Mutex::new(0) }
    }
    pub fn path_for(&self, req: &ChatRequest) -> PathBuf {
        self.dir.join(format!("{}.json", cache_key(&self.id, req)))
    }
    pub fn lookup(&self, req: &ChatRequest) -> Option<String> {
        let text = std::fs::read_to_string(self.path_for(req)).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        v.get("reply").and_then(|r| r.as_str()).map(|s| s.to_string())
    }
    pub fn stats(&self) -> (u64, u64) {
        (*self.hits.lock().unwrap(), *self.misses.lock().unwrap())
    }
}

impl ChatModel for CachedModel<'_> {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn chat(&self, req: &ChatRequest) -> Result<String, String> {
        if let Some(r) = self.lookup(req) {
            *self.hits.lock().unwrap() += 1;
            return Ok(r);
        }
        *self.misses.lock().unwrap() += 1;
        let inner = self.inner.ok_or_else(|| CACHE_MISS_OFFLINE.to_string())?;
        let reply = inner.chat(req)?;
        let _ = std::fs::create_dir_all(&self.dir);
        let rec = json!({"model": self.id, "reply": reply});
        let _ = std::fs::write(self.path_for(req), serde_json::to_string_pretty(&rec).unwrap_or_default());
        Ok(reply)
    }
}

/// Pull the first JSON object out of a model reply: tolerates ```json fences and
/// prose around the object.
pub fn extract_json(reply: &str) -> Result<Value, String> {
    let s = reply.trim();
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return Ok(v);
    }
    // scan for a balanced {...}, honouring strings
    let bytes: Vec<char> = s.chars().collect();
    let mut start = None;
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &c) in bytes.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' if start.is_some() => in_str = true,
            '{' => {
                if start.is_none() {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' if start.is_some() => {
                depth -= 1;
                if depth == 0 {
                    let cand: String = bytes[start.unwrap()..=i].iter().collect();
                    return serde_json::from_str(&cand).map_err(|e| format!("invalid JSON in reply: {e}"));
                }
            }
            _ => {}
        }
    }
    Err(format!("no JSON object in reply: {}", truncate(s, 200)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_model::{ChatMessage, ScriptedModel};

    fn req(text: &str) -> ChatRequest {
        ChatRequest { system: "sys".into(), messages: vec![ChatMessage::user(text)], max_tokens: 100 }
    }

    #[test]
    fn anthropic_body_shape() {
        let cfg = ProviderConfig::default();
        let b = anthropic_body(&cfg, &req("hi"));
        assert_eq!(b["model"], "claude-sonnet-5-5");
        assert_eq!(b["system"], "sys");
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["fallbacks"], "default");
        assert!(b.get("temperature").is_none());
        let mut local = cfg.clone();
        local.base_url = "http://127.0.0.1:9".into();
        assert!(anthropic_body(&local, &req("hi")).get("fallbacks").is_none());
    }

    #[test]
    fn anthropic_parse_skips_thinking_and_flags_refusal() {
        let ok = json!({"stop_reason":"end_turn","content":[{"type":"thinking","thinking":""},{"type":"text","text":"{\"a\":1}"}],
                        "usage":{"input_tokens":10,"output_tokens":3}});
        let (t, u) = anthropic_parse(&ok).unwrap();
        assert_eq!(t, "{\"a\":1}");
        assert_eq!(u.input_tokens, 10);
        let refused = json!({"stop_reason":"refusal","stop_details":{"category":"cyber"},"content":[]});
        assert!(anthropic_parse(&refused).unwrap_err().contains("cyber"));
        let cut = json!({"stop_reason":"max_tokens","content":[{"type":"text","text":"{"}]});
        assert!(anthropic_parse(&cut).is_err());
    }

    #[test]
    fn openai_parse_reads_choice() {
        let v = json!({"choices":[{"message":{"content":"x"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":1}});
        assert_eq!(openai_parse(&v).unwrap().0, "x");
    }

    #[test]
    fn remote_detection() {
        assert!(!is_remote_url("http://localhost:11434/v1"));
        assert!(!is_remote_url("http://127.0.0.1:8080"));
        assert!(!is_remote_url("http://[::1]:8080/v1"));
        assert!(is_remote_url("https://api.anthropic.com"));
    }

    #[test]
    fn retries_transient_then_succeeds_and_stops_on_fatal() {
        let mut n = 0;
        let r = with_retries(3, || {
            n += 1;
            if n < 2 {
                Err(CallError::Transient("busy".into(), Some(Duration::from_millis(1))))
            } else {
                Ok("done".into())
            }
        });
        assert_eq!(r.unwrap(), "done");
        let mut m = 0;
        let r = with_retries(3, || {
            m += 1;
            Err(CallError::Fatal("bad key".into()))
        });
        assert!(r.is_err());
        assert_eq!(m, 1);
    }

    #[test]
    fn cache_hits_and_offline_miss() {
        let dir = crate::util::make_temp_dir("arl-ai-cache").unwrap();
        let inner = ScriptedModel::new(["first", "second"]);
        let c = CachedModel::new(&inner, &dir);
        assert_eq!(c.chat(&req("q")).unwrap(), "first");
        assert_eq!(c.chat(&req("q")).unwrap(), "first"); // cached, inner not called
        assert_eq!(inner.requests().len(), 1);
        let off = CachedModel::offline(&inner.id(), &dir);
        assert_eq!(off.chat(&req("q")).unwrap(), "first");
        assert_eq!(off.chat(&req("other")).unwrap_err(), CACHE_MISS_OFFLINE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_extraction() {
        assert_eq!(extract_json("```json\n{\"a\": \"}\"}\n```").unwrap()["a"], "}");
        assert_eq!(extract_json("Sure: {\"b\":2} done").unwrap()["b"], 2);
        assert!(extract_json("nothing").is_err());
    }

    #[test]
    fn resolve_flags_beat_toml() {
        let dir = crate::util::make_temp_dir("arl-ai-cfg").unwrap();
        let p = dir.join("c.toml");
        std::fs::write(&p, "[ai]\nprovider = \"openai\"\nmodel = \"m-toml\"\nmax_tokens = 900\n").unwrap();
        let o = ProviderOverrides { config: Some(p.clone()), ..Default::default() };
        let c = ProviderConfig::resolve(&o).unwrap();
        assert_eq!(c.kind, ProviderKind::OpenAi);
        assert_eq!(c.max_tokens, 900);
        assert_eq!(c.base_url, DEFAULT_OPENAI_BASE_URL);
        let o2 = ProviderOverrides { config: Some(p), model: Some("m-flag".into()), ..Default::default() };
        if std::env::var("ARLECCHINO_AI_MODEL").is_err() {
            assert_eq!(ProviderConfig::resolve(&o2).unwrap().model, "m-flag");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
