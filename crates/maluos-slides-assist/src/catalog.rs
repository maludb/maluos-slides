//! The command catalog: every engine command, its params doc and a JSON Schema made from it.
//!
//! The engine documents each command's parameters in one line (`{layout?: name, at?: index,
//! title?: text}`) that `maluos_slides_engine::cmd::parse_params` reads. The catalog turns that
//! line into a schema (property names, which are required, a description per property) so a
//! `run_command` call is checked before it runs, and into a compact listing for the prompt.

use maluos_slides_engine::cmd::{Param, parse_params};
use maluos_slides_engine::{CommandInfo, Session, find_command};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// One command as the catalog shows it.
#[derive(Debug, Clone, Serialize)]
pub struct CommandEntry {
    pub id: &'static str,
    pub label: &'static str,
    /// Ribbon tab and group, or menu.
    pub menu: Vec<&'static str>,
    pub shortcut: Option<&'static str>,
    /// The params doc, verbatim.
    pub params: &'static str,
    /// It edits the presentation and pushes an undo step.
    pub undoable: bool,
    /// It is recorded in the session journal (every command but queries).
    pub journal: bool,
    /// Enabled in the session the catalog was built from.
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

impl CommandEntry {
    /// `query` commands change nothing: `slide.inspect`, `document.inspect`, `file.describe`…
    pub fn is_query(&self) -> bool {
        !self.journal && !self.undoable
    }

    /// The JSON Schema of the params object (see [`params_schema`]).
    pub fn schema(&self) -> Value {
        params_schema(self.params).unwrap_or_else(free_object)
    }

    /// One line for the prompt: `id — label {params}`.
    pub fn line(&self) -> String {
        let params = self.params.split('→').next().unwrap_or("").trim();
        if params.is_empty() || params == "{}" {
            format!("{} — {}", self.id, self.label)
        } else {
            format!("{} — {} {params}", self.id, self.label)
        }
    }
}

/// The catalog of a session: the commands in the engine's order, with each one's enabled state
/// as it stood when the catalog was built.
#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<CommandEntry>,
}

impl Catalog {
    /// The catalog with enabled flags from `session`.
    pub fn from_session(session: &Session) -> Self {
        Self::from_infos(session.commands())
    }

    /// The catalog of a fresh presentation (what is enabled with one slide and nothing selected).
    pub fn new() -> Self {
        Self::from_session(&Session::with_new())
    }

    fn from_infos(infos: Vec<CommandInfo>) -> Self {
        let entries = infos
            .into_iter()
            .map(|i| {
                let spec = find_command(i.id);
                CommandEntry {
                    id: i.id,
                    label: i.label,
                    menu: i.menu,
                    shortcut: i.shortcut,
                    params: i.params,
                    undoable: spec.is_some_and(|s| s.undoable),
                    journal: spec.is_some_and(|s| s.journal),
                    enabled: i.enabled,
                    disabled_reason: i.disabled_reason,
                }
            })
            .collect();
        Catalog { entries }
    }

    pub fn entries(&self) -> &[CommandEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&CommandEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The schema of `id`'s params, or `None` for an unknown command.
    pub fn schema(&self, id: &str) -> Option<Value> {
        self.get(id).map(CommandEntry::schema)
    }

    /// Checks a `run_command` call before it runs: the command must exist and `params` must be an
    /// object whose keys the command documents. A key the doc does not list is refused, so a
    /// misspelled parameter never succeeds silently with the command's default (the MCP server
    /// refuses unknown tool arguments the same way). Docs the grammar cannot read, and commands
    /// documenting no parameters at all, accept any object.
    pub fn validate(&self, id: &str, params: &Value) -> Result<(), String> {
        let Some(entry) = self.get(id) else {
            let near = self.nearest(id);
            return Err(match near {
                Some(n) => format!("unknown command `{id}` (did you mean `{n}`? list_commands shows every command)"),
                None => format!("unknown command `{id}` (list_commands shows every command)"),
            });
        };
        let Some(object) = params.as_object() else {
            if params.is_null() {
                return Ok(());
            }
            return Err(format!("{id}: params must be a JSON object, got {}", kind(params)));
        };
        let Ok(doc) = parse_params(entry.params) else { return Ok(()) };
        if doc.is_empty() {
            return Ok(());
        }
        let mut unknown: Vec<&str> = object.keys().map(String::as_str).filter(|k| !doc.iter().any(|p| p.name == *k)).collect();
        if unknown.is_empty() {
            return Ok(());
        }
        unknown.sort_unstable();
        let mut known: Vec<&str> = Vec::new();
        for p in &doc {
            if !known.contains(&p.name) {
                known.push(p.name);
            }
        }
        let what = if unknown.len() == 1 { "unknown parameter" } else { "unknown parameters" };
        let list = unknown.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>().join(", ");
        Err(format!("{id}: {what} {list} (expected: {}; params doc: {})", known.join(", "), entry.params))
    }

    /// The compact listing for a prompt: one line per command whose id or label contains
    /// `filter` (case-insensitive; empty for all), cut at `max_bytes` with a marker saying how
    /// many lines were left out.
    pub fn prompt_text(&self, filter: &str, max_bytes: usize) -> String {
        let filter = filter.to_lowercase();
        let lines: Vec<String> = self
            .entries
            .iter()
            .filter(|e| filter.is_empty() || e.id.to_lowercase().contains(&filter) || e.label.to_lowercase().contains(&filter))
            .map(CommandEntry::line)
            .collect();
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            if out.len() + line.len() + 1 > max_bytes {
                let left = lines.len() - i;
                out.push_str(&format!("… {left} more (list_commands with a filter shows them)\n"));
                break;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// The closest id for a typo: at most two edits away (`slide.nwe`, `shape.fil`), the
    /// nearest first and the shorter on a tie.
    fn nearest(&self, id: &str) -> Option<&'static str> {
        let lower = id.to_lowercase();
        self.entries
            .iter()
            .map(|e| (edit_distance(&lower, &e.id.to_lowercase()), e.id))
            .filter(|(d, _)| *d <= 2)
            .min_by_key(|(d, c)| (*d, c.len()))
            .map(|(_, c)| c)
    }
}

/// Levenshtein distance over chars; ids are short, so the simple two-row version is fine.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new()
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn free_object() -> Value {
    json!({"type": "object"})
}

/// A JSON Schema for one params doc, or `None` when the doc does not follow the grammar.
///
/// Properties come from the keys; a key's doc text becomes its `description`, and a type is
/// given only where the doc is unambiguous (`bool`, `[…]`, `text`, an index). Keys are required
/// only when the doc has one object, the key is not marked `?` and it is not one of several
/// alternatives (`{text | body}`); a doc with several objects (`{index} | {id}`) requires nothing
/// and says so in the schema's description. `additionalProperties` is false, which is what
/// [`Catalog::validate`] enforces.
pub fn params_schema(doc: &str) -> Option<Value> {
    let params = parse_params(doc).ok()?;
    let objects = params.iter().map(|p| p.object + 1).max().unwrap_or(1);
    let mut properties = Map::new();
    let mut required: Vec<&str> = Vec::new();
    for (i, p) in params.iter().enumerate() {
        let next_is_alternative = params.get(i + 1).is_some_and(|n| n.or && n.object == p.object);
        let is_alternative = p.or || next_is_alternative;
        if objects == 1 && !p.optional && !is_alternative && !required.contains(&p.name) {
            required.push(p.name);
        }
        if properties.contains_key(p.name) {
            continue;
        }
        properties.insert(p.name.to_string(), property(p));
    }
    let mut schema = json!({"type": "object", "properties": properties, "additionalProperties": false});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    let mut notes: Vec<String> = Vec::new();
    if objects > 1 {
        let head = doc.split('→').next().unwrap_or("").trim();
        notes.push(format!("One of the alternative forms: {head}"));
    }
    if let Some(remark) = top_level_remark(doc) {
        notes.push(remark);
    }
    if let Some(result) = doc.split('→').nth(1).map(str::trim).filter(|s| !s.is_empty()) {
        notes.push(format!("Returns: {result}"));
    }
    if !notes.is_empty() {
        schema["description"] = json!(notes.join(". "));
    }
    Some(schema)
}

fn property(p: &Param<'_>) -> Value {
    let about = p.about.trim();
    let mut v = Map::new();
    if let Some(t) = infer_type(p.name, about) {
        v.insert("type".into(), json!(t));
    }
    if !about.is_empty() {
        v.insert("description".into(), json!(about));
    } else if p.optional {
        v.insert("description".into(), json!("optional"));
    }
    Value::Object(v)
}

/// A type only where the doc leaves no doubt.
fn infer_type(name: &str, about: &str) -> Option<&'static str> {
    let a = about.trim_start_matches(':').trim();
    if a == "bool" || a.starts_with("bool ") || a.starts_with("bool(") {
        return Some("boolean");
    }
    if a.starts_with('[') {
        return Some("array");
    }
    if a == "text" || a == "name" || a == "path" || a.starts_with("text ") || a.starts_with("text(") {
        return Some("string");
    }
    if a == "index" || a == "slide index" || (a.is_empty() && matches!(name, "index" | "at" | "rows" | "cols")) {
        return Some("integer");
    }
    None
}

/// A remark in parentheses after the object, like `(text as typed: …)`.
fn top_level_remark(doc: &str) -> Option<String> {
    let head = doc.split('→').next()?;
    let close = head.rfind('}')?;
    let tail = head.get(close + 1..)?.trim().trim_start_matches('—').trim();
    if tail.is_empty() { None } else { Some(tail.trim_start_matches('(').trim_end_matches(')').to_string()) }
}
