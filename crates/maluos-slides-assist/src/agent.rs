//! The loop: one user message in, tool calls through the engine, one answer out, one undo step.
//!
//! `Assistant::ask` builds the prompt (instructions, the command catalog, the deck outline from
//! [`Context`]), sends it with the [`ToolTable`], runs every tool call the model makes through
//! [`run_tool`] (so through `Session::execute`), feeds the results back and repeats until the
//! model answers in text or a limit stops it. When the turn ends, the undo entries the turn's
//! commands pushed are folded into one labelled `Assistant: …`, so Undo reverts the whole turn.
//!
//! The conversation (`messages`) persists across turns; the deck outline does not pile up in it
//! because it lives in the system text, rebuilt each turn.

use std::sync::Arc;
use std::time::Duration;

use maluos_llm::{Completion, ContentBlock, Effort, Event, Flow, LlmClient, LlmError, Message, Request, StopReason, Usage};
use maluos_slides_engine::Session;
use maluos_slides_mcp::{Backend, Headless};
use serde_json::Value;

use crate::catalog::Catalog;
use crate::context::{Budget, ContextCache};
use crate::exec::{ToolOutcome, run_tool};
use crate::policy::{Confirm, Decision, Mode, Plan, PlannedCall, Policy, Verdict};
use crate::tools::ToolTable;

/// What the loop runs against: an MCP backend whose session it can also read, for the deck
/// outline and the undo history. [`Headless`] is one; the desktop app supplies another.
pub trait Host: Backend {
    fn session(&self) -> &Session;
    fn session_mut(&mut self) -> &mut Session;
}

impl Host for Headless {
    fn session(&self) -> &Session {
        &self.session
    }
    fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

/// Where a turn stops when the model does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Model calls per user message (each tool round is one).
    pub max_rounds: usize,
    /// Tool calls per user message; past it every call is answered with an error.
    pub max_tool_calls: usize,
    /// `max_tokens` of each model call.
    pub max_output_tokens: u32,
    /// Bytes of the command catalog in the system text (0 leaves it out; `list_commands`
    /// still works).
    pub catalog_bytes: usize,
    /// Retries on a retryable error (rate limit, overload, transport), with `retry_delay`
    /// doubling each time.
    pub retries: usize,
    pub retry_delay: Duration,
    /// Messages kept in the conversation; older turns are dropped from the front (whole turns,
    /// never splitting a tool call from its result).
    pub max_messages: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_rounds: 12,
            max_tool_calls: 40,
            max_output_tokens: 8_000,
            catalog_bytes: 32 * 1024,
            retries: 3,
            retry_delay: Duration::from_secs(2),
            max_messages: 60,
        }
    }
}

/// What the pane shows while a turn runs. Return [`Flow::Stop`] to cancel.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// A model call starts (1-based).
    Round(usize),
    Text(String),
    Thinking(String),
    /// A tool call is about to run.
    ToolStart {
        name: String,
        input: Value,
    },
    ToolDone(ToolOutcome),
}

/// One tool call of a turn, for the report.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub input: Value,
    pub ok: bool,
    /// The result text, cut to a line or two.
    pub summary: String,
}

/// What a turn did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TurnReport {
    /// The model's final text (empty when it ended on an error or a limit).
    pub answer: String,
    pub rounds: usize,
    pub tool_calls: Vec<ToolCall>,
    /// Command ids the engine ran this turn, in order (from the session journal).
    pub commands: Vec<String>,
    /// Usage summed over the turn's model calls.
    pub usage: Usage,
    /// The one undo entry's label when the turn changed the deck.
    pub undo_label: Option<String>,
    /// Why the turn ended early, if it did.
    pub stopped: Option<Stop>,
    pub error: Option<LlmError>,
    /// Plans the user was asked about, and how many were declined.
    pub plans: usize,
    pub declined: usize,
    /// Calls the mode refused (answer-only).
    pub refused: usize,
    /// Slides (0-based, as they stand after the turn) the turn added or changed, and how many
    /// it removed.
    pub touched_slides: Vec<usize>,
    pub removed_slides: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    RoundLimit,
    ToolCallLimit,
    Cancelled,
    Refusal,
    MaxTokens,
}

/// The assistant: a model, the tools, the conversation.
pub struct Assistant {
    client: Arc<dyn LlmClient>,
    pub model: String,
    pub effort: Option<Effort>,
    pub limits: Limits,
    pub budget: Budget,
    pub instructions: String,
    pub policy: Policy,
    pub messages: Vec<Message>,
    catalog: Option<Catalog>,
    cache: ContextCache,
}

/// The standing instructions. The catalog and the deck outline follow them in the system text.
pub const INSTRUCTIONS: &str = "You are Malu, the assistant inside MaluOS Slides, a presentation editor. You help the user build and change the open presentation by calling tools; every change goes through the editor's own commands and the user can undo a whole turn at once.

How to work:
- The deck outline below is the state right now: it is rebuilt after every tool call, so changes you made earlier in this turn already show in it (do not take them for pre-existing content). Slides are numbered from 1 in prose but tools take 0-based indexes and shape ids; both are in the outline.
- When the user says \"this\", \"here\" or \"the title\", the selection is the target; without a selection, the current slide is.
- To change text, use set_text with the shape id (it keeps the formatting). To add content, use add_slide, add_shape, add_text_box, insert_table or insert_chart. For anything else, run_command with a command id from the catalog; its params are documented there. Use batch for several commands.
- Check the result of a tool before building on it: inspect_slide shows ids and boxes, render_slide shows the picture.
- Make the smallest change that does what was asked; keep the deck's theme, layouts and style unless asked otherwise.
- Answer briefly: what changed, on which slides. Do not describe tool mechanics. If something cannot be done, say so.";

impl Assistant {
    pub fn new(client: Arc<dyn LlmClient>, model: impl Into<String>) -> Self {
        Assistant {
            client,
            model: model.into(),
            effort: None,
            limits: Limits::default(),
            budget: Budget::default(),
            instructions: INSTRUCTIONS.into(),
            policy: Policy::new(Mode::Confirm),
            messages: Vec::new(),
            catalog: None,
            cache: ContextCache::new(),
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = Some(effort);
        self
    }

    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.policy = Policy::new(mode);
        self
    }

    pub fn mode(&self) -> Mode {
        self.policy.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.policy.mode = mode;
    }

    pub fn provider(&self) -> &str {
        self.client.provider()
    }

    /// Forgets the conversation (not the deck).
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// The system text for the next call: instructions, the catalog, the deck outline.
    pub fn system_text(&mut self, host: &dyn Host) -> String {
        let catalog = self.catalog.get_or_insert_with(Catalog::new);
        let mut s = String::with_capacity(self.limits.catalog_bytes + 4096);
        s.push_str(&self.instructions);
        s.push_str("\n\n");
        s.push_str(self.policy.mode.instruction());
        if self.limits.catalog_bytes > 0 {
            s.push_str("\n\n# Command catalog (run_command ids with their params)\n");
            s.push_str(&catalog.prompt_text("", self.limits.catalog_bytes));
        }
        s.push_str("\n\n# The open presentation (as it is now, your edits so far included)\n");
        s.push_str(&self.cache.get(host.session(), &self.budget).text);
        s
    }

    /// One user message: the loop runs until the model answers or a limit stops it. `confirm`
    /// answers the plans the mode requires (see [`Policy`]); `on_event` sees the stream and each
    /// tool call, and returning [`Flow::Stop`] cancels after the current tool call.
    pub fn ask(&mut self, host: &mut dyn Host, text: &str, confirm: &mut dyn Confirm, on_event: &mut dyn FnMut(TurnEvent) -> Flow) -> TurnReport {
        let mut report = TurnReport::default();
        let table = ToolTable::for_backend(host.has_ui());
        let catalog = Catalog::from_session(host.session());
        let undo_mark = history_mark(host.session());
        let journal_mark = host.session().journal.len();
        let slide_mark = slide_marks(host.session());
        self.messages.push(Message::user(text));
        self.trim_messages();

        let mut tool_calls = 0usize;
        let mut cancelled = false;
        'rounds: for round in 1..=self.limits.max_rounds {
            report.rounds = round;
            if on_event(TurnEvent::Round(round)) == Flow::Stop {
                cancelled = true;
                break;
            }
            let system = self.system_text(host);
            let tools: Vec<_> = self.policy.tools(&table).into_iter().cloned().collect();
            let mut request = Request::new(self.model.clone(), self.messages.clone())
                .with_system(system)
                .with_tools(tools)
                .with_max_tokens(self.limits.max_output_tokens);
            if let Some(e) = self.effort {
                request = request.with_effort(e);
            }
            let completion = match self.complete_with_retry(&request, on_event) {
                Ok(c) => c,
                Err(LlmError::Cancelled) => {
                    cancelled = true;
                    break;
                }
                Err(e) => {
                    report.error = Some(e);
                    break;
                }
            };
            add_usage(&mut report.usage, &completion.usage);
            let uses: Vec<(String, String, Value)> =
                completion.tool_uses().map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone())).collect();
            let answer = completion.text();
            self.messages.push(completion.as_message());
            match completion.stop_reason {
                StopReason::Refusal => {
                    report.answer = answer;
                    report.stopped = Some(Stop::Refusal);
                    break;
                }
                StopReason::MaxTokens if uses.is_empty() => {
                    report.answer = answer;
                    report.stopped = Some(Stop::MaxTokens);
                    break;
                }
                _ if uses.is_empty() => {
                    report.answer = answer;
                    break;
                }
                _ => {}
            }
            // Classify every call first, so the user sees one plan per round.
            let planned: Vec<(PlannedCall, Decision)> = uses
                .iter()
                .map(|(id, name, input)| {
                    let call = self.policy.classify(&table, &catalog, id, name, input);
                    let decision = self.policy.decide(call.class);
                    (call, decision)
                })
                .collect();
            let plan = Plan { calls: planned.iter().filter(|(_, d)| *d == Decision::Confirm).map(|(c, _)| c.clone()).collect() };
            let verdict = if plan.calls.is_empty() || cancelled {
                Verdict::Approve
            } else {
                report.plans += 1;
                let v = confirm.confirm(&plan);
                if v == Verdict::Cancel {
                    cancelled = true;
                }
                v
            };
            let mut results = Vec::with_capacity(uses.len());
            for ((id, name, input), (_, decision)) in uses.into_iter().zip(planned) {
                let outcome = if cancelled {
                    ToolOutcome::error_for(&id, &name, "cancelled by the user")
                } else if let Decision::Refuse(reason) = &decision {
                    report.refused += 1;
                    ToolOutcome::error_for(&id, &name, reason.clone())
                } else if decision == Decision::Confirm && verdict != Verdict::Approve {
                    report.declined += 1;
                    let reason = match &verdict {
                        Verdict::Decline(Some(r)) => format!("the user declined this change: {r}"),
                        _ => "the user declined this change; do not retry it, ask what they want instead".into(),
                    };
                    ToolOutcome::error_for(&id, &name, reason)
                } else if tool_calls >= self.limits.max_tool_calls {
                    report.stopped = Some(Stop::ToolCallLimit);
                    ToolOutcome::error_for(
                        &id,
                        &name,
                        format!("tool call limit of {} reached for this turn; answer with what you have", self.limits.max_tool_calls),
                    )
                } else {
                    tool_calls += 1;
                    if on_event(TurnEvent::ToolStart { name: name.clone(), input: input.clone() }) == Flow::Stop {
                        cancelled = true;
                        ToolOutcome::error_for(&id, &name, "cancelled by the user")
                    } else {
                        let o = run_tool(host, &table, &catalog, &id, &name, &input);
                        if on_event(TurnEvent::ToolDone(o.clone())) == Flow::Stop {
                            cancelled = true;
                        }
                        o
                    }
                };
                report.tool_calls.push(ToolCall { name: name.clone(), input, ok: !outcome.is_error, summary: summary(&outcome.text) });
                results.push(outcome.block());
            }
            self.messages.push(Message::tool_results(results));
            if cancelled {
                break 'rounds;
            }
        }
        if cancelled {
            report.stopped = Some(Stop::Cancelled);
            // A dangling tool call would make the next request invalid: answer it.
            self.close_open_tool_calls();
        } else if report.answer.is_empty() && report.error.is_none() && report.stopped.is_none() && report.rounds >= self.limits.max_rounds {
            report.stopped = Some(Stop::RoundLimit);
        }
        if report.error.is_some() {
            self.close_open_tool_calls();
        }
        report.commands = host.session().journal.iter().skip(journal_mark).map(|(id, _)| id.clone()).collect();
        (report.touched_slides, report.removed_slides) = slides_touched(host.session(), &slide_mark);
        report.undo_label = fold_history(host.session_mut(), undo_mark, text);
        report
    }

    fn complete_with_retry(&self, request: &Request, on_event: &mut dyn FnMut(TurnEvent) -> Flow) -> Result<Completion, LlmError> {
        let mut delay = self.limits.retry_delay;
        let mut attempt = 0;
        loop {
            let mut forward = |e: Event| match e {
                Event::TextDelta(t) => on_event(TurnEvent::Text(t)),
                Event::ThinkingDelta(t) => on_event(TurnEvent::Thinking(t)),
                _ => Flow::Continue,
            };
            match self.client.complete(request, &mut forward) {
                Err(e) if e.retryable() && attempt < self.limits.retries => {
                    attempt += 1;
                    log::warn!("model call failed ({e}); retry {attempt} in {delay:?}");
                    let wait = match &e {
                        LlmError::RateLimited { retry_after: Some(s), .. } => (*s).max(delay),
                        _ => delay,
                    };
                    if !wait.is_zero() {
                        std::thread::sleep(wait);
                    }
                    delay *= 2;
                }
                other => return other,
            }
        }
    }

    /// If the last assistant message asked for tools nobody answered, answer them as cancelled.
    fn close_open_tool_calls(&mut self) {
        let Some(Message { role: maluos_llm::Role::Assistant, content }) = self.messages.last() else { return };
        let ids: Vec<String> =
            content.iter().filter_map(|b| if let ContentBlock::ToolUse { id, .. } = b { Some(id.clone()) } else { None }).collect();
        if ids.is_empty() {
            return;
        }
        let results = ids.into_iter().map(|id| ContentBlock::ToolResult { tool_use_id: id, content: "cancelled".into(), is_error: true }).collect();
        self.messages.push(Message::tool_results(results));
    }

    /// Drops the oldest turns past `max_messages`, always cutting at a user text message so a
    /// tool result never loses its call.
    fn trim_messages(&mut self) {
        let max = self.limits.max_messages.max(2);
        while self.messages.len() > max {
            let next_user = self
                .messages
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, m)| m.role == maluos_llm::Role::User && m.content.iter().all(|b| matches!(b, ContentBlock::Text { .. })))
                .map(|(i, _)| i);
            match next_user {
                Some(i) if i > 0 => {
                    self.messages.drain(..i);
                }
                _ => break,
            }
        }
    }
}

fn add_usage(total: &mut Usage, u: &Usage) {
    total.input_tokens += u.input_tokens;
    total.output_tokens += u.output_tokens;
    total.cache_read_input_tokens += u.cache_read_input_tokens;
    total.cache_creation_input_tokens += u.cache_creation_input_tokens;
}

fn summary(text: &str) -> String {
    let line: String = text.lines().take(2).collect::<Vec<_>>().join(" ");
    let n = line.chars().count();
    if n <= 160 { line } else { line.chars().take(159).chain(std::iter::once('…')).collect() }
}

/// The active document's identity and its document pointer before a turn: the first undo entry
/// the turn pushes holds exactly this document.
struct HistoryMark {
    uid: u64,
    doc: Option<*const maluos_slides_engine::model::Presentation>,
}

fn history_mark(s: &Session) -> HistoryMark {
    match s.active() {
        Some(st) => HistoryMark { uid: st.uid, doc: Some(Arc::as_ptr(&st.doc)) },
        None => HistoryMark { uid: 0, doc: None },
    }
}

/// Folds the undo entries pushed since `mark` into one, labelled after the request, and returns
/// the label. Nothing happens when the turn changed nothing, opened another document, or the
/// history dropped the mark.
fn fold_history(s: &mut Session, mark: HistoryMark, request: &str) -> Option<String> {
    let st = s.active_mut()?;
    let doc = mark.doc?;
    if st.uid != mark.uid {
        return None;
    }
    let first = st.history.undo.iter().rposition(|e| std::ptr::eq(Arc::as_ptr(&e.doc), doc))?;
    if Arc::ptr_eq(&st.history.undo[first].doc, &st.doc) {
        return None; // the document is back where it started
    }
    let label = format!("Assistant: {}", summary_label(request));
    st.history.undo.truncate(first + 1);
    st.history.undo[first].label = label.clone();
    st.history.redo.clear();
    Some(label)
}

type SlideMark = (maluos_slides_engine::model::SlideId, Arc<maluos_slides_engine::model::Slide>);

/// Each slide as it stood: the id and the slide itself (an `Arc` clone, no copy).
fn slide_marks(s: &Session) -> Vec<SlideMark> {
    s.active().map(|st| st.doc.slides.iter().map(|sl| (sl.id, sl.clone())).collect()).unwrap_or_default()
}

/// Slides added or changed since `before` (indexes now), and how many are gone. A slide whose
/// pointer moved but whose content is equal counts as unchanged (the engine rewrites the
/// current slide's `Arc` on every edit while rerouting connectors).
fn slides_touched(s: &Session, before: &[SlideMark]) -> (Vec<usize>, usize) {
    let Some(st) = s.active() else { return (vec![], before.len()) };
    let touched = st
        .doc
        .slides
        .iter()
        .enumerate()
        .filter(|(_, sl)| !before.iter().any(|(id, was)| *id == sl.id && (Arc::ptr_eq(was, sl) || **was == ***sl)))
        .map(|(i, _)| i)
        .collect();
    let removed = before.iter().filter(|(id, _)| !st.doc.slides.iter().any(|sl| sl.id == *id)).count();
    (touched, removed)
}

fn summary_label(request: &str) -> String {
    let one: String = request.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = one.chars().count();
    if n <= 48 { one } else { one.chars().take(47).chain(std::iter::once('…')).collect() }
}
