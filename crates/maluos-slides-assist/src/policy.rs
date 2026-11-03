//! Safety rails: what the assistant may do on its own, what it must ask about, what it may not.
//!
//! Three modes (owner decision D11, step 3.5):
//! - **answer only**: nothing changes; the model reads and explains.
//! - **confirm**: every change is shown as a plan (the tool calls and the engine commands behind
//!   them) and applied only when the user approves it.
//! - **autopilot**: changes apply at once; destructive commands (deleting slides or sections,
//!   starting or opening another presentation, clearing a deck's content) are still confirmed.

use serde_json::Value;

use crate::catalog::Catalog;
use crate::tools::ToolTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    AnswerOnly,
    #[default]
    Confirm,
    Autopilot,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::AnswerOnly => "answer-only",
            Mode::Confirm => "confirm",
            Mode::Autopilot => "autopilot",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "answer" | "answer-only" | "answeronly" | "ask" | "read" => Mode::AnswerOnly,
            "confirm" | "review" => Mode::Confirm,
            "autopilot" | "auto" | "apply" => Mode::Autopilot,
            _ => return None,
        })
    }

    /// One line for the system text.
    pub fn instruction(self) -> &'static str {
        match self {
            Mode::AnswerOnly => {
                "Mode: answer only. You cannot change the presentation in this mode: inspect it and say what you would do and which commands you would run. run_command accepts only query commands (inspect, describe, list)."
            }
            Mode::Confirm => {
                "Mode: confirm. Every change you make is shown to the user as a plan and applied only when they approve it. Group the calls of one change together. If the user declines a plan, do not retry it; ask what they want instead."
            }
            Mode::Autopilot => {
                "Mode: autopilot. Your changes apply at once; destructive ones (deleting slides or sections, replacing the presentation) are confirmed by the user first. If the user declines one, do not retry it."
            }
        }
    }
}

/// Commands that destroy content or replace the presentation: confirmed in every mode that
/// allows changes.
pub const DESTRUCTIVE_COMMANDS: &[&str] = &[
    "slide.delete",
    "section.remove",
    "section.removeAll",
    "master.deleteLayout",
    "file.new",
    "file.open",
    "file.openBytes",
    "file.close",
    "file.revert",
    "file.adopt",
    "file.recovery.open",
];

/// Tools that run a destructive command.
pub const DESTRUCTIVE_TOOLS: &[&str] = &["new_presentation", "open_presentation"];

/// What a tool call does, in the policy's eyes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    ReadOnly,
    Change,
    Destructive,
}

/// What to do with one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Ask the user first.
    Confirm,
    /// Do not run; the model gets this reason.
    Refuse(String),
}

/// One tool call as the user sees it before approving.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedCall {
    pub tool_use_id: String,
    pub name: String,
    pub input: Value,
    pub class: Class,
    /// The engine commands behind it: (id, label, params). For `run_command` and `batch` the
    /// real ones; for the other tools the command the MCP server maps them to.
    pub commands: Vec<(String, String, Value)>,
}

impl PlannedCall {
    /// A few lines for a dialog or the terminal.
    pub fn describe(&self) -> String {
        let mut s = String::new();
        s.push_str(&self.name);
        if self.class == Class::Destructive {
            s.push_str(" (destructive)");
        }
        match self.name.as_str() {
            "run_command" | "batch" => {
                for (id, label, params) in &self.commands {
                    let p = if params.is_null() || params == &Value::Object(Default::default()) { String::new() } else { format!(" {params}") };
                    s.push_str(&format!("\n  {id} — {label}{p}"));
                }
            }
            _ => {
                if let Some(o) = self.input.as_object().filter(|o| !o.is_empty()) {
                    let args: Vec<String> = o.iter().map(|(k, v)| format!("{k}={}", short(v))).collect();
                    s.push_str(&format!(" {}", args.join(" ")));
                }
            }
        }
        s
    }
}

/// The calls of one round that need the user's approval, shown together.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub calls: Vec<PlannedCall>,
}

impl Plan {
    pub fn destructive(&self) -> bool {
        self.calls.iter().any(|c| c.class == Class::Destructive)
    }

    pub fn describe(&self) -> String {
        self.calls.iter().map(PlannedCall::describe).collect::<Vec<_>>().join("\n")
    }
}

/// The user's answer to a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    /// Not this; the reason, if any, goes to the model.
    Decline(Option<String>),
    /// Stop the whole turn.
    Cancel,
}

/// Who answers a plan: a dialog, a terminal prompt, a test.
pub trait Confirm {
    fn confirm(&mut self, plan: &Plan) -> Verdict;
}

impl<F: FnMut(&Plan) -> Verdict> Confirm for F {
    fn confirm(&mut self, plan: &Plan) -> Verdict {
        self(plan)
    }
}

/// Approves everything: scripts, tests, the CLI with `--yes`.
pub struct ApproveAll;

impl Confirm for ApproveAll {
    fn confirm(&mut self, _: &Plan) -> Verdict {
        Verdict::Approve
    }
}

/// Declines everything: answer-only behaviour for a host without a dialog.
pub struct DeclineAll;

impl Confirm for DeclineAll {
    fn confirm(&mut self, _: &Plan) -> Verdict {
        Verdict::Decline(Some("no one is there to approve changes".into()))
    }
}

/// The rails for one mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub mode: Mode,
    pub destructive_commands: Vec<String>,
    pub destructive_tools: Vec<String>,
}

impl Policy {
    pub fn new(mode: Mode) -> Self {
        Policy {
            mode,
            destructive_commands: DESTRUCTIVE_COMMANDS.iter().map(|s| s.to_string()).collect(),
            destructive_tools: DESTRUCTIVE_TOOLS.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn is_destructive_command(&self, id: &str) -> bool {
        self.destructive_commands.iter().any(|c| c == id)
    }

    /// The tools the model is offered in this mode: everything, or in answer-only mode the
    /// read-only ones plus `run_command` (for query commands) and `list_commands`.
    pub fn tools<'a>(&self, table: &'a ToolTable) -> Vec<&'a maluos_llm::ToolDef> {
        table
            .defs()
            .iter()
            .filter(|t| self.mode != Mode::AnswerOnly || table.is_read_only(&t.name) || matches!(t.name.as_str(), "run_command" | "list_commands"))
            .collect()
    }

    /// What one call does and the commands behind it.
    pub fn classify(&self, table: &ToolTable, catalog: &Catalog, tool_use_id: &str, name: &str, input: &Value) -> PlannedCall {
        let mut commands = Vec::new();
        let mut class = if table.is_read_only(name) { Class::ReadOnly } else { Class::Change };
        let mut note = |id: &str, params: Value, class: &mut Class| {
            let (label, query) = catalog.get(id).map(|e| (e.label.to_string(), e.is_query())).unwrap_or_else(|| (String::new(), false));
            let c = if self.is_destructive_command(id) {
                Class::Destructive
            } else if query {
                Class::ReadOnly
            } else {
                Class::Change
            };
            *class = (*class).max(c);
            commands.push((id.to_string(), label, params));
        };
        match name {
            "run_command" => {
                class = Class::ReadOnly;
                let id = input.get("command").and_then(Value::as_str).unwrap_or("");
                note(id, input.get("params").cloned().unwrap_or(Value::Null), &mut class);
            }
            "batch" => {
                class = Class::ReadOnly;
                for c in input.get("commands").and_then(Value::as_array).into_iter().flatten() {
                    let id = c.get("command").and_then(Value::as_str).unwrap_or("");
                    note(id, c.get("params").cloned().unwrap_or(Value::Null), &mut class);
                }
            }
            _ => {
                if self.destructive_tools.iter().any(|t| t == name)
                    || (name == crate::tools::BUILD_DECK && input.get("replace").and_then(Value::as_bool).unwrap_or(false))
                {
                    class = Class::Destructive;
                }
                if let Some(id) = tool_command(name) {
                    let label = catalog.get(id).map(|e| e.label.to_string()).unwrap_or_default();
                    commands.push((id.to_string(), label, input.clone()));
                }
            }
        }
        PlannedCall { tool_use_id: tool_use_id.into(), name: name.into(), input: input.clone(), class, commands }
    }

    pub fn decide(&self, class: Class) -> Decision {
        match (self.mode, class) {
            (_, Class::ReadOnly) => Decision::Allow,
            (Mode::AnswerOnly, _) => {
                Decision::Refuse("answer-only mode: the presentation cannot be changed now; tell the user what you would do".into())
            }
            (Mode::Confirm, _) => Decision::Confirm,
            (Mode::Autopilot, Class::Change) => Decision::Allow,
            (Mode::Autopilot, Class::Destructive) => Decision::Confirm,
        }
    }
}

/// The engine command the MCP server runs for a tool (the first one, for tools that run
/// several), for the plan's description.
pub fn tool_command(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "new_presentation" => "file.new",
        "open_presentation" => "file.open",
        "save_presentation" => "file.save",
        "export" => "file.export",
        "inspect_slide" => "slide.inspect",
        "add_slide" => "slide.new",
        "go_to_slide" => "slide.go",
        "add_shape" => "shape.insert",
        "add_text_box" => "insert.textBox",
        "set_text" => "text.set",
        "insert_picture" => "insert.picture",
        "insert_table" => "insert.table",
        "insert_chart" => "insert.chart",
        "select" => "edit.select",
        "build_deck" => "slide.new",
        "deck_timing" => "document.inspect",
        "office_picture" => "insert.picture",
        "remember" | "forget" => "document.tags",
        _ => return None,
    })
}

fn short(v: &Value) -> String {
    let s = match v {
        Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    };
    if s.chars().count() > 60 { s.chars().take(59).chain(std::iter::once('…')).collect() } else { s }
}
