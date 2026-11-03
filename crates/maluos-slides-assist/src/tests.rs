//! Tests run the tool table and the catalog against the headless engine; no model, no network.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use maluos_llm::ContentBlock;
use maluos_slides_engine::command_specs;
use maluos_slides_mcp::{Backend, Headless};
use serde_json::{Value, json};

use crate::{Catalog, MAX_RESULT_BYTES, ToolTable, params_schema, run_tool};

/// Compares `text` with `snapshots/<name>`; `MALUOS_SLIDES_UPDATE_SNAPSHOTS=1` rewrites it.
fn assert_snapshot(name: &str, text: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshots").join(name);
    if std::env::var_os("MALUOS_SLIDES_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e} (MALUOS_SLIDES_UPDATE_SNAPSHOTS=1 writes it)", path.display()));
    if want != text {
        let diff: Vec<String> = want
            .lines()
            .zip(text.lines())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .take(5)
            .map(|(i, (a, b))| format!("line {}:\n  - {a}\n  + {b}", i + 1))
            .collect();
        panic!(
            "{} differs ({} vs {} lines):\n{}\nMALUOS_SLIDES_UPDATE_SNAPSHOTS=1 rewrites it",
            path.display(),
            want.lines().count(),
            text.lines().count(),
            diff.join("\n")
        );
    }
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap();
    s.push('\n');
    s
}

fn session() -> Headless {
    Headless::with_document()
}

fn slide_count(h: &Headless) -> usize {
    h.session.active().unwrap().doc.slides.len()
}

// ---------- the tool table ----------

#[test]
fn table_mirrors_the_mcp_server() {
    let all = ToolTable::all();
    let mcp = maluos_slides_mcp::tool_definitions();
    assert_eq!(all.defs().len(), mcp.len() + ToolTable::extra_definitions().len());
    assert_eq!(all.defs().last().map(|t| t.name.as_str()), Some("build_deck"), "the fork's tools come last");
    for (t, m) in all.defs().iter().zip(&mcp) {
        assert_eq!(t.name, m["name"]);
        assert_eq!(t.description, m["description"]);
        assert_eq!(t.input_schema, m["inputSchema"]);
        assert_eq!(t.input_schema["type"], "object", "{}", t.name);
    }
    for n in ["run_command", "batch", "set_text", "add_shape", "insert_chart", "go_to_slide", "render_slide", "export", "inspect_document"] {
        assert!(all.get(n).is_some(), "{n}");
    }
    assert!(all.is_read_only("inspect_slide"));
    assert!(all.is_read_only("render_slide"));
    assert!(!all.is_read_only("run_command"));
    assert!(all.is_app_only("screenshot"));
    assert!(all.is_app_only("call_app"));
    assert!(!all.is_app_only("render_slide"));
}

#[test]
fn headless_table_leaves_out_app_only_tools() {
    let headless = ToolTable::for_backend(false);
    let app = ToolTable::for_backend(true);
    assert!(headless.get("screenshot").is_none());
    assert!(headless.get("call_app").is_none());
    assert!(app.get("screenshot").is_some());
    assert_eq!(app.defs().len(), headless.defs().len() + 2);
    assert!(headless.get("build_deck").is_some(), "the fork's tool is offered headless too");
    // The app-only names are still known, so a stray call gets a clear answer.
    assert!(headless.is_app_only("screenshot"));
}

#[test]
fn tool_schema_snapshot() {
    let v: Vec<Value> =
        ToolTable::all().defs().iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema})).collect();
    assert_snapshot("tools.json", &pretty(&Value::Array(v)));
}

// ---------- the catalog ----------

#[test]
fn every_params_doc_parses_into_a_schema() {
    let bad: Vec<&str> = command_specs().iter().filter(|s| params_schema(s.params).is_none()).map(|s| s.id).collect();
    assert!(bad.is_empty(), "docs the grammar cannot read: {bad:?}");
}

#[test]
fn catalog_lists_every_command() {
    let c = Catalog::new();
    assert_eq!(c.len(), command_specs().len());
    assert!(c.len() > 300, "{}", c.len());
    let new = c.get("slide.new").unwrap();
    assert!(new.undoable && new.journal && !new.is_query());
    let go = c.get("slide.go").unwrap();
    assert!(!go.undoable && go.journal);
    let inspect = c.get("slide.inspect").unwrap();
    assert!(inspect.is_query());
    assert!(c.get("edit.undo").is_some_and(|e| !e.enabled), "nothing to undo in a new deck");
}

#[test]
fn schema_from_params_doc() {
    let s = params_schema("{layout?: layout name or kind (title, blank), at?: index, title?: text, body?: text}").unwrap();
    assert_eq!(s["type"], "object");
    assert_eq!(s["additionalProperties"], false);
    assert!(s.get("required").is_none());
    assert_eq!(s["properties"]["at"]["type"], "integer");
    assert_eq!(s["properties"]["title"]["type"], "string");
    assert_eq!(s["properties"]["layout"]["description"], "layout name or kind (title, blank)");

    let s = params_schema("{index, name}").unwrap();
    assert_eq!(s["required"], json!(["index", "name"]));
    assert_eq!(s["properties"]["index"]["type"], "integer");

    let s = params_schema("{index} | {id}").unwrap();
    assert!(s.get("required").is_none());
    assert!(s["description"].as_str().unwrap().contains("{index} | {id}"));

    let s = params_schema("{text | body: formatted notes, index?} (text as typed: untouched notes keep their formatting)").unwrap();
    assert!(s.get("required").is_none(), "alternatives are never required: {s}");
    assert_eq!(s["properties"]["body"]["description"], "formatted notes");
    assert!(s["description"].as_str().unwrap().starts_with("text as typed"));

    let s = params_schema("{index?, hidden?: bool} → {hidden}").unwrap();
    assert_eq!(s["properties"]["hidden"]["type"], "boolean");
    assert_eq!(s["description"], "Returns: {hidden}");

    let s = params_schema("{}").unwrap();
    assert_eq!(s["properties"], json!({}));
}

#[test]
fn validate_refuses_unknown_commands_and_keys() {
    let c = Catalog::new();
    assert!(c.validate("slide.new", &json!({"layout": "blank", "title": "Hi"})).is_ok());
    assert!(c.validate("slide.new", &Value::Null).is_ok());
    let e = c.validate("slide.new", &json!({"layot": "blank"})).unwrap_err();
    assert!(e.contains("unknown parameter \"layot\"") && e.contains("layout"), "{e}");
    let e = c.validate("slide.new", &json!({"a": 1, "b": 2})).unwrap_err();
    assert!(e.contains("unknown parameters \"a\", \"b\""), "{e}");
    let e = c.validate("slide.new", &json!([1])).unwrap_err();
    assert!(e.contains("must be a JSON object"), "{e}");
    let e = c.validate("slide.nwe", &json!({})).unwrap_err();
    assert!(e.contains("unknown command `slide.nwe`") && e.contains("slide.new"), "{e}");
    let e = c.validate("nothing.here", &json!({})).unwrap_err();
    assert!(e.contains("list_commands"), "{e}");
    // Alternatives are both accepted.
    assert!(c.validate("slide.go", &json!({"index": 0})).is_ok());
    assert!(c.validate("slide.go", &json!({"id": "s1"})).is_ok());
}

#[test]
fn prompt_text_lists_filters_and_cuts() {
    let c = Catalog::new();
    let all = c.prompt_text("", usize::MAX);
    assert_eq!(all.lines().count(), c.len());
    assert!(all.contains("slide.new — New Slide {layout?"), "{}", all.lines().next().unwrap());
    let slides = c.prompt_text("slide.", usize::MAX);
    assert!(slides.lines().all(|l| l.to_lowercase().contains("slide")), "{slides}");
    assert!(slides.lines().count() < all.lines().count());
    let cut = c.prompt_text("", 2000);
    assert!(cut.len() <= 2000 + 80, "{}", cut.len());
    assert!(cut.lines().last().unwrap().starts_with("… ") && cut.contains("more"), "{cut}");
}

#[test]
fn command_schema_snapshot() {
    let c = Catalog::new();
    let v: Vec<Value> = c
        .entries()
        .iter()
        .map(|e| json!({"id": e.id, "label": e.label, "params": e.params, "undoable": e.undoable, "journal": e.journal, "schema": e.schema()}))
        .collect();
    assert_snapshot("commands.json", &pretty(&Value::Array(v)));
}

// ---------- execution ----------

#[test]
fn run_tool_goes_through_engine_commands() {
    let mut h = session();
    let table = ToolTable::for_backend(h.has_ui());
    let catalog = Catalog::from_session(&h.session);
    let before = slide_count(&h);

    let out = run_tool(&mut h, &table, &catalog, "t1", "add_slide", &json!({"layout": "titleAndContent", "title": "Agenda", "body": "One\nTwo"}));
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(slide_count(&h), before + 1);
    assert!(matches!(out.block(), ContentBlock::ToolResult { tool_use_id, is_error: false, .. } if tool_use_id == "t1"));

    let out = run_tool(&mut h, &table, &catalog, "t2", "inspect_slide", &json!({}));
    assert!(!out.is_error);
    let v: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(v["title"], "Agenda");

    let out = run_tool(&mut h, &table, &catalog, "t3", "run_command", &json!({"command": "slide.duplicate", "params": {}}));
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(slide_count(&h), before + 2);
    // One undo step per command; the loop (3.4) folds a turn's steps into one.
    assert_eq!(h.session.active().unwrap().history.undo.len(), 2);
}

#[test]
fn run_tool_checks_before_it_acts() {
    let mut h = session();
    let table = ToolTable::for_backend(false);
    let catalog = Catalog::from_session(&h.session);
    let before = (slide_count(&h), h.session.active().unwrap().revision);

    let out = run_tool(&mut h, &table, &catalog, "t1", "run_command", &json!({"command": "slide.new", "params": {"layot": "blank"}}));
    assert!(out.is_error);
    assert!(out.text.contains("unknown parameter \"layot\""), "{}", out.text);

    let out = run_tool(&mut h, &table, &catalog, "t2", "run_command", &json!({"command": "slide.nwe"}));
    assert!(out.is_error && out.text.contains("did you mean `slide.new`"), "{}", out.text);

    // A batch with a bad second command runs nothing.
    let out = run_tool(
        &mut h,
        &table,
        &catalog,
        "t3",
        "batch",
        &json!({"commands": [{"command": "slide.new", "params": {"layout": "blank"}}, {"command": "slide.new", "params": {"nope": 1}}]}),
    );
    assert!(out.is_error && out.text.contains("batch: command 1:"), "{}", out.text);

    let out = run_tool(&mut h, &table, &catalog, "t4", "no_such_tool", &json!({}));
    assert!(out.is_error && out.text.contains("unknown tool `no_such_tool`") && out.text.contains("run_command"), "{}", out.text);

    let out = run_tool(&mut h, &table, &catalog, "t5", "screenshot", &json!({}));
    assert!(out.is_error && out.text.contains("needs the desktop app"), "{}", out.text);

    // The MCP server's own argument check still applies to the other tools.
    let out = run_tool(&mut h, &table, &catalog, "t6", "add_slide", &json!({"titel": "x"}));
    assert!(out.is_error && out.text.contains("unknown argument"), "{}", out.text);

    assert_eq!((slide_count(&h), h.session.active().unwrap().revision), before, "nothing ran");
    assert!(matches!(out.block(), ContentBlock::ToolResult { is_error: true, .. }));
}

#[test]
fn images_and_long_results_are_shaped_for_the_model() {
    let mut h = session();
    let table = ToolTable::for_backend(false);
    let catalog = Catalog::from_session(&h.session);
    let out = run_tool(&mut h, &table, &catalog, "t1", "render_slide", &json!({"scale": 0.25}));
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.images[0].1, "image/png");
    assert!(out.text.starts_with("[image image/png, "), "{}", out.text);
    assert!(out.text.contains("\"width\""), "{}", out.text);

    let long = "x".repeat(MAX_RESULT_BYTES + 100);
    let out = run_tool(&mut h, &table, &catalog, "t2", "add_slide", &json!({"layout": "blank"}));
    assert!(!out.is_error);
    let shaped = crate::exec::ToolOutcome::from_result_for_test("t3", "x", maluos_slides_mcp::ToolResult::text(long));
    assert!(shaped.text.len() < MAX_RESULT_BYTES + 100);
    assert!(shaped.text.ends_with(&format!("{MAX_RESULT_BYTES} shown]")), "{}", &shaped.text[shaped.text.len() - 60..]);
}

// ---------- the context ----------

use crate::{Budget, Context, ContextCache};
use maluos_slides_engine::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn sample_deck_context_snapshot() {
    let mut s = Session::new();
    maluos_slides_engine::sample::open_sample(&mut s).unwrap();
    run(&mut s, "slide.go", json!({"index": 1}));
    let ctx = Context::build(&s, &Budget::default());
    assert!(ctx.cut_slides.is_empty(), "{:?}", ctx.cut_slides);
    assert!(ctx.text.contains("# Northern Lights"), "{}", ctx.text);
    assert!(ctx.text.contains("## Current slide\n2. "), "{}", ctx.text);
    assert!(ctx.text.contains("notes: \"Welcome everyone."), "{}", ctx.text);
    assert_snapshot("context-sample.md", &ctx.text);
}

#[test]
fn context_without_a_document() {
    let s = Session::new();
    let ctx = Context::build(&s, &Budget::default());
    assert!(ctx.text.starts_with("No presentation is open"));
    assert_eq!((ctx.uid, ctx.revision), (0, 0));
}

#[test]
fn context_shows_the_selection_and_edited_text() {
    let mut s = Session::with_new();
    let title = s.doc().unwrap().current_slide().unwrap().shapes[0].id;
    run(&mut s, "text.set", json!({"id": title, "text": "Quarterly Review"}));
    run(&mut s, "edit.select", json!({"ids": [title]}));
    let ctx = Context::build(&s, &Budget::default());
    assert!(ctx.text.contains("Selected shapes (1), the target of an edit:"), "{}", ctx.text);
    assert!(ctx.text.contains(&format!("placeholder id {title} ")), "{}", ctx.text);
    assert!(ctx.text.contains("text \"Quarterly Review\""), "{}", ctx.text);

    run(&mut s, "text.edit", json!({"id": title}));
    run(&mut s, "text.select", json!({"anchor": [0, 0], "caret": [0, 9]}));
    let ctx = Context::build(&s, &Budget::default());
    assert!(
        ctx.text.contains(&format!("editing text in shape {title}, selected from paragraph 0 char 0 to paragraph 0 char 9: \"Quarterly\"")),
        "{}",
        ctx.text
    );

    run(&mut s, "text.exit", json!({}));
    run(&mut s, "edit.deselect", json!({}));
    let ctx = Context::build(&s, &Budget::default());
    assert!(ctx.text.contains("Nothing selected on it."), "{}", ctx.text);
}

#[test]
fn context_cuts_a_long_deck_but_keeps_the_current_slide_whole() {
    let mut s = Session::with_new();
    let body: String = (1..=12).map(|i| format!("Point {i} with some words to fill the line\n")).collect();
    for i in 0..120 {
        run(&mut s, "slide.new", json!({"layout": "titleAndContent", "title": format!("Slide {i}"), "body": body}));
    }
    run(&mut s, "section.add", json!({"name": "Part two", "at": 30}));
    // Slide 3 gets ten shapes, more than its per-slide budget shows.
    run(&mut s, "slide.go", json!({"index": 2}));
    for k in 0..10 {
        run(&mut s, "shape.insert", json!({"preset": "rect", "rect": [20 + k * 40, 400, 30, 30], "text": format!("box {k}")}));
    }
    run(&mut s, "slide.go", json!({"index": 45}));
    let budget = Budget { total_bytes: 6000, slide_bytes: 300, text_chars: 60, current_text_chars: 2000, notes_chars: 50 };
    let ctx = Context::build(&s, &budget);
    let t = &ctx.text;
    assert!(t.len() < budget.total_bytes + 6000, "{}", t.len());
    // The current slide is whole: all twelve points, before the outline.
    let cur = t.split("## Outline").next().unwrap();
    assert!(cur.contains("## Current slide\n46. Slide 44"), "{cur}");
    assert_eq!(cur.matches("- Point ").count(), 12, "{cur}");
    assert!(t.contains("46. Slide 44 (current, above)"), "{t}");
    // Other slides are cut per slide, then to titles, then counted.
    assert!(t.contains("… more shapes [cut: inspect_slide 2]"), "{t}");
    assert!(t.contains("box 0") && !t.contains("box 9"), "{t}");
    assert!(t.contains("[body left out: inspect_slide"), "{t}");
    assert!(t.contains("more slides (inspect_document lists them)"), "{t}");
    assert!(t.contains("### Section: Part two\n31. Slide 29"), "{t}");
    assert!(!ctx.cut_slides.is_empty() && !ctx.cut_slides.contains(&45));
    assert!(t.contains("121 slides"), "{}", t.lines().nth(1).unwrap());
}

#[test]
fn context_cache_rebuilds_only_on_change() {
    let mut s = Session::with_new();
    let mut cache = ContextCache::new();
    let budget = Budget::default();
    let a = cache.get(&s, &budget).revision;
    let _ = cache.get(&s, &budget);
    assert_eq!(cache.builds, 1);
    run(&mut s, "slide.new", json!({"layout": "blank"}));
    let b = cache.get(&s, &budget).revision;
    assert!(b > a);
    assert_eq!(cache.builds, 2);
    let _ = cache.get(&s, &budget);
    assert_eq!(cache.builds, 2);
    // A selection change without an edit also rebuilds (the selection is in the text).
    run(&mut s, "slide.go", json!({"index": 0}));
    assert!(cache.get(&s, &budget).text.contains("Current slide: 1"));
    assert_eq!(cache.builds, 3);
    // A new document is a new uid.
    run(&mut s, "file.new", json!({}));
    let _ = cache.get(&s, &budget);
    assert_eq!(cache.builds, 4);
}

// ---------- the loop ----------

use std::sync::{Arc, Mutex};
use std::time::Duration;

use maluos_llm::mock::MockClient;
use maluos_llm::{Completion, Flow, LlmError, Role, StopReason, Usage};

use crate::{ApproveAll, Assistant, Limits, Stop, TurnEvent};

fn usage(input: u64, output: u64) -> Usage {
    Usage { input_tokens: input, output_tokens: output, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 }
}

fn tool_use(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse { id: id.into(), name: name.into(), input }
}

fn calls(blocks: Vec<ContentBlock>) -> Completion {
    Completion { model: "mock".into(), content: blocks, stop_reason: StopReason::ToolUse, usage: usage(100, 20) }
}

fn answer(text: &str) -> Completion {
    Completion {
        model: "mock".into(),
        content: vec![ContentBlock::Text { text: text.into() }],
        stop_reason: StopReason::EndTurn,
        usage: usage(100, 10),
    }
}

fn quick_limits() -> Limits {
    Limits { retry_delay: Duration::ZERO, ..Limits::default() }
}

fn assistant(mock: &Arc<MockClient>) -> Assistant {
    Assistant::new(mock.clone(), "mock-model").with_limits(quick_limits()).with_mode(crate::Mode::Autopilot)
}

fn no_events() -> impl FnMut(TurnEvent) -> Flow {
    |_| Flow::Continue
}

#[test]
fn closing_slide_through_the_mock_backend() {
    // The ids a fresh deck hands out are deterministic: a shadow session tells the script
    // which shape the body placeholder of the new slide will be.
    let body_id = {
        let mut shadow = Session::with_new();
        run(&mut shadow, "slide.new", json!({"layout": "titleAndContent", "title": "Thank you"}));
        shadow.doc().unwrap().current_slide().unwrap().shapes.iter().find(|s| s.ph.as_ref().is_some_and(|p| !p.kind.is_title())).unwrap().id
    };
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use(
        "c1",
        "run_command",
        json!({"command": "slide.new", "params": {"layout": "titleAndContent", "title": "Thank you"}}),
    )]));
    mock.push(calls(vec![tool_use("c2", "set_text", json!({"id": body_id, "text": "Thanks for listening\nQuestions welcome"}))]));
    mock.push(answer("Added a closing slide (slide 2) that thanks the audience."));

    let mut host = Headless::with_document();
    let mut a = assistant(&mock);
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let report = a.ask(&mut host, "add a closing slide that thanks the audience", &mut ApproveAll, &mut |e| {
        ev.lock().unwrap().push(e);
        Flow::Continue
    });

    assert_eq!(report.answer, "Added a closing slide (slide 2) that thanks the audience.");
    assert_eq!(report.rounds, 3);
    assert!(report.error.is_none() && report.stopped.is_none(), "{report:?}");
    assert_eq!(report.commands, vec!["slide.new", "text.set"]);
    assert_eq!(report.tool_calls.len(), 2);
    assert!(report.tool_calls.iter().all(|c| c.ok), "{:?}", report.tool_calls);
    assert_eq!(report.usage.input_tokens, 300);
    assert_eq!(report.usage.output_tokens, 50);

    // The deck: two slides, the text is there.
    let st = host.session.active().unwrap();
    assert_eq!(st.doc.slides.len(), 2);
    assert_eq!(st.doc.slides[1].title(), "Thank you");
    assert!(st.doc.slides[1].shapes.iter().any(|s| s.text.as_ref().is_some_and(|t| t.text().contains("Questions welcome"))));

    // One undo step for the whole turn.
    assert_eq!(st.history.undo.len(), 1, "{:?}", st.history.undo.iter().map(|e| &e.label).collect::<Vec<_>>());
    assert_eq!(report.undo_label.as_deref(), Some("Assistant: add a closing slide that thanks the audience"));
    assert_eq!(st.history.undo[0].label, "Assistant: add a closing slide that thanks the audience");
    run(&mut host.session, "edit.undo", json!({}));
    assert_eq!(host.session.active().unwrap().doc.slides.len(), 1);

    // The requests: the prompt carried the instructions, the catalog, the outline and the tools.
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 3);
    let system = reqs[0].system.as_deref().unwrap();
    assert!(system.starts_with("You are Malu"), "{}", &system[..40]);
    assert!(system.contains("# Command catalog") && system.contains("slide.new — New Slide"), "no catalog");
    assert!(system.contains("# The open presentation") && system.contains("1 slides"), "no outline");
    assert_eq!(reqs[0].tools.len(), crate::ToolTable::for_backend(false).defs().len());
    assert_eq!(reqs[0].messages.len(), 1);
    // The second request carries the assistant's tool call and its result, and a fresh outline.
    assert_eq!(reqs[1].messages.len(), 3);
    assert!(matches!(&reqs[1].messages[2].content[0], ContentBlock::ToolResult { tool_use_id, is_error: false, .. } if tool_use_id == "c1"));
    assert!(reqs[1].system.as_deref().unwrap().contains("2 slides"), "the outline is rebuilt each round");
    assert_eq!(a.messages.len(), 6);

    // Events, in order.
    let ev = events.lock().unwrap();
    assert!(matches!(ev[0], TurnEvent::Round(1)));
    assert!(matches!(&ev[1], TurnEvent::ToolStart { name, .. } if name == "run_command"));
    assert!(matches!(&ev[2], TurnEvent::ToolDone(o) if o.name == "run_command" && !o.is_error));
    assert!(matches!(ev[3], TurnEvent::Round(2)));
    assert!(matches!(ev.last(), Some(TurnEvent::Text(t)) if t.starts_with("Added")));
}

#[test]
fn round_limit_ends_a_turn_that_never_answers() {
    let mock = Arc::new(MockClient::new());
    for i in 0..5 {
        mock.push(calls(vec![tool_use(&format!("i{i}"), "inspect_document", json!({}))]));
    }
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_limits(Limits { max_rounds: 2, retry_delay: Duration::ZERO, ..Limits::default() });
    let report = a.ask(&mut host, "look around", &mut ApproveAll, &mut no_events());
    assert_eq!(report.stopped, Some(Stop::RoundLimit));
    assert_eq!(report.rounds, 2);
    assert_eq!(report.tool_calls.len(), 2);
    assert!(report.answer.is_empty());
    assert!(report.commands.is_empty(), "inspect is a query: {:?}", report.commands);
    assert!(host.session.active().unwrap().history.undo.is_empty());
    assert!(report.undo_label.is_none());
    // The conversation is left answerable: the last message is the tool results.
    assert!(matches!(a.messages.last().unwrap().content[0], ContentBlock::ToolResult { .. }));
}

#[test]
fn tool_call_limit_answers_the_extra_calls_with_an_error() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("a", "add_slide", json!({"layout": "blank"})), tool_use("b", "add_slide", json!({"layout": "blank"}))]));
    mock.push(answer("Added one slide; the limit stopped the second."));
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_limits(Limits { max_tool_calls: 1, retry_delay: Duration::ZERO, ..Limits::default() });
    let report = a.ask(&mut host, "add two blank slides", &mut ApproveAll, &mut no_events());
    assert_eq!(report.stopped, Some(Stop::ToolCallLimit));
    assert_eq!(report.tool_calls.len(), 2);
    assert!(report.tool_calls[0].ok && !report.tool_calls[1].ok);
    assert!(report.tool_calls[1].summary.contains("limit"), "{}", report.tool_calls[1].summary);
    assert_eq!(host.session.active().unwrap().doc.slides.len(), 2);
    assert_eq!(report.answer, "Added one slide; the limit stopped the second.");
    assert_eq!(report.commands, vec!["slide.new"]);
    assert_eq!(host.session.active().unwrap().history.undo.len(), 1);
}

#[test]
fn cancel_stops_before_the_tool_runs_and_keeps_the_conversation_valid() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("a", "add_slide", json!({"layout": "blank"}))]));
    mock.push(answer("ok"));
    let mut host = Headless::with_document();
    let mut a = assistant(&mock);
    let report =
        a.ask(&mut host, "add a slide", &mut ApproveAll, &mut |e| if matches!(e, TurnEvent::ToolStart { .. }) { Flow::Stop } else { Flow::Continue });
    assert_eq!(report.stopped, Some(Stop::Cancelled));
    assert!(report.commands.is_empty());
    assert_eq!(host.session.active().unwrap().doc.slides.len(), 1);
    assert!(report.undo_label.is_none());
    // The dangling tool call was answered, so the next turn sends a valid conversation.
    assert!(
        matches!(&a.messages.last().unwrap().content[0], ContentBlock::ToolResult { tool_use_id, is_error: true, content, .. } if tool_use_id == "a" && content.contains("cancel"))
    );
    let report = a.ask(&mut host, "never mind", &mut ApproveAll, &mut no_events());
    assert_eq!(report.answer, "ok");
    let reqs = mock.requests();
    assert_eq!(reqs[1].messages.len(), 4);
    assert_eq!(reqs[1].messages[0].role, Role::User);
}

#[test]
fn retryable_errors_are_retried_and_others_are_not() {
    let mock = Arc::new(MockClient::new());
    mock.push_error(LlmError::RateLimited { message: "slow down".into(), retry_after: Some(Duration::ZERO) });
    mock.push_error(LlmError::Overloaded("busy".into()));
    mock.push(answer("fine now"));
    let mut host = Headless::with_document();
    let mut a = assistant(&mock);
    let report = a.ask(&mut host, "hello", &mut ApproveAll, &mut no_events());
    assert_eq!(report.answer, "fine now");
    assert!(report.error.is_none());
    assert_eq!(mock.requests().len(), 3);

    let mock = Arc::new(MockClient::new());
    mock.push_error(LlmError::Auth("bad key".into()));
    mock.push(answer("unreachable"));
    let mut a = assistant(&mock);
    let report = a.ask(&mut host, "hello", &mut ApproveAll, &mut no_events());
    assert_eq!(report.error, Some(LlmError::Auth("bad key".into())));
    assert!(report.answer.is_empty());
    assert_eq!(mock.requests().len(), 1);
    // The failed turn's user message stays, so the user can retry without retyping.
    assert_eq!(a.messages.len(), 1);
}

#[test]
fn an_error_after_a_tool_call_keeps_the_change_as_one_undo_step() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("a", "add_slide", json!({"layout": "blank", "title": "Half done"}))]));
    mock.push_error(LlmError::Server { status: 500, message: "boom".into() });
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_limits(Limits { retries: 0, retry_delay: Duration::ZERO, ..Limits::default() });
    let report = a.ask(&mut host, "add a slide and then explain it", &mut ApproveAll, &mut no_events());
    assert!(matches!(report.error, Some(LlmError::Server { .. })));
    assert_eq!(report.commands, vec!["slide.new"]);
    let st = host.session.active().unwrap();
    assert_eq!(st.doc.slides.len(), 2);
    assert_eq!(st.history.undo.len(), 1);
    assert_eq!(report.undo_label.as_deref(), Some("Assistant: add a slide and then explain it"));
    // Conversation stays valid: the tool result is in, after it nothing dangles.
    assert!(matches!(a.messages.last().unwrap().content[0], ContentBlock::ToolResult { .. }));
}

#[test]
fn old_turns_are_trimmed_at_user_messages() {
    let mock = Arc::new(MockClient::new());
    for _ in 0..4 {
        mock.push(calls(vec![tool_use("q", "inspect_document", json!({}))]));
        mock.push(answer("done"));
    }
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_limits(Limits { max_messages: 5, retry_delay: Duration::ZERO, ..Limits::default() });
    for i in 0..4 {
        a.ask(&mut host, &format!("turn {i}"), &mut ApproveAll, &mut no_events());
    }
    // Each turn is four messages (user, call, result, answer). Trimming happens when a turn
    // starts, cutting at a user message, so the previous turn stays whole beside the new one.
    assert_eq!(a.messages.len(), 8);
    assert!(matches!(&a.messages[0].content[0], ContentBlock::Text { text } if text == "turn 2"), "{:?}", a.messages[0]);
    assert!(matches!(&a.messages[4].content[0], ContentBlock::Text { text } if text == "turn 3"));
}

#[test]
fn system_text_follows_the_limits() {
    let mock = Arc::new(MockClient::new());
    let host = Headless::with_document();
    let mut a = assistant(&mock);
    let full = a.system_text(&host);
    assert!(full.contains("# Command catalog") && full.contains("# The open presentation"));
    let mut a = assistant(&mock).with_limits(Limits { catalog_bytes: 0, ..quick_limits() });
    let bare = a.system_text(&host);
    assert!(!bare.contains("# Command catalog") && bare.contains("# The open presentation"));
    assert!(bare.len() < full.len() / 4, "{} vs {}", bare.len(), full.len());
}

// ---------- safety rails ----------

use crate::{Class, Decision, Mode, Plan, Policy, Verdict};

/// Records every plan it is asked about and answers with a fixed verdict.
struct Recorder {
    plans: Vec<Plan>,
    verdict: Verdict,
}

impl Recorder {
    fn new(verdict: Verdict) -> Self {
        Recorder { plans: vec![], verdict }
    }
}

impl crate::Confirm for Recorder {
    fn confirm(&mut self, plan: &Plan) -> Verdict {
        self.plans.push(plan.clone());
        self.verdict.clone()
    }
}

fn slides(h: &Headless) -> usize {
    h.session.active().unwrap().doc.slides.len()
}

#[test]
fn destructive_lists_name_real_commands_and_tools() {
    let c = Catalog::new();
    for id in crate::policy::DESTRUCTIVE_COMMANDS {
        assert!(c.get(id).is_some(), "{id} is not a command");
    }
    let t = ToolTable::all();
    for name in crate::policy::DESTRUCTIVE_TOOLS {
        assert!(t.get(name).is_some(), "{name} is not a tool");
    }
    for name in t.names() {
        if let Some(id) = crate::policy::tool_command(name) {
            assert!(c.get(id).is_some(), "{name} maps to unknown {id}");
        }
    }
    assert_eq!(Mode::parse("Auto"), Some(Mode::Autopilot));
    assert_eq!(Mode::parse("answer"), Some(Mode::AnswerOnly));
    assert_eq!(Mode::parse("nope"), None);
}

#[test]
fn policy_classifies_calls_and_decides_per_mode() {
    let table = ToolTable::for_backend(false);
    let catalog = Catalog::new();
    let p = Policy::new(Mode::Confirm);
    let read = p.classify(&table, &catalog, "1", "inspect_slide", &json!({}));
    let query = p.classify(&table, &catalog, "2", "run_command", &json!({"command": "slide.inspect", "params": {"index": 0}}));
    let change = p.classify(&table, &catalog, "3", "add_slide", &json!({"layout": "blank"}));
    let cmd = p.classify(&table, &catalog, "4", "run_command", &json!({"command": "slide.new", "params": {"layout": "blank"}}));
    let del = p.classify(&table, &catalog, "5", "run_command", &json!({"command": "slide.delete", "params": {"index": 1}}));
    let batch = p.classify(
        &table,
        &catalog,
        "6",
        "batch",
        &json!({"commands": [{"command": "slide.inspect"}, {"command": "section.remove", "params": {"index": 0, "slides": true}}]}),
    );
    let fresh = p.classify(&table, &catalog, "7", "new_presentation", &json!({}));
    assert_eq!(read.class, Class::ReadOnly);
    assert_eq!(query.class, Class::ReadOnly);
    assert_eq!(change.class, Class::Change);
    assert_eq!(change.commands[0].0, "slide.new");
    assert_eq!(cmd.class, Class::Change);
    assert_eq!(cmd.commands[0].1, "New Slide");
    assert_eq!(del.class, Class::Destructive);
    assert_eq!(batch.class, Class::Destructive, "a batch is as dangerous as its worst command");
    assert_eq!(batch.commands.len(), 2);
    assert_eq!(fresh.class, Class::Destructive);

    let decide = |mode: Mode, class: Class| Policy::new(mode).decide(class);
    assert_eq!(decide(Mode::AnswerOnly, Class::ReadOnly), Decision::Allow);
    assert!(matches!(decide(Mode::AnswerOnly, Class::Change), Decision::Refuse(_)));
    assert!(matches!(decide(Mode::AnswerOnly, Class::Destructive), Decision::Refuse(_)));
    assert_eq!(decide(Mode::Confirm, Class::ReadOnly), Decision::Allow);
    assert_eq!(decide(Mode::Confirm, Class::Change), Decision::Confirm);
    assert_eq!(decide(Mode::Confirm, Class::Destructive), Decision::Confirm);
    assert_eq!(decide(Mode::Autopilot, Class::Change), Decision::Allow);
    assert_eq!(decide(Mode::Autopilot, Class::Destructive), Decision::Confirm);

    // The plan reads well.
    let plan = Plan { calls: vec![change.clone(), del.clone()] };
    assert!(plan.destructive());
    let text = plan.describe();
    assert!(text.contains("add_slide layout=\"blank\""), "{text}");
    assert!(text.contains("run_command (destructive)\n  slide.delete — Delete Slide {\"index\":1}"), "{text}");

    // Answer-only offers the read-only tools, run_command and list_commands, nothing else.
    let offered: Vec<&str> = Policy::new(Mode::AnswerOnly).tools(&table).iter().map(|t| t.name.as_str()).collect();
    assert!(
        offered.contains(&"inspect_slide")
            && offered.contains(&"run_command")
            && offered.contains(&"list_commands")
            && offered.contains(&"render_slide")
    );
    assert!(!offered.contains(&"add_slide") && !offered.contains(&"set_text") && !offered.contains(&"batch"), "{offered:?}");
    assert_eq!(Policy::new(Mode::Confirm).tools(&table).len(), table.defs().len());
}

#[test]
fn answer_only_mode_reads_but_never_changes() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![
        tool_use("a", "inspect_document", json!({})),
        tool_use("b", "run_command", json!({"command": "slide.inspect", "params": {}})),
    ]));
    mock.push(calls(vec![tool_use("c", "run_command", json!({"command": "slide.new", "params": {"layout": "blank"}}))]));
    mock.push(answer("I would add a blank slide after slide 1 with slide.new."));
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_mode(Mode::AnswerOnly);
    let mut rec = Recorder::new(Verdict::Approve);
    let report = a.ask(&mut host, "what would you do to add a slide?", &mut rec, &mut no_events());
    assert_eq!(report.answer, "I would add a blank slide after slide 1 with slide.new.");
    assert_eq!(report.refused, 1);
    assert!(rec.plans.is_empty(), "answer-only never asks");
    assert!(report.tool_calls[0].ok && report.tool_calls[1].ok && !report.tool_calls[2].ok);
    assert!(report.tool_calls[2].summary.contains("answer-only mode"), "{}", report.tool_calls[2].summary);
    assert_eq!(slides(&host), 1);
    assert!(report.commands.is_empty(), "{:?}", report.commands);
    assert!(report.undo_label.is_none() && report.touched_slides.is_empty());
    let reqs = mock.requests();
    assert!(reqs[0].system.as_deref().unwrap().contains("Mode: answer only"));
    assert!(reqs[0].tools.iter().all(|t| t.name != "add_slide"));
}

#[test]
fn confirm_mode_shows_the_plan_and_applies_only_on_approval() {
    let script = |mock: &MockClient| {
        mock.push(calls(vec![
            tool_use("r", "inspect_slide", json!({})),
            tool_use("a", "add_slide", json!({"layout": "titleOnly", "title": "Agenda"})),
            tool_use("b", "run_command", json!({"command": "slide.notes", "params": {"text": "Walk through the agenda"}})),
        ]));
        mock.push(answer("Done."));
    };

    // Approved: the plan names both changing calls with their commands; the read-only one ran
    // without asking.
    let mock = Arc::new(MockClient::new());
    script(&mock);
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_mode(Mode::Confirm);
    let mut rec = Recorder::new(Verdict::Approve);
    let report = a.ask(&mut host, "add an agenda slide with notes", &mut rec, &mut no_events());
    assert_eq!(rec.plans.len(), 1);
    let plan = &rec.plans[0];
    assert_eq!(plan.calls.len(), 2, "{plan:?}");
    assert_eq!(plan.calls[0].name, "add_slide");
    assert_eq!(plan.calls[0].commands[0].0, "slide.new");
    assert_eq!(plan.calls[1].commands[0], ("slide.notes".to_string(), "Notes".to_string(), json!({"text": "Walk through the agenda"})));
    assert!(!plan.destructive());
    assert_eq!(report.plans, 1);
    assert_eq!(report.declined, 0);
    assert_eq!(report.commands, vec!["slide.new", "slide.notes"]);
    assert_eq!(slides(&host), 2);
    assert_eq!(report.touched_slides, vec![1]);
    assert_eq!(host.session.active().unwrap().history.undo.len(), 1);
    assert!(mock.requests()[0].system.as_deref().unwrap().contains("Mode: confirm"));

    // Declined with a reason: nothing ran, the model heard the reason and answered.
    let mock = Arc::new(MockClient::new());
    script(&mock);
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_mode(Mode::Confirm);
    let mut rec = Recorder::new(Verdict::Decline(Some("not before the title is fixed".into())));
    let report = a.ask(&mut host, "add an agenda slide with notes", &mut rec, &mut no_events());
    assert_eq!(report.answer, "Done.");
    assert_eq!((report.plans, report.declined), (1, 2));
    assert!(report.tool_calls[0].ok && !report.tool_calls[1].ok && !report.tool_calls[2].ok);
    assert!(report.tool_calls[1].summary.contains("declined this change: not before the title is fixed"), "{}", report.tool_calls[1].summary);
    assert_eq!(slides(&host), 1);
    assert!(report.commands.is_empty());
    assert!(report.undo_label.is_none());
    let reqs = mock.requests();
    assert!(matches!(&reqs[1].messages[2].content[1], ContentBlock::ToolResult { is_error: true, content, .. } if content.contains("declined")));

    // Cancelled from the dialog: the turn stops, nothing ran, the conversation stays valid.
    let mock = Arc::new(MockClient::new());
    script(&mock);
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_mode(Mode::Confirm);
    let mut rec = Recorder::new(Verdict::Cancel);
    let report = a.ask(&mut host, "add an agenda slide with notes", &mut rec, &mut no_events());
    assert_eq!(report.stopped, Some(Stop::Cancelled));
    assert_eq!(slides(&host), 1);
    assert!(matches!(a.messages.last().unwrap().content[0], ContentBlock::ToolResult { .. }));
}

#[test]
fn autopilot_applies_changes_and_confirms_only_destructive_ones() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("a", "add_slide", json!({"layout": "blank"})), tool_use("b", "add_slide", json!({"layout": "blank"}))]));
    mock.push(calls(vec![tool_use("d", "run_command", json!({"command": "slide.delete", "params": {"index": 1}}))]));
    mock.push(calls(vec![tool_use(
        "e",
        "batch",
        json!({"commands": [{"command": "slide.go", "params": {"index": 0}}, {"command": "slide.delete", "params": {"index": 1}}]}),
    )]));
    mock.push(calls(vec![tool_use("f", "new_presentation", json!({}))]));
    mock.push(answer("Added two slides; the deletions and the new deck were declined."));
    let mut host = Headless::with_document();
    let mut a = assistant(&mock).with_mode(Mode::Autopilot);
    let mut rec = Recorder::new(Verdict::Decline(None));
    let report = a.ask(&mut host, "add two slides then delete one and start over", &mut rec, &mut no_events());
    // The two additions ran without a plan; each destructive round asked and was declined.
    assert_eq!(rec.plans.len(), 3, "{:?}", rec.plans.iter().map(Plan::describe).collect::<Vec<_>>());
    assert!(rec.plans.iter().all(Plan::destructive));
    assert!(rec.plans[1].describe().contains("slide.go — Go to Slide"), "the whole batch is shown: {}", rec.plans[1].describe());
    assert_eq!((report.plans, report.declined), (3, 3));
    assert_eq!(report.commands, vec!["slide.new", "slide.new"]);
    assert_eq!(slides(&host), 3);
    assert_eq!(report.touched_slides, vec![1, 2]);
    assert_eq!(report.removed_slides, 0);
    assert_eq!(host.session.active().unwrap().history.undo.len(), 1);
    assert!(mock.requests()[0].system.as_deref().unwrap().contains("Mode: autopilot"));

    // Approved: the deletion goes through and is counted as removed.
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("d", "run_command", json!({"command": "slide.delete", "params": {"index": 1}}))]));
    mock.push(answer("Deleted slide 2."));
    let mut a = assistant(&mock).with_mode(Mode::Autopilot);
    let mut rec = Recorder::new(Verdict::Approve);
    let report = a.ask(&mut host, "delete slide 2", &mut rec, &mut no_events());
    assert_eq!(rec.plans.len(), 1);
    assert_eq!(report.commands, vec!["slide.delete"]);
    assert_eq!(slides(&host), 2);
    assert_eq!(report.removed_slides, 1);
    assert!(report.touched_slides.is_empty(), "{:?}", report.touched_slides);
}

#[test]
fn touched_slides_follow_edits() {
    let mock = Arc::new(MockClient::new());
    let title = Headless::with_document().session.doc().unwrap().current_slide().unwrap().shapes[0].id;
    mock.push(calls(vec![
        tool_use("a", "set_text", json!({"id": title, "text": "New title"})),
        tool_use("b", "add_slide", json!({"layout": "blank"})),
    ]));
    mock.push(answer("ok"));
    let mut host = Headless::with_document();
    let mut a = assistant(&mock);
    let report = a.ask(&mut host, "retitle and add", &mut ApproveAll, &mut no_events());
    assert_eq!(report.touched_slides, vec![0, 1]);
    assert_eq!(report.removed_slides, 0);
}

// ---------- a deck from a document (step 4.1) ----------

use crate::outline::{Outline, apply};

const DOC: &str = "# Northern Lights\nA short tour of the aurora\n\n## Science\n\n### What causes it\n- Solar wind meets the magnetosphere\n- Charged particles collide with gases\n  - Oxygen glows green and red\n  - Nitrogen adds blue\nNotes: keep this one short\n\n### Best months\n| Month | Clear nights |\n|---|---|\n| Dec | 12 |\n| Jan | 14 |\n\n## In the field\n\n### Camera settings\n1. Wide lens\n2. ISO 1600\n3. 10 seconds\n\n### Thank you\n";

#[test]
fn markdown_becomes_an_outline() {
    let o = Outline::from_markdown(DOC);
    assert_eq!(o.title.as_deref(), Some("Northern Lights"));
    assert_eq!(o.subtitle.as_deref(), Some("A short tour of the aurora"));
    assert_eq!(o.sections.len(), 2);
    assert_eq!(o.sections[0].name.as_deref(), Some("Science"));
    let causes = &o.sections[0].slides[0];
    assert_eq!(causes.title, "What causes it");
    assert_eq!(
        causes.body,
        vec!["Solar wind meets the magnetosphere", "Charged particles collide with gases", "\tOxygen glows green and red", "\tNitrogen adds blue"]
    );
    assert_eq!(causes.notes.as_deref(), Some("keep this one short"));
    let months = &o.sections[0].slides[1];
    assert_eq!(months.table.as_ref().unwrap(), &vec![vec!["Month", "Clear nights"], vec!["Dec", "12"], vec!["Jan", "14"]]);
    assert!(months.body.is_empty());
    let camera = &o.sections[1].slides[0];
    assert_eq!(camera.body, vec!["Wide lens", "ISO 1600", "10 seconds"]);
    assert_eq!(o.sections[1].slides[1].title, "Thank you");
    assert_eq!(o.slide_count(), 1 + 2 + 4);

    // `##` only: slides, no sections.
    let flat = Outline::from_markdown("# Deck\n\n## One\n- a\n\n## Two\n- b\n");
    assert_eq!(flat.sections.len(), 1);
    assert!(flat.sections[0].name.is_none());
    assert_eq!(flat.sections[0].slides.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), vec!["One", "Two"]);
    // Text with no headings: one slide.
    let bare = Outline::from_markdown("Just a paragraph\n- and a bullet\n");
    assert_eq!(bare.slide_count(), 1);
    assert_eq!(bare.sections[0].slides[0].title, "Overview");
    assert!(Outline::from_markdown("").slide_count() == 0);

    let j = Outline::from_json(&json!({"title": "T", "slides": [{"title": "A", "body": "x\ny", "section": "S"}, {"title": "B", "bullets": ["p", "q"], "notes": "n", "layout": "twoContent", "table": [["h"], ["1"]]}]})).unwrap();
    assert_eq!(j.sections[0].name.as_deref(), Some("S"));
    assert_eq!(j.sections[0].slides[1].layout, "twoContent");
    assert_eq!(j.sections[0].slides[1].body, vec!["p", "q"]);
    assert!(Outline::from_json(&json!({"slides": [{"body": "no title"}]})).is_err());
}

#[test]
fn apply_builds_the_deck_with_commands() {
    let mut h = Headless::with_document();
    let o = Outline::from_markdown(DOC);
    let a = apply(&mut h, &o, Some("harbor"), false, None).unwrap();
    assert_eq!(a.slides.len(), 7);
    assert_eq!((a.sections, a.tables, a.notes), (2, 1, 1));
    let st = h.session.active().unwrap();
    assert_eq!(st.doc.slides.len(), 8, "the original slide plus seven");
    assert_eq!(st.doc.slides[1].title(), "Northern Lights");
    assert_eq!(st.doc.slides[2].title(), "Science");
    assert_eq!(st.doc.slides[3].title(), "What causes it");
    assert_eq!(st.doc.slides[3].notes_text(), "keep this one short");
    assert!(st.doc.slides[4].shapes.iter().any(|s| matches!(s.kind, maluos_slides_engine::model::ShapeKind::Table(_))));
    // The engine puts the slides before the first section into a default section.
    assert_eq!(st.doc.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Default Section", "Science", "In the field"]);
    assert_eq!(st.doc.masters[0].theme.name, "Harbor");
    assert!(st.doc.slides[3].shapes.iter().any(|s| s.text.as_ref().is_some_and(|t| t.text().contains("Oxygen glows"))));
    assert!(apply(&mut h, &o, Some("nope"), false, None).unwrap_err().contains("unknown theme"));

    // Replace: a fresh presentation with only the outline.
    let mut h = Headless::with_document();
    let a = apply(&mut h, &o, None, true, None).unwrap();
    assert_eq!(a.slides, (0..7).collect::<Vec<_>>());
    assert_eq!(h.session.active().unwrap().doc.slides.len(), 7);
}

#[test]
fn build_deck_is_a_tool_everywhere() {
    // Through run_tool with the policy: replace is destructive, plain is a change.
    let mut h = Headless::with_document();
    let table = ToolTable::for_backend(false);
    let catalog = Catalog::from_session(&h.session);
    let out = run_tool(&mut h, &table, &catalog, "t1", "build_deck", &json!({"markdown": "## A\n- x\n## B\n- y"}));
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("\"count\": 2"), "{}", out.text);
    assert_eq!(h.session.active().unwrap().doc.slides.len(), 3);
    let out = run_tool(&mut h, &table, &catalog, "t2", "build_deck", &json!({"markdown": "", "bogus": 1}));
    assert!(out.is_error && out.text.contains("unknown argument"), "{}", out.text);
    let out = run_tool(&mut h, &table, &catalog, "t3", "build_deck", &json!({}));
    assert!(out.is_error, "{}", out.text);
    let p = Policy::new(Mode::Autopilot);
    assert_eq!(p.classify(&table, &catalog, "x", "build_deck", &json!({"markdown": "## A"})).class, Class::Change);
    assert_eq!(p.classify(&table, &catalog, "x", "build_deck", &json!({"markdown": "## A", "replace": true})).class, Class::Destructive);

    // Through the MCP wrapper: listed and callable; upstream methods untouched.
    let mut s = crate::mcp_ext::Server::new(Box::new(Headless::with_document()));
    let init = s.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "maluos-slides");
    let list = s.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"build_deck") && names.contains(&"add_slide"));
    let call = s.handle(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "build_deck", "arguments": {"slides": [{"title": "Only"}]}}})).unwrap();
    assert_eq!(call["id"], 3);
    assert_eq!(call["result"]["isError"], false, "{call}");
    let doc = s.handle(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "inspect_document", "arguments": {}}})).unwrap();
    assert!(doc["result"]["content"][0]["text"].as_str().unwrap().contains("Only"));
    assert!(s.handle_line("not json").is_some());
}

#[test]
fn the_model_builds_a_deck_from_a_document_in_one_call() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use(
        "b",
        "build_deck",
        json!({"markdown": "# Aurora\nA tour\n## Science\n### Why\n- wind\n## Field\n### How\n- lens\n### Thanks", "theme": "Nocturne"}),
    )]));
    mock.push(answer("Built a 6-slide deck in two sections."));
    let mut host = Headless::with_document();
    let mut a = assistant(&mock);
    let report = a.ask(&mut host, "make a deck from this outline", &mut ApproveAll, &mut no_events());
    assert_eq!(report.answer, "Built a 6-slide deck in two sections.");
    assert_eq!(report.plans, 0, "no replace: autopilot applies it");
    assert_eq!(host.session.active().unwrap().doc.slides.len(), 7);
    assert_eq!(report.touched_slides, (1..7).collect::<Vec<_>>());
    assert_eq!(host.session.active().unwrap().history.undo.len(), 1, "one undo step for the whole deck");
    assert!(report.commands.iter().filter(|c| *c == "slide.new").count() == 6, "{:?}", report.commands);
}
