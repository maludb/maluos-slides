//! The tool table: the MCP server's definitions, offered to a model.

use std::collections::BTreeSet;

use maluos_llm::ToolDef;
use serde_json::Value;

/// What the MCP server writes into the description of a tool that needs the desktop app.
pub const APP_ONLY_MARKER: &str = "Desktop app only";

/// The fork's own tool: a whole deck from a document (step 4.1).
pub const BUILD_DECK: &str = "build_deck";
/// The fork's tool: how long the deck takes to present (step 4.4).
pub const DECK_TIMING: &str = "deck_timing";

/// The fork's tools, in the MCP definition shape, appended to upstream's table everywhere the
/// table is served (the assistant, the HTTP API, the MCP server through `mcp_ext`).
pub fn extra_definitions() -> Vec<Value> {
    vec![
        serde_json::json!({
            "name": BUILD_DECK,
            "title": "Build a deck",
            "description": "Build many slides at once from an outline: Markdown (# deck title, ## section, ### slide, - bullets with two spaces per level, Notes: lines, | pipe tables |) or a `slides` list. Slides go after the current one; `replace: true` starts a new presentation (asks the user). Prefer this over many add_slide calls when making a whole deck.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "markdown": {"type": "string", "description": "The outline as Markdown"},
                    "slides": {"type": "array", "description": "Instead of markdown: slides in order", "items": {"type": "object", "properties": {
                        "title": {"type": "string"},
                        "body": {"description": "Body text, one paragraph per line, leading tabs indent; or a list of lines", "oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]},
                        "notes": {"type": "string", "description": "Speaker notes"},
                        "layout": {"type": "string", "description": "title, titleAndContent, sectionHeader, twoContent, comparison, titleOnly, blank, contentWithCaption, pictureWithCaption (default from the content)"},
                        "section": {"type": "string", "description": "Starts a section with this name before the slide"},
                        "table": {"type": "array", "description": "Rows of cells; the first row is the header", "items": {"type": "array", "items": {"type": "string"}}}
                    }, "required": ["title"]}},
                    "title": {"type": "string", "description": "Deck title (with `slides`): makes the title slide"},
                    "subtitle": {"type": "string", "description": "Subtitle of the title slide"},
                    "theme": {"type": "string", "enum": ["MaluOS Slides", "Harbor", "Ember", "Meadow", "Nocturne", "Paper", "Coral Reef", "Slate"], "description": "Theme for the deck"},
                    "replace": {"type": "boolean", "description": "Start a new presentation instead of adding to the open one (default false)"},
                    "after": {"type": "integer", "minimum": 0, "description": "Insert after this slide index (default: the last slide)"}
                }
            },
            "annotations": {"title": "Build a deck", "readOnlyHint": false, "openWorldHint": false}
        }),
        serde_json::json!({
            "name": DECK_TIMING,
            "title": "Deck timing",
            "description": "How long the presentation takes to present: seconds per slide from the words in its notes (130 wpm) and on the slide (60 wpm) plus 8 s each, the total for the shown slides, and the longest slide. Use it for timing questions and before trimming a deck to a time limit.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {"title": "Deck timing", "readOnlyHint": true, "openWorldHint": false}
        }),
    ]
}

/// Whether `name` is one of the fork's tools.
pub fn is_extra(name: &str) -> bool {
    name == BUILD_DECK || name == DECK_TIMING
}

/// The tools a model may call. Built from [`maluos_slides_mcp::tool_definitions`], so the MCP
/// server and the assistant never disagree on a name, a schema or a description.
#[derive(Debug, Clone)]
pub struct ToolTable {
    tools: Vec<ToolDef>,
    read_only: BTreeSet<String>,
    app_only: BTreeSet<String>,
}

impl ToolTable {
    /// Every tool, including the ones that need a window.
    pub fn all() -> Self {
        Self::build(true)
    }

    /// The tools a backend can serve: without a UI the app-only tools (`screenshot`,
    /// `call_app`) are left out, so the model is never offered a call that cannot succeed.
    pub fn for_backend(has_ui: bool) -> Self {
        Self::build(has_ui)
    }

    fn build(with_app_only: bool) -> Self {
        let mut tools = Vec::new();
        let mut read_only = BTreeSet::new();
        let mut app_only = BTreeSet::new();
        for def in maluos_slides_mcp::tool_definitions().into_iter().chain(extra_definitions()) {
            let Some(tool) = to_llm(&def) else { continue };
            let is_app_only = tool.description.contains(APP_ONLY_MARKER);
            if is_app_only {
                app_only.insert(tool.name.clone());
                if !with_app_only {
                    continue;
                }
            }
            if def.pointer("/annotations/readOnlyHint").and_then(Value::as_bool).unwrap_or(false) {
                read_only.insert(tool.name.clone());
            }
            tools.push(tool);
        }
        ToolTable { tools, read_only, app_only }
    }

    /// The definitions, in the MCP server's order, ready for `Request::with_tools`.
    pub fn defs(&self) -> &[ToolDef] {
        &self.tools
    }

    pub fn get(&self, name: &str) -> Option<&ToolDef> {
        self.tools.iter().find(|t| t.name == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.iter().map(|t| t.name.as_str())
    }

    /// The MCP server marks it `readOnlyHint`: it changes nothing.
    pub fn is_read_only(&self, name: &str) -> bool {
        self.read_only.contains(name)
    }

    /// It needs the desktop app (a backend with a UI).
    pub fn is_app_only(&self, name: &str) -> bool {
        self.app_only.contains(name)
    }

    /// The raw MCP definitions (name, title, description, `inputSchema`, annotations), upstream's
    /// and the fork's, for a network API that wants to publish the same table.
    pub fn mcp_definitions() -> Vec<Value> {
        let mut v = maluos_slides_mcp::tool_definitions();
        v.extend(extra_definitions());
        v
    }

    /// The fork's own definitions.
    pub fn extra_definitions() -> Vec<Value> {
        extra_definitions()
    }

    pub fn is_extra(name: &str) -> bool {
        is_extra(name)
    }
}

/// One MCP definition as a [`ToolDef`]; `None` when it lacks a name.
fn to_llm(def: &Value) -> Option<ToolDef> {
    let name = def.get("name").and_then(Value::as_str)?.to_string();
    let description = def.get("description").and_then(Value::as_str).unwrap_or("").to_string();
    let input_schema = def.get("inputSchema").cloned().unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
    Some(ToolDef { name, description, input_schema })
}
