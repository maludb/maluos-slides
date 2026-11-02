//! MaluOS: `maluos-slides-cli ask` — one assistant turn on a presentation, headless
//! (fork code; `docs/maluos-slides/plan.md` step 3.7).

use std::io::{IsTerminal, Write as _};
use std::sync::Arc;

use maluos_llm::{Flow, LlmClient};
use maluos_slides_assist::providers::{Choice, default_choice};
use maluos_slides_assist::{Assistant, Headless, Limits, Mode, Plan, TurnEvent, TurnReport, Verdict};
use serde_json::{Value, json};

/// Everything the command line said.
#[derive(Debug, Clone)]
pub struct Options {
    pub request: String,
    pub input: Option<String>,
    pub sample: bool,
    pub out: Option<String>,
    pub provider: Option<Choice>,
    pub model: Option<String>,
    pub mode: Mode,
    /// Approve every plan without asking.
    pub yes: bool,
    pub json: bool,
    pub quiet: bool,
    pub max_rounds: Option<usize>,
}

/// Parses the arguments; `Ok(None)` when help was printed.
pub fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut o = Options {
        request: String::new(),
        input: None,
        sample: false,
        out: None,
        provider: None,
        model: None,
        mode: Mode::Confirm,
        yes: false,
        json: false,
        quiet: false,
        max_rounds: None,
    };
    let mut stdin = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--in" => o.input = Some(it.next().cloned().ok_or("--in needs a file")?),
            "--sample" => o.sample = true,
            "--new" => {}
            "--out" | "--save" => o.out = Some(it.next().cloned().ok_or("--out needs a path")?),
            "--provider" => {
                let p = it.next().ok_or("--provider needs anthropic, openai or ollama")?;
                o.provider = Some(Choice::parse(p).ok_or_else(|| format!("unknown provider `{p}` (anthropic|openai|ollama)"))?);
            }
            "--model" => o.model = Some(it.next().cloned().ok_or("--model needs a name")?),
            "--mode" => {
                let m = it.next().ok_or("--mode needs answer, confirm or autopilot")?;
                o.mode = Mode::parse(m).ok_or_else(|| format!("unknown mode `{m}` (answer|confirm|autopilot)"))?;
            }
            "--yes" | "-y" => o.yes = true,
            "--json" => o.json = true,
            "--quiet" | "-q" => o.quiet = true,
            "--stdin" => stdin = true,
            "--max-rounds" => {
                let n = it.next().ok_or("--max-rounds needs a number")?;
                o.max_rounds = Some(n.parse().map_err(|_| format!("--max-rounds: not a number: {n}"))?);
            }
            "--help" | "-h" => return Ok(None),
            other if other.starts_with("--") => return Err(format!("unknown ask option `{other}`")),
            text if o.request.is_empty() => o.request = text.to_string(),
            extra => return Err(format!("unexpected argument `{extra}` (quote the request as one argument)")),
        }
    }
    if stdin {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).map_err(|e| format!("stdin: {e}"))?;
        let mut rest = String::new();
        while std::io::stdin().read_line(&mut rest).map_err(|e| format!("stdin: {e}"))? > 0 {
            s.push_str(&rest);
            rest.clear();
        }
        o.request = s.trim().to_string();
    }
    if o.request.is_empty() {
        return Err("ask needs a request: maluos-slides-cli ask \"add a closing slide\" --sample --out out.pptx".into());
    }
    if o.input.is_some() && o.sample {
        return Err("--in and --sample exclude each other".into());
    }
    Ok(Some(o))
}

/// Runs the turn with the provider's client and prints as the options say.
pub fn run(o: &Options) -> Result<(), String> {
    let provider = o.provider.unwrap_or_else(default_choice);
    let client = provider.client()?;
    let model = o.model.clone().unwrap_or_else(|| provider.default_model().to_string());
    let interactive = !o.yes && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let (report, saved) = run_with(o, client, &model, interactive)?;
    if o.json {
        println!("{}", report_json(&report, saved.as_deref()));
    } else if !o.quiet {
        if report.answer.is_empty() {
            eprintln!("(no answer)");
        }
        if let Some(p) = &saved {
            eprintln!("[saved] {p}");
        }
    }
    match &report.error {
        Some(e) => Err(format!("the model call failed: {e}")),
        None => Ok(()),
    }
}

/// The turn itself, on any client: opens the deck, asks, saves. Plans are answered by `--yes`,
/// by a terminal prompt when `interactive`, else declined. Returns the report and the saved
/// path. The answer streams to stdout unless `--json` (then stdout holds only the JSON).
pub fn run_with(o: &Options, client: Arc<dyn LlmClient>, model: &str, interactive: bool) -> Result<(TurnReport, Option<String>), String> {
    let mut host = Headless::new();
    if o.sample {
        maluos_slides_engine::sample::open_sample(&mut host.session).map_err(|e| e.to_string())?;
    } else if let Some(p) = &o.input {
        host.session.execute("file.open", &json!({"path": p})).map_err(|e| e.to_string())?;
    } else {
        host.session.execute("file.new", &json!({})).map_err(|e| e.to_string())?;
    }
    let mut limits = Limits::default();
    if let Some(n) = o.max_rounds {
        limits.max_rounds = n.max(1);
    }
    let mut assistant = Assistant::new(client, model).with_mode(o.mode).with_limits(limits);
    let quiet = o.quiet || o.json;
    let yes = o.yes;
    let mut confirm = move |plan: &Plan| -> Verdict {
        if !quiet || interactive {
            eprintln!("[plan]{}\n{}", if plan.destructive() { " destructive" } else { "" }, plan.describe());
        }
        if yes {
            return Verdict::Approve;
        }
        if !interactive {
            if !quiet {
                eprintln!("[declined] not a terminal; pass --yes to approve plans in scripts");
            }
            return Verdict::Decline(Some("the script did not pass --yes".into()));
        }
        eprint!("Apply? [y/N/q] ");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => Verdict::Approve,
            "q" | "quit" => Verdict::Cancel,
            "" | "n" | "no" => Verdict::Decline(None),
            other => Verdict::Decline(Some(other.to_string())),
        }
    };
    let to_stdout = !o.json;
    let mut out = std::io::stdout();
    let report = assistant.ask(&mut host, &o.request, &mut confirm, &mut |e| {
        match e {
            TurnEvent::Text(t) if to_stdout => {
                let _ = out.write_all(t.as_bytes());
                let _ = out.flush();
            }
            TurnEvent::ToolStart { name, input } if !quiet => eprintln!("[tool] {name} {input}"),
            TurnEvent::ToolDone(r) if !quiet => {
                eprintln!("[{}] {}", if r.is_error { "error" } else { "ok" }, maluos_slides_assist::summarize(&r.text))
            }
            _ => {}
        }
        Flow::Continue
    });
    if to_stdout && !report.answer.is_empty() {
        let _ = out.write_all(b"\n");
    }
    let mut saved = None;
    if let Some(p) = &o.out {
        host.session.execute("file.saveAs", &json!({"path": p})).map_err(|e| e.to_string())?;
        saved = Some(p.clone());
    }
    Ok((report, saved))
}

/// The report as JSON for scripts.
pub fn report_json(r: &TurnReport, saved: Option<&str>) -> Value {
    json!({
        "answer": r.answer,
        "rounds": r.rounds,
        "toolCalls": r.tool_calls.iter().map(|c| json!({"name": c.name, "input": c.input, "ok": c.ok, "summary": c.summary})).collect::<Vec<_>>(),
        "commands": r.commands,
        "touchedSlides": r.touched_slides,
        "removedSlides": r.removed_slides,
        "plans": r.plans,
        "declined": r.declined,
        "refused": r.refused,
        "usage": {"inputTokens": r.usage.input_tokens, "outputTokens": r.usage.output_tokens, "cacheReadInputTokens": r.usage.cache_read_input_tokens},
        "undo": r.undo_label,
        "stopped": r.stopped.as_ref().map(|s| format!("{s:?}")),
        "error": r.error.as_ref().map(|e| e.to_string()),
        "saved": saved,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use maluos_llm::mock::MockClient;
    use maluos_llm::{Completion, ContentBlock, StopReason, Usage};

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn usage() -> Usage {
        Usage { input_tokens: 10, output_tokens: 2, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 }
    }

    fn calls(blocks: Vec<ContentBlock>) -> Completion {
        Completion { model: "mock".into(), content: blocks, stop_reason: StopReason::ToolUse, usage: usage() }
    }

    fn answer(text: &str) -> Completion {
        Completion { model: "mock".into(), content: vec![ContentBlock::Text { text: text.into() }], stop_reason: StopReason::EndTurn, usage: usage() }
    }

    fn tmp(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("maluos-ask-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&d);
        d.to_string_lossy().to_string()
    }

    #[test]
    fn parse_reads_the_request_and_flags() {
        let o = parse(&args(&[
            "add a slide",
            "--sample",
            "--out",
            "x.pptx",
            "--mode",
            "auto",
            "--yes",
            "--json",
            "--provider",
            "ollama",
            "--model",
            "m",
            "--max-rounds",
            "3",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(o.request, "add a slide");
        assert!(o.sample && o.yes && o.json);
        assert_eq!(o.out.as_deref(), Some("x.pptx"));
        assert_eq!(o.mode, Mode::Autopilot);
        assert_eq!(o.provider, Some(Choice::Ollama));
        assert_eq!(o.model.as_deref(), Some("m"));
        assert_eq!(o.max_rounds, Some(3));
        assert!(parse(&args(&["--help"])).unwrap().is_none());
        assert!(parse(&args(&[])).unwrap_err().contains("needs a request"));
        assert!(parse(&args(&["x", "--mode", "nope"])).unwrap_err().contains("unknown mode"));
        assert!(parse(&args(&["x", "--provider", "nope"])).unwrap_err().contains("unknown provider"));
        assert!(parse(&args(&["x", "--in", "a", "--sample"])).unwrap_err().contains("exclude"));
        assert!(parse(&args(&["x", "y"])).unwrap_err().contains("quote the request"));
        assert!(parse(&args(&["x", "--bogus"])).unwrap_err().contains("unknown ask option"));
    }

    #[test]
    fn a_scripted_turn_edits_the_deck_and_saves_it() {
        let mock = Arc::new(MockClient::new());
        mock.push(calls(vec![ContentBlock::ToolUse {
            id: "a".into(),
            name: "add_slide".into(),
            input: json!({"layout": "titleOnly", "title": "Thank you"}),
        }]));
        mock.push(answer("Added a closing slide."));
        let out = tmp("closing.pptx");
        let o = parse(&args(&["add a closing slide", "--sample", "--out", &out, "--mode", "autopilot", "--json"])).unwrap().unwrap();
        let (report, saved) = run_with(&o, mock, "mock", false).unwrap();
        assert_eq!(report.answer, "Added a closing slide.");
        assert_eq!(report.commands, vec!["slide.new"]);
        assert_eq!(saved.as_deref(), Some(out.as_str()));
        assert!(std::fs::metadata(&out).map(|m| m.len() > 1000).unwrap_or(false));
        let j = report_json(&report, saved.as_deref());
        assert_eq!(j["touchedSlides"], json!([1]), "add_slide inserts after the current slide, slide 1 of the sample");
        assert_eq!(j["undo"], "Assistant: add a closing slide");
        assert_eq!(j["usage"]["inputTokens"], 20);
        // The saved deck has the slide.
        let mut s = maluos_slides_engine::Session::new();
        s.execute("file.open", &json!({"path": out})).unwrap();
        assert_eq!(s.active().unwrap().doc.slides.len(), 9);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn plans_are_declined_in_scripts_without_yes_and_approved_with_it() {
        let script = |mock: &MockClient| {
            mock.push(calls(vec![ContentBlock::ToolUse { id: "a".into(), name: "add_slide".into(), input: json!({"layout": "blank"}) }]));
            mock.push(answer("ok"));
        };
        let mock = Arc::new(MockClient::new());
        script(&mock);
        let o = parse(&args(&["add a slide", "--quiet"])).unwrap().unwrap();
        let (report, _) = run_with(&o, mock, "mock", false).unwrap();
        assert_eq!(report.declined, 1);
        assert!(report.commands.is_empty());

        let mock = Arc::new(MockClient::new());
        script(&mock);
        let o = parse(&args(&["add a slide", "--quiet", "--yes"])).unwrap().unwrap();
        let (report, _) = run_with(&o, mock, "mock", false).unwrap();
        assert_eq!(report.declined, 0);
        assert_eq!(report.commands, vec!["slide.new"]);
    }

    #[test]
    fn a_model_error_is_reported() {
        let mock = Arc::new(MockClient::new());
        mock.push_error(maluos_llm::LlmError::Auth("no".into()));
        let o = parse(&args(&["hi", "--quiet"])).unwrap().unwrap();
        let (report, _) = run_with(&o, mock, "mock", false).unwrap();
        assert!(report.error.is_some());
        assert_eq!(report_json(&report, None)["error"], "authentication failed: no");
        assert!(parse(&args(&["x", "--in", "/nonexistent/deck.pptx"])).is_ok());
        let mock = Arc::new(MockClient::new());
        let o = parse(&args(&["x", "--in", "/nonexistent/deck.pptx"])).unwrap().unwrap();
        assert!(run_with(&o, mock, "mock", false).is_err());
    }
}
