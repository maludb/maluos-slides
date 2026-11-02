//! The HTTP seam. Backends post through a [`Transport`] and read the reply body as a stream, so
//! a test replaces the network with a scripted reply and a wasm host with `fetch`.

use std::io::Read;

use crate::LlmError;

/// A request to send: always a POST with a JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    /// Header name and value, in order. Backends set `content-type`.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpRequest {
    /// The value of a header, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A reply whose body is read as it arrives.
pub struct HttpResponse {
    pub status: u16,
    /// Header name (lower-case) and value.
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The whole body as text, capped at `limit` bytes (for error replies).
    pub fn read_text(&mut self, limit: u64) -> Result<String, LlmError> {
        let mut bytes = Vec::new();
        self.body.by_ref().take(limit).read_to_end(&mut bytes).map_err(|e| LlmError::Transport(format!("reading the reply: {e}")))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Sends a request and hands back the reply, whatever its status. Any status is a success here;
/// the backend decides what a 4xx or 5xx means.
pub trait Transport: Send + Sync {
    fn post(&self, request: &HttpRequest) -> Result<HttpResponse, LlmError>;
}

/// The most bytes read from an error reply's body.
pub const ERROR_BODY_LIMIT: u64 = 1024 * 1024;

/// Turn a non-2xx reply into the matching [`LlmError`], reading the body for the message and
/// the `retry-after` header for rate limits.
pub fn error_from_response(mut response: HttpResponse) -> LlmError {
    let status = response.status;
    let retry_after = response.header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()).map(std::time::Duration::from_secs);
    let body = response.read_text(ERROR_BODY_LIMIT).unwrap_or_default();
    let message = crate::error_message(&body);
    match LlmError::from_status(status, message) {
        LlmError::RateLimited { message, .. } => LlmError::RateLimited { message, retry_after },
        other => other,
    }
}
