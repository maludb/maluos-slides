//! The tool table: the MCP server's definitions, offered to a model.

use std::collections::BTreeSet;

use maluos_llm::ToolDef;
use serde_json::Value;

/// What the MCP server writes into the description of a tool that needs the desktop app.
pub const APP_ONLY_MARKER: &str = "Desktop app only";

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
        for def in maluos_slides_mcp::tool_definitions() {
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

    /// The raw MCP definitions (name, title, description, `inputSchema`, annotations), for a
    /// network API that wants to publish the same table.
    pub fn mcp_definitions() -> Vec<Value> {
        maluos_slides_mcp::tool_definitions()
    }
}

/// One MCP definition as a [`ToolDef`]; `None` when it lacks a name.
fn to_llm(def: &Value) -> Option<ToolDef> {
    let name = def.get("name").and_then(Value::as_str)?.to_string();
    let description = def.get("description").and_then(Value::as_str).unwrap_or("").to_string();
    let input_schema = def.get("inputSchema").cloned().unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
    Some(ToolDef { name, description, input_schema })
}
