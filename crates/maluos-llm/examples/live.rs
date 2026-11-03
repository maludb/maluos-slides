//! A live check against a real server. Not a test: it needs a daemon or a key.
//!
//! ```text
//! cargo run -p maluos-llm --example live -- ollama glm-5.3-flash:cloud
//! cargo run -p maluos-llm --example live -- anthropic            # ANTHROPIC_API_KEY
//! cargo run -p maluos-llm --example live -- openai               # OPENAI_API_KEY
//! cargo run -p maluos-llm --example live -- compatible http://host:port/v1 model
//! ```
//!
//! Sends one prompt with one tool, streams the answer to stdout, then answers the tool call so
//! the whole exchange (request, stream, tool use, tool result) is exercised once.

use std::io::Write;
use std::sync::Arc;

use maluos_llm::{
    AnthropicClient, ContentBlock, Event, Flow, LlmClient, LlmError, Message, OpenAiClient, Request, StopReason, ToolDef, UreqTransport,
};

fn usage() -> ! {
    eprintln!("usage: live ollama <model> | anthropic [model] | openai [model] | compatible <base_url> <model>");
    std::process::exit(2)
}

fn client(args: &[String]) -> Result<(Box<dyn LlmClient>, String), LlmError> {
    let transport = Arc::new(UreqTransport::default());
    let backend = args.first().map(String::as_str).unwrap_or("");
    let model = |i: usize, default: &str| args.get(i).cloned().unwrap_or_else(|| default.to_string());
    match backend {
        "ollama" => {
            let Some(m) = args.get(1) else { usage() };
            Ok((Box::new(OpenAiClient::ollama(transport)), m.clone()))
        }
        "anthropic" => Ok((Box::new(AnthropicClient::from_env()?), model(1, maluos_llm::anthropic::DEFAULT_MODEL))),
        "openai" => Ok((Box::new(OpenAiClient::openai_from_env()?), model(1, maluos_llm::openai::DEFAULT_MODEL))),
        "compatible" => {
            let (Some(base), Some(m)) = (args.get(1), args.get(2)) else { usage() };
            Ok((Box::new(OpenAiClient::compatible(transport, "compatible", base.clone(), None)), m.clone()))
        }
        _ => usage(),
    }
}

fn run(client: &dyn LlmClient, model: &str) -> Result<(), LlmError> {
    let tool = ToolDef {
        name: "slide_count".into(),
        description: "Returns how many slides the open deck has.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false}),
    };
    let system = "You are the MaluOS Slides assistant. Use the slide_count tool when asked about the deck. Keep answers to one sentence.";
    let mut messages = vec![Message::user("How many slides does the open deck have?")];
    let mut out = std::io::stdout();

    for turn in 1..=3 {
        let request = Request::new(model, messages.clone()).with_system(system).with_tools(vec![tool.clone()]).with_max_tokens(400);
        eprintln!("[{}] turn {turn}: {model}", client.provider());
        let completion = client.complete(&request, &mut |event| {
            match event {
                Event::TextDelta(t) => {
                    let _ = out.write_all(t.as_bytes());
                    let _ = out.flush();
                }
                Event::ThinkingDelta(t) => eprint!("\x1b[2m{t}\x1b[0m"),
                Event::ToolUseStart { name, id, .. } => eprintln!("\n[tool use] {name} ({id})"),
                Event::ToolInputDelta { partial_json, .. } => eprint!("{partial_json}"),
                Event::Usage(u) => eprintln!("\n[usage] in={} out={} cached={}", u.input_tokens, u.output_tokens, u.cache_read_input_tokens),
            }
            Flow::Continue
        })?;
        println!();
        eprintln!("[stop] {:?} model={}", completion.stop_reason, completion.model);
        let calls: Vec<(String, String)> = completion.tool_uses().map(|(id, name, _)| (id.to_string(), name.to_string())).collect();
        messages.push(completion.as_message());
        if completion.stop_reason != StopReason::ToolUse || calls.is_empty() {
            return Ok(());
        }
        let results = calls
            .into_iter()
            .map(|(id, name)| ContentBlock::ToolResult {
                tool_use_id: id,
                content: if name == "slide_count" { "8".into() } else { format!("unknown tool {name}") },
                is_error: name != "slide_count",
                images: vec![],
            })
            .collect();
        messages.push(Message::tool_results(results));
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (client, model) = match client(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1)
        }
    };
    if let Err(e) = run(client.as_ref(), &model) {
        eprintln!("error: {e} (retryable: {})", e.retryable());
        std::process::exit(1)
    }
}
