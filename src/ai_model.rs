//! Minimal chat-model interface shared by the optional AI stages.
//!
//! This file deliberately holds ONLY the contract: a request shape and a trait.
//! Network transport, response caching, API keys and provider selection live in the
//! provider module, which implements [`ChatModel`]; a stage such as `ai-fix` is
//! written against the trait alone, so it can be driven by a real provider, by a
//! local OpenAI-compatible server, or by the scripted model used in tests — and it
//! never carries an HTTP client of its own.
//!
//! Every AI stage is OFF by default and only runs from an explicit subcommand.

use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: Role::User, content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage { role: Role::Assistant, content: content.into() }
    }
}

/// One completion request: a system prompt plus an alternating user/assistant
/// conversation that ends with a user turn.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
}

/// A text model.  `chat` returns the assistant's reply text, or a human-readable
/// error (network failure, refusal, missing key, ...).
pub trait ChatModel {
    /// Short identifier for logs and journals, e.g. `anthropic:claude-sonnet-5-5`.
    fn id(&self) -> String;
    fn chat(&self, req: &ChatRequest) -> Result<String, String>;
}

/// Replays a fixed list of replies, one per call, and records every request.
/// When the script runs out it keeps returning the last reply (or an error if the
/// script was empty).  Used by the test-suite; never talks to the network.
pub struct ScriptedModel {
    replies: Mutex<VecDeque<Result<String, String>>>,
    last: Mutex<Option<Result<String, String>>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl ScriptedModel {
    pub fn new<I, S>(replies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::with_results(replies.into_iter().map(|s| Ok(s.into())))
    }

    pub fn with_results<I>(replies: I) -> Self
    where
        I: IntoIterator<Item = Result<String, String>>,
    {
        ScriptedModel {
            replies: Mutex::new(replies.into_iter().collect()),
            last: Mutex::new(None),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl ChatModel for ScriptedModel {
    fn id(&self) -> String {
        "scripted".into()
    }
    fn chat(&self, req: &ChatRequest) -> Result<String, String> {
        self.seen.lock().unwrap().push(req.clone());
        let next = self.replies.lock().unwrap().pop_front();
        let mut last = self.last.lock().unwrap();
        match next {
            Some(r) => {
                *last = Some(r.clone());
                r
            }
            None => last.clone().unwrap_or_else(|| Err("scripted model: no replies".into())),
        }
    }
}
