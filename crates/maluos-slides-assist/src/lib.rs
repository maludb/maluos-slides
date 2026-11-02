//! The MaluOS Slides assistant (plan `docs/maluos-slides/plan.md`, Phase 3).
//!
//! This crate is fork code: it sits beside upstream's crates and never edits them. It gives the
//! assistant the same view of the application the MCP server has, so one table drives both
//! (owner decision D9):
//!
//! - [`ToolTable`]: the MCP tool definitions (`maluos_slides_mcp::tool_definitions`) as
//!   [`maluos_llm::ToolDef`]s a model can call, minus the tools a backend without a window cannot
//!   serve.
//! - [`Catalog`]: every engine command with its label, params doc and a JSON Schema generated from
//!   that doc (`maluos_slides_engine::cmd::parse_params`), so `run_command` calls are checked
//!   before they run and the model can be shown a compact listing.
//! - [`Context`]: the open deck as an outline for the prompt (sections, titles, placeholder
//!   text, notes, the selection, the theme), cut to a budget, cached on the revision.
//! - [`Assistant::ask`]: the loop (step 3.4): prompt, model, tool calls, results, answer, one
//!   undo entry per turn, limits on rounds, tool calls and tokens, retries on retryable errors.
//! - [`Policy`] (step 3.5): the modes answer-only, confirm and autopilot; destructive commands
//!   always confirmed; plans the user approves through [`Confirm`].
//! - [`run_tool`]: one tool call executed through an MCP [`Backend`] (the headless session or a
//!   running app), the result shaped as the tool-result block the next request carries.
//!
//! Everything the assistant does to a deck goes through `Session::execute` behind the backend; it
//! never touches the model directly. The loop that ties this to an `LlmClient` is step 3.4.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod agent;
pub mod catalog;
pub mod context;
pub mod exec;
pub mod policy;
pub mod providers;
pub mod tools;

pub use agent::{Assistant, Host, INSTRUCTIONS, Limits, ModelCall, Step, Stop, ToolCall, Turn, TurnEvent, TurnReport, summarize};
pub use catalog::{Catalog, CommandEntry, params_schema};
pub use context::{Budget, Context, ContextCache};
pub use exec::{MAX_RESULT_BYTES, ToolOutcome, run_tool};
pub use maluos_slides_mcp::{Backend, Headless, ToolResult};
pub use policy::{ApproveAll, Class, Confirm, Decision, DeclineAll, Mode, Plan, PlannedCall, Policy, Verdict};
pub use tools::ToolTable;

#[cfg(test)]
mod tests;
