//! A small HTTP/1.1 server: one request per connection, read in full, answered in full.
//! No async runtime, no dependency; enough for JSON calls on the loopback interface.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Most bytes of headers accepted.
const MAX_HEAD: usize = 64 * 1024;
/// Most bytes of body accepted (a pasted deck outline, a base64 picture).
pub const MAX_BODY: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, v: &serde_json::Value) -> Self {
        Response { status, content_type: "application/json", body: v.to_string().into_bytes() }
    }

    pub fn bytes(content_type: &'static str, body: Vec<u8>) -> Self {
        Response { status: 200, content_type, body }
    }
}

/// Reads one request; `Err` is the status and message to answer with.
pub fn read_request(stream: &mut TcpStream) -> Result<Request, (u16, String)> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(stream);
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = reader.read_until(b'\n', &mut line).map_err(|e| (400, format!("cannot read request: {e}")))?;
        if n == 0 {
            return Err((400, "connection closed before the request ended".into()));
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEAD {
            return Err((431, "request headers too large".into()));
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_ascii_uppercase();
    let target = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/1.") {
        return Err((400, format!("bad request line: {request_line:?}")));
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_query(q)),
        None => (target.to_string(), vec![]),
    };
    let mut headers = Vec::new();
    for l in lines {
        if l.is_empty() {
            break;
        }
        if let Some((n, v)) = l.split_once(':') {
            headers.push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let length: usize = match headers.iter().find(|(n, _)| n == "content-length") {
        Some((_, v)) => v.parse().map_err(|_| (400, "bad Content-Length".to_string()))?,
        None => 0,
    };
    if headers.iter().any(|(n, v)| n == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked")) {
        return Err((411, "chunked bodies are not accepted; send Content-Length".into()));
    }
    if length > MAX_BODY {
        return Err((413, format!("body over {MAX_BODY} bytes")));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|e| (400, format!("cannot read the body: {e}")))?;
    Ok(Request { method, path, query, headers, body })
}

pub fn write_response(stream: &mut TcpStream, r: &Response) {
    let reason = match r.status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Status",
    };
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        r.status,
        r.content_type,
        r.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&r.body);
    let _ = stream.flush();
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(p), String::new()),
        })
        .collect()
}

/// `%20` and `+` as in a query string; bad escapes are kept as they are.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() + 1 && i + 2 <= bytes.len() - 1 + 1 => {
                let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
