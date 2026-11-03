//! MaluOS: the assistant pane, "Ask Malu" (fork code; `docs/maluos-slides/plan.md` step 3.6).
//!
//! The pane is thin: it shows the conversation and drives `maluos_slides_assist::Assistant`
//! one step at a time from the frame loop. The model call runs on a worker thread that streams
//! back through a channel; every change to the deck happens on the UI thread through
//! `SlideApp::run`, so it is an ordinary command with the app's own undo, status and repaint.
//! A plan the mode wants approved is shown in the pane with Approve and Decline buttons.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Align, CornerRadius, Key, Layout, Modifiers, RichText, Ui};
#[cfg(not(target_arch = "wasm32"))]
use maluos_llm::Event;
use maluos_llm::{Completion, Flow, LlmClient, LlmError};
use maluos_slides_assist::{Assistant, Backend, Host, Mode, Plan, Step, ToolCall, Turn, TurnEvent, TurnReport, Verdict};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::SlideApp;
use crate::theme::{self, Tokens};

/// The pane's name in `view.pane`.
pub const PANE: &str = "assistant";

pub use maluos_slides_assist::providers::Choice as ProviderChoice;

/// What is saved between runs (`assistant.json` beside `ui.json`): never a key.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub provider: ProviderChoice,
    pub model: String,
    pub mode: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { provider: ProviderChoice::Anthropic, model: ProviderChoice::Anthropic.default_model().into(), mode: Mode::Confirm.as_str().into() }
    }
}

/// One item of the conversation as the pane shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User(String),
    Assistant {
        text: String,
        thinking: String,
        calls: Vec<ToolCall>,
        /// Set when the turn ended.
        report: Option<TurnReport>,
    },
    /// Something the pane tells the user (an error, a cancel).
    Note(String),
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Msg {
    Text(String),
    Thinking(String),
    Done(Result<Completion, LlmError>),
}

struct Worker {
    rx: Receiver<Msg>,
    stop: Arc<AtomicBool>,
    generation: u64,
}

/// Pane state (nothing here is part of the document).
pub struct State {
    pub provider: ProviderChoice,
    pub model: String,
    pub mode: Mode,
    pub input: String,
    pub transcript: Vec<Entry>,
    /// Open the pane with the input focused (Ctrl+Shift+A, the ribbon button).
    pub focus_input: bool,
    /// Show the model's reasoning under each answer.
    pub show_thinking: bool,
    /// The plan waiting for the user.
    pub pending_plan: Option<Plan>,
    /// Send the box's text on the next frame (`assist.ask {text, send: true}` from a script).
    pub send_requested: bool,
    /// Answer the waiting plan on the next frame (`assist.plan {verdict}` from a script).
    pub verdict_requested: Option<Verdict>,
    assistant: Option<Assistant>,
    custom_client: Option<Arc<dyn LlmClient>>,
    turn: Option<Turn>,
    worker: Option<Worker>,
    generation: u64,
    config_loaded: bool,
    config_dirty: bool,
    /// Why the last send could not start (no key, no model).
    pub problem: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            provider: ProviderChoice::Anthropic,
            model: ProviderChoice::Anthropic.default_model().into(),
            mode: Mode::Confirm,
            input: String::new(),
            transcript: vec![],
            focus_input: false,
            show_thinking: false,
            pending_plan: None,
            send_requested: false,
            verdict_requested: None,
            assistant: None,
            custom_client: None,
            turn: None,
            worker: None,
            generation: 0,
            config_loaded: true,
            config_dirty: false,
            problem: None,
        }
    }
}

impl State {
    /// A pane that answers through `client` (tests, embedding hosts); nothing is read from the
    /// environment or saved.
    pub fn with_client(client: Arc<dyn LlmClient>, model: &str) -> Self {
        State { provider: ProviderChoice::Custom, model: model.into(), custom_client: Some(client), ..State::default() }
    }

    /// A pane that loads `assistant.json` from the app's config folder on first use.
    pub fn from_config() -> Self {
        State { config_loaded: false, ..State::default() }
    }

    /// A turn is running (a model call in flight, or a plan waiting).
    pub fn busy(&self) -> bool {
        self.turn.is_some()
    }

    pub fn waiting_for_model(&self) -> bool {
        self.worker.is_some()
    }

    /// The last answer's report, for tests and the status line.
    pub fn last_report(&self) -> Option<&TurnReport> {
        self.transcript.iter().rev().find_map(|e| match e {
            Entry::Assistant { report: Some(r), .. } => Some(r),
            _ => None,
        })
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            if let Some(a) = &mut self.assistant {
                a.set_mode(mode);
            }
            self.config_dirty = true;
        }
    }

    fn set_provider(&mut self, provider: ProviderChoice) {
        if self.provider != provider {
            self.provider = provider;
            self.model = provider.default_model().into();
            self.assistant = None;
            self.config_dirty = true;
        }
    }

    fn set_model(&mut self, model: String) {
        if self.model != model {
            self.model = model;
            if let Some(a) = &mut self.assistant {
                a.model = self.model.clone();
            }
            self.config_dirty = true;
        }
    }

    /// Where the key comes from, or why there is none.
    pub fn provider_status(&self) -> (bool, String) {
        self.provider.status()
    }

    fn client(&self) -> Result<Arc<dyn LlmClient>, String> {
        if let Some(c) = &self.custom_client {
            return Ok(c.clone());
        }
        self.provider.client()
    }

    fn assistant(&mut self) -> Result<&mut Assistant, String> {
        if self.assistant.is_none() {
            let client = self.client()?;
            let a = Assistant::new(client, self.model.clone()).with_mode(self.mode);
            self.assistant = Some(a);
        }
        self.assistant.as_mut().ok_or_else(|| "no assistant".to_string())
    }
}

// ---------- config ----------

const CONFIG_FILE: &str = "assistant.json";

fn config_dir() -> Option<std::path::PathBuf> {
    if std::env::var_os("MALUOS_SLIDES_NO_PREFS").is_some() {
        return None;
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|d| d.join("maluos-slides"))
}

fn load_config(state: &mut State) {
    state.config_loaded = true;
    let saved =
        config_dir().and_then(|dir| std::fs::read_to_string(dir.join(CONFIG_FILE)).ok()).and_then(|t| serde_json::from_str::<Config>(&t).ok());
    let Some(c) = saved else {
        state.provider = maluos_slides_assist::providers::default_choice();
        state.model = state.provider.default_model().into();
        return;
    };
    state.provider = c.provider;
    state.model = if c.model.trim().is_empty() { c.provider.default_model().into() } else { c.model };
    state.mode = Mode::parse(&c.mode).unwrap_or(Mode::Confirm);
}

fn save_config(state: &mut State) {
    state.config_dirty = false;
    if state.custom_client.is_some() {
        return;
    }
    let Some(dir) = config_dir() else { return };
    let c = Config { provider: state.provider, model: state.model.clone(), mode: state.mode.as_str().into() };
    let Ok(text) = serde_json::to_string_pretty(&c) else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Err(e) = std::fs::write(dir.join(CONFIG_FILE), text) {
        log::warn!("assistant.json: {e}");
    }
}

// ---------- the host: the app as the assistant's backend ----------

struct AppHost<'a> {
    app: &'a mut SlideApp,
}

impl Backend for AppHost<'_> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let s = |k: &str| params.get(k).and_then(Value::as_str).map(String::from);
        match method {
            "engine.execute" | "command" | "ui.menu.invoke" => {
                let id = s("command").or_else(|| s("id")).ok_or("missing `command`")?;
                let p = params.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
                self.app.run(&id, p)
            }
            "engine.commands" => Ok(serde_json::to_value(self.app.session.commands()).unwrap_or_default()),
            "document.inspect" => self.app.run("document.inspect", json!({})),
            "ui.render" | "app.renderSlide" => {
                let d = self.app.session.active().ok_or("no presentation open")?;
                let i = params.get("slide").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(d.selection.slide);
                if i >= d.doc.slides.len() {
                    return Err(format!("no slide {i}"));
                }
                let scale = params.get("scale").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.05, 8.0);
                let (png, w, h) =
                    maluos_slides_engine::cmd::file::render_png(&d.doc, i, scale, params.get("edit").and_then(Value::as_bool).unwrap_or(false));
                Ok(json!({"png": maluos_slides_engine::cmd::base64_encode(&png), "width": w, "height": h, "slide": i}))
            }
            "ui.tool.select" => {
                let name = s("tool").ok_or("missing `tool`")?;
                let kind =
                    crate::control::tool_kind(&name, params.get("preset").and_then(Value::as_str)).ok_or_else(|| format!("unknown tool `{name}`"))?;
                self.app.session.set_tool(kind);
                Ok(json!({"tool": name}))
            }
            "ui.key" => {
                let key = s("key").ok_or("missing `key`")?;
                let m = maluos_slides_engine::Mods {
                    shift: params.get("shift").and_then(Value::as_bool).unwrap_or(false),
                    alt: params.get("alt").and_then(Value::as_bool).unwrap_or(false),
                    cmd: params.get("cmd").and_then(Value::as_bool).unwrap_or(false) || params.get("ctrl").and_then(Value::as_bool).unwrap_or(false),
                };
                self.app.session.key(&key, m).map_err(|e| e.to_string())
            }
            "ui.text" => {
                let t = s("text").ok_or("missing `text`")?;
                self.app.session.type_text(&t).map_err(|e| e.to_string())
            }
            "ui.pointer" => {
                let events = params.get("events").and_then(Value::as_array).ok_or("missing `events`")?;
                let mut last = Value::Null;
                for e in events {
                    let kind = match e.get("kind").and_then(Value::as_str).unwrap_or("") {
                        "down" => maluos_slides_engine::PointerKind::Down,
                        "drag" => maluos_slides_engine::PointerKind::Drag,
                        "up" => maluos_slides_engine::PointerKind::Up,
                        "move" => maluos_slides_engine::PointerKind::Move,
                        "doubleclick" => maluos_slides_engine::PointerKind::DoubleClick,
                        "tripleclick" => maluos_slides_engine::PointerKind::TripleClick,
                        "contextclick" => maluos_slides_engine::PointerKind::ContextClick,
                        other => return Err(format!("unknown pointer kind `{other}`")),
                    };
                    let x = e.get("x").and_then(Value::as_f64).ok_or("pointer event needs `x`")?;
                    let y = e.get("y").and_then(Value::as_f64).ok_or("pointer event needs `y`")?;
                    let m = e.get("mods").unwrap_or(&Value::Null);
                    let mods = maluos_slides_engine::Mods {
                        shift: m.get("shift").and_then(Value::as_bool).unwrap_or(false),
                        alt: m.get("alt").and_then(Value::as_bool).unwrap_or(false),
                        cmd: m.get("cmd").and_then(Value::as_bool).unwrap_or(false),
                    };
                    last = self
                        .app
                        .session
                        .pointer(maluos_slides_engine::PointerEvent { kind, x, y, mods, tol: 3.0, pressure: None })
                        .map_err(|e| e.to_string())?;
                }
                Ok(json!({"result": last}))
            }
            other => Err(format!("`{other}` is not available from the assistant pane")),
        }
    }

    /// Screenshots go through the frame loop asynchronously; `render_slide` serves the model.
    fn has_ui(&self) -> bool {
        false
    }

    fn describe(&self) -> String {
        "the MaluOS Slides window".into()
    }
}

impl Host for AppHost<'_> {
    fn session(&self) -> &maluos_slides_engine::Session {
        &self.app.session
    }
    fn session_mut(&mut self) -> &mut maluos_slides_engine::Session {
        &mut self.app.session
    }
}

// ---------- driving a turn ----------

/// Sends what is in the input box. Nothing happens while a turn runs or the box is empty.
pub fn send(app: &mut SlideApp, ctx: &egui::Context) {
    let text = app.malu.input.trim().to_string();
    if text.is_empty() || app.malu.busy() {
        return;
    }
    if app.session.active().is_none() {
        app.malu.problem = Some("Open or create a presentation first.".into());
        return;
    }
    if let Err(e) = app.malu.assistant() {
        app.malu.problem = Some(e);
        return;
    }
    app.malu.problem = None;
    app.malu.input.clear();
    app.malu.transcript.push(Entry::User(text.clone()));
    app.malu.transcript.push(Entry::Assistant { text: String::new(), thinking: String::new(), calls: vec![], report: None });
    let Some(mut assistant) = app.malu.assistant.take() else { return };
    let mut events = Vec::new();
    let (turn, step) = {
        let mut host = AppHost { app };
        assistant.begin(&mut host, &text, &mut |e| {
            events.push(e);
            Flow::Continue
        })
    };
    app.malu.assistant = Some(assistant);
    app.malu.turn = Some(turn);
    apply_events(app, events);
    handle_step(app, ctx, step);
}

/// Stops the running turn: the model call is abandoned, a waiting plan is cancelled.
pub fn stop(app: &mut SlideApp, ctx: &egui::Context) {
    if let Some(w) = &app.malu.worker {
        w.stop.store(true, Ordering::Relaxed);
    }
    app.malu.worker = None;
    app.malu.pending_plan = None;
    let (Some(mut assistant), Some(mut turn)) = (app.malu.assistant.take(), app.malu.turn.take()) else { return };
    let step = {
        let mut host = AppHost { app };
        assistant.cancel(&mut turn, &mut host)
    };
    app.malu.assistant = Some(assistant);
    app.malu.turn = Some(turn);
    handle_step(app, ctx, step);
}

/// The user answered the plan.
pub fn answer_plan(app: &mut SlideApp, ctx: &egui::Context, verdict: Verdict) {
    app.malu.pending_plan = None;
    let (Some(mut assistant), Some(mut turn)) = (app.malu.assistant.take(), app.malu.turn.take()) else { return };
    let mut events = Vec::new();
    let step = {
        let mut host = AppHost { app };
        assistant.on_verdict(&mut turn, &mut host, verdict, &mut |e| {
            events.push(e);
            Flow::Continue
        })
    };
    app.malu.assistant = Some(assistant);
    app.malu.turn = Some(turn);
    apply_events(app, events);
    handle_step(app, ctx, step);
}

/// Called every frame: takes what the worker streamed and advances the turn when the model
/// answered.
pub fn poll(app: &mut SlideApp, ctx: &egui::Context) {
    if app.malu.config_dirty {
        save_config(&mut app.malu);
    }
    let Some(worker) = app.malu.worker.as_ref() else { return };
    if worker.generation != app.malu.generation {
        app.malu.worker = None;
        return;
    }
    let mut done = None;
    let mut text = String::new();
    let mut thinking = String::new();
    while let Ok(m) = worker.rx.try_recv() {
        match m {
            Msg::Text(t) => text.push_str(&t),
            Msg::Thinking(t) => thinking.push_str(&t),
            Msg::Done(r) => {
                done = Some(r);
                break;
            }
        }
    }
    if !text.is_empty() || !thinking.is_empty() {
        append_streaming(app, &text, &thinking);
    }
    if app.malu.worker.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    let Some(result) = done else { return };
    app.malu.worker = None;
    let (Some(mut assistant), Some(mut turn)) = (app.malu.assistant.take(), app.malu.turn.take()) else { return };
    let mut events = Vec::new();
    let step = {
        let mut host = AppHost { app };
        assistant.on_completion(&mut turn, &mut host, result, &mut |e| {
            events.push(e);
            Flow::Continue
        })
    };
    app.malu.assistant = Some(assistant);
    app.malu.turn = Some(turn);
    apply_events(app, events);
    handle_step(app, ctx, step);
}

fn handle_step(app: &mut SlideApp, ctx: &egui::Context, step: Step) {
    match step {
        Step::Model(request) => {
            let Some(assistant) = app.malu.assistant.as_ref() else { return };
            let call = assistant.model_call();
            let (tx, rx) = channel();
            let stop = Arc::new(AtomicBool::new(false));
            app.malu.generation += 1;
            let generation = app.malu.generation;
            app.malu.worker = Some(Worker { rx, stop: stop.clone(), generation });
            spawn_model_call(call, request, tx, stop, ctx.clone());
            ctx.request_repaint();
        }
        Step::Confirm(plan) => {
            app.malu.pending_plan = Some(plan);
            ctx.request_repaint();
        }
        Step::Done(report) => {
            app.malu.turn = None;
            app.malu.pending_plan = None;
            finish_entry(app, report);
            ctx.request_repaint();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_model_call(call: maluos_slides_assist::ModelCall, request: maluos_llm::Request, tx: Sender<Msg>, stop: Arc<AtomicBool>, ctx: egui::Context) {
    std::thread::Builder::new()
        .name("malu-model".into())
        .spawn(move || {
            let result = call.run(&request, &mut |e| {
                if stop.load(Ordering::Relaxed) {
                    return Flow::Stop;
                }
                match e {
                    Event::TextDelta(t) => {
                        let _ = tx.send(Msg::Text(t));
                        ctx.request_repaint();
                    }
                    Event::ThinkingDelta(t) => {
                        let _ = tx.send(Msg::Thinking(t));
                        ctx.request_repaint();
                    }
                    _ => {}
                }
                Flow::Continue
            });
            let _ = tx.send(Msg::Done(result));
            ctx.request_repaint();
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::error!("assistant thread: {e}"));
}

#[cfg(target_arch = "wasm32")]
fn spawn_model_call(
    _call: maluos_slides_assist::ModelCall,
    _request: maluos_llm::Request,
    tx: Sender<Msg>,
    _stop: Arc<AtomicBool>,
    _ctx: egui::Context,
) {
    let _ = tx.send(Msg::Done(Err(LlmError::Transport("the assistant needs the desktop app".into()))));
}

/// Markdown emphasis as plain text: labels do not render `**bold**` or `*italic*`.
fn plain(text: &str) -> String {
    let mut out = text.replace("**", "");
    if out.contains("\n- ") || out.starts_with("- ") {
        out = out.replace("\n- ", "\n• ");
        if let Some(rest) = out.strip_prefix("- ") {
            out = format!("• {rest}");
        }
    }
    out
}

fn current_entry(app: &mut SlideApp) -> Option<&mut Entry> {
    app.malu.transcript.iter_mut().rev().find(|e| matches!(e, Entry::Assistant { report: None, .. }))
}

fn append_streaming(app: &mut SlideApp, text: &str, thinking: &str) {
    if let Some(Entry::Assistant { text: t, thinking: th, .. }) = current_entry(app) {
        t.push_str(text);
        th.push_str(thinking);
    }
}

fn apply_events(app: &mut SlideApp, events: Vec<TurnEvent>) {
    for e in events {
        match e {
            TurnEvent::Round(n) if n > 1 => {
                // A new round: the text so far was the model thinking aloud before its calls;
                // keep it, separated, so the final answer reads on its own.
                if let Some(Entry::Assistant { text, .. }) = current_entry(app)
                    && !text.is_empty()
                    && !text.ends_with('\n')
                {
                    text.push_str("\n\n");
                }
            }
            TurnEvent::ToolDone(o) => {
                if let Some(Entry::Assistant { calls, .. }) = current_entry(app) {
                    calls.push(ToolCall {
                        name: o.name.clone(),
                        input: Value::Null,
                        ok: !o.is_error,
                        summary: maluos_slides_assist::summarize(&o.text),
                    });
                }
            }
            _ => {}
        }
    }
}

fn finish_entry(app: &mut SlideApp, report: TurnReport) {
    let note = match (&report.error, &report.stopped) {
        (Some(e), _) => Some(format!("The model call failed: {e}")),
        (None, Some(maluos_slides_assist::Stop::Cancelled)) => Some("Stopped.".into()),
        (None, Some(maluos_slides_assist::Stop::RoundLimit)) => {
            Some("Stopped after the round limit; the deck may be half-changed (Undo reverts the turn).".into())
        }
        (None, Some(maluos_slides_assist::Stop::ToolCallLimit)) => Some("Reached the tool-call limit for one turn.".into()),
        (None, Some(maluos_slides_assist::Stop::Refusal)) => Some("The model declined this request.".into()),
        (None, Some(maluos_slides_assist::Stop::MaxTokens)) => Some("The answer was cut by the output limit.".into()),
        (None, None) => None,
    };
    if let Some(Entry::Assistant { text, calls, report: r, .. }) = current_entry(app) {
        if !report.answer.is_empty() && !text.ends_with(&report.answer) {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&report.answer);
        }
        // The report's calls carry the inputs; keep its view, it is complete.
        *calls = report.tool_calls.clone();
        *r = Some(report);
    }
    if let Some(n) = note {
        app.malu.transcript.push(Entry::Note(n));
    }
    if app.session.active().is_some() {
        app.set_status("Malu finished a turn");
    }
}

// ---------- the pane ----------

pub fn show(app: &mut SlideApp, ui: &mut Ui) {
    if !app.malu.config_loaded {
        load_config(&mut app.malu);
    }
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    poll(app, &ctx);
    if app.malu.send_requested {
        app.malu.send_requested = false;
        send(app, &ctx);
    }
    if let Some(v) = app.malu.verdict_requested.take()
        && app.malu.pending_plan.is_some()
    {
        answer_plan(app, &ctx, v);
    }

    settings_row(app, ui, &t);
    ui.add_space(4.0);
    let (ok, status) = app.malu.provider_status();
    ui.label(RichText::new(format!("{} · {}", app.malu.provider.label(), status)).small().color(if ok { t.text_dim } else { crate::icons::RED }));
    if let Some(p) = &app.malu.problem {
        ui.label(RichText::new(p).small().color(crate::icons::RED));
    }
    ui.add_space(6.0);

    // The conversation fills what the input box leaves.
    let input_h = 96.0;
    let avail = ui.available_height() - input_h;
    egui::ScrollArea::vertical().id_salt("malu_transcript").max_height(avail.max(80.0)).auto_shrink([false, false]).stick_to_bottom(true).show(
        ui,
        |ui| {
            transcript(app, ui, &ctx, &t);
        },
    );
    ui.add_space(6.0);
    quick_actions(app, ui, &ctx, &t);
    input_row(app, ui, &ctx, &t);
}

/// The usual slide requests as one click each (step 4.2): they fill the box and send.
pub const QUICK_ACTIONS: &[(&str, &str)] = &[
    ("Tighten title", "Tighten the title of this slide to six words at most."),
    ("Rewrite bullets", "Rewrite the bullets on this slide: at most five, parallel phrasing, under twelve words each; keep the facts."),
    ("Write notes", "Write speaker notes for this slide: three to six spoken sentences that add what the slide does not say."),
    ("Chart from table", "Suggest a chart for the table on this slide and insert it next to the table."),
    ("Fix layout", "Fix the layout of this slide: align the shapes, even out the spacing, and shrink any text that overflows."),
    (
        "Add a picture",
        "Add a picture that fits this slide from the office's image agent, placed where the slide has room; tell me if there is no image agent.",
    ),
    (
        "Q&A prep",
        "Prepare me for questions: six to ten likely audience questions with answers from the deck, on one hidden slide at the end titled Q&A prep.",
    ),
];

fn quick_actions(app: &mut SlideApp, ui: &mut Ui, ctx: &egui::Context, t: &Tokens) {
    if app.session.active().is_none() || app.malu.busy() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("This slide:").small().color(t.text_dim));
        for (label, request) in QUICK_ACTIONS {
            if ui.add(egui::Button::new(RichText::new(*label).small())).on_hover_text(*request).clicked() {
                app.malu.input = (*request).to_string();
                app.malu.send_requested = true;
                ctx.request_repaint();
            }
        }
    });
    ui.add_space(4.0);
}

fn settings_row(app: &mut SlideApp, ui: &mut Ui, _t: &Tokens) {
    let busy = app.malu.busy();
    ui.horizontal_wrapped(|ui| {
        let mut provider = app.malu.provider;
        if provider != ProviderChoice::Custom {
            egui::ComboBox::from_id_salt("malu_provider").selected_text(provider.label()).width(96.0).show_ui(ui, |ui| {
                for p in ProviderChoice::ALL {
                    ui.selectable_value(&mut provider, p, p.label());
                }
            });
            if !busy {
                app.malu.set_provider(provider);
            }
        }
        let mut model = app.malu.model.clone();
        let suggestions = app.malu.provider.suggestions();
        let r = egui::ComboBox::from_id_salt("malu_model").selected_text(model.clone()).width(150.0).show_ui(ui, |ui| {
            for m in suggestions {
                ui.selectable_value(&mut model, m.to_string(), *m);
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut model).hint_text("model name").desired_width(140.0));
        });
        r.response.on_hover_text("The model to ask; type any name your provider serves");
        if !busy {
            app.malu.set_model(model);
        }
        let mut mode = app.malu.mode;
        egui::ComboBox::from_id_salt("malu_mode")
            .selected_text(match mode {
                Mode::AnswerOnly => "Answer only",
                Mode::Confirm => "Confirm changes",
                Mode::Autopilot => "Autopilot",
            })
            .width(130.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut mode, Mode::AnswerOnly, "Answer only").on_hover_text("Reads and explains; changes nothing");
                ui.selectable_value(&mut mode, Mode::Confirm, "Confirm changes").on_hover_text("Shows each change as a plan to approve");
                ui.selectable_value(&mut mode, Mode::Autopilot, "Autopilot").on_hover_text("Applies changes at once; deletions still ask");
            });
        app.malu.set_mode(mode);
    });
}

fn transcript(app: &mut SlideApp, ui: &mut Ui, ctx: &egui::Context, t: &Tokens) {
    if app.malu.transcript.is_empty() {
        ui.label(
            RichText::new("Ask for a change to the open presentation, or a question about it. The selection is the target when you say \"this\".")
                .color(t.text_dim),
        );
        ui.add_space(4.0);
        for ex in ["Tighten the title on this slide", "Add a closing slide that thanks the audience", "Turn these bullets into a 2×3 table"] {
            if ui.add(egui::Button::new(RichText::new(ex).small()).frame(false)).clicked() {
                app.malu.input = ex.into();
                app.malu.focus_input = true;
            }
        }
        return;
    }
    let top_undo = app.session.active().and_then(|d| d.history.undo.last().map(|e| e.label.clone()));
    let mut undo_click = false;
    let mut toggle_thinking = false;
    let show_thinking = app.malu.show_thinking;
    let streaming = app.malu.waiting_for_model();
    let n = app.malu.transcript.len();
    for (i, entry) in app.malu.transcript.iter().enumerate() {
        match entry {
            Entry::User(text) => {
                ui.add_space(4.0);
                ui.label(RichText::new("You").font(theme::bold(12.0)).color(t.text_dim));
                ui.label(text);
            }
            Entry::Assistant { text, thinking, calls, report } => {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Malu").font(theme::bold(12.0)).color(t.accent));
                    if report.is_none() && i + 1 == n && streaming {
                        ui.add(egui::Spinner::new().size(12.0));
                    }
                });
                if !thinking.is_empty() {
                    let label = if show_thinking { "Hide reasoning" } else { "Show reasoning" };
                    if ui.add(egui::Button::new(RichText::new(label).small().color(t.text_dim)).frame(false)).clicked() {
                        toggle_thinking = true;
                    }
                    if show_thinking {
                        ui.label(RichText::new(thinking).small().color(t.text_dim));
                    }
                }
                if !text.is_empty() {
                    ui.label(plain(text));
                }
                for c in calls {
                    let mark = if c.ok { "✓" } else { "✗" };
                    let line = if c.summary.is_empty() { format!("{mark} {}", c.name) } else { format!("{mark} {} — {}", c.name, c.summary) };
                    ui.label(RichText::new(line).small().color(if c.ok { t.text_dim } else { crate::icons::RED }));
                }
                if let Some(r) = report {
                    ui.horizontal_wrapped(|ui| {
                        if !r.touched_slides.is_empty() || r.removed_slides > 0 {
                            let slides: Vec<String> = r.touched_slides.iter().map(|i| (i + 1).to_string()).collect();
                            let mut s = String::new();
                            if !slides.is_empty() {
                                s.push_str(&format!("Changed slide{} {}", if slides.len() == 1 { "" } else { "s" }, slides.join(", ")));
                            }
                            if r.removed_slides > 0 {
                                if !s.is_empty() {
                                    s.push_str("; ");
                                }
                                s.push_str(&format!("removed {}", r.removed_slides));
                            }
                            ui.label(RichText::new(s).small().color(t.text_dim));
                        }
                        if let Some(label) = &r.undo_label {
                            let can = top_undo.as_deref() == Some(label.as_str());
                            let b = ui.add_enabled(can, egui::Button::new(RichText::new("Undo").small()));
                            if b.on_hover_text(if can { "Revert this turn" } else { "Later changes sit on top; undo those first" }).clicked() {
                                undo_click = true;
                            }
                        }
                        if r.usage.input_tokens > 0 {
                            ui.label(
                                RichText::new(format!("{} in · {} out", r.usage.input_tokens, r.usage.output_tokens)).small().color(t.text_faint),
                            );
                        }
                    });
                }
            }
            Entry::Note(text) => {
                ui.add_space(2.0);
                ui.label(RichText::new(text).small().color(crate::icons::RED));
            }
        }
    }
    if toggle_thinking {
        app.malu.show_thinking = !app.malu.show_thinking;
    }
    if undo_click {
        let _ = app.run("edit.undo", json!({}));
    }
    if let Some(plan) = app.malu.pending_plan.clone() {
        ui.add_space(6.0);
        let frame = egui::Frame::NONE
            .fill(t.field_bg)
            .stroke(egui::Stroke::new(1.0, if plan.destructive() { crate::icons::RED } else { t.border }))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::same(8));
        frame.show(ui, |ui| {
            ui.label(
                RichText::new(if plan.destructive() { "Malu wants to make a destructive change:" } else { "Malu wants to:" }).font(theme::bold(12.0)),
            );
            for call in &plan.calls {
                ui.label(RichText::new(call.describe()).small().monospace());
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("Approve").fill(t.accent)).clicked() {
                    answer_plan(app, ctx, Verdict::Approve);
                }
                if ui.button("Decline").clicked() {
                    answer_plan(app, ctx, Verdict::Decline(None));
                }
                if ui.button("Stop").on_hover_text("End this turn").clicked() {
                    answer_plan(app, ctx, Verdict::Cancel);
                }
            });
        });
    }
}

fn input_row(app: &mut SlideApp, ui: &mut Ui, ctx: &egui::Context, t: &Tokens) {
    let id = egui::Id::new("malu_input");
    let busy = app.malu.busy();
    // Enter sends; Shift+Enter breaks the line. The key is taken before the box sees it.
    let had_focus = ctx.memory(|m| m.has_focus(id));
    let mut send_now = false;
    if had_focus && !busy && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
        send_now = true;
    }
    let edit = egui::TextEdit::multiline(&mut app.malu.input)
        .id(id)
        .hint_text("Ask Malu… (Enter sends, Shift+Enter for a new line)")
        .desired_rows(3)
        .desired_width(f32::INFINITY)
        .frame(
            egui::Frame::NONE
                .fill(t.field_bg)
                .stroke(egui::Stroke::new(1.0, if had_focus { t.accent } else { t.border }))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::same(6)),
        );
    let r = ui.add_enabled(!busy || app.malu.pending_plan.is_some(), edit);
    if app.malu.focus_input {
        r.request_focus();
        app.malu.focus_input = false;
    }
    ui.horizontal(|ui| {
        if busy {
            if ui.button("Stop").clicked() {
                stop(app, ctx);
            }
            ui.label(RichText::new(if app.malu.pending_plan.is_some() { "Waiting for your answer" } else { "Thinking…" }).small().color(t.text_dim));
        } else {
            let can = !app.malu.input.trim().is_empty();
            if ui.add_enabled(can, egui::Button::new("Send")).clicked() || (send_now && can) {
                send(app, ctx);
                app.malu.focus_input = true;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !app.malu.transcript.is_empty()
                && !busy
                && ui
                    .add(egui::Button::new(RichText::new("Clear").small()).frame(false))
                    .on_hover_text("Forget this conversation (the deck stays)")
                    .clicked()
            {
                app.malu.transcript.clear();
                if let Some(a) = &mut app.malu.assistant {
                    a.clear();
                }
            }
        });
    });
}
