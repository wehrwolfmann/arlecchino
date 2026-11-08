//! The `claude-code` provider against a stand-in `claude` program — a shell script
//! that records its arguments and stdin and prints a canned answer.  The real CLI is
//! never started here.

use arlecchino::ai::provider::{CachedModel, ClaudeCodeModel, ProviderConfig, ProviderKind};
use arlecchino::ai_model::{ChatMessage, ChatModel, ChatRequest};
use arlecchino::util;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A fake `claude` in a fresh directory.  `body` is the script after the lines that
/// save argv (one per line) to `args` and stdin to `stdin`, and bump `calls`.
fn fake(tag: &str, body: &str) -> (PathBuf, PathBuf) {
    let dir = util::make_temp_dir(&format!("arl-cc-{tag}")).unwrap();
    let bin = dir.join("claude");
    let script = format!(
        "#!/bin/sh\nd='{d}'\nprintf '%s\\n' \"$@\" > \"$d/args\"\ncat > \"$d/stdin\"\necho x >> \"$d/calls\"\n{body}\n",
        d = dir.display()
    );
    std::fs::write(&bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    (dir, bin)
}

fn cfg(bin: &Path, timeout: Duration) -> ProviderConfig {
    ProviderConfig {
        kind: ProviderKind::ClaudeCode,
        model: String::new(),
        base_url: String::new(),
        claude_bin: bin.to_string_lossy().into_owned(),
        timeout,
        retries: 0,
        fallbacks: false,
        ..Default::default()
    }
}

fn req(text: &str) -> ChatRequest {
    ChatRequest { system: "You name things.".into(), messages: vec![ChatMessage::user(text)], max_tokens: 100 }
}

fn calls(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("calls")).map(|s| s.lines().count()).unwrap_or(0)
}

#[test]
fn success_reads_result_and_sends_prompt_on_stdin_with_tools_off() {
    let (dir, bin) = fake(
        "ok",
        r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"{\"a\":1}","usage":{"input_tokens":7,"output_tokens":2}}'"#,
    );
    let m = ClaudeCodeModel::new(cfg(&bin, Duration::from_secs(30))).unwrap();
    assert_eq!(m.chat(&req("rename class a")).unwrap(), "{\"a\":1}");
    assert_eq!(m.usage().calls, 1);
    assert_eq!(m.usage().input_tokens, 7);
    assert_eq!(std::fs::read_to_string(dir.join("stdin")).unwrap(), "rename class a");
    let args: Vec<String> = std::fs::read_to_string(dir.join("args")).unwrap().lines().map(String::from).collect();
    let i = args.iter().position(|a| a == "--tools").expect("--tools");
    assert_eq!(args[i + 1], "", "every tool disabled");
    for want in ["-p", "--output-format", "json", "--strict-mcp-config", "--no-session-persistence"] {
        assert!(args.iter().any(|a| a == want), "missing {want} in {args:?}");
    }
    assert!(!args.iter().any(|a| a == "--model"), "no model unless one is configured");
    assert!(!args.iter().any(|a| a.contains("rename class a")), "prompt must not be on the command line");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reported_error_and_usage_limit_are_clear_and_not_retried() {
    let (dir, bin) = fake(
        "err",
        r#"echo '{"type":"result","subtype":"success","is_error":true,"result":"Claude AI usage limit reached|1762600000"}'; exit 1"#,
    );
    let mut c = cfg(&bin, Duration::from_secs(30));
    c.retries = 3;
    let m = ClaudeCodeModel::new(c).unwrap();
    let e = m.chat(&req("q")).unwrap_err();
    assert!(e.contains("usage limit"), "{e}");
    assert_eq!(calls(&dir), 1, "a usage limit is not retried");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hung_program_is_killed_at_the_timeout() {
    let (dir, bin) = fake("hang", "sleep 30");
    let m = ClaudeCodeModel::new(cfg(&bin, Duration::from_secs(1))).unwrap();
    let t = Instant::now();
    let e = m.chat(&req("q")).unwrap_err();
    assert!(e.contains("timed out"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(10), "took {:?}", t.elapsed());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn garbage_output_is_an_error_that_shows_what_came_back() {
    let (dir, bin) = fake("junk", "echo 'Welcome! something went sideways'; echo oops >&2; exit 3");
    let m = ClaudeCodeModel::new(cfg(&bin, Duration::from_secs(30))).unwrap();
    let e = m.chat(&req("q")).unwrap_err();
    assert!(e.contains("no JSON") && e.contains("exit 3") && e.contains("sideways"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_program_fails_before_any_request() {
    let c = cfg(Path::new("/nonexistent/arlecchino-test/claude"), Duration::from_secs(1));
    let e = ClaudeCodeModel::new(c).err().expect("must fail");
    assert!(e.contains("not found") && e.contains("ARLECCHINO_CLAUDE_BIN"), "{e}");
}

#[test]
fn cache_answers_the_second_time_and_offline_mode_never_runs_it() {
    let (dir, bin) = fake(
        "cache",
        r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"cached-answer"}'"#,
    );
    let cache = dir.join("cache");
    let m = ClaudeCodeModel::new(cfg(&bin, Duration::from_secs(30))).unwrap();
    let c = CachedModel::new(&m, &cache);
    assert_eq!(c.chat(&req("q")).unwrap(), "cached-answer");
    assert_eq!(c.chat(&req("q")).unwrap(), "cached-answer");
    assert_eq!(calls(&dir), 1);
    // --dry-run path: cache only
    let off = CachedModel::offline(&m.id(), &cache);
    assert_eq!(off.chat(&req("q")).unwrap(), "cached-answer");
    assert!(off.chat(&req("other")).is_err());
    assert_eq!(calls(&dir), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn provider_is_off_unless_asked_for_and_reads_the_program_from_config() {
    let dir = util::make_temp_dir("arl-cc-cfg").unwrap();
    let p = dir.join("ai.toml");
    std::fs::write(&p, "[ai]\nprovider = \"claude-code\"\nclaude_bin = \"/opt/x/claude\"\nclaude_args = [\"--safe-mode\"]\ntimeout_secs = 42\n").unwrap();
    if std::env::var("ARLECCHINO_AI_PROVIDER").is_err() && std::env::var("ARLECCHINO_CLAUDE_BIN").is_err() {
        let d = ProviderConfig::resolve(&Default::default()).unwrap();
        assert_eq!(d.kind, ProviderKind::Anthropic, "claude-code is never the default");
        let o = arlecchino::ai::provider::ProviderOverrides { config: Some(p), ..Default::default() };
        let c = ProviderConfig::resolve(&o).unwrap();
        assert_eq!(c.kind, ProviderKind::ClaudeCode);
        assert_eq!(c.claude_bin, "/opt/x/claude");
        assert_eq!(c.claude_args, vec!["--safe-mode".to_string()]);
        assert_eq!(c.timeout, Duration::from_secs(42));
        assert_eq!(c.id(), "claude-code");
        assert!(c.is_remote(), "the CLI sends the prompt off the machine, so the consent prompt applies");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
