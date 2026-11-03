//! MaluOS Slides' HTTP/JSON API (owner decision D9: every function callable from an API and
//! from MCP). The same table drives both: `GET /v1/tools` is `maluos_slides_mcp::tool_definitions`
//! and `POST /v1/tools/{name}` is `call_tool`; the command catalog and `document.inspect` come
//! from the engine; `POST /v1/ask` is one turn of `maluos_slides_assist::Assistant`.
//!
//! The server is hand-written HTTP/1.1 (no async runtime), one request at a time on the
//! loopback interface. Binding anywhere else needs a bearer token.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod http;

use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::sync::Arc;

use maluos_llm::LlmClient;
use maluos_slides_assist::call_tool;
use maluos_slides_assist::providers::{Choice, default_choice};
use maluos_slides_assist::{ApproveAll, Assistant, DeclineAll, Headless, Host, Mode, ToolTable};
use maluos_slides_mcp::{Backend, Remote, ToolResult};
use serde_json::{Value, json};

use http::{Request, Response};

pub const DEFAULT_LISTEN: &str = "127.0.0.1:7980";

/// What `maluos-slides-cli api` was asked.
pub struct Options {
    pub listen: String,
    /// Required in `Authorization: Bearer …` (or `X-Api-Token`) when set; required when
    /// `listen` is not a loopback address.
    pub token: Option<String>,
    /// A client for `/v1/ask` instead of the provider's (tests).
    pub client: Option<Arc<dyn LlmClient>>,
}

impl Options {
    pub fn new(listen: impl Into<String>) -> Self {
        Options { listen: listen.into(), token: None, client: None }
    }

    /// Refuses a non-loopback address without a token.
    pub fn validate(&self) -> Result<(), String> {
        let addrs: Vec<SocketAddr> = self.listen.to_socket_addrs().map_err(|e| format!("--listen {}: {e}", self.listen))?.collect();
        if addrs.is_empty() {
            return Err(format!("--listen {}: no address", self.listen));
        }
        if self.token.as_deref().is_none_or(str::is_empty) && addrs.iter().any(|a| !a.ip().is_loopback()) {
            return Err(format!("--listen {} is not loopback; set --token (or MALUOS_SLIDES_API_TOKEN) to serve beyond this machine", self.listen));
        }
        Ok(())
    }
}

/// Where calls go: the in-process engine or a running app.
pub enum Target {
    Headless(Headless),
    Remote(Remote),
}

impl Target {
    fn backend(&mut self) -> &mut dyn Backend {
        match self {
            Target::Headless(h) => h,
            Target::Remote(r) => r,
        }
    }

    fn host(&mut self) -> Option<&mut dyn Host> {
        match self {
            Target::Headless(h) => Some(h),
            Target::Remote(_) => None,
        }
    }
}

/// The server: binds, then answers forever.
pub struct Server {
    target: Target,
    options: Options,
}

impl Server {
    pub fn new(target: Target, options: Options) -> Self {
        Server { target, options }
    }

    /// Binds `options.listen`; the bound address (port 0 picks one) comes back.
    pub fn bind(&self) -> Result<TcpListener, String> {
        self.options.validate()?;
        TcpListener::bind(&self.options.listen).map_err(|e| format!("cannot listen on {}: {e}", self.options.listen))
    }

    /// Serves until the process ends.
    pub fn serve(&mut self, listener: TcpListener) {
        for conn in listener.incoming() {
            let mut stream = match conn {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("accept: {e}");
                    continue;
                }
            };
            let response = match http::read_request(&mut stream) {
                Ok(req) => self.handle(&req),
                Err((status, msg)) => error(status, "bad_request", &msg),
            };
            http::write_response(&mut stream, &response);
        }
    }

    /// One request → one response (the routing, testable without a socket).
    pub fn handle(&mut self, req: &Request) -> Response {
        if let Some(t) = &self.options.token {
            let given = req.header("authorization").and_then(|a| a.strip_prefix("Bearer ")).or_else(|| req.header("x-api-token")).map(str::trim);
            if given != Some(t.as_str()) {
                return error(401, "unauthorized", "a bearer token is required");
            }
        }
        let parts: Vec<&str> = req.path.trim_start_matches('/').split('/').filter(|p| !p.is_empty()).collect();
        let body: Result<Value, Response> = if req.body.is_empty() {
            Ok(json!({}))
        } else {
            serde_json::from_slice::<Value>(&req.body).map_err(|e| error(400, "bad_json", &format!("body is not JSON: {e}")))
        };
        match (req.method.as_str(), parts.as_slice()) {
            ("GET", ["v1", "health"]) => Response::json(
                200,
                &json!({"ok": true, "server": "maluos-slides", "version": env!("CARGO_PKG_VERSION"), "backend": self.target.backend().describe()}),
            ),
            ("GET", ["v1", "tools"]) => Response::json(200, &json!({"tools": ToolTable::mcp_definitions()})),
            ("POST", ["v1", "tools", name]) => {
                let args = match body {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                if !ToolTable::all().names().any(|n| n == *name) {
                    return error(404, "unknown_tool", &format!("no tool `{name}` (GET /v1/tools lists them)"));
                }
                let r = call_tool(self.target.backend(), name, &args);
                tool_response(r)
            }
            ("GET", ["v1", "commands"]) => {
                let filter = req.query("filter").unwrap_or("").to_lowercase();
                match self.target.backend().call("engine.commands", json!({})) {
                    Ok(Value::Array(cmds)) => {
                        let list: Vec<Value> = cmds
                            .into_iter()
                            .filter(|c| {
                                filter.is_empty()
                                    || c.get("id").and_then(Value::as_str).is_some_and(|i| i.to_lowercase().contains(&filter))
                                    || c.get("label").and_then(Value::as_str).is_some_and(|i| i.to_lowercase().contains(&filter))
                            })
                            .collect();
                        Response::json(200, &json!({"commands": list}))
                    }
                    Ok(other) => Response::json(200, &json!({"commands": other})),
                    Err(e) => error(500, "backend", &e),
                }
            }
            ("POST", ["v1", "commands", id]) => {
                let params = match body {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                match self.target.backend().call("engine.execute", json!({"command": id, "params": params})) {
                    Ok(v) => Response::json(200, &json!({"ok": true, "result": v})),
                    Err(e) => error(400, "command_failed", &e),
                }
            }
            ("GET", ["v1", "document"]) => match self.target.backend().call("document.inspect", json!({})) {
                Ok(v) => Response::json(200, &v),
                Err(e) => error(400, "no_document", &e),
            },
            ("GET", ["v1", "slides", index]) => {
                let Ok(i) = index.parse::<usize>() else { return error(400, "bad_index", "slide index must be a number") };
                match self.target.backend().call("engine.execute", json!({"command": "slide.inspect", "params": {"index": i}})) {
                    Ok(v) => Response::json(200, &v),
                    Err(e) => error(404, "no_slide", &e),
                }
            }
            ("GET", ["v1", "slides", index, "render.png"]) => {
                let Ok(i) = index.parse::<usize>() else { return error(400, "bad_index", "slide index must be a number") };
                let scale = req.query("scale").and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
                let edit = req.query("edit").is_some_and(|e| e == "1" || e == "true");
                match self.target.backend().call("ui.render", json!({"slide": i, "scale": scale, "edit": edit})) {
                    Ok(v) => match v.get("png").and_then(Value::as_str).and_then(maluos_slides_engine::cmd::base64_decode) {
                        Some(png) => Response::bytes("image/png", png),
                        None => error(500, "render", "the backend returned no image"),
                    },
                    Err(e) => error(404, "no_slide", &e),
                }
            }
            ("POST", ["v1", "ask"]) => {
                let b = match body {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                self.ask(&b)
            }
            ("GET", ["v1"]) | ("GET", []) => Response::json(
                200,
                &json!({"routes": ["GET /v1/health", "GET /v1/tools", "POST /v1/tools/{name}", "GET /v1/commands?filter=", "POST /v1/commands/{id}", "GET /v1/document", "GET /v1/slides/{i}", "GET /v1/slides/{i}/render.png?scale=", "POST /v1/ask"]}),
            ),
            (_, ["v1", ..]) => error(404, "not_found", &format!("no route {} {}", req.method, req.path)),
            _ => error(404, "not_found", "the API lives under /v1"),
        }
    }

    fn ask(&mut self, b: &Value) -> Response {
        let Some(text) = b.get("text").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) else {
            return error(400, "no_text", "ask needs {\"text\": \"…\"}");
        };
        let mode = match b.get("mode").and_then(Value::as_str) {
            None => Mode::Autopilot,
            Some(m) => match Mode::parse(m) {
                Some(m) => m,
                None => return error(400, "bad_mode", "mode must be answer, confirm or autopilot"),
            },
        };
        let yes = b.get("yes").and_then(Value::as_bool).unwrap_or(false);
        let provider = match b.get("provider").and_then(Value::as_str) {
            None => default_choice(),
            Some(p) => match Choice::parse(p) {
                Some(c) => c,
                None => return error(400, "bad_provider", "provider must be anthropic, openai or ollama"),
            },
        };
        let model = b.get("model").and_then(Value::as_str).map(String::from).unwrap_or_else(|| provider.default_model().to_string());
        let client = match &self.options.client {
            Some(c) => c.clone(),
            None => match provider.client() {
                Ok(c) => c,
                Err(e) => return error(400, "no_provider", &e),
            },
        };
        let Some(host) = self.target.host() else {
            return error(501, "not_in_process", "ask runs only on the in-process presentation (start the API without --connect)");
        };
        if host.session().active().is_none() {
            return error(400, "no_document", "no presentation is open (POST /v1/tools/new_presentation first)");
        }
        let mut assistant = Assistant::new(client, model).with_mode(mode);
        let report = if yes {
            assistant.ask(host, text, &mut ApproveAll, &mut |_| maluos_llm::Flow::Continue)
        } else {
            assistant.ask(host, text, &mut DeclineAll, &mut |_| maluos_llm::Flow::Continue)
        };
        let status = if report.error.is_some() { 502 } else { 200 };
        Response::json(
            status,
            &json!({
                "answer": report.answer,
                "rounds": report.rounds,
                "toolCalls": report.tool_calls.iter().map(|c| json!({"name": c.name, "input": c.input, "ok": c.ok, "summary": c.summary})).collect::<Vec<_>>(),
                "commands": report.commands,
                "touchedSlides": report.touched_slides,
                "removedSlides": report.removed_slides,
                "plans": report.plans,
                "declined": report.declined,
                "refused": report.refused,
                "usage": {"inputTokens": report.usage.input_tokens, "outputTokens": report.usage.output_tokens},
                "undo": report.undo_label,
                "stopped": report.stopped.as_ref().map(|s| format!("{s:?}")),
                "error": report.error.as_ref().map(|e| e.to_string()),
            }),
        )
    }
}

fn tool_response(r: ToolResult) -> Response {
    let status = if r.is_error { 400 } else { 200 };
    // One text block that parses as JSON is returned as that JSON; otherwise the MCP content.
    if let [only] = r.content.as_slice()
        && only.get("type").and_then(Value::as_str) == Some("text")
        && let Some(t) = only.get("text").and_then(Value::as_str)
    {
        return match serde_json::from_str::<Value>(t) {
            Ok(v) if !r.is_error => Response::json(status, &json!({"ok": true, "result": v})),
            _ => Response::json(status, &json!({"ok": !r.is_error, "text": t})),
        };
    }
    Response::json(status, &json!({"ok": !r.is_error, "content": r.content}))
}

fn error(status: u16, code: &str, message: &str) -> Response {
    Response::json(status, &json!({"ok": false, "error": {"code": code, "message": message}}))
}

#[cfg(test)]
mod tests;
