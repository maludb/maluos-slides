//! Server-Sent Events, read incrementally from any [`std::io::Read`].
//!
//! The parser follows the WHATWG event stream format: `event:` and `data:` fields, `data` lines
//! of one event joined with newlines, comment lines (`:`) skipped, an empty line dispatching the
//! event. `id` and `retry` are ignored. CRLF line endings are accepted.

use std::io::{BufRead, BufReader, Read};

use crate::{Flow, LlmError};

/// One event. `event` is empty when the stream gave no `event:` line (OpenAI's style).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// The most bytes one event may carry before the stream counts as malformed.
pub const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

/// Feed a byte stream to `on_event` one event at a time. Ends at end of stream, when
/// `on_event` returns [`Flow::Stop`] (then [`LlmError::Cancelled`]), or on an I/O error.
pub fn read_events<R: Read>(reader: R, on_event: &mut dyn FnMut(SseEvent) -> Result<Flow, LlmError>) -> Result<(), LlmError> {
    let mut lines = BufReader::new(reader);
    let mut parser = Parser::default();
    let mut line = String::new();
    loop {
        line.clear();
        let n = lines.read_line(&mut line).map_err(|e| LlmError::Transport(format!("reading the stream: {e}")))?;
        if n == 0 {
            if let Some(event) = parser.finish() {
                return match on_event(event)? {
                    Flow::Continue => Ok(()),
                    Flow::Stop => Err(LlmError::Cancelled),
                };
            }
            return Ok(());
        }
        if let Some(event) = parser.push_line(&line)? {
            match on_event(event)? {
                Flow::Continue => {}
                Flow::Stop => return Err(LlmError::Cancelled),
            }
        }
    }
}

/// Line-at-a-time state. Public so a host with its own byte source (wasm `fetch`) can drive it.
#[derive(Debug, Default)]
pub struct Parser {
    event: String,
    data: Vec<String>,
    bytes: usize,
}

impl Parser {
    /// Feed one line (with or without its line ending). Returns the event an empty line closes.
    pub fn push_line(&mut self, raw: &str) -> Result<Option<SseEvent>, LlmError> {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return Ok(self.take());
        }
        if line.starts_with(':') {
            return Ok(None);
        }
        self.bytes += line.len();
        if self.bytes > MAX_EVENT_BYTES {
            return Err(LlmError::Protocol(format!("an event exceeds {MAX_EVENT_BYTES} bytes")));
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
        Ok(None)
    }

    /// The event still open at end of stream, if any data was seen.
    pub fn finish(&mut self) -> Option<SseEvent> {
        self.take()
    }

    fn take(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() && self.event.is_empty() {
            return None;
        }
        let event = SseEvent { event: std::mem::take(&mut self.event), data: self.data.join("\n") };
        self.data.clear();
        self.bytes = 0;
        if event.data.is_empty() && event.event.is_empty() { None } else { Some(event) }
    }
}
