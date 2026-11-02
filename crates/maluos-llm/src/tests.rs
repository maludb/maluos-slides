#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::keys::{Provider, api_key_with};
use crate::mock::{MockClient, MockTransport, sse};
use crate::sse::{Parser, SseEvent, read_events};
use crate::{
    AnthropicClient, Completion, ContentBlock, Effort, Event, Flow, LlmClient, LlmError, Message, OpenAiClient, Request, Role, StopReason, ToolDef,
    Usage,
};

fn tool() -> ToolDef {
    ToolDef {
        name: "set_text".into(),
        description: "Replace a shape's text".into(),
        input_schema: json!({"type": "object", "properties": {"id": {"type": "integer"}, "text": {"type": "string"}}, "required": ["id", "text"]}),
    }
}

fn collect(client: &dyn LlmClient, request: &Request) -> (Result<Completion, LlmError>, Vec<Event>) {
    let mut events = Vec::new();
    let result = client.complete(request, &mut |e| {
        events.push(e);
        Flow::Continue
    });
    (result, events)
}

// SSE

#[test]
fn sse_joins_data_lines_skips_comments_and_accepts_crlf() {
    let text = ": keep-alive\r\nevent: a\r\ndata: one\r\ndata: two\r\n\r\ndata: {\"x\":1}\n\n";
    let mut seen = Vec::new();
    read_events(text.as_bytes(), &mut |e| {
        seen.push(e);
        Ok(Flow::Continue)
    })
    .unwrap();
    assert_eq!(seen, vec![SseEvent { event: "a".into(), data: "one\ntwo".into() }, SseEvent { event: String::new(), data: "{\"x\":1}".into() }]);
}

#[test]
fn sse_dispatches_the_last_event_without_a_trailing_blank_line_and_stops_on_request() {
    let mut p = Parser::default();
    assert_eq!(p.push_line("data: tail").unwrap(), None);
    assert_eq!(p.finish(), Some(SseEvent { event: String::new(), data: "tail".into() }));
    assert_eq!(p.finish(), None);

    let text = "data: 1\n\ndata: 2\n\n";
    let mut seen = 0;
    let err = read_events(text.as_bytes(), &mut |_| {
        seen += 1;
        Ok(Flow::Stop)
    })
    .unwrap_err();
    assert_eq!(err, LlmError::Cancelled);
    assert_eq!(seen, 1);
}

#[test]
fn sse_rejects_an_event_over_the_size_cap() {
    let mut p = Parser::default();
    let big = "x".repeat(crate::sse::MAX_EVENT_BYTES - 10);
    p.push_line(&format!("data: {big}")).unwrap();
    assert!(matches!(p.push_line("data: more"), Err(LlmError::Protocol(_))));
}

// Anthropic

fn anthropic_stream() -> String {
    sse(&[
        (
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-5-5","content":[],"usage":{"input_tokens":25,"cache_read_input_tokens":10,"output_tokens":1}}}"#,
        ),
        ("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#),
        ("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Plan: "}}"#),
        ("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-abc"}}"#),
        ("content_block_stop", r#"{"type":"content_block_stop","index":0}"#),
        ("content_block_start", r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#),
        ("ping", r#"{"type":"ping"}"#),
        ("content_block_delta", r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Changing "}}"#),
        ("content_block_delta", r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"the title."}}"#),
        ("content_block_stop", r#"{"type":"content_block_stop","index":1}"#),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"set_text","input":{}}}"#,
        ),
        ("content_block_delta", r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"id\": 7, "}}"#),
        ("content_block_delta", r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"text\": \"Q3\"}"}}"#),
        ("content_block_stop", r#"{"type":"content_block_stop","index":2}"#),
        ("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":42}}"#),
        ("message_stop", r#"{"type":"message_stop"}"#),
    ])
}

#[test]
fn anthropic_streams_text_thinking_and_a_tool_call_into_a_completion() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(200, anthropic_stream());
    let client = AnthropicClient::new(transport.clone(), "sk-test").with_thinking_display(Some("summarized".into()));
    let request = Request::new("claude-opus-5-5", vec![Message::user("Rename the title to Q3")])
        .with_system("You edit decks.")
        .with_tools(vec![tool()])
        .with_effort(Effort::High);
    let (result, events) = collect(&client, &request);
    let completion = result.unwrap();

    assert_eq!(completion.model, "claude-opus-5-5");
    assert_eq!(completion.stop_reason, StopReason::ToolUse);
    assert_eq!(completion.usage, Usage { input_tokens: 25, output_tokens: 42, cache_read_input_tokens: 10, cache_creation_input_tokens: 0 });
    assert_eq!(completion.text(), "Changing the title.");
    assert_eq!(
        completion.content,
        vec![
            ContentBlock::Thinking { text: "Plan: ".into(), signature: Some("sig-abc".into()) },
            ContentBlock::Text { text: "Changing the title.".into() },
            ContentBlock::ToolUse { id: "toolu_1".into(), name: "set_text".into(), input: json!({"id": 7, "text": "Q3"}) },
        ]
    );
    assert_eq!(completion.tool_uses().map(|(_, name, _)| name).collect::<Vec<_>>(), vec!["set_text"]);
    assert_eq!(
        events,
        vec![
            Event::Usage(Usage { input_tokens: 25, output_tokens: 1, cache_read_input_tokens: 10, cache_creation_input_tokens: 0 }),
            Event::ThinkingDelta("Plan: ".into()),
            Event::TextDelta("Changing ".into()),
            Event::TextDelta("the title.".into()),
            Event::ToolUseStart { index: 2, id: "toolu_1".into(), name: "set_text".into() },
            Event::ToolInputDelta { index: 2, partial_json: "{\"id\": 7, ".into() },
            Event::ToolInputDelta { index: 2, partial_json: "\"text\": \"Q3\"}".into() },
            Event::Usage(Usage { input_tokens: 25, output_tokens: 42, cache_read_input_tokens: 10, cache_creation_input_tokens: 0 }),
        ]
    );

    // The request on the wire.
    let http = &transport.requests()[0];
    assert_eq!(http.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(http.header("x-api-key"), Some("sk-test"));
    assert_eq!(http.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(http.header("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
    assert_eq!(http.header("content-type"), Some("application/json"));
    let body = transport.last_body().unwrap();
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 16000);
    assert_eq!(body["system"], "You edit decks.");
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["output_config"]["effort"], "high");
    assert_eq!(body["thinking"], json!({"type": "adaptive", "display": "summarized"}));
    assert_eq!(body["tools"][0]["name"], "set_text");
    assert_eq!(body["tools"][0]["input_schema"]["required"][0], "id");
    assert_eq!(body["messages"], json!([{"role": "user", "content": [{"type": "text", "text": "Rename the title to Q3"}]}]));
    assert!(body.get("temperature").is_none());
}

#[test]
fn anthropic_passes_thinking_and_tool_blocks_back_and_drops_foreign_ones() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(200, anthropic_stream());
    let client = AnthropicClient::new(transport.clone(), "k").with_refusal_fallbacks(false).with_beta("x-beta");
    let assistant = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking { text: "signed".into(), signature: Some("sig".into()) },
            ContentBlock::Thinking { text: "unsigned, from another provider".into(), signature: None },
            ContentBlock::Raw { provider: "anthropic".into(), value: json!({"type": "redacted_thinking", "data": "zzz"}) },
            ContentBlock::Raw { provider: "openai".into(), value: json!({"ignored": true}) },
            ContentBlock::ToolUse { id: "toolu_1".into(), name: "set_text".into(), input: json!({"id": 7}) },
        ],
    };
    let results = Message::tool_results(vec![
        ContentBlock::ToolResult { tool_use_id: "toolu_1".into(), content: "ok".into(), is_error: false },
        ContentBlock::ToolResult { tool_use_id: "toolu_2".into(), content: "no such shape".into(), is_error: true },
    ]);
    let request = Request::new("claude-opus-5-5", vec![Message::user("hi"), assistant, results]);
    client.complete_blocking(&request).unwrap();
    let body = transport.last_body().unwrap();
    assert_eq!(
        body["messages"][1]["content"],
        json!([
            {"type": "thinking", "thinking": "signed", "signature": "sig"},
            {"type": "redacted_thinking", "data": "zzz"},
            {"type": "tool_use", "id": "toolu_1", "name": "set_text", "input": {"id": 7}},
        ])
    );
    assert_eq!(
        body["messages"][2]["content"],
        json!([
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"},
            {"type": "tool_result", "tool_use_id": "toolu_2", "content": "no such shape", "is_error": true},
        ])
    );
    assert!(body.get("fallbacks").is_none());
    assert!(body.get("thinking").is_none());
    assert_eq!(transport.requests()[0].header("anthropic-beta"), Some("x-beta"));
}

#[test]
fn anthropic_maps_http_errors_with_the_body_message_and_retry_after() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#);
    transport.reply_with_headers(
        429,
        vec![("retry-after".into(), "7".into())],
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
    );
    transport.reply(529, r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#);
    transport.reply(400, "<html>not json</html>");
    transport.reply(502, "");
    let client = AnthropicClient::new(transport, "k");
    let request = Request::new("claude-opus-5-5", vec![Message::user("hi")]);
    assert_eq!(client.complete_blocking(&request).unwrap_err(), LlmError::Auth("authentication_error: invalid x-api-key".into()));
    let err = client.complete_blocking(&request).unwrap_err();
    assert_eq!(err, LlmError::RateLimited { message: "rate_limit_error: slow down".into(), retry_after: Some(Duration::from_secs(7)) });
    assert!(err.retryable());
    assert_eq!(client.complete_blocking(&request).unwrap_err(), LlmError::Overloaded("overloaded_error: busy".into()));
    let err = client.complete_blocking(&request).unwrap_err();
    assert_eq!(err, LlmError::BadRequest("<html>not json</html>".into()));
    assert!(!err.retryable());
    assert_eq!(client.complete_blocking(&request).unwrap_err(), LlmError::Server { status: 502, message: "no details".into() });
}

#[test]
fn anthropic_reports_an_error_event_a_truncated_stream_and_a_cancel() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(
        200,
        sse(&[
            ("message_start", r#"{"type":"message_start","message":{"model":"m","usage":{"input_tokens":1}}}"#),
            ("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#),
        ]),
    );
    transport.reply(
        200,
        sse(&[
            ("message_start", r#"{"type":"message_start","message":{"model":"m"}}"#),
            ("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
        ]),
    );
    transport.reply(200, anthropic_stream());
    transport.reply(200, sse(&[("content_block_delta", r#"{"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"x"}}"#)]));
    let client = AnthropicClient::new(transport, "k");
    let request = Request::new("m", vec![Message::user("hi")]);
    assert_eq!(client.complete_blocking(&request).unwrap_err(), LlmError::Overloaded("Overloaded".into()));
    assert!(matches!(client.complete_blocking(&request).unwrap_err(), LlmError::Protocol(m) if m.contains("message_stop")));
    let mut texts = 0;
    let err = client
        .complete(&request, &mut |e| {
            if matches!(e, Event::TextDelta(_)) {
                texts += 1;
                Flow::Stop
            } else {
                Flow::Continue
            }
        })
        .unwrap_err();
    assert_eq!(err, LlmError::Cancelled);
    assert_eq!(texts, 1);
    assert!(matches!(client.complete_blocking(&request).unwrap_err(), LlmError::Protocol(m) if m.contains("not started")));
}

#[test]
fn anthropic_keeps_a_refusal_and_unknown_blocks() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(
        200,
        sse(&[
            ("message_start", r#"{"type":"message_start","message":{"model":"claude-opus-4-8","usage":{"input_tokens":3}}}"#),
            ("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"fallback","from":{"model":"claude-fable-5-1"},"to":{"model":"claude-opus-4-8"}}}"#),
            ("content_block_stop", r#"{"type":"content_block_stop","index":0}"#),
            ("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":0}}"#),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ]),
    );
    let client = AnthropicClient::new(transport, "k");
    let completion = client.complete_blocking(&Request::new("claude-fable-5-1", vec![Message::user("hi")])).unwrap();
    assert_eq!(completion.stop_reason, StopReason::Refusal);
    assert_eq!(completion.model, "claude-opus-4-8");
    assert!(matches!(&completion.content[0], ContentBlock::Raw { provider, value } if provider == "anthropic" && value["type"] == "fallback"));
    assert_eq!(completion.text(), "");
}

#[test]
fn anthropic_base_url_and_empty_tool_input() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(
        200,
        sse(&[
            ("message_start", r#"{"type":"message_start","message":{"model":"m"}}"#),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"inspect_document","input":{}}}"#,
            ),
            ("content_block_stop", r#"{"type":"content_block_stop","index":0}"#),
            ("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ]),
    );
    let client = AnthropicClient::new(transport.clone(), "k").with_base_url("http://proxy.local:8080/");
    let completion = client.complete_blocking(&Request::new("m", vec![Message::user("hi")])).unwrap();
    assert_eq!(transport.requests()[0].url, "http://proxy.local:8080/v1/messages");
    assert_eq!(completion.content, vec![ContentBlock::ToolUse { id: "t".into(), name: "inspect_document".into(), input: json!({}) }]);
    assert_eq!(completion.as_message().role, Role::Assistant);
}

// OpenAI and compatible servers

fn openai_stream() -> String {
    sse(&[
        (
            "",
            r#"{"id":"c1","object":"chat.completion.chunk","model":"gpt-5.1","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
        ),
        ("", r#"{"id":"c1","model":"gpt-5.1","choices":[{"index":0,"delta":{"reasoning_content":"think "},"finish_reason":null}]}"#),
        ("", r#"{"id":"c1","model":"gpt-5.1","choices":[{"index":0,"delta":{"content":"Sure, "},"finish_reason":null}]}"#),
        ("", r#"{"id":"c1","model":"gpt-5.1","choices":[{"index":0,"delta":{"content":"renaming."},"finish_reason":null}]}"#),
        (
            "",
            r#"{"id":"c1","model":"gpt-5.1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"set_text","arguments":""}}]},"finish_reason":null}]}"#,
        ),
        (
            "",
            r#"{"id":"c1","model":"gpt-5.1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"id\":7,"}}]},"finish_reason":null}]}"#,
        ),
        (
            "",
            r#"{"id":"c1","model":"gpt-5.1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"text\":\"Q3\"}"}}]},"finish_reason":null}]}"#,
        ),
        (
            "",
            r#"{"id":"c1","model":"gpt-5.1","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_2","type":"function","function":{"name":"go_to_slide","arguments":"{\"index\":2}"}}]},"finish_reason":null}]}"#,
        ),
        ("", r#"{"id":"c1","model":"gpt-5.1","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#),
        (
            "",
            r#"{"id":"c1","model":"gpt-5.1","choices":[],"usage":{"prompt_tokens":30,"completion_tokens":12,"total_tokens":42,"prompt_tokens_details":{"cached_tokens":8}}}"#,
        ),
        ("", "[DONE]"),
    ])
}

#[test]
fn openai_streams_text_reasoning_and_parallel_tool_calls() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(200, openai_stream());
    let client = OpenAiClient::openai(transport.clone(), "sk-openai");
    let request = Request::new("gpt-5.1", vec![Message::user("Rename the title to Q3")])
        .with_system("You edit decks.")
        .with_tools(vec![tool()])
        .with_effort(Effort::XHigh);
    let (result, events) = collect(&client, &request);
    let completion = result.unwrap();
    assert_eq!(completion.stop_reason, StopReason::ToolUse);
    assert_eq!(completion.usage, Usage { input_tokens: 30, output_tokens: 12, cache_read_input_tokens: 8, cache_creation_input_tokens: 0 });
    assert_eq!(
        completion.content,
        vec![
            ContentBlock::Thinking { text: "think ".into(), signature: None },
            ContentBlock::Text { text: "Sure, renaming.".into() },
            ContentBlock::ToolUse { id: "call_1".into(), name: "set_text".into(), input: json!({"id": 7, "text": "Q3"}) },
            ContentBlock::ToolUse { id: "call_2".into(), name: "go_to_slide".into(), input: json!({"index": 2}) },
        ]
    );
    assert_eq!(
        events,
        vec![
            Event::ThinkingDelta("think ".into()),
            Event::TextDelta("Sure, ".into()),
            Event::TextDelta("renaming.".into()),
            Event::ToolUseStart { index: 2, id: "call_1".into(), name: "set_text".into() },
            Event::ToolInputDelta { index: 2, partial_json: "{\"id\":7,".into() },
            Event::ToolInputDelta { index: 2, partial_json: "\"text\":\"Q3\"}".into() },
            Event::ToolUseStart { index: 3, id: "call_2".into(), name: "go_to_slide".into() },
            Event::ToolInputDelta { index: 3, partial_json: "{\"index\":2}".into() },
            Event::Usage(Usage { input_tokens: 30, output_tokens: 12, cache_read_input_tokens: 8, cache_creation_input_tokens: 0 }),
        ]
    );

    let http = &transport.requests()[0];
    assert_eq!(http.url, "https://api.openai.com/v1/chat/completions");
    assert_eq!(http.header("authorization"), Some("Bearer sk-openai"));
    let body = transport.last_body().unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert_eq!(body["max_completion_tokens"], 16000);
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["reasoning_effort"], "high");
    assert_eq!(body["messages"], json!([{"role": "system", "content": "You edit decks."}, {"role": "user", "content": "Rename the title to Q3"}]));
    assert_eq!(
        body["tools"][0],
        json!({"type": "function", "function": {"name": "set_text", "description": "Replace a shape's text", "parameters": tool().input_schema}})
    );
}

#[test]
fn openai_maps_tool_results_to_tool_messages_and_assistant_calls_to_tool_calls() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(200, openai_stream());
    let client = OpenAiClient::ollama(transport.clone());
    let assistant = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking { text: "dropped".into(), signature: None },
            ContentBlock::Text { text: "Renaming.".into() },
            ContentBlock::ToolUse { id: "call_1".into(), name: "set_text".into(), input: json!({"id": 7}) },
        ],
    };
    let results = Message {
        role: Role::User,
        content: vec![
            ContentBlock::ToolResult { tool_use_id: "call_1".into(), content: "ok".into(), is_error: false },
            ContentBlock::Text { text: "Now the notes.".into() },
        ],
    };
    let request = Request::new("llama3.3", vec![Message::user("hi"), assistant, results]).with_effort(Effort::Max);
    client.complete_blocking(&request).unwrap();
    assert_eq!(client.provider(), "ollama");
    let http = &transport.requests()[0];
    assert_eq!(http.url, "http://127.0.0.1:11434/v1/chat/completions");
    assert_eq!(http.header("authorization"), None);
    let body = transport.last_body().unwrap();
    assert_eq!(body["max_tokens"], 16000);
    assert!(body.get("max_completion_tokens").is_none());
    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(
        body["messages"],
        json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "Renaming.", "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "set_text", "arguments": "{\"id\":7}"}}]},
            {"role": "tool", "tool_call_id": "call_1", "content": "ok"},
            {"role": "user", "content": "Now the notes."},
        ])
    );
}

#[test]
fn openai_errors_stop_reasons_and_a_tool_call_reported_as_stop() {
    let transport = Arc::new(MockTransport::new());
    transport.reply(404, r#"{"error":{"message":"model `nope` not found","type":"invalid_request_error","code":"model_not_found"}}"#);
    transport.reply(200, sse(&[("", r#"{"error":{"message":"The server had an error","type":"server_error","code":500}}"#)]));
    transport.reply(200, sse(&[("", r#"{"choices":[{"index":0,"delta":{"content":"cut"},"finish_reason":"length"}]}"#), ("", "[DONE]")]));
    transport.reply(200, sse(&[("", r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"inspect_document","arguments":""}}]},"finish_reason":"stop"}]}"#), ("", "[DONE]")]));
    transport.reply(200, sse(&[("", r#"{"choices":[{"index":0,"delta":{"content":"half"},"finish_reason":null}]}"#)]));
    let client = OpenAiClient::compatible(transport, "gateway", "http://gateway.local/v1/", Some("tok".into()));
    let request = Request::new("m", vec![Message::user("hi")]);
    assert_eq!(client.complete_blocking(&request).unwrap_err(), LlmError::NotFound("invalid_request_error: model `nope` not found".into()));
    assert_eq!(client.complete_blocking(&request).unwrap_err(), LlmError::Server { status: 500, message: "The server had an error".into() });
    let cut = client.complete_blocking(&request).unwrap();
    assert_eq!((cut.text(), cut.stop_reason), ("cut".to_string(), StopReason::MaxTokens));
    let call = client.complete_blocking(&request).unwrap();
    assert_eq!(call.stop_reason, StopReason::ToolUse);
    assert_eq!(call.content, vec![ContentBlock::ToolUse { id: "c".into(), name: "inspect_document".into(), input: json!({}) }]);
    assert!(matches!(client.complete_blocking(&request).unwrap_err(), LlmError::Protocol(m) if m.contains("ended early")));
}

// Keys

#[test]
fn keys_prefer_the_maluos_variable_and_ignore_blank_values() {
    let lookup = |name: &str| match name {
        "MALUOS_SLIDES_ANTHROPIC_API_KEY" => Some(" sk-maluos ".to_string()),
        "ANTHROPIC_API_KEY" => Some("sk-general".to_string()),
        "OPENAI_API_KEY" => Some("   ".to_string()),
        _ => None,
    };
    assert_eq!(api_key_with(Provider::Anthropic, lookup).unwrap(), "sk-maluos");
    assert_eq!(api_key_with(Provider::Anthropic, |n| (n == "ANTHROPIC_API_KEY").then(|| "sk-general".to_string())).unwrap(), "sk-general");
    let err = api_key_with(Provider::OpenAi, lookup).unwrap_err();
    assert_eq!(err, LlmError::NoApiKey("set MALUOS_SLIDES_OPENAI_API_KEY or OPENAI_API_KEY for openai".into()));
    assert!(!err.retryable());
}

// Mock client and the shared types

#[test]
fn mock_client_replays_blocks_as_events_records_requests_and_honours_stop() {
    let client = MockClient::new();
    let completion = Completion {
        model: "mock-1".into(),
        content: vec![
            ContentBlock::Text { text: "Adding a slide.".into() },
            ContentBlock::ToolUse { id: "1".into(), name: "slide.new".into(), input: json!({"title": "Thanks"}) },
        ],
        stop_reason: StopReason::ToolUse,
        usage: Usage { input_tokens: 5, output_tokens: 6, ..Usage::default() },
    };
    client.push(completion.clone()).push(completion.clone()).push_error(LlmError::Overloaded("later".into()));
    let request = Request::new("mock-1", vec![Message::user("Add a closing slide")]);
    let (result, events) = collect(&client, &request);
    assert_eq!(result.unwrap(), completion);
    assert_eq!(
        events,
        vec![
            Event::TextDelta("Adding a slide.".into()),
            Event::ToolUseStart { index: 1, id: "1".into(), name: "slide.new".into() },
            Event::ToolInputDelta { index: 1, partial_json: "{\"title\":\"Thanks\"}".into() },
            Event::Usage(Usage { input_tokens: 5, output_tokens: 6, ..Usage::default() }),
        ]
    );
    assert_eq!(client.complete(&request, &mut |_| Flow::Stop).unwrap_err(), LlmError::Cancelled);
    assert_eq!(client.complete_blocking(&request).unwrap_err(), LlmError::Overloaded("later".into()));
    assert!(matches!(client.complete_blocking(&request).unwrap_err(), LlmError::Transport(_)));
    assert_eq!(client.requests().len(), 4);
    assert_eq!(client.provider(), "mock");
}

#[test]
fn request_and_completion_round_trip_through_json() {
    let request = Request::new("m", vec![Message::user("hi"), Message::assistant("hello")])
        .with_system("s")
        .with_tools(vec![tool()])
        .with_max_tokens(500)
        .with_effort(Effort::XHigh);
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["effort"], "xhigh");
    assert_eq!(json["messages"][0], json!({"role": "user", "content": [{"type": "text", "text": "hi"}]}));
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let completion = Completion {
        model: "m".into(),
        content: vec![
            ContentBlock::Thinking { text: "t".into(), signature: None },
            ContentBlock::ToolResult { tool_use_id: "x".into(), content: "c".into(), is_error: true },
        ],
        stop_reason: StopReason::Other("pause_turn".into()),
        usage: Usage::default(),
    };
    let json: Value = serde_json::to_value(&completion).unwrap();
    assert_eq!(json["stop_reason"], json!({"other": "pause_turn"}));
    assert!(json["content"][0].get("signature").is_none());
    assert_eq!(serde_json::from_value::<Completion>(json).unwrap(), completion);
}

#[test]
fn error_messages_come_from_the_body_or_a_shortened_raw_text() {
    assert_eq!(crate::error_message(r#"{"error":{"type":"x","message":"m"}}"#), "x: m");
    assert_eq!(crate::error_message(r#"{"message":"bare"}"#), "bare");
    assert_eq!(crate::error_message(r#"{"error":{"message":"m","code":"c"}}"#), "c: m");
    assert_eq!(crate::error_message("   "), "no details");
    let long = "é".repeat(400);
    let short = crate::error_message(&long);
    assert!(short.len() <= 300 && short.chars().all(|c| c == 'é'));
    assert_eq!(LlmError::from_status(418, "teapot".into()), LlmError::BadRequest("teapot".into()));
    assert_eq!(LlmError::from_status(302, "moved".into()), LlmError::Http { status: 302, message: "moved".into() });
}
