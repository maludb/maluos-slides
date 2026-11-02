//! One assistant turn against a real model, headless. A preview of `maluos-slides-cli ask`
//! (step 3.7); not a test.
//!
//! ```text
//! cargo run -p maluos-slides-assist --example ask -- ollama glm-5.3-flash:cloud "add a closing slide that thanks the audience" --save out.pptx
//! cargo run -p maluos-slides-assist --example ask -- anthropic "" "tighten the title"          # ANTHROPIC_API_KEY, default model
//! cargo run -p maluos-slides-assist --example ask -- openai gpt-5.1 "…" --open deck.pptx
//! ```

use std::io::Write;
use std::sync::Arc;

use maluos_llm::{AnthropicClient, Flow, LlmClient, OpenAiClient, UreqTransport};
use maluos_slides_assist::{Assistant, Headless, Mode, Plan, TurnEvent, Verdict};
use serde_json::json;

fn usage() -> ! {
    eprintln!(
        "usage: ask (ollama|anthropic|openai) <model or \"\"> \"<request>\" [--open deck.pptx] [--save out.pptx] [--sample] [--mode answer|confirm|autopilot] [--yes]"
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(backend), Some(model), Some(request)) = (args.first(), args.get(1), args.get(2)) else { usage() };
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let open = flag("--open");
    let save = flag("--save");
    let sample = args.iter().any(|a| a == "--sample");
    let yes = args.iter().any(|a| a == "--yes");
    let mode = flag("--mode").map(|m| Mode::parse(&m).unwrap_or_else(|| usage())).unwrap_or(Mode::Autopilot);

    let transport = Arc::new(UreqTransport::default());
    let (client, model): (Arc<dyn LlmClient>, String) = match backend.as_str() {
        "ollama" => (Arc::new(OpenAiClient::ollama(transport)), model.clone()),
        "anthropic" => match AnthropicClient::from_env() {
            Ok(c) => (Arc::new(c), if model.is_empty() { maluos_llm::anthropic::DEFAULT_MODEL.into() } else { model.clone() }),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1)
            }
        },
        "openai" => match OpenAiClient::openai_from_env() {
            Ok(c) => (Arc::new(c), if model.is_empty() { maluos_llm::openai::DEFAULT_MODEL.into() } else { model.clone() }),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1)
            }
        },
        _ => usage(),
    };

    let mut host = Headless::new();
    let opened = if let Some(p) = &open {
        host.session.execute("file.open", &json!({"path": p}))
    } else if sample {
        maluos_slides_engine::sample::open_sample(&mut host.session).map(|_| json!({}))
    } else {
        host.session.execute("file.new", &json!({}))
    };
    if let Err(e) = opened {
        eprintln!("error: {e}");
        std::process::exit(1)
    }

    let mut assistant = Assistant::new(client, model).with_mode(mode);
    let mut confirm = |plan: &Plan| -> Verdict {
        eprintln!("\x1b[33m[plan]{}\n{}\x1b[0m", if plan.destructive() { " destructive" } else { "" }, plan.describe());
        if yes {
            return Verdict::Approve;
        }
        eprint!("Apply? [y/N/q] ");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => Verdict::Approve,
            "q" | "quit" => Verdict::Cancel,
            other => Verdict::Decline(if other.is_empty() || other == "n" || other == "no" { None } else { Some(other.to_string()) }),
        }
    };
    let mut out = std::io::stdout();
    let report = assistant.ask(&mut host, request, &mut confirm, &mut |e| {
        match e {
            TurnEvent::Round(n) => eprintln!("\x1b[2m[round {n}]\x1b[0m"),
            TurnEvent::Text(t) => {
                let _ = out.write_all(t.as_bytes());
                let _ = out.flush();
            }
            TurnEvent::Thinking(t) => eprint!("\x1b[2m{t}\x1b[0m"),
            TurnEvent::ToolStart { name, input } => eprintln!("\x1b[36m[tool] {name} {input}\x1b[0m"),
            TurnEvent::ToolDone(o) => {
                let first = o.text.lines().next().unwrap_or("");
                eprintln!(
                    "\x1b[2m[{}] {}{}\x1b[0m",
                    if o.is_error { "error" } else { "ok" },
                    &first[..first.len().min(200)],
                    if o.text.len() > 200 { "…" } else { "" }
                );
            }
        }
        Flow::Continue
    });
    println!();
    eprintln!(
        "[turn] rounds={} tool calls={} commands={:?} tokens in={} out={} undo={:?} stopped={:?} error={:?}",
        report.rounds,
        report.tool_calls.len(),
        report.commands,
        report.usage.input_tokens,
        report.usage.output_tokens,
        report.undo_label,
        report.stopped,
        report.error
    );
    if let Some(p) = save {
        match host.session.execute("file.save", &json!({"path": p})) {
            Ok(_) => eprintln!("[saved] {p}"),
            Err(e) => eprintln!("[save failed] {e}"),
        }
    }
    if report.error.is_some() {
        std::process::exit(1)
    }
}
