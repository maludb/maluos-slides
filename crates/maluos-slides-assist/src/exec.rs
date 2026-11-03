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
    /// The block to append to the next request's user turn.
    pub fn block(&self) -> ContentBlock {
        ContentBlock::ToolResult { tool_use_id: self.tool_use_id.clone(), content: self.text.clone(), is_error: self.is_error }
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
    maluos_slides_mcp::call_tool(backend, name, args)
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
