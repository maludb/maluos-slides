//! The API driven over a real socket and through `handle` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::cognitive_complexity)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use maluos_llm::mock::MockClient;
use maluos_llm::{Completion, ContentBlock, StopReason, Usage};
use serde_json::{Value, json};

use crate::http::{Request, percent_decode};
use crate::{Headless, Options, Server, Target};

fn server(options: Options) -> Server {
    Server::new(Target::Headless(Headless::with_document()), options)
}

fn request(method: &str, path: &str, body: Option<Value>) -> Request {
    let body = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
    let (path, query) = match path.split_once('?') {
        Some((p, q)) => (
            p.to_string(),
            q.split('&')
                .map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), percent_decode(v))).unwrap_or((kv.to_string(), String::new())))
                .collect(),
        ),
        None => (path.to_string(), vec![]),
    };
    Request { method: method.into(), path, query, headers: vec![("content-type".into(), "application/json".into())], body }
}

fn json_of(r: &crate::http::Response) -> Value {
    serde_json::from_slice(&r.body).unwrap_or_else(|_| panic!("not JSON: {:?}", String::from_utf8_lossy(&r.body)))
}

#[test]
fn routes_cover_tools_commands_document_and_slides() {
    let mut s = server(Options::new("127.0.0.1:0"));
    let r = s.handle(&request("GET", "/v1/health", None));
    assert_eq!(r.status, 200);
    assert_eq!(json_of(&r)["ok"], true);

    let tools = json_of(&s.handle(&request("GET", "/v1/tools", None)));
    let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    let mcp: Vec<String> = maluos_slides_assist::ToolTable::mcp_definitions().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, mcp, "the API serves the shared table unchanged");
    assert!(names.contains(&"build_deck"));

    let r = s.handle(&request("POST", "/v1/tools/add_slide", Some(json!({"layout": "titleOnly", "title": "Agenda"}))));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v = json_of(&r);
    assert_eq!(v["ok"], true);
    assert!(v["result"]["id"].is_number());

    let doc = json_of(&s.handle(&request("GET", "/v1/document", None)));
    assert_eq!(doc["slides"].as_array().unwrap().len(), 2);

    let slide = json_of(&s.handle(&request("GET", "/v1/slides/1", None)));
    assert_eq!(slide["title"], "Agenda");
    assert_eq!(s.handle(&request("GET", "/v1/slides/9", None)).status, 404);
    assert_eq!(s.handle(&request("GET", "/v1/slides/x", None)).status, 400);

    let png = s.handle(&request("GET", "/v1/slides/1/render.png?scale=0.25", None));
    assert_eq!(png.status, 200);
    assert_eq!(png.content_type, "image/png");
    assert_eq!(&png.body[..8], b"\x89PNG\r\n\x1a\n");

    let cmds = json_of(&s.handle(&request("GET", "/v1/commands?filter=slide.", None)));
    assert!(cmds["commands"].as_array().unwrap().iter().all(|c| c["id"].as_str().unwrap().contains("slide")));
    assert!(cmds["commands"].as_array().unwrap().len() > 5);

    let r = s.handle(&request("POST", "/v1/commands/slide.notes", Some(json!({"text": "Say hello"}))));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let slide = json_of(&s.handle(&request("GET", "/v1/slides/1", None)));
    assert_eq!(slide["notes"], "Say hello");

    // Errors are JSON with a code.
    let r = s.handle(&request("POST", "/v1/commands/slide.nope", Some(json!({}))));
    assert_eq!(r.status, 400);
    assert_eq!(json_of(&r)["error"]["code"], "command_failed");
    let r = s.handle(&request("POST", "/v1/tools/no_such_tool", Some(json!({}))));
    assert_eq!(r.status, 404);
    let r = s.handle(&request("POST", "/v1/tools/add_slide", Some(json!({"titel": "x"}))));
    assert_eq!(r.status, 400);
    assert!(json_of(&r)["text"].as_str().unwrap().contains("unknown argument"));
    let mut bad = request("POST", "/v1/tools/add_slide", None);
    bad.body = b"{not json".to_vec();
    assert_eq!(s.handle(&bad).status, 400);
    assert_eq!(s.handle(&request("GET", "/v1/nothing", None)).status, 404);
    assert_eq!(s.handle(&request("GET", "/", None)).status, 200, "the root lists the routes too");
    assert!(json_of(&s.handle(&request("GET", "/v1", None)))["routes"].is_array());

    // The fork's tool goes through the same route.
    let r = s.handle(&request("POST", "/v1/tools/build_deck", Some(json!({"slides": [{"title": "From the API"}]}))));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(json_of(&r)["result"]["count"], 1);
    assert_eq!(json_of(&s.handle(&request("GET", "/v1/slides/2", None)))["title"], "From the API");
}

#[test]
fn a_token_is_enforced_when_set_and_required_off_loopback() {
    let mut s = server(Options { token: Some("s3cret".into()), ..Options::new("127.0.0.1:0") });
    assert_eq!(s.handle(&request("GET", "/v1/health", None)).status, 401);
    let mut r = request("GET", "/v1/health", None);
    r.headers.push(("authorization".into(), "Bearer s3cret".into()));
    assert_eq!(s.handle(&r).status, 200);
    let mut r = request("GET", "/v1/health", None);
    r.headers.push(("x-api-token".into(), "s3cret".into()));
    assert_eq!(s.handle(&r).status, 200);
    let mut r = request("GET", "/v1/health", None);
    r.headers.push(("authorization".into(), "Bearer wrong".into()));
    assert_eq!(s.handle(&r).status, 401);

    assert!(Options::new("127.0.0.1:0").validate().is_ok());
    assert!(Options::new("0.0.0.0:7980").validate().unwrap_err().contains("not loopback"));
    assert!(Options { token: Some("t".into()), ..Options::new("0.0.0.0:7980") }.validate().is_ok());
    assert!(Options::new("not an address").validate().is_err());
}

fn usage() -> Usage {
    Usage { input_tokens: 10, output_tokens: 2, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 }
}

#[test]
fn ask_runs_a_turn_with_the_mock_model() {
    let mock = Arc::new(MockClient::new());
    mock.push(Completion {
        model: "mock".into(),
        content: vec![ContentBlock::ToolUse { id: "a".into(), name: "add_slide".into(), input: json!({"layout": "blank"}) }],
        stop_reason: StopReason::ToolUse,
        usage: usage(),
    });
    mock.push(Completion {
        model: "mock".into(),
        content: vec![ContentBlock::Text { text: "Added.".into() }],
        stop_reason: StopReason::EndTurn,
        usage: usage(),
    });
    let mut s = server(Options { client: Some(mock), ..Options::new("127.0.0.1:0") });
    let r = s.handle(&request("POST", "/v1/ask", Some(json!({"text": "add a slide"}))));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v = json_of(&r);
    assert_eq!(v["answer"], "Added.");
    assert_eq!(v["commands"], json!(["slide.new"]));
    assert_eq!(v["touchedSlides"], json!([1]));
    assert_eq!(v["undo"], "Assistant: add a slide");
    assert_eq!(json_of(&s.handle(&request("GET", "/v1/document", None)))["slides"].as_array().unwrap().len(), 2);
    assert_eq!(s.handle(&request("POST", "/v1/ask", Some(json!({})))).status, 400);
    assert_eq!(s.handle(&request("POST", "/v1/ask", Some(json!({"text": "x", "mode": "nope"})))).status, 400);

    // Confirm mode without `yes` declines every change in the API.
    let mock = Arc::new(MockClient::new());
    mock.push(Completion {
        model: "mock".into(),
        content: vec![ContentBlock::ToolUse { id: "a".into(), name: "add_slide".into(), input: json!({"layout": "blank"}) }],
        stop_reason: StopReason::ToolUse,
        usage: usage(),
    });
    mock.push(Completion {
        model: "mock".into(),
        content: vec![ContentBlock::Text { text: "ok".into() }],
        stop_reason: StopReason::EndTurn,
        usage: usage(),
    });
    let mut s = server(Options { client: Some(mock), ..Options::new("127.0.0.1:0") });
    let v = json_of(&s.handle(&request("POST", "/v1/ask", Some(json!({"text": "add a slide", "mode": "confirm"})))));
    assert_eq!(v["declined"], 1);
    assert_eq!(v["commands"], json!([]));
}

/// Over a real socket: the wire format, a body, a bad request line.
#[test]
fn serves_over_tcp() {
    let s = server(Options::new("127.0.0.1:0"));
    let listener = s.bind().unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut s = s;
        s.serve(listener);
    });
    let send = |raw: &str| -> String {
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        out
    };
    let body = r#"{"layout":"blank"}"#;
    let reply = send(&format!(
        "POST /v1/tools/add_slide HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
    assert!(reply.contains("Content-Type: application/json"));
    assert!(reply.contains("\"ok\":true"));
    let reply = send("GET /v1/document HTTP/1.1\r\nHost: x\r\n\r\n");
    let json_start = reply.find("\r\n\r\n").unwrap() + 4;
    let v: Value = serde_json::from_str(&reply[json_start..]).unwrap();
    assert_eq!(v["slides"].as_array().unwrap().len(), 2);
    let reply = send("NOPE\r\n\r\n");
    assert!(reply.starts_with("HTTP/1.1 400"), "{reply}");
    let reply = send("POST /v1/tools/add_slide HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
    assert!(reply.starts_with("HTTP/1.1 411"), "{reply}");
}

#[test]
fn percent_decoding() {
    assert_eq!(percent_decode("a%20b+c"), "a b c");
    assert_eq!(percent_decode("slide.%2Fx"), "slide./x");
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%zz"), "%zz");
}
