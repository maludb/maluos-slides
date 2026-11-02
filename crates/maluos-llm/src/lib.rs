//! MaluOS LLM client: one trait over several model providers, for MaluOS Slides' assistant and
//! for Malu123 (plan decision D6).
//!
//! The crate is deliberately small and synchronous: no async runtime, no SDK. A backend builds a
//! request, posts it through a [`Transport`], reads the Server-Sent Events of the reply as they
//! arrive and reports them through an [`Event`] callback, then returns the whole [`Completion`].
//! The caller (the assistant pane, the CLI, a test) runs it on a worker thread and polls events.
//!
//! Backends:
//! - [`AnthropicClient`]: the Anthropic Messages API (`POST /v1/messages`), streaming, tools,
//!   thinking blocks passed back unchanged, refusal fallbacks.
//! - [`OpenAiClient`]: OpenAI Chat Completions and every server that speaks that protocol:
//!   Ollama (`http://127.0.0.1:11434/v1`), the MaluOS gateway, vLLM, LM Studio.
//! - [`mock::MockClient`] and [`mock::MockTransport`]: scripted replies for tests.
//!
//! Keys come from the environment ([`keys`]) or from the caller; the crate never reads a settings
//! file or a deck. On wasm32 the `native` feature (the ureq transport) is off, and a host supplies
//! its own [`Transport`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod anthropic;
pub mod keys;
pub mod mock;
pub mod openai;
pub mod sse;
pub mod transport;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod ureq_transport;

pub use anthropic::AnthropicClient;
pub use openai::OpenAiClient;
pub use transport::{HttpRequest, HttpResponse, Transport};
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub use ureq_transport::UreqTransport;

/// Who said it. System text is a field of the [`Request`], not a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// One turn of the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// A user turn of plain text.
    pub fn user(text: impl Into<String>) -> Self {
        Message { role: Role::User, content: vec![ContentBlock::Text { text: text.into() }] }
    }

    /// An assistant turn of plain text.
    pub fn assistant(text: impl Into<String>) -> Self {
        Message { role: Role::Assistant, content: vec![ContentBlock::Text { text: text.into() }] }
    }

    /// A user turn carrying the results of the tool calls of the previous assistant turn.
    pub fn tool_results(results: Vec<ContentBlock>) -> Self {
        Message { role: Role::User, content: results }
    }
}

/// A piece of a message. Providers that lack a kind drop it (OpenAI has no thinking blocks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Reasoning the model returned. Pass it back unchanged in the assistant message, with its
    /// signature: Anthropic validates it, and a model reads only blocks it produced itself.
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// The model asks the caller to run a tool.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// The caller answers a [`ContentBlock::ToolUse`].
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    /// A provider-specific block kept verbatim (a redacted thinking block, a fallback marker). It
    /// goes back to the provider that produced it and is dropped for every other one.
    Raw {
        provider: String,
        value: Value,
    },
}

/// A tool the model may call, in JSON Schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// A JSON Schema object (`{"type": "object", "properties": …, "required": …}`).
    pub input_schema: Value,
}

/// How hard the model should think (where the provider offers the knob).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// One completion request. Provider-neutral; each backend maps it to its wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    /// The output ceiling. Backends stream, so a large value costs nothing until used.
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
}

impl Request {
    /// A request with the given model and one user message.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Request { model: model.into(), system: None, messages, tools: Vec::new(), max_tokens: 16_000, effort: None }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = Some(effort);
        self
    }
}

/// Token counts of one completion. Zero where the provider does not report a field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens served from the provider's prompt cache (part of `input_tokens` for OpenAI,
    /// reported separately by Anthropic).
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The answer is complete.
    EndTurn,
    /// The model wants tool results: answer every [`ContentBlock::ToolUse`] and call again.
    ToolUse,
    /// `max_tokens` cut the answer; its last block may be incomplete.
    MaxTokens,
    /// A safety system declined. The content so far is kept.
    Refusal,
    /// A reason this crate does not know, as the provider spelled it.
    Other(String),
}

/// The whole answer, after the stream ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Completion {
    /// The model that produced the answer (a fallback model may differ from the one requested).
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

impl Completion {
    /// All text blocks joined, in order.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            if let ContentBlock::Text { text } = block {
                out.push_str(text);
            }
        }
        out
    }

    /// The tool calls the model made, in order.
    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }

    /// The assistant message to append to the conversation before the next request.
    pub fn as_message(&self) -> Message {
        Message { role: Role::Assistant, content: self.content.clone() }
    }
}

/// What a backend reports while the answer streams in. Text and tool input arrive in pieces;
/// the [`Completion`] returned at the end has them whole.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    TextDelta(String),
    ThinkingDelta(String),
    ToolUseStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolInputDelta {
        index: usize,
        partial_json: String,
    },
    /// Usage so far (providers report input tokens first, output tokens at the end).
    Usage(Usage),
}

/// The event callback's answer: keep streaming, or stop and return [`LlmError::Cancelled`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

/// What can go wrong. [`LlmError::retryable`] tells a loop whether waiting and trying again
/// makes sense.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    #[error("no API key: {0}")]
    NoApiKey(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("rate limited: {message}")]
    RateLimited { message: String, retry_after: Option<Duration> },
    #[error("provider overloaded: {0}")]
    Overloaded(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("provider error {status}: {message}")]
    Server { status: u16, message: String },
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("transport: {0}")]
    Transport(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
}

impl LlmError {
    /// Whether the same request may succeed after a pause.
    pub fn retryable(&self) -> bool {
        matches!(self, LlmError::RateLimited { .. } | LlmError::Overloaded(_) | LlmError::Server { .. } | LlmError::Transport(_))
    }

    /// Classify an HTTP error status with the message its body carried. Both providers use the
    /// same codes for the same conditions.
    pub fn from_status(status: u16, message: String) -> Self {
        match status {
            401 | 403 => LlmError::Auth(message),
            404 => LlmError::NotFound(message),
            429 => LlmError::RateLimited { message, retry_after: None },
            529 | 503 => LlmError::Overloaded(message),
            400..=499 => LlmError::BadRequest(message),
            500..=599 => LlmError::Server { status, message },
            _ => LlmError::Http { status, message },
        }
    }
}

/// A model provider. One call is one request; the conversation is the caller's.
pub trait LlmClient: Send + Sync {
    /// A short name for the UI and logs: `anthropic`, `openai`, `ollama`, `mock`.
    fn provider(&self) -> &str;

    /// Run one request. `on_event` is called on the current thread as the answer streams in;
    /// returning [`Flow::Stop`] ends the request with [`LlmError::Cancelled`].
    fn complete(&self, request: &Request, on_event: &mut dyn FnMut(Event) -> Flow) -> Result<Completion, LlmError>;

    /// Run one request and ignore the stream.
    fn complete_blocking(&self, request: &Request) -> Result<Completion, LlmError> {
        self.complete(request, &mut |_| Flow::Continue)
    }
}

/// Parse `{"error": {"type": …, "message": …}}` (both providers) into the message, falling
/// back to the raw body, shortened.
pub(crate) fn error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let error = v.get("error").unwrap_or(&v);
        if let Some(message) = error.get("message").and_then(Value::as_str) {
            let kind = error.get("type").and_then(Value::as_str).or_else(|| error.get("code").and_then(Value::as_str));
            return match kind {
                Some(kind) => format!("{kind}: {message}"),
                None => message.to_string(),
            };
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "no details".to_string();
    }
    let mut end = trimmed.len().min(300);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

#[cfg(test)]
mod tests;
