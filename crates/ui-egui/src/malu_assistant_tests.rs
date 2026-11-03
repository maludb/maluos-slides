//! MaluOS: the assistant pane driven like a person would, with a scripted model.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use maluos_llm::mock::MockClient;
use maluos_llm::{Completion, ContentBlock, StopReason, Usage};
use maluos_slides_engine::Session;
use serde_json::{Value, json};

use crate::malu_assistant::{self, Entry};
use crate::{Services, SlideApp};

fn calls(blocks: Vec<ContentBlock>) -> Completion {
    Completion {
        model: "mock".into(),
        content: blocks,
        stop_reason: StopReason::ToolUse,
        usage: Usage { input_tokens: 50, output_tokens: 5, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
    }
}

fn answer(text: &str) -> Completion {
    Completion {
        model: "mock".into(),
        content: vec![ContentBlock::Text { text: text.into() }],
        stop_reason: StopReason::EndTurn,
        usage: Usage { input_tokens: 50, output_tokens: 5, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
    }
}

fn tool_use(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse { id: id.into(), name: name.into(), input }
}

/// A new deck, the pane open on a scripted model.
fn harness(mock: Arc<MockClient>, mode: maluos_slides_assist::Mode) -> Harness<'static, SlideApp> {
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    app.malu = malu_assistant::State::with_client(mock, "mock-model");
    app.malu.mode = mode;
    app.ui.pane = Some(malu_assistant::PANE.into());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn slides(h: &Harness<'_, SlideApp>) -> usize {
    h.state().session.active().unwrap().doc.slides.len()
}

/// Steps frames until the turn ends (the worker thread answers at once with the mock).
fn settle(h: &mut Harness<'_, SlideApp>) {
    for _ in 0..200 {
        h.step();
        if !h.state().malu.busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the turn never ended");
}

fn last_assistant(h: &Harness<'_, SlideApp>) -> (String, Vec<String>) {
    h.state()
        .malu
        .transcript
        .iter()
        .rev()
        .find_map(|e| match e {
            Entry::Assistant { text, calls, .. } => Some((text.clone(), calls.iter().map(|c| c.name.clone()).collect())),
            _ => None,
        })
        .unwrap()
}

#[test]
fn the_command_and_the_shortcut_open_the_pane_with_the_box_focused() {
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    app.malu = malu_assistant::State::with_client(Arc::new(MockClient::new()), "mock");
    assert!(app.ui.pane.is_none());
    app.run("assist.ask", json!({"text": "hello"})).unwrap();
    assert_eq!(app.ui.pane.as_deref(), Some("assistant"));
    assert!(app.malu.focus_input);
    assert_eq!(app.malu.input, "hello");
    app.ui.pane = None;
    assert!(crate::menus::ui_shortcut(&mut app, egui::Key::A, maluos_slides_engine::Mods { shift: true, alt: false, cmd: true }));
    assert_eq!(app.ui.pane.as_deref(), Some("assistant"));
    // The pane shows up in the window with its title and the settings.
    app.malu.mode = maluos_slides_assist::Mode::Autopilot;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    h.get_by_label("Malu");
    h.get_by_label("Send");
}

#[test]
fn autopilot_turn_adds_a_slide_and_undo_reverts_it() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("a", "add_slide", json!({"layout": "titleOnly", "title": "Thank you"}))]));
    mock.push(answer("Added a closing slide."));
    let mut h = harness(mock.clone(), maluos_slides_assist::Mode::Autopilot);
    h.state_mut().malu.input = "add a closing slide".into();
    let ctx = h.ctx.clone();
    malu_assistant::send(h.state_mut(), &ctx);
    assert!(h.state().malu.busy());
    settle(&mut h);
    assert_eq!(slides(&h), 2);
    let (text, names) = last_assistant(&h);
    assert_eq!(text, "Added a closing slide.");
    assert_eq!(names, vec!["add_slide"]);
    let report = h.state().malu.last_report().unwrap().clone();
    assert_eq!(report.undo_label.as_deref(), Some("Assistant: add a closing slide"));
    assert_eq!(report.touched_slides, vec![1]);
    assert_eq!(h.state().session.active().unwrap().history.undo.len(), 1);
    // The prompt carried the deck and the mode.
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 2);
    let system = reqs[0].system.as_deref().unwrap();
    assert!(system.contains("Mode: autopilot") && system.contains("# The open presentation"), "{}", &system[..200]);
    // The transcript's Undo button reverts the turn.
    h.run_steps(2);
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(slides(&h), 1);
    // A second turn continues the same conversation.
    mock.push(answer("Still here."));
    h.state_mut().malu.input = "thanks".into();
    malu_assistant::send(h.state_mut(), &ctx);
    settle(&mut h);
    assert_eq!(last_assistant(&h).0, "Still here.");
    assert_eq!(mock.requests()[2].messages.len(), 5, "user, call, result, answer, user");
}

#[test]
fn confirm_mode_shows_the_plan_and_approve_applies_it() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("a", "run_command", json!({"command": "slide.new", "params": {"layout": "blank"}}))]));
    mock.push(answer("Done."));
    let mut h = harness(mock, maluos_slides_assist::Mode::Confirm);
    h.state_mut().malu.input = "add a blank slide".into();
    let ctx = h.ctx.clone();
    malu_assistant::send(h.state_mut(), &ctx);
    for _ in 0..200 {
        h.step();
        if h.state().malu.pending_plan.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let plan = h.state().malu.pending_plan.clone().expect("a plan");
    assert_eq!(plan.calls.len(), 1);
    assert!(plan.describe().contains("slide.new — New Slide"), "{}", plan.describe());
    assert_eq!(slides(&h), 1, "nothing applied before approval");
    h.run_steps(2);
    h.get_by_label("Approve").click();
    settle(&mut h);
    assert_eq!(slides(&h), 2);
    assert_eq!(last_assistant(&h).0, "Done.");
    // The same through the command, for scripts: a plan is answered on the next frame.
    assert!(h.state_mut().run("assist.plan", json!({"verdict": "approve"})).is_err(), "no plan waiting");
    assert_eq!(h.state().malu.last_report().unwrap().plans, 1);
}

#[test]
fn confirm_mode_decline_keeps_the_deck_and_stop_ends_the_turn() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("a", "add_slide", json!({"layout": "blank"}))]));
    mock.push(answer("Understood, nothing changed."));
    let mut h = harness(mock.clone(), maluos_slides_assist::Mode::Confirm);
    let ctx = h.ctx.clone();
    h.state_mut().malu.input = "add a blank slide".into();
    malu_assistant::send(h.state_mut(), &ctx);
    for _ in 0..200 {
        h.step();
        if h.state().malu.pending_plan.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    h.get_by_label("Decline").click();
    settle(&mut h);
    assert_eq!(slides(&h), 1);
    assert_eq!(last_assistant(&h).0, "Understood, nothing changed.");
    assert_eq!(h.state().malu.last_report().unwrap().declined, 1);
    assert!(h.state().session.active().unwrap().history.undo.is_empty());

    // Stop while a plan waits ends the turn as cancelled; the next turn still works.
    mock.push(calls(vec![tool_use("b", "add_slide", json!({"layout": "blank"}))]));
    mock.push(answer("ok"));
    h.state_mut().malu.input = "again".into();
    malu_assistant::send(h.state_mut(), &ctx);
    for _ in 0..200 {
        h.step();
        if h.state().malu.pending_plan.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    malu_assistant::stop(h.state_mut(), &ctx);
    settle(&mut h);
    assert_eq!(h.state().malu.last_report().unwrap().stopped, Some(maluos_slides_assist::Stop::Cancelled));
    assert!(matches!(h.state().malu.transcript.last(), Some(Entry::Note(n)) if n == "Stopped."));
    assert_eq!(slides(&h), 1);
}

#[test]
fn a_model_error_is_shown_and_the_pane_recovers() {
    let mock = Arc::new(MockClient::new());
    mock.push_error(maluos_llm::LlmError::Auth("bad key".into()));
    mock.push(answer("fine"));
    let mut h = harness(mock, maluos_slides_assist::Mode::Autopilot);
    let ctx = h.ctx.clone();
    h.state_mut().malu.input = "hello".into();
    malu_assistant::send(h.state_mut(), &ctx);
    settle(&mut h);
    assert!(matches!(h.state().malu.transcript.last(), Some(Entry::Note(n)) if n.contains("bad key")), "{:?}", h.state().malu.transcript.last());
    h.state_mut().malu.input = "hello again".into();
    malu_assistant::send(h.state_mut(), &ctx);
    settle(&mut h);
    assert_eq!(last_assistant(&h).0, "fine");
}

#[test]
fn a_quick_action_sends_its_request_for_the_current_slide() {
    let mock = Arc::new(MockClient::new());
    mock.push(calls(vec![tool_use("n", "run_command", json!({"command": "slide.notes", "params": {"text": "Start with the question."}}))]));
    mock.push(answer("Notes written."));
    let mut h = harness(mock.clone(), maluos_slides_assist::Mode::Autopilot);
    h.get_by_label("Write notes").click();
    h.run_steps(2);
    assert!(
        !h.state().malu.transcript.is_empty(),
        "nothing sent: input={:?} requested={} busy={}",
        h.state().malu.input,
        h.state().malu.send_requested,
        h.state().malu.busy()
    );
    settle(&mut h);
    assert_eq!(last_assistant(&h).0, "Notes written.");
    assert_eq!(h.state().session.active().unwrap().doc.slides[0].notes_text(), "Start with the question.");
    let sent = match &h.state().malu.transcript[0] {
        Entry::User(t) => t.clone(),
        other => panic!("{other:?}"),
    };
    assert!(sent.starts_with("Write speaker notes for this slide"), "{sent}");
    assert!(matches!(&mock.requests()[0].messages[0].content[0], ContentBlock::Text { text } if text == &sent));
}

/// MaluOS step 4.4: presenter view shows the timing hint and the cue line.
#[test]
fn presenter_view_shows_timing_and_a_cue_line() {
    let mut s = Session::new();
    maluos_slides_engine::sample::open_sample(&mut s).unwrap();
    s.execute("slide.go", &json!({"index": 1})).unwrap();
    let long_notes = "Welcome to the science part. Start with the Sun: the solar wind is a stream of charged particles that reaches Earth in two to three days. Then the magnetosphere, which guides them to the poles. Keep the physics light; the pictures carry it. Mention the colours only after the collision slide.";
    s.execute("slide.notes", &json!({"text": long_notes})).unwrap();
    let timing = maluos_slides_assist::timing::DeckTiming::of(&s.active().unwrap().doc);
    let expected = format!(
        "≈ {} · deck {}",
        maluos_slides_assist::timing::clock(timing.slide(1).unwrap().seconds),
        maluos_slides_assist::timing::clock(timing.total_seconds)
    );
    let cue = maluos_slides_assist::timing::cue(long_notes, 30).unwrap();
    let mut app = SlideApp::new(s, Services::default());
    app.ui.pane = None;
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.state_mut().run("show.presenter", json!({"on": true})).unwrap();
    h.state_mut().run("show.start", json!({"from": 1})).unwrap();
    h.run_steps(6);
    h.get_by_label(expected.as_str());
    h.get_by_label(cue.as_str());
    if let Some(dir) = std::env::var_os("MALUOS_SLIDES_SHOT_DIR")
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(&dir).join("4.4-presenter.png")).unwrap();
    }
    h.state_mut().run("show.end", json!({})).unwrap();
}

/// MaluOS step 4.6: the pane lists what is remembered about the deck and Forget drops a note
/// through `document.tags`.
#[test]
fn the_pane_lists_remembered_notes_and_forgets_one() {
    let mock = Arc::new(MockClient::new());
    let mut h = harness(mock, maluos_slides_assist::Mode::Autopilot);
    let mut memory = maluos_slides_assist::memory::DeckMemory::default();
    memory.remember("For the executive board.", "2026-11-03").unwrap();
    memory.remember("Ten minutes at most.", "2026-11-03").unwrap();
    h.state_mut().run("document.tags", memory.tag_params()).unwrap();
    h.run_steps(2);
    h.get_by_label("Remembered (2)").click();
    h.run_steps(2);
    assert!(h.query_by_label_contains("Ten minutes at most.").is_some());
    h.get_all_by_label("Forget").next().unwrap().click();
    h.run_steps(2);
    let m = maluos_slides_assist::memory::DeckMemory::from_tags(&h.state().session.active().unwrap().doc.tags);
    assert_eq!(m.notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), vec!["Ten minutes at most."]);
    assert!(h.query_by_label("Remembered (1)").is_some());
}
