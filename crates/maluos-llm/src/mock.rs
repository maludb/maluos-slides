//! Scripted replacements for tests: a [`MockTransport`] that answers posts with canned HTTP
//! replies (to test a backend's wire format and parser) and a [`MockClient`] that answers
//! requests with canned completions (to test whatever drives an [`LlmClient`]).

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::Mutex;

use crate::transport::{HttpRequest, HttpResponse, Transport};
use crate::{Completion, ContentBlock, Event, Flow, LlmClient, LlmError, Request};

/// One canned HTTP reply.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Answers each post with the next scripted reply and records every request.
#[derive(Debug, Default)]
pub struct MockTransport {
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a reply with the given status and body.
    pub fn reply(&self, status: u16, body: impl Into<String>) -> &Self {
        self.reply_with_headers(status, Vec::new(), body)
    }

    pub fn reply_with_headers(&self, status: u16, headers: Vec<(String, String)>, body: impl Into<String>) -> &Self {
        self.replies.lock().unwrap_or_else(|e| e.into_inner()).push_back(Reply { status, headers, body: body.into() });
        self
    }

    /// Every request posted so far, in order.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The JSON body of the last request.
    pub fn last_body(&self) -> Option<serde_json::Value> {
        self.requests().last().and_then(|r| serde_json::from_str(&r.body).ok())
    }
}

impl Transport for MockTransport {
    fn post(&self, request: &HttpRequest) -> Result<HttpResponse, LlmError> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).push(request.clone());
        let reply = self.replies.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        match reply {
            Some(reply) => Ok(HttpResponse { status: reply.status, headers: reply.headers, body: Box::new(Cursor::new(reply.body.into_bytes())) }),
            None => Err(LlmError::Transport("mock transport: no scripted reply left".into())),
        }
    }
}

/// Server-Sent Events text from (event name, data) pairs; an empty name gives a bare `data:`
/// line (OpenAI's style).
pub fn sse(events: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (event, data) in events {
        if !event.is_empty() {
            out.push_str("event: ");
            out.push_str(event);
            out.push('\n');
        }
        out.push_str("data: ");
        out.push_str(data);
        out.push_str("\n\n");
    }
    out
}

/// Answers each request with the next scripted completion, replaying its blocks as events,
/// and records every request.
#[derive(Debug, Default)]
pub struct MockClient {
    completions: Mutex<VecDeque<Result<Completion, LlmError>>>,
    requests: Mutex<Vec<Request>>,
}

impl MockClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, completion: Completion) -> &Self {
        self.completions.lock().unwrap_or_else(|e| e.into_inner()).push_back(Ok(completion));
        self
    }

    pub fn push_error(&self, error: LlmError) -> &Self {
        self.completions.lock().unwrap_or_else(|e| e.into_inner()).push_back(Err(error));
        self
    }

    /// Every request made so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl LlmClient for MockClient {
    fn provider(&self) -> &str {
        "mock"
    }

    fn complete(&self, request: &Request, on_event: &mut dyn FnMut(Event) -> Flow) -> Result<Completion, LlmError> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).push(request.clone());
        let next = self.completions.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        let completion = match next {
            Some(Ok(c)) => c,
            Some(Err(e)) => return Err(e),
            None => return Err(LlmError::Transport("mock client: no scripted completion left".into())),
        };
        for (index, block) in completion.content.iter().enumerate() {
            let flow = match block {
                ContentBlock::Text { text } => on_event(Event::TextDelta(text.clone())),
                ContentBlock::Thinking { text, .. } => on_event(Event::ThinkingDelta(text.clone())),
                ContentBlock::ToolUse { id, name, input } => {
                    if on_event(Event::ToolUseStart { index, id: id.clone(), name: name.clone() }) == Flow::Stop {
                        return Err(LlmError::Cancelled);
                    }
                    on_event(Event::ToolInputDelta { index, partial_json: input.to_string() })
                }
                ContentBlock::ToolResult { .. } | ContentBlock::Raw { .. } | ContentBlock::Image { .. } => Flow::Continue,
            };
            if flow == Flow::Stop {
                return Err(LlmError::Cancelled);
            }
        }
        if on_event(Event::Usage(completion.usage)) == Flow::Stop {
            return Err(LlmError::Cancelled);
        }
        Ok(completion)
    }
}
