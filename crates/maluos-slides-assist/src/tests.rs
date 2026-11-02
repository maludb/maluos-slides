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
    assert_eq!(all.defs().len(), mcp.len());
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
