//! OpenAI Chat Completions (`POST {base}/chat/completions`, streaming) and every server that
//! speaks it: Ollama, the MaluOS gateway, vLLM, LM Studio.
//!
//! Two flavours differ in one field: OpenAI's own API takes `max_completion_tokens` and
//! `reasoning_effort`; compatible servers take `max_tokens` and get no effort field.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::sse::{SseEvent, read_events};
use crate::transport::{HttpRequest, Transport, error_from_response};
use crate::{Completion, ContentBlock, Effort, Event, Flow, LlmClient, LlmError, Message, Request, Role, StopReason, Usage};

pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434/v1";
/// The model used when the caller names none.
pub const DEFAULT_MODEL: &str = "gpt-5.1";

/// Which dialect of the protocol the server speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// api.openai.com: `max_completion_tokens`, `reasoning_effort`.
    OpenAi,
    /// Ollama, the MaluOS gateway and other compatible servers: `max_tokens`.
    Compatible,
}

/// A client for one server.
pub struct OpenAiClient {
    transport: Arc<dyn Transport>,
    base_url: String,
    api_key: Option<String>,
    flavor: Flavor,
    provider: String,
}

impl OpenAiClient {
    /// OpenAI's own API.
    pub fn openai(transport: Arc<dyn Transport>, api_key: impl Into<String>) -> Self {
        OpenAiClient { transport, base_url: OPENAI_BASE_URL.into(), api_key: Some(api_key.into()), flavor: Flavor::OpenAi, provider: "openai".into() }
    }

    /// OpenAI with the native transport and the key from the environment ([`crate::keys`]).
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    pub fn openai_from_env() -> Result<Self, LlmError> {
        let key = crate::keys::api_key_from_env(crate::keys::Provider::OpenAi)?;
        Ok(Self::openai(Arc::new(crate::UreqTransport::default()), key))
    }

    /// Any compatible server. `base_url` includes the `/v1` (or whatever prefix the server uses);
    /// `api_key` is sent as a bearer token when given.
    pub fn compatible(transport: Arc<dyn Transport>, provider: impl Into<String>, base_url: impl Into<String>, api_key: Option<String>) -> Self {
        let mut base_url: String = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        OpenAiClient { transport, base_url, api_key, flavor: Flavor::Compatible, provider: provider.into() }
    }

    /// A local Ollama at its default address.
    pub fn ollama(transport: Arc<dyn Transport>) -> Self {
        Self::compatible(transport, "ollama", OLLAMA_BASE_URL, None)
    }

    pub fn flavor(&self) -> Flavor {
        self.flavor
    }

    /// The HTTP request for `request`, as it would be sent.
    pub fn build_request(&self, request: &Request) -> HttpRequest {
        let mut body = Map::new();
        body.insert("model".into(), Value::String(request.model.clone()));
        body.insert("messages".into(), Value::Array(messages_json(request)));
        body.insert("stream".into(), Value::Bool(true));
        body.insert("stream_options".into(), json!({"include_usage": true}));
        let max_key = match self.flavor {
            Flavor::OpenAi => "max_completion_tokens",
            Flavor::Compatible => "max_tokens",
        };
        body.insert(max_key.into(), json!(request.max_tokens));
        if !request.tools.is_empty() {
            let tools = request
                .tools
                .iter()
                .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}}))
                .collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        if let (Flavor::OpenAi, Some(effort)) = (self.flavor, request.effort) {
            let effort = match effort {
                Effort::Low => "low",
                Effort::Medium => "medium",
                Effort::High | Effort::XHigh | Effort::Max => "high",
            };
            body.insert("reasoning_effort".into(), Value::String(effort.into()));
        }
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        if let Some(key) = &self.api_key {
            headers.push(("authorization".to_string(), format!("Bearer {key}")));
        }
        HttpRequest { url: format!("{}/chat/completions", self.base_url), headers, body: Value::Object(body).to_string() }
    }
}

fn messages_json(request: &Request) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(system) = &request.system {
        out.push(json!({"role": "system", "content": system}));
    }
    for message in &request.messages {
        match message.role {
            Role::User => user_json(message, &mut out),
            Role::Assistant => out.push(assistant_json(message)),
        }
    }
    out
}

/// Text and image blocks become one `user` message (an array of parts when an image is in it);
/// each tool result is its own `tool` message, in the order the blocks have. Images inside tool
/// results cannot go in a `tool` message, so they follow the tool messages as one user message.
fn user_json(message: &Message, out: &mut Vec<Value>) {
    let mut parts: Vec<Value> = Vec::new();
    let flush = |parts: &mut Vec<Value>, out: &mut Vec<Value>| {
        if parts.is_empty() {
            return;
        }
        let only_text = parts.iter().all(|p| p.get("type").and_then(Value::as_str) == Some("text"));
        let content = if only_text {
            Value::String(parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().concat())
        } else {
            Value::Array(std::mem::take(parts))
        };
        parts.clear();
        out.push(json!({"role": "user", "content": content}));
    };
    let mut tool_images: Vec<Value> = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text: t } => parts.push(json!({"type": "text", "text": t})),
            ContentBlock::Image { media_type, data } => parts.push(image_part(media_type, data)),
            ContentBlock::ToolResult { tool_use_id, content, images, .. } => {
                flush(&mut parts, out);
                out.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": content}));
                for img in images {
                    if tool_images.is_empty() {
                        tool_images.push(json!({"type": "text", "text": "Images from the tool results above:"}));
                    }
                    tool_images.push(image_part(&img.media_type, &img.data));
                }
            }
            ContentBlock::Thinking { .. } | ContentBlock::ToolUse { .. } | ContentBlock::Raw { .. } => {}
        }
    }
    flush(&mut parts, out);
    if !tool_images.is_empty() {
        out.push(json!({"role": "user", "content": tool_images}));
    }
}

fn image_part(media_type: &str, data: &str) -> Value {
    json!({"type": "image_url", "image_url": {"url": format!("data:{media_type};base64,{data}")}})
}

fn assistant_json(message: &Message) -> Value {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(json!({"id": id, "type": "function", "function": {"name": name, "arguments": input.to_string()}}));
            }
            ContentBlock::Thinking { .. } | ContentBlock::ToolResult { .. } | ContentBlock::Raw { .. } | ContentBlock::Image { .. } => {}
        }
    }
    let mut v = Map::new();
    v.insert("role".into(), "assistant".into());
    v.insert("content".into(), if text.is_empty() { Value::Null } else { Value::String(text) });
    if !tool_calls.is_empty() {
        v.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    Value::Object(v)
}

impl LlmClient for OpenAiClient {
    fn provider(&self) -> &str {
        &self.provider
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

#[derive(Debug)]
enum Block {
    Text(String),
    Thinking(String),
    ToolCall { id: String, name: String, arguments: String, started: bool },
}

#[derive(Debug)]
struct Stream {
    model: String,
    blocks: Vec<Block>,
    /// Block position of each tool call index.
    calls: Vec<usize>,
    stop: Option<StopReason>,
    usage: Usage,
    done: bool,
}

impl Stream {
    fn new(model: String) -> Self {
        Stream { model, blocks: Vec::new(), calls: Vec::new(), stop: None, usage: Usage::default(), done: false }
    }

    fn handle(&mut self, event: SseEvent, on_event: &mut dyn FnMut(Event) -> Flow) -> Result<Flow, LlmError> {
        if event.data.trim() == "[DONE]" {
            self.done = true;
            return Ok(Flow::Continue);
        }
        let chunk: Value = serde_json::from_str(&event.data).map_err(|e| LlmError::Protocol(format!("chunk is not JSON: {e}")))?;
        if let Some(error) = chunk.get("error") {
            let message = error.get("message").and_then(Value::as_str).unwrap_or("no details").to_string();
            let code = error.get("code").and_then(Value::as_u64).map(|c| c as u16);
            return Err(match code {
                Some(status) => LlmError::from_status(status, message),
                None => LlmError::Protocol(crate::error_message(&chunk.to_string())),
            });
        }
        if let Some(model) = chunk.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
            let get = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
            self.usage.input_tokens = get("prompt_tokens");
            self.usage.output_tokens = get("completion_tokens");
            self.usage.cache_read_input_tokens =
                usage.get("prompt_tokens_details").and_then(|d| d.get("cached_tokens")).and_then(Value::as_u64).unwrap_or(0);
            if on_event(Event::Usage(self.usage)) == Flow::Stop {
                return Ok(Flow::Stop);
            }
        }
        let Some(choice) = chunk.get("choices").and_then(Value::as_array).and_then(|c| c.first()) else {
            return Ok(Flow::Continue);
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop = Some(match reason {
                "stop" => StopReason::EndTurn,
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "content_filter" => StopReason::Refusal,
                other => StopReason::Other(other.to_string()),
            });
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(Flow::Continue);
        };
        for key in ["reasoning_content", "reasoning"] {
            if let Some(piece) = delta.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                match self.blocks.last_mut() {
                    Some(Block::Thinking(text)) => text.push_str(piece),
                    _ => self.blocks.push(Block::Thinking(piece.to_string())),
                }
                if on_event(Event::ThinkingDelta(piece.to_string())) == Flow::Stop {
                    return Ok(Flow::Stop);
                }
            }
        }
        if let Some(piece) = delta.get("content").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            match self.blocks.last_mut() {
                Some(Block::Text(text)) => text.push_str(piece),
                _ => self.blocks.push(Block::Text(piece.to_string())),
            }
            if on_event(Event::TextDelta(piece.to_string())) == Flow::Stop {
                return Ok(Flow::Stop);
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                if self.tool_call_delta(call, on_event)? == Flow::Stop {
                    return Ok(Flow::Stop);
                }
            }
        }
        Ok(Flow::Continue)
    }

    fn tool_call_delta(&mut self, call: &Value, on_event: &mut dyn FnMut(Event) -> Flow) -> Result<Flow, LlmError> {
        let index = call.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(self.calls.len().saturating_sub(1));
        if index > self.calls.len() {
            return Err(LlmError::Protocol(format!("tool call {index} before tool call {}", self.calls.len())));
        }
        if index == self.calls.len() {
            self.calls.push(self.blocks.len());
            self.blocks.push(Block::ToolCall { id: String::new(), name: String::new(), arguments: String::new(), started: false });
        }
        let position = self.calls[index];
        let Some(Block::ToolCall { id, name, arguments, started }) = self.blocks.get_mut(position) else {
            return Err(LlmError::Protocol(format!("tool call {index} lost its block")));
        };
        if let Some(piece) = call.get("id").and_then(Value::as_str) {
            id.push_str(piece);
        }
        let function = call.get("function").cloned().unwrap_or(Value::Null);
        if let Some(piece) = function.get("name").and_then(Value::as_str) {
            name.push_str(piece);
        }
        if !*started && !name.is_empty() {
            *started = true;
            if on_event(Event::ToolUseStart { index: position, id: id.clone(), name: name.clone() }) == Flow::Stop {
                return Ok(Flow::Stop);
            }
        }
        if let Some(piece) = function.get("arguments").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            arguments.push_str(piece);
            if on_event(Event::ToolInputDelta { index: position, partial_json: piece.to_string() }) == Flow::Stop {
                return Ok(Flow::Stop);
            }
        }
        Ok(Flow::Continue)
    }

    fn finish(self) -> Result<Completion, LlmError> {
        let Some(mut stop_reason) = self.stop else {
            return Err(LlmError::Protocol(if self.done { "the stream carried no finish_reason".into() } else { "the stream ended early".into() }));
        };
        let mut content = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            content.push(match block {
                Block::Text(text) => ContentBlock::Text { text },
                Block::Thinking(text) => ContentBlock::Thinking { text, signature: None },
                Block::ToolCall { id, name, arguments, .. } => {
                    let input = if arguments.trim().is_empty() {
                        Value::Object(Map::new())
                    } else {
                        serde_json::from_str(&arguments)
                            .map_err(|e| LlmError::Protocol(format!("tool `{name}` arguments are not valid JSON: {e}")))?
                    };
                    if stop_reason == StopReason::EndTurn {
                        // Some servers report `stop` even when they called a tool.
                        stop_reason = StopReason::ToolUse;
                    }
                    ContentBlock::ToolUse { id, name, input }
                }
            });
        }
        Ok(Completion { model: self.model, content, stop_reason, usage: self.usage })
    }
}
