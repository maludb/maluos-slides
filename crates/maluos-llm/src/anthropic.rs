//! The Anthropic Messages API (`POST /v1/messages`, streaming).
//!
//! Wire shapes follow the API reference of 2026-10: `anthropic-version: 2026-10-21`, adaptive
//! thinking by default on current models (the `thinking` parameter is omitted unless a display
//! mode is set), `output_config.effort`, tools with `input_schema`, and the server-side refusal
//! fallback (`fallbacks: "default"` under its beta header) on by default so a safety decline is
//! re-run on another model instead of ending the turn.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::sse::{SseEvent, read_events};
use crate::transport::{HttpRequest, Transport, error_from_response};
use crate::{Completion, ContentBlock, Event, Flow, LlmClient, LlmError, Message, Request, Role, StopReason, Usage};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const API_VERSION: &str = "2026-10-21";
/// The beta that enables `fallbacks: "default"`.
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// The model used when the caller names none.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const PROVIDER: &str = "anthropic";

/// A client for one API key.
pub struct AnthropicClient {
    transport: Arc<dyn Transport>,
    base_url: String,
    api_key: String,
    betas: Vec<String>,
    refusal_fallbacks: bool,
    thinking_display: Option<String>,
}

impl AnthropicClient {
    pub fn new(transport: Arc<dyn Transport>, api_key: impl Into<String>) -> Self {
        AnthropicClient {
            transport,
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.into(),
            betas: Vec::new(),
            refusal_fallbacks: true,
            thinking_display: None,
        }
    }

    /// The native transport and the key from the environment ([`crate::keys`]).
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    pub fn from_env() -> Result<Self, LlmError> {
        let key = crate::keys::api_key_from_env(crate::keys::Provider::Anthropic)?;
        Ok(Self::new(Arc::new(crate::UreqTransport::default()), key))
    }

    /// Another host (a proxy, a test server). No trailing slash.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        while self.base_url.ends_with('/') {
            self.base_url.pop();
        }
        self
    }

    /// Whether a refusal is re-run on a fallback model inside the same call (default: yes).
    pub fn with_refusal_fallbacks(mut self, on: bool) -> Self {
        self.refusal_fallbacks = on;
        self
    }

    /// An extra `anthropic-beta` value.
    pub fn with_beta(mut self, beta: impl Into<String>) -> Self {
        self.betas.push(beta.into());
        self
    }

    /// `"summarized"` streams readable reasoning as [`Event::ThinkingDelta`]; the API's default,
    /// `"omitted"`, streams empty thinking blocks. Thinking itself is on either way.
    pub fn with_thinking_display(mut self, display: Option<String>) -> Self {
        self.thinking_display = display;
        self
    }

    /// The HTTP request for `request`, as it would be sent.
    pub fn build_request(&self, request: &Request) -> HttpRequest {
        let mut body = Map::new();
        body.insert("model".into(), Value::String(request.model.clone()));
        body.insert("max_tokens".into(), json!(request.max_tokens));
        body.insert("stream".into(), Value::Bool(true));
        if let Some(system) = &request.system {
            body.insert("system".into(), Value::String(system.clone()));
        }
        body.insert("messages".into(), Value::Array(request.messages.iter().map(message_json).collect()));
        if !request.tools.is_empty() {
            let tools = request.tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema})).collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        if let Some(effort) = request.effort {
            body.insert("output_config".into(), json!({"effort": effort.as_str()}));
        }
        if let Some(display) = &self.thinking_display {
            body.insert("thinking".into(), json!({"type": "adaptive", "display": display}));
        }
        let mut betas = self.betas.clone();
        if self.refusal_fallbacks {
            body.insert("fallbacks".into(), Value::String("default".into()));
            betas.push(FALLBACK_BETA.to_string());
        }
        let mut headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("x-api-key".to_string(), self.api_key.clone()),
            ("anthropic-version".to_string(), API_VERSION.to_string()),
        ];
        if !betas.is_empty() {
            headers.push(("anthropic-beta".to_string(), betas.join(",")));
        }
        HttpRequest { url: format!("{}/v1/messages", self.base_url), headers, body: Value::Object(body).to_string() }
    }
}

fn message_json(message: &Message) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content: Vec<Value> = message.content.iter().filter_map(block_json).collect();
    json!({"role": role, "content": content})
}

fn block_json(block: &ContentBlock) -> Option<Value> {
    Some(match block {
        ContentBlock::Text { text } => json!({"type": "text", "text": text}),
        // An unsigned thinking block came from another provider; the API would reject it.
        ContentBlock::Thinking { signature: None, .. } => return None,
        ContentBlock::Thinking { text, signature: Some(signature) } => json!({"type": "thinking", "thinking": text, "signature": signature}),
        ContentBlock::ToolUse { id, name, input } => json!({"type": "tool_use", "id": id, "name": name, "input": input}),
        ContentBlock::ToolResult { tool_use_id, content, is_error, images } => {
            let content_json = if images.is_empty() {
                json!(content)
            } else {
                let mut parts = vec![json!({"type": "text", "text": content})];
                parts.extend(images.iter().map(image_json));
                Value::Array(parts)
            };
            let mut v = json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content_json});
            if *is_error && let Some(map) = v.as_object_mut() {
                map.insert("is_error".into(), Value::Bool(true));
            }
            v
        }
        ContentBlock::Image { media_type, data } => image_json(&crate::Image { media_type: media_type.clone(), data: data.clone() }),
        ContentBlock::Raw { provider, value } if provider == PROVIDER => value.clone(),
        ContentBlock::Raw { .. } => return None,
    })
}

fn image_json(img: &crate::Image) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": img.media_type, "data": img.data}})
}

impl LlmClient for AnthropicClient {
    fn provider(&self) -> &str {
        PROVIDER
    }

    fn complete(&self, request: &Request, on_event: &mut dyn FnMut(Event) -> Flow) -> Result<Completion, LlmError> {
        let http = self.build_request(request);
        let response = self.transport.post(&http)?;
        if !(200..300).contains(&response.status) {
            return Err(error_from_response(response));
        }
        let mut stream = Stream::new(request.model.clone());
        read_events(response.body, &mut |event| stream.handle(event, on_event))?;
        stream.finish()
    }
}

/// One content block as it streams in.
#[derive(Debug)]
enum Block {
    Text(String),
    Thinking { text: String, signature: Option<String> },
    ToolUse { id: String, name: String, json: String },
    Raw(Value),
}

/// The state of one streamed message.
#[derive(Debug)]
struct Stream {
    model: String,
    blocks: Vec<Block>,
    stop: Option<StopReason>,
    usage: Usage,
    stopped: bool,
}

impl Stream {
    fn new(model: String) -> Self {
        Stream { model, blocks: Vec::new(), stop: None, usage: Usage::default(), stopped: false }
    }

    fn handle(&mut self, event: SseEvent, on_event: &mut dyn FnMut(Event) -> Flow) -> Result<Flow, LlmError> {
        let data: Value = serde_json::from_str(&event.data).map_err(|e| LlmError::Protocol(format!("event `{}` is not JSON: {e}", event.event)))?;
        let kind = data.get("type").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| event.event.clone());
        match kind.as_str() {
            "message_start" => {
                let message = data.get("message").cloned().unwrap_or(Value::Null);
                if let Some(model) = message.get("model").and_then(Value::as_str) {
                    self.model = model.to_string();
                }
                if let Some(usage) = message.get("usage") {
                    self.merge_usage(usage);
                    return Ok(on_event(Event::Usage(self.usage)));
                }
            }
            "content_block_start" => {
                let index = index_of(&data)?;
                if index != self.blocks.len() {
                    return Err(LlmError::Protocol(format!("content block {index} started but {} blocks exist", self.blocks.len())));
                }
                let block = data.get("content_block").cloned().unwrap_or(Value::Null);
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => self.blocks.push(Block::Text(block.get("text").and_then(Value::as_str).unwrap_or("").to_string())),
                    "thinking" => self.blocks.push(Block::Thinking {
                        text: block.get("thinking").and_then(Value::as_str).unwrap_or("").to_string(),
                        signature: block.get("signature").and_then(Value::as_str).map(str::to_string),
                    }),
                    "tool_use" => {
                        let id = block.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        self.blocks.push(Block::ToolUse { id: id.clone(), name: name.clone(), json: String::new() });
                        return Ok(on_event(Event::ToolUseStart { index, id, name }));
                    }
                    _ => self.blocks.push(Block::Raw(block)),
                }
            }
            "content_block_delta" => {
                let index = index_of(&data)?;
                let Some(block) = self.blocks.get_mut(index) else {
                    return Err(LlmError::Protocol(format!("delta for content block {index}, which has not started")));
                };
                let delta = data.get("delta").cloned().unwrap_or(Value::Null);
                match (delta.get("type").and_then(Value::as_str).unwrap_or(""), block) {
                    ("text_delta", Block::Text(text)) => {
                        let piece = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        text.push_str(piece);
                        return Ok(on_event(Event::TextDelta(piece.to_string())));
                    }
                    ("thinking_delta", Block::Thinking { text, .. }) => {
                        let piece = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        text.push_str(piece);
                        return Ok(on_event(Event::ThinkingDelta(piece.to_string())));
                    }
                    ("signature_delta", Block::Thinking { signature, .. }) => {
                        let piece = delta.get("signature").and_then(Value::as_str).unwrap_or("");
                        signature.get_or_insert_with(String::new).push_str(piece);
                    }
                    ("input_json_delta", Block::ToolUse { json, .. }) => {
                        let piece = delta.get("partial_json").and_then(Value::as_str).unwrap_or("");
                        json.push_str(piece);
                        return Ok(on_event(Event::ToolInputDelta { index, partial_json: piece.to_string() }));
                    }
                    (other, _) => log::debug!("anthropic: ignoring delta `{other}` on block {index}"),
                }
            }
            "content_block_stop" | "ping" => {}
            "message_delta" => {
                if let Some(reason) = data.get("delta").and_then(|d| d.get("stop_reason")).and_then(Value::as_str) {
                    self.stop = Some(stop_reason(reason));
                }
                if let Some(usage) = data.get("usage") {
                    self.merge_usage(usage);
                    return Ok(on_event(Event::Usage(self.usage)));
                }
            }
            "message_stop" => self.stopped = true,
            "error" => {
                let error = data.get("error").cloned().unwrap_or(Value::Null);
                let message = error.get("message").and_then(Value::as_str).unwrap_or("no details").to_string();
                return Err(match error.get("type").and_then(Value::as_str).unwrap_or("") {
                    "overloaded_error" => LlmError::Overloaded(message),
                    "rate_limit_error" => LlmError::RateLimited { message, retry_after: None },
                    "authentication_error" | "permission_error" => LlmError::Auth(message),
                    "invalid_request_error" => LlmError::BadRequest(message),
                    "not_found_error" => LlmError::NotFound(message),
                    "api_error" => LlmError::Server { status: 500, message },
                    other => LlmError::Protocol(format!("{other}: {message}")),
                });
            }
            other => log::debug!("anthropic: ignoring event `{other}`"),
        }
        Ok(Flow::Continue)
    }

    fn merge_usage(&mut self, usage: &Value) {
        let get = |key: &str| usage.get(key).and_then(Value::as_u64);
        if let Some(n) = get("input_tokens") {
            self.usage.input_tokens = n;
        }
        if let Some(n) = get("output_tokens") {
            self.usage.output_tokens = n;
        }
        if let Some(n) = get("cache_read_input_tokens") {
            self.usage.cache_read_input_tokens = n;
        }
        if let Some(n) = get("cache_creation_input_tokens") {
            self.usage.cache_creation_input_tokens = n;
        }
    }

    fn finish(self) -> Result<Completion, LlmError> {
        if !self.stopped {
            return Err(LlmError::Protocol("the stream ended before message_stop".into()));
        }
        let Some(stop_reason) = self.stop else {
            return Err(LlmError::Protocol("the stream carried no stop_reason".into()));
        };
        let mut content = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            content.push(match block {
                Block::Text(text) => ContentBlock::Text { text },
                Block::Thinking { text, signature } => ContentBlock::Thinking { text, signature },
                Block::ToolUse { id, name, json } => {
                    let input = if json.trim().is_empty() {
                        Value::Object(Map::new())
                    } else {
                        serde_json::from_str(&json).map_err(|e| LlmError::Protocol(format!("tool `{name}` input is not valid JSON: {e}")))?
                    };
                    ContentBlock::ToolUse { id, name, input }
                }
                Block::Raw(value) => ContentBlock::Raw { provider: PROVIDER.to_string(), value },
            });
        }
        Ok(Completion { model: self.model, content, stop_reason, usage: self.usage })
    }
}

fn index_of(data: &Value) -> Result<usize, LlmError> {
    data.get("index").and_then(Value::as_u64).map(|i| i as usize).ok_or_else(|| LlmError::Protocol("content block event without an index".into()))
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "refusal" => StopReason::Refusal,
        other => StopReason::Other(other.to_string()),
    }
}
