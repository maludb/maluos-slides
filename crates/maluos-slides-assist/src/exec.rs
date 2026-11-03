//! One tool call, executed through an MCP backend and shaped for the model.

use maluos_llm::ContentBlock;
use maluos_slides_mcp::{Backend, ToolResult};
use serde_json::{Value, json};

use crate::catalog::Catalog;
use crate::tools::ToolTable;

/// Most bytes of text a tool result carries back to the model; longer results are cut with a
/// marker. `inspect_document` on a big deck can be hundreds of KiB.
pub const MAX_RESULT_BYTES: usize = 64 * 1024;

/// What one tool call produced.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub tool_use_id: String,
    pub name: String,
    /// The text the model gets (joined text blocks, cut at [`MAX_RESULT_BYTES`]).
    pub text: String,
    pub is_error: bool,
    /// Images the tool returned (`render_slide`, `screenshot`): base64 data and media type. The
    /// model sees a placeholder line in `text` until image inputs exist (step 4.2).
    pub images: Vec<(String, String)>,
    /// The raw MCP result, for a pane that wants to show it as the server would.
    pub raw: ToolResult,
}

impl ToolOutcome {
    /// The block to append to the next request's user turn, text only.
    pub fn block(&self) -> ContentBlock {
        self.block_with_images(false)
    }

    /// The block with the tool's images attached (a rendered slide the model asked for), so a
    /// model that sees pictures can check its work.
    pub fn block_with_images(&self, with_images: bool) -> ContentBlock {
        let images = if with_images {
            self.images.iter().map(|(data, mime)| maluos_llm::Image { media_type: mime.clone(), data: data.clone() }).collect()
        } else {
            vec![]
        };
        let content =
            if with_images && !self.images.is_empty() { self.text.replace("bytes base64]", "bytes base64, attached]") } else { self.text.clone() };
        ContentBlock::ToolResult { tool_use_id: self.tool_use_id.clone(), content, is_error: self.is_error, images }
    }

    fn from_result(tool_use_id: &str, name: &str, raw: ToolResult) -> Self {
        let mut text = String::new();
        let mut images = Vec::new();
        for c in &raw.content {
            match c.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(c.get("text").and_then(Value::as_str).unwrap_or(""));
                }
                Some("image") => {
                    let data = c.get("data").and_then(Value::as_str).unwrap_or("").to_string();
                    let mime = c.get("mimeType").and_then(Value::as_str).unwrap_or("image/png").to_string();
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("[image {mime}, {} bytes base64]", data.len()));
                    images.push((data, mime));
                }
                _ => {}
            }
        }
        let text = truncate(text, MAX_RESULT_BYTES);
        ToolOutcome { tool_use_id: tool_use_id.into(), name: name.into(), text, is_error: raw.is_error, images, raw }
    }

    /// An error outcome made without a backend (a limit, a cancel).
    pub fn error_for(tool_use_id: &str, name: &str, message: impl Into<String>) -> Self {
        Self::error(tool_use_id, name, message)
    }

    #[cfg(test)]
    pub(crate) fn from_result_for_test(tool_use_id: &str, name: &str, raw: ToolResult) -> Self {
        Self::from_result(tool_use_id, name, raw)
    }

    fn error(tool_use_id: &str, name: &str, message: impl Into<String>) -> Self {
        Self::from_result(tool_use_id, name, ToolResult::error(message))
    }
}

/// Runs `name` with `input` on `backend`. Before anything executes: the tool must be in
/// `table`; an app-only tool needs a backend with a UI; `run_command` and `batch` must name
/// commands the catalog knows, with documented parameter keys (every command of a batch is
/// checked first, so a bad second command never leaves the first one applied).
pub fn run_tool(backend: &mut dyn Backend, table: &ToolTable, catalog: &Catalog, tool_use_id: &str, name: &str, input: &Value) -> ToolOutcome {
    if table.is_app_only(name) && !backend.has_ui() {
        return ToolOutcome::error(tool_use_id, name, format!("`{name}` needs the desktop app; this session is {}", backend.describe()));
    }
    if table.get(name).is_none() {
        let names: Vec<&str> = table.names().collect();
        return ToolOutcome::error(tool_use_id, name, format!("unknown tool `{name}` (available: {})", names.join(", ")));
    }
    let input = if input.is_object() { input.clone() } else { json!({}) };
    if let Err(e) = check_commands(catalog, name, &input) {
        return ToolOutcome::error(tool_use_id, name, e);
    }
    let raw = call_tool(backend, name, &input);
    ToolOutcome::from_result(tool_use_id, name, raw)
}

/// One tool call on a backend: the fork's tools here, every other upstream's `call_tool`. The
/// HTTP API and the MCP wrapper use this, so all three callers run the same code.
pub fn call_tool(backend: &mut dyn Backend, name: &str, args: &Value) -> ToolResult {
    if name == crate::tools::BUILD_DECK {
        return build_deck(backend, args);
    }
    if name == crate::tools::DECK_TIMING {
        return deck_timing(backend);
    }
    if name == crate::tools::OFFICE_PICTURE {
        return office_picture(backend, args);
    }
    maluos_slides_mcp::call_tool(backend, name, args)
}

/// The timing of the backend's presentation, read through `document.inspect` and `slide.inspect`
/// when there is no session at hand (a remote app), from the model itself otherwise.
fn deck_timing(backend: &mut dyn Backend) -> ToolResult {
    let doc = match backend.call("document.inspect", json!({})) {
        Ok(v) => v,
        Err(e) => return ToolResult::error(format!("deck_timing: {e}")),
    };
    let Some(slides) = doc.get("slides").and_then(Value::as_array) else { return ToolResult::error("deck_timing: no presentation open") };
    let mut out = Vec::with_capacity(slides.len());
    let mut total = 0.0;
    for (i, s) in slides.iter().enumerate() {
        let notes = s.get("notes").and_then(Value::as_str).unwrap_or("");
        let slide_words = match backend.call("engine.execute", json!({"command": "slide.inspect", "params": {"index": i}})) {
            Ok(v) => v.get("shapes").and_then(Value::as_array).map(|shapes| shapes.iter().map(shape_words_json).sum()).unwrap_or(0),
            Err(_) => 0,
        };
        let notes_words = crate::timing::words(notes);
        let seconds = crate::timing::estimate(slide_words, notes_words);
        let hidden = s.get("hidden").and_then(Value::as_bool).unwrap_or(false);
        if !hidden {
            total += seconds;
        }
        out.push(json!({"index": i, "title": s.get("title").and_then(Value::as_str).unwrap_or(""), "slideWords": slide_words, "notesWords": notes_words, "seconds": seconds.round(), "hidden": hidden}));
    }
    let longest = out
        .iter()
        .filter(|s| s["hidden"] == false)
        .max_by(|a, b| a["seconds"].as_f64().unwrap_or(0.0).total_cmp(&b["seconds"].as_f64().unwrap_or(0.0)))
        .cloned();
    ToolResult::json(&json!({"totalSeconds": total.round(), "total": crate::timing::clock(total), "slides": out, "longest": longest}))
}

fn shape_words_json(s: &Value) -> usize {
    let mut n = s.get("text").and_then(Value::as_str).map(crate::timing::words).unwrap_or(0);
    if let Some(cells) = s.pointer("/table/cells").and_then(Value::as_array) {
        n += cells.iter().flat_map(|r| r.as_array().into_iter().flatten()).filter_map(Value::as_str).map(crate::timing::words).sum::<usize>();
    }
    if let Some(children) = s.get("children").and_then(Value::as_array) {
        n += children.iter().map(shape_words_json).sum::<usize>();
    }
    n
}

fn build_deck(backend: &mut dyn Backend, args: &Value) -> ToolResult {
    let allowed = ["markdown", "slides", "title", "subtitle", "theme", "replace", "after"];
    if let Some(o) = args.as_object()
        && let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str()))
    {
        return ToolResult::error(format!("build_deck: unknown argument {k:?} (expected: {})", allowed.join(", ")));
    }
    let outline = match args.get("markdown").and_then(Value::as_str) {
        Some(md) => {
            let o = crate::outline::Outline::from_markdown(md);
            if o.slide_count() == 0 {
                return ToolResult::error("build_deck: the markdown has no headings or text to make slides from");
            }
            o
        }
        None => match crate::outline::Outline::from_json(args) {
            Ok(o) => o,
            Err(e) => return ToolResult::error(format!("build_deck: {e}")),
        },
    };
    let theme = args.get("theme").and_then(Value::as_str);
    let replace = args.get("replace").and_then(Value::as_bool).unwrap_or(false);
    let after = args.get("after").and_then(Value::as_u64).map(|v| v as usize);
    match crate::outline::apply(backend, &outline, theme, replace, after) {
        Ok(a) => ToolResult::json(
            &json!({"slides": a.slides, "count": a.slides.len(), "sections": a.sections, "tables": a.tables, "notes": a.notes, "replaced": replace}),
        ),
        Err(e) => ToolResult::error(format!("build_deck: {e}")),
    }
}

/// A picture from the office's image agent, inserted with `insert.picture` (step 4.5). The
/// office is found from the desk's enrolment; see [`crate::office`].
#[cfg(all(feature = "native", unix))]
fn office_picture(backend: &mut dyn Backend, args: &Value) -> ToolResult {
    let allowed = ["prompt", "slide", "rect"];
    if let Some(o) = args.as_object()
        && let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str()))
    {
        return ToolResult::error(format!("office_picture: unknown argument {k:?} (expected: {})", allowed.join(", ")));
    }
    let Some(prompt) = args.get("prompt").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()) else {
        return ToolResult::error("office_picture: `prompt` is required");
    };
    let office = match crate::office::Office::discover() {
        Ok(o) => o,
        Err(e) => return ToolResult::error(format!("office_picture: {e}")),
    };
    let agent = match office.image_agent() {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("office_picture: {e}")),
    };
    let picture = match office.picture(&agent, prompt) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("office_picture: {agent}: {e}", agent = agent.id)),
    };
    if let Some(slide) = args.get("slide").and_then(Value::as_u64)
        && let Err(e) = backend.call("engine.execute", json!({"command": "slide.go", "params": {"index": slide}}))
    {
        return ToolResult::error(format!("office_picture: slide {slide}: {e}"));
    }
    let name = if picture.name.contains('.') {
        picture.name.clone()
    } else {
        format!("{}.{}", picture.name, picture.media_type.rsplit('/').next().unwrap_or("png"))
    };
    let mut params = json!({"data": maluos_slides_engine::cmd::base64_encode(&picture.bytes), "name": name});
    if let Some(rect) = args.get("rect") {
        params["rect"] = rect.clone();
    }
    match backend.call("engine.execute", json!({"command": "insert.picture", "params": params})) {
        Ok(inserted) => ToolResult::json(&json!({
            "agent": agent.id, "office": office.name, "task": picture.task_id, "name": name, "mediaType": picture.media_type,
            "bytes": picture.bytes.len(), "agentSaid": picture.text, "inserted": inserted,
        })),
        Err(e) => ToolResult::error(format!("office_picture: insert.picture: {e}")),
    }
}

#[cfg(not(all(feature = "native", unix)))]
fn office_picture(_backend: &mut dyn Backend, _args: &Value) -> ToolResult {
    ToolResult::error("office_picture: the office is reachable only in the native Linux build")
}

fn check_commands(catalog: &Catalog, name: &str, input: &Value) -> Result<(), String> {
    match name {
        "run_command" => {
            let id = input.get("command").and_then(Value::as_str).ok_or("run_command: missing `command`")?;
            catalog.validate(id, input.get("params").unwrap_or(&Value::Null))
        }
        "batch" => {
            let commands = input.get("commands").and_then(Value::as_array).ok_or("batch: missing `commands`")?;
            for (i, c) in commands.iter().enumerate() {
                let id = c.get("command").and_then(Value::as_str).ok_or_else(|| format!("batch: command {i} has no `command`"))?;
                catalog.validate(id, c.get("params").unwrap_or(&Value::Null)).map_err(|e| format!("batch: command {i}: {e}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn truncate(mut text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let total = text.len();
    text.truncate(cut);
    text.push_str(&format!("\n… [cut: {total} bytes, {max} shown]"));
    text
}
