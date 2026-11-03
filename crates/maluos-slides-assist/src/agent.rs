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
    /// Images a tool returned (`render_slide`) go to the model, at most this many per turn; 0
    /// sends none (a text-only model). Images from earlier turns are dropped from the history
    /// when a new turn starts.
    pub max_images: usize,
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
            max_images: 6,
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

/// One user message in progress, driven by [`Assistant::begin`], [`Assistant::on_completion`]
/// and [`Assistant::on_verdict`]. Nothing in it touches the deck outside those calls.
pub struct Turn {
    request_text: String,
    table: ToolTable,
    catalog: Catalog,
    undo_mark: HistoryMark,
    journal_mark: usize,
    slide_mark: Vec<SlideMark>,
    report: TurnReport,
    tool_calls: usize,
    images: usize,
    cancelled: bool,
    pending: Option<Pending>,
    done: bool,
}

impl Turn {
    /// The user's request.
    pub fn request_text(&self) -> &str {
        &self.request_text
    }

    /// What has happened so far (the final report comes with [`Step::Done`]).
    pub fn report(&self) -> &TurnReport {
        &self.report
    }

    pub fn is_done(&self) -> bool {
        self.done
    }
}

struct Pending {
    uses: Vec<(String, String, Value)>,
    planned: Vec<(PlannedCall, Decision)>,
}

/// What the driver does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Send this to the model (on a worker thread, through [`ModelCall::run`]) and pass the
    /// result to [`Assistant::on_completion`].
    Model(Request),
    /// Show this plan; pass the verdict to [`Assistant::on_verdict`].
    Confirm(Plan),
    Done(TurnReport),
}

/// A model call with the retry policy, owned by whichever thread makes it.
pub struct ModelCall {
    client: Arc<dyn LlmClient>,
    retries: usize,
    retry_delay: Duration,
}

impl ModelCall {
    /// Calls the model, retrying a retryable error (rate limit, overload, transport) with the
    /// delay doubling and `retry-after` honoured. `on_event` sees the stream.
    pub fn run(&self, request: &Request, on_event: &mut dyn FnMut(Event) -> Flow) -> Result<Completion, LlmError> {
        let mut delay = self.retry_delay;
        let mut attempt = 0;
        loop {
            match self.client.complete(request, on_event) {
                Err(e) if e.retryable() && attempt < self.retries => {
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

    pub fn provider(&self) -> &str {
        self.client.provider()
    }
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
- To change text, use set_text with the shape id (it keeps the formatting). To add content, use add_slide, add_shape, add_text_box, insert_table or insert_chart. For a whole deck or several slides at once, write the outline and call build_deck once (Markdown: # title, ## section, ### slide, - bullets, Notes: lines, | tables |); it makes the title slide, sections, slides, notes and tables in one call. When the user gives a document, build the deck from its content: one idea per slide, five bullets at most, a title slide, sections for its parts, a closing slide. For anything else, run_command with a command id from the catalog; its params are documented there. Use batch for several commands.
- Check the result of a tool before building on it: inspect_slide shows ids and boxes, render_slide shows the picture (it is attached to the result when you can see images; look at it after a visual change).

Slide help, the usual requests:
- Tighten a title: set_text on the title placeholder (ctrTitle or title in the outline), at most six words, no trailing period.
- Rewrite bullets: set_text on the body placeholder with the whole body: at most five bullets, parallel phrasing, each under twelve words, keep the levels with leading tabs, keep facts and numbers as they are.
- Speaker notes: run_command slide.notes {text, index?} with three to six spoken sentences that add what the slide does not say; never repeat the bullets.
- A chart from a table: inspect_slide for every cell; the first column is the categories, each numeric column a series; insert_chart with a matching type (column for comparisons over categories, line for a series over time, pie only for parts of one whole with at most six slices); keep the table unless asked to replace it.
- Timing: deck_timing gives seconds per slide and the total; to fit a time limit, cut the longest slides' notes and bullets first, never the title or closing slide.
- Q&A prep (prepare me for questions): read the whole deck (inspect_document, then inspect_slide where the outline is cut), write six to ten questions an audience would ask with a two-sentence answer each drawn only from the deck (say 'not in the deck' when it is not), and put them on one new slide at the end titled Q&A prep, questions as bullets and the answers in its notes, then hide it with run_command slide.hide {hidden: true} so it never shows.
- Fix a layout: the outline gives each shape's box [x, y, w, h] on a 960×540 slide; align edges and spacing with shape.move / shape.resize through run_command (list_commands with the filter align shows arrange.align and arrange.distribute), use format.autofit {mode: shrink} for text that overflows, and render_slide to confirm.
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

    /// One user message, driven to the end on this thread: model calls block (with retries),
    /// plans go to `confirm`, the stream and each tool call reach `on_event`, and returning
    /// [`Flow::Stop`] from it cancels after the current tool call. A pane that must not block
    /// drives the same machinery itself with [`Assistant::begin`], [`Assistant::on_completion`]
    /// and [`Assistant::on_verdict`].
    pub fn ask(&mut self, host: &mut dyn Host, text: &str, confirm: &mut dyn Confirm, on_event: &mut dyn FnMut(TurnEvent) -> Flow) -> TurnReport {
        let (mut turn, mut step) = self.begin(host, text, on_event);
        loop {
            match step {
                Step::Model(request) => {
                    let call = self.model_call();
                    let result = call.run(&request, &mut |e| match e {
                        Event::TextDelta(t) => on_event(TurnEvent::Text(t)),
                        Event::ThinkingDelta(t) => on_event(TurnEvent::Thinking(t)),
                        _ => Flow::Continue,
                    });
                    step = self.on_completion(&mut turn, host, result, on_event);
                }
                Step::Confirm(plan) => {
                    let verdict = confirm.confirm(&plan);
                    step = self.on_verdict(&mut turn, host, verdict, on_event);
                }
                Step::Done(report) => return report,
            }
        }
    }

    /// Starts a turn: the user message joins the conversation and the first request is built.
    /// The caller sends it (`Step::Model`) and comes back with [`Assistant::on_completion`].
    pub fn begin(&mut self, host: &mut dyn Host, text: &str, on_event: &mut dyn FnMut(TurnEvent) -> Flow) -> (Turn, Step) {
        let mut turn = Turn {
            request_text: text.to_string(),
            table: ToolTable::for_backend(host.has_ui()),
            catalog: Catalog::from_session(host.session()),
            undo_mark: history_mark(host.session()),
            journal_mark: host.session().journal.len(),
            slide_mark: slide_marks(host.session()),
            report: TurnReport::default(),
            tool_calls: 0,
            images: 0,
            cancelled: false,
            pending: None,
            done: false,
        };
        self.strip_old_images();
        self.messages.push(Message::user(text));
        self.trim_messages();
        let step = self.next_round(&mut turn, host, on_event);
        (turn, step)
    }

    /// The model answered (or failed). Runs the tool calls it made, or returns the plan the
    /// mode wants approved first, or ends the turn.
    pub fn on_completion(
        &mut self,
        turn: &mut Turn,
        host: &mut dyn Host,
        result: Result<Completion, LlmError>,
        on_event: &mut dyn FnMut(TurnEvent) -> Flow,
    ) -> Step {
        if turn.done {
            return Step::Done(turn.report.clone());
        }
        let completion = match result {
            Ok(c) => c,
            Err(LlmError::Cancelled) => {
                turn.cancelled = true;
                return self.finish(turn, host);
            }
            Err(e) => {
                turn.report.error = Some(e);
                return self.finish(turn, host);
            }
        };
        add_usage(&mut turn.report.usage, &completion.usage);
        let uses: Vec<(String, String, Value)> =
            completion.tool_uses().map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone())).collect();
        let answer = completion.text();
        self.messages.push(completion.as_message());
        match completion.stop_reason {
            StopReason::Refusal => {
                turn.report.answer = answer;
                turn.report.stopped = Some(Stop::Refusal);
                return self.finish(turn, host);
            }
            StopReason::MaxTokens if uses.is_empty() => {
                turn.report.answer = answer;
                turn.report.stopped = Some(Stop::MaxTokens);
                return self.finish(turn, host);
            }
            _ if uses.is_empty() => {
                turn.report.answer = answer;
                return self.finish(turn, host);
            }
            _ => {}
        }
        // Classify every call first, so the user sees one plan per round.
        let planned: Vec<(PlannedCall, Decision)> = uses
            .iter()
            .map(|(id, name, input)| {
                let call = self.policy.classify(&turn.table, &turn.catalog, id, name, input);
                let decision = self.policy.decide(call.class);
                (call, decision)
            })
            .collect();
        let plan = Plan { calls: planned.iter().filter(|(_, d)| *d == Decision::Confirm).map(|(c, _)| c.clone()).collect() };
        turn.pending = Some(Pending { uses, planned });
        if plan.calls.is_empty() || turn.cancelled {
            self.run_pending(turn, host, Verdict::Approve, on_event)
        } else {
            turn.report.plans += 1;
            Step::Confirm(plan)
        }
    }

    /// The user answered the plan: the round's calls run (or are declined), and the next
    /// request is built.
    pub fn on_verdict(&mut self, turn: &mut Turn, host: &mut dyn Host, verdict: Verdict, on_event: &mut dyn FnMut(TurnEvent) -> Flow) -> Step {
        if turn.done {
            return Step::Done(turn.report.clone());
        }
        if verdict == Verdict::Cancel {
            turn.cancelled = true;
        }
        self.run_pending(turn, host, verdict, on_event)
    }

    /// Ends the turn now (the user pressed Stop while the model was being called): the next
    /// completion, if one still arrives, is ignored.
    pub fn cancel(&mut self, turn: &mut Turn, host: &mut dyn Host) -> Step {
        if turn.done {
            return Step::Done(turn.report.clone());
        }
        turn.cancelled = true;
        self.finish(turn, host)
    }

    /// The model caller a worker thread can own: the client and the retry policy.
    pub fn model_call(&self) -> ModelCall {
        ModelCall { client: self.client.clone(), retries: self.limits.retries, retry_delay: self.limits.retry_delay }
    }

    fn next_round(&mut self, turn: &mut Turn, host: &mut dyn Host, on_event: &mut dyn FnMut(TurnEvent) -> Flow) -> Step {
        if turn.cancelled {
            return self.finish(turn, host);
        }
        if turn.report.rounds >= self.limits.max_rounds {
            if turn.report.answer.is_empty() && turn.report.error.is_none() && turn.report.stopped.is_none() {
                turn.report.stopped = Some(Stop::RoundLimit);
            }
            return self.finish(turn, host);
        }
        turn.report.rounds += 1;
        if on_event(TurnEvent::Round(turn.report.rounds)) == Flow::Stop {
            turn.cancelled = true;
            return self.finish(turn, host);
        }
        let system = self.system_text(host);
        let tools: Vec<_> = self.policy.tools(&turn.table).into_iter().cloned().collect();
        let mut request = Request::new(self.model.clone(), self.messages.clone())
            .with_system(system)
            .with_tools(tools)
            .with_max_tokens(self.limits.max_output_tokens);
        if let Some(e) = self.effort {
            request = request.with_effort(e);
        }
        Step::Model(request)
    }

    fn run_pending(&mut self, turn: &mut Turn, host: &mut dyn Host, verdict: Verdict, on_event: &mut dyn FnMut(TurnEvent) -> Flow) -> Step {
        let Some(Pending { uses, planned }) = turn.pending.take() else { return self.next_round(turn, host, on_event) };
        let mut results = Vec::with_capacity(uses.len());
        for ((id, name, input), (_, decision)) in uses.into_iter().zip(planned) {
            let outcome = if turn.cancelled {
                ToolOutcome::error_for(&id, &name, "cancelled by the user")
            } else if let Decision::Refuse(reason) = &decision {
                turn.report.refused += 1;
                ToolOutcome::error_for(&id, &name, reason.clone())
            } else if decision == Decision::Confirm && verdict != Verdict::Approve {
                turn.report.declined += 1;
                let reason = match &verdict {
                    Verdict::Decline(Some(r)) => format!("the user declined this change: {r}"),
                    _ => "the user declined this change; do not retry it, ask what they want instead".into(),
                };
                ToolOutcome::error_for(&id, &name, reason)
            } else if turn.tool_calls >= self.limits.max_tool_calls {
                turn.report.stopped = Some(Stop::ToolCallLimit);
                ToolOutcome::error_for(
                    &id,
                    &name,
                    format!("tool call limit of {} reached for this turn; answer with what you have", self.limits.max_tool_calls),
                )
            } else {
                turn.tool_calls += 1;
                if on_event(TurnEvent::ToolStart { name: name.clone(), input: input.clone() }) == Flow::Stop {
                    turn.cancelled = true;
                    ToolOutcome::error_for(&id, &name, "cancelled by the user")
                } else {
                    let o = run_tool(host, &turn.table, &turn.catalog, &id, &name, &input);
                    if on_event(TurnEvent::ToolDone(o.clone())) == Flow::Stop {
                        turn.cancelled = true;
                    }
                    o
                }
            };
            turn.report.tool_calls.push(ToolCall { name: name.clone(), input, ok: !outcome.is_error, summary: summarize(&outcome.text) });
            let with_images = !outcome.images.is_empty() && turn.images + outcome.images.len() <= self.limits.max_images;
            if with_images {
                turn.images += outcome.images.len();
            }
            results.push(outcome.block_with_images(with_images));
        }
        self.messages.push(Message::tool_results(results));
        self.next_round(turn, host, on_event)
    }

    fn finish(&mut self, turn: &mut Turn, host: &mut dyn Host) -> Step {
        turn.done = true;
        if turn.cancelled {
            turn.report.stopped = Some(Stop::Cancelled);
            // A dangling tool call would make the next request invalid: answer it.
            self.close_open_tool_calls();
        }
        if turn.report.error.is_some() {
            self.close_open_tool_calls();
        }
        turn.report.commands = host.session().journal.iter().skip(turn.journal_mark).map(|(id, _)| id.clone()).collect();
        (turn.report.touched_slides, turn.report.removed_slides) = slides_touched(host.session(), &turn.slide_mark);
        turn.report.undo_label = fold_history(host.session_mut(), turn.undo_mark.clone(), &turn.request_text);
        Step::Done(turn.report.clone())
    }

    /// Pictures from earlier turns are not worth their tokens: the text of each result stays.
    fn strip_old_images(&mut self) {
        for m in &mut self.messages {
            for b in &mut m.content {
                if let ContentBlock::ToolResult { images, .. } = b
                    && !images.is_empty()
                {
                    images.clear();
                }
            }
            m.content.retain(|b| !matches!(b, ContentBlock::Image { .. }));
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
        let results = ids
            .into_iter()
            .map(|id| ContentBlock::ToolResult { tool_use_id: id, content: "cancelled".into(), is_error: true, images: vec![] })
            .collect();
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

/// A line or two of a result: JSON results compact (`{"id":291,"index":8}`), text as its
/// first lines, at most 160 characters.
pub fn summarize(text: &str) -> String {
    let line: String = match serde_json::from_str::<Value>(text) {
        Ok(v) => v.to_string(),
        Err(_) => text.lines().take(2).collect::<Vec<_>>().join(" "),
    };
    let n = line.chars().count();
    if n <= 160 { line } else { line.chars().take(159).chain(std::iter::once('…')).collect() }
}

/// The active document's identity and its document pointer before a turn: the first undo entry
/// the turn pushes holds exactly this document.
#[derive(Clone)]
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
