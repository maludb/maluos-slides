//! The native transport: `ureq` with rustls, no async runtime, streaming reads.

use std::time::Duration;

use crate::LlmError;
use crate::transport::{HttpRequest, HttpResponse, Transport};

/// Posts with `ureq`. One agent per client; connections are reused.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new(Duration::from_secs(30), Duration::from_secs(180))
    }
}

impl UreqTransport {
    /// `connect` bounds the TCP and TLS handshake; `per_read` bounds the wait for the next byte
    /// of a streaming reply (a model that stops producing tokens for that long is given up on).
    /// There is no whole-request timeout: long answers stream for minutes.
    pub fn new(connect: Duration, per_read: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(connect))
            .timeout_per_read(Some(per_read))
            .timeout_send_request(Some(connect))
            .timeout_recv_response(Some(per_read))
            .user_agent("maluos-llm")
            .build();
        UreqTransport { agent: config.new_agent() }
    }
}

impl Transport for UreqTransport {
    fn post(&self, request: &HttpRequest) -> Result<HttpResponse, LlmError> {
        let mut builder = self.agent.post(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let response = builder.send(request.body.as_str()).map_err(|e| match e {
            ureq::Error::Timeout(_) => LlmError::Transport(format!("timed out: {e}")),
            other => LlmError::Transport(other.to_string()),
        })?;
        let status = response.status().as_u16();
        let headers =
            response.headers().iter().map(|(k, v)| (k.as_str().to_ascii_lowercase(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
        let body = response.into_body().into_reader();
        Ok(HttpResponse { status, headers, body: Box::new(body) })
    }
}
