# maluos-slides-assist: the assistant

> **Created:** 2026-11-02 · plan step 3.3 · decisions D9 (one table for MCP and the assistant), D11 (the pane)

`crates/maluos-slides-assist` is the fork's assistant crate. It sits with `mcp` at layer 6 and
depends on `maluos-slides-engine`, `maluos-slides-mcp` and `maluos-llm`; `ui-egui` will depend on
it for the pane (step 3.6). It never edits upstream crates: everything it knows about the
application it reads from the MCP server's table and the engine's command registry.

## One table (step 3.3)

| Piece | What it is | Source |
|---|---|---|
| `ToolTable` | The tools a model may call, as `maluos_llm::ToolDef` (name, description, JSON Schema) | `maluos_slides_mcp::tool_definitions()`, unchanged; `for_backend(has_ui)` leaves out the two app-only tools (`screenshot`, `call_app`) when there is no window; `is_read_only` carries the server's `readOnlyHint` for the safety rails (3.5) |
| `Catalog` | Every engine command: id, label, menu, shortcut, params doc, `undoable`, `journal`, enabled state | `Session::commands()` plus `command_specs()`; `Catalog::new()` is a fresh deck's view |
| `params_schema(doc)` | A JSON Schema generated from a command's params doc | `maluos_slides_engine::cmd::parse_params`: keys become properties, the key's text its description, a type only where the doc is unambiguous (`bool`, `[…]`, `text`, an index); a key is required only in a single-object doc, unmarked `?` and not an alternative (`{text \| body}`); multi-object docs (`{index} \| {id}`) require nothing and say so |
| `Catalog::validate(id, params)` | What runs before any `run_command`: the command must exist (a typo gets a "did you mean" within two edits) and every key must be documented | the schema's `additionalProperties: false`, enforced |
| `Catalog::prompt_text(filter, max_bytes)` | One line per command for a prompt, filtered by id or label, cut with a "… N more" marker | the entries |
| `run_tool(backend, table, catalog, tool_use_id, name, input)` | One tool call through an MCP `Backend` (`Headless` or `Remote`), returning a `ToolOutcome` whose `block()` is the `ContentBlock::ToolResult` for the next request | `maluos_slides_mcp::call_tool`; before it: unknown tool, app-only without a UI, and (for `run_command` and `batch`) validation of every command, so a bad second command leaves the first unrun |

Results are text joined from the server's text blocks, cut at `MAX_RESULT_BYTES` (64 KiB) with a
marker; images (`render_slide`) are kept on the outcome with a placeholder line in the text until
image inputs exist (4.2).

The MCP server keeps its own refusal of unknown tool arguments, so `add_slide {titel}` fails the
same way for both callers. What the assistant adds on top is the command-level check: the MCP
server passes `run_command` params straight to the engine, where an unknown key silently takes
the default; the assistant refuses it with the documented keys in the message.

## The context (step 3.2)

`Context::build(&session, &Budget)` writes the open deck as Markdown-ish text for the system
prompt, read straight from the engine state (reading is free; only edits go through commands):

1. A header: title and path, slide count, slide size, the theme with its heading and body fonts
   and the dark, light and six accent colours as hex, unsaved-changes flag, section count.
2. The selection: current slide, selected slides, the text being edited (shape, cell, caret or
   range with the selected text quoted), selected table cells, and every selected shape as one
   line (`ctrTitle placeholder id 258 "Title 1" at [120,88,720,188] text "…"`), named as the
   target of an edit.
3. The current slide, whole: title line with layout, id, hidden flag, animation count and
   transition; each text placeholder as `[obj id 263]` followed by its paragraphs as indented
   bullets (outline levels kept, so the model can rewrite one line); every other shape as one
   line from `shape_summary` (preset or kind, id, name, box, rotation, fill, table size and first
   rows, chart type and series count, group size, text, alt text); notes.
4. The outline: every slide the same way, with `### Section: name` headings where sections
   start, and the current slide as a one-line pointer.

`Budget` (default 24 KiB total, 1200 bytes per slide, 400 chars per shape text, 4000 on the
current slide, 300 for notes) is enforced slide by slide with markers the model can act on:
`… more shapes [cut: inspect_slide 7]`, `[body left out: inspect_slide 40]` once the total is
reached, and `… N more slides (inspect_document lists them)` past a margin. `Context::cut_slides`
lists what was cut. The current slide is written first and is never cut by the total.

`ContextCache::get` returns the last outline unless the document uid, its revision or the
selection changed, so a pane polling every frame builds nothing between edits.

## The loop (step 3.4)

`Assistant::ask(&mut host, text, on_event) -> TurnReport` is one user message:

1. The user message joins the conversation (`messages`, kept across turns; older turns are
   trimmed at user messages past `Limits::max_messages`, so a tool call never loses its result).
2. Each round builds a request: system text = `INSTRUCTIONS` + the command catalog listing
   (`Limits::catalog_bytes`, 32 KiB by default, 0 leaves it out) + the deck outline from
   [`Context`] rebuilt every round, so the model sees its own edits; tools = `ToolTable` for the
   host; `max_tokens` = `Limits::max_output_tokens`; `effort` when set.
3. The model streams; text and thinking reach `on_event` as `TurnEvent::Text` / `Thinking`.
   A retryable error (rate limit, overload, transport) is retried `Limits::retries` times with
   `retry_delay` doubling and `retry-after` honoured; any other error ends the turn with
   `report.error` set and the user message kept, so the user can retry without retyping.
4. Every tool call runs through [`run_tool`], so through `Session::execute` on the host, with
   `ToolStart` / `ToolDone` events; `Flow::Stop` from the callback cancels before the next call.
   Past `Limits::max_tool_calls` each call is answered with an error telling the model to answer
   with what it has. The results go back as one user turn of `tool_result` blocks.
5. The loop ends when the model answers in text, refuses, hits `max_tokens` without a call, or
   `Limits::max_rounds` is reached (`report.stopped` says which). A cancelled or failed turn
   answers any dangling tool call with an error so the conversation stays valid.
6. The undo entries the turn's commands pushed are folded into one labelled
   `Assistant: <request>` (found by the document pointer recorded before the turn; nothing
   happens when the deck is unchanged or another document was opened). `report.commands` is the
   command ids from the session journal, `report.usage` the summed tokens.

[`Host`] is what the loop runs against: an MCP `Backend` plus `session()` / `session_mut()` for
the outline and the history. `Headless` is one; the desktop app supplies its own (3.6).
`Remote` (a running app over the control channel) is not a host: it has no session to read.

`maluos-slides-cli ask` (step 3.7, [cli.md](cli.md)) runs one turn against a real model on a new or
opened deck, headless, and saves the result. Verified live with Ollama `glm-5.3-flash:cloud` on the
sample deck. The system text tells the model the outline already includes the turn's edits,
because without that it mistook its new slide for an existing one.

## Safety rails (step 3.5)

`Policy` (in `policy.rs`) decides what a tool call may do in the current `Mode`:

| Mode | Read-only calls | Changes | Destructive |
|---|---|---|---|
| answer-only | run | refused with a reason the model reads; only read-only tools, `run_command` (query commands) and `list_commands` are offered | refused |
| confirm (default) | run | shown as a plan, applied on approval | shown as a plan, applied on approval |
| autopilot | run | applied at once | shown as a plan, applied on approval |

A call's `Class` comes from the MCP `readOnlyHint` for the named tools, from the catalog for
`run_command` (query commands are read-only) and from the worst command of a `batch`.
`DESTRUCTIVE_COMMANDS` (`slide.delete`, `section.remove`, `section.removeAll`,
`master.deleteLayout`, `file.new`, `file.open`, `file.openBytes`, `file.close`, `file.revert`,
`file.adopt`, `file.recovery.open`) and `DESTRUCTIVE_TOOLS` (`new_presentation`,
`open_presentation`) are confirmed in every mode that allows changes; a test checks every name
against the catalog and the tool table. Saving and exporting are ordinary changes: writing files
is what those tools are for.

The loop classifies all calls of a round first and asks once per round with a `Plan`: each
`PlannedCall` names the tool, its arguments and the engine commands behind it (the real ones
for `run_command` and `batch`, the mapped one for the others), with `describe()` for a dialog
or terminal. `Confirm` answers it (`ApproveAll`, `DeclineAll`, any closure, the pane's dialog):
`Approve` runs the calls; `Decline(reason)` answers each with an error the model reads ("the
user declined this change: …; do not retry it, ask what they want instead"); `Cancel` ends the
turn. The mode's one-line instruction is in the system text. `TurnReport` counts `plans`,
`declined` and `refused`, and lists `touched_slides` (added or changed, by content, not pointer)
and `removed_slides`, which the autopilot answer shows.

## The pane (step 3.6)

`crates/ui-egui/src/malu_assistant.rs` is the fork's pane; the upstream crate carries only fenced
hooks (a `mod` line, the `malu` field on `SlideApp`, two UI commands, a shortcut row, one
title-bar button, one icon, two lines in the pane dispatcher, one poll call in the frame loop).

- **Opening it**: the spark button in the title bar next to Comments, `Ctrl+Shift+A`, or the
  command `assist.ask {text?, send?, toggle?}`; `view.pane {pane: "assistant"}` opens it too.
  The pane is titled "Malu".
- **Settings row**: provider (Anthropic, OpenAI, Ollama), model (a list of suggestions plus a
  text field), mode (Answer only, Confirm changes, Autopilot). A status line says where the key
  comes from (`key from ANTHROPIC_API_KEY`) or that there is none; Ollama needs no key
  (`MALUOS_SLIDES_OLLAMA_URL` overrides the address). The first run picks the first provider
  with credentials, else Ollama. Provider, model and mode are saved in `assistant.json` beside
  `ui.json`; keys never are.
- **A turn**: the request joins the transcript; the model call runs on a worker thread
  (`ModelCall::run`, streaming text and reasoning back through a channel); every tool call runs
  on the UI thread through `SlideApp::run`, so it is an ordinary command with the app's undo,
  status line and repaint. The pane drives `Assistant::begin` / `on_completion` / `on_verdict`
  from the frame loop; nothing blocks the window.
- **Plans**: in confirm mode (and for destructive commands in autopilot) a card shows "Malu wants
  to:" with each call and the engine commands behind it; Approve, Decline and Stop answer it. The
  command `assist.plan {verdict: approve|decline|cancel, reason?}` answers it from a script.
- **The answer**: the final text, "Show reasoning" when the model reasoned, one line per tool call
  (✓ or ✗ and a compact result), "Changed slides …", an **Undo** button that reverts the whole
  turn while it is the top of the history, and the token counts. Stop ends a turn; Clear forgets
  the conversation (the deck stays).
- The app host offers the headless tool table (`render_slide`, not `screenshot`, which needs the
  async frame path) and answers `ui.pointer`, `ui.key`, `ui.text` and `ui.tool.select` through the
  session like the headless backend does.

Screenshots (sample deck, Ollama `glm-5.3-flash:cloud`, confirm mode) in
[`screenshots/`](screenshots/): the pane opened with a request typed
(`3.6-pane-open.png`), the plan waiting (`3.6-pane-plan.png`), the turn done with the new
slide, the tool line, Changed slide 9 and Undo (`3.6-pane-done.png`).

Tests (`cargo nextest run -p maluos-slides-ui-egui malu_assistant`, 5 tests on the egui kittest
harness with `MockClient`): the command and the shortcut open the pane with the box focused; an
autopilot turn adds a slide, the Undo button reverts it and a second turn continues the
conversation; confirm mode shows the plan and Approve applies it; Decline keeps the deck and Stop
ends the turn as cancelled; a model error is shown and the pane recovers.

## A deck from a document (step 4.1)

`outline.rs` turns Markdown or a JSON slide list into an `Outline` (title and subtitle, sections,
slides with body lines, notes and tables) and `apply` builds it through any MCP `Backend` with
engine commands only, so it works on the headless session, a running app and from the pane
alike. The tool `build_deck` wraps it and sits in the shared table as the fork's first extra
tool: `ToolTable` appends `extra_definitions()` to upstream's list, `call_tool` in `exec.rs`
answers the extras and delegates the rest, the HTTP API uses that, and `mcp_ext::Server` wraps
upstream's MCP server so `tools/list` and `tools/call` see the extras too. `replace: true`
starts a new presentation and is classed destructive. The instructions tell the model to write
the outline and call `build_deck` once for a whole deck, with one idea per slide and a closing
slide; the undo step for the turn then reverts the whole deck.

## Slide-level help and pictures (step 4.2)

`render_slide` now shows the model the picture: the tool outcome keeps the PNG, and the loop
attaches it to the tool result (`ToolOutcome::block_with_images`) up to `Limits::max_images` per
turn (six; zero for a text-only model). When the next turn starts, pictures from earlier turns
are dropped from the history and their text stays, so a long conversation does not carry old
screenshots. In `maluos-llm` an image is `Image {media_type, data}`: Anthropic takes it inside
the `tool_result` content and as an `image` block in user messages; OpenAI-style APIs cannot
put images in `tool` messages, so they follow the tool messages as one user message of
`image_url` parts (Ollama's compatible endpoint accepts the same for vision models).

The instructions carry the five everyday recipes, each tied to the tools that do it: a title
through `set_text` (six words at most), bullets through `set_text` on the body (five, parallel,
levels kept), notes through `slide.notes`, a chart from a table through `inspect_slide` then
`insert_chart` with the first column as categories, a layout through `arrange.align`,
`arrange.distribute`, `shape.move` / `shape.resize` and `format.autofit`, checked with
`render_slide`. The pane offers them as one-click quick actions above the box ("This slide:").

## The presenter assistant (step 4.4)

`timing.rs` estimates how long a deck takes: for each slide the words on it (text, tables,
groups) and in its notes, seconds = 8 + notes ÷ 130 wpm + slide text ÷ 60 wpm, at least 15;
the total counts shown slides only. Presenter view shows "≈ 1:20 · deck 14:30" beside the slide
counter and a cue line (the notes' first sentences, 30 words) above the full notes. The
read-only tool `deck_timing` gives the model the same numbers, through `document.inspect` and
`slide.inspect` so it also works on a running app. The instructions add two recipes: timing
(trim the longest slides' notes and bullets first) and Q&A prep (six to ten questions with
answers drawn only from the deck, on one hidden slide at the end, answers in its notes); the
pane offers Q&A prep as a quick action.

## Pictures from the office (step 4.5)

`office.rs` reaches the office this desk is enrolled with: the enrolment file
`/etc/maluos/office.yaml` (`MALUOS_OFFICE_CONF` overrides; no secret in it) gives the office's
name, URL and `endpoints.agents`; the desk token comes from `MALUOS_OFFICE_TOKEN` when set, else
from the vault over its Unix socket (`GET /v1/secrets/office-desk-token/value?mode=read&purpose=…`,
identity by peer credentials, `MALUOS_VAULT_SOCKET` overrides). `Office::agents()` lists the
agents the office publishes; `image_agent()` picks the first whose capabilities say `image*` or
`picture*`, never a fixed id; `picture(agent, prompt)` runs the MaluOS task protocol
(`tasks/send`, then `tasks/get` every half second while the task is `working`, up to the task
timeout) and takes the first image artifact of the output (`{kind|type: image, media_type|mime,
data (base64) | url, name}`). An artifact given by URL is read only when the URL is under the
office's own URL; anything else is refused with "nothing is fetched from the web". The tool
`office_picture {prompt, slide?, rect?}` in the shared table (so the API and MCP have it too)
moves to the slide with `slide.go` when asked, then inserts the bytes with `insert.picture`;
the policy shows it as a change through that command, so confirm mode makes a plan of it. When
the desk is not enrolled or the office publishes no image agent, the tool fails with that
sentence and the instructions tell the model to say so and look nowhere else. The pane offers
"Add a picture" as a quick action. The Phase 13 delegation envelope is not shipped by the
platform yet; it goes on the same requests when it is. `Office::set_override` installs an
office for tests or a CLI pointed elsewhere.

## Snapshots

`snapshots/tools.json` is the tool table (every tool, app-only included),
`snapshots/commands.json` the catalog with each command's generated schema, and
`snapshots/context-sample.md` the outline of the sample deck (`engine::sample`) on slide 2. The tests compare
against them; `MALUOS_SLIDES_UPDATE_SNAPSHOTS=1 cargo nextest run -p maluos-slides-assist`
rewrites them after a deliberate change (an upstream merge that adds or re-documents a command
shows up here as a diff to read, not a failure to silence).

## Tests

`cargo nextest run -p maluos-slides-assist`: 31 tests. The table mirrors the server definition
for definition; the headless table drops the app-only tools; every one of the engine's params
docs parses into a schema (a guard against upstream drift in the grammar); schema rules on hand-
written docs; validation of unknown commands, keys and non-object params; the prompt listing's
filter and cut; and, on the headless engine, tool calls that add and inspect slides, a
`run_command` that pushes one undo step, refusals that leave the revision untouched, image and
long-result shaping. For the context: the sample-deck snapshot, no document, the selection and an edited text range, a 121-slide deck cut at a small budget with the current slide whole and every marker present, and the cache rebuilding only on revision, selection or document change. For the loop, with `maluos_llm::mock::MockClient`: "add a closing slide that thanks the audience" scripted as `run_command slide.new` then `set_text` produces the slide, the text, the journal `[slide.new, text.set]` and exactly one undo entry `Assistant: add a closing slide…` that `edit.undo` reverts whole; the requests carry instructions, catalog, outline (rebuilt per round) and the tool table; the round limit; the tool-call limit; cancel at `ToolStart` leaving the conversation valid; retry on rate limit and overload, none on auth; an error after a tool call keeping the change as one undo step; trimming at user messages; the system text with and without the catalog. For the rails: the destructive lists name real commands and tools; classification and decisions per mode and the plan text; answer-only reads and refuses with the tools it offers; confirm mode approved, declined with a reason the model hears, and cancelled; autopilot applying additions without a plan and confirming each destructive round (a batch shown whole), approved deletion counted as removed; touched slides after an edit and an addition. The MCP crate's 25 tests still pass.

## Next


## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | major | Created with step 3.3: tool table, catalog, generated schemas, validation, `run_tool`, snapshots, 12 tests |
| 2026-11-02 | major | Step 3.2: `Context`, `Budget`, `ContextCache`, the sample-deck snapshot, 5 more tests |
| 2026-11-02 | major | Step 3.4: `Assistant::ask`, `Host`, `Limits`, `TurnEvent`, `TurnReport`, one undo entry per turn, `examples/ask.rs`, 8 more tests |
| 2026-11-02 | major | Step 3.5: `Policy`, `Mode`, `Plan`, `Confirm`, destructive lists, touched slides, `--mode`/`--yes` on the example, 6 more tests |
| 2026-11-02 | major | Step 3.6: the loop as a step machine (`Turn`, `Step`, `ModelCall`), the pane in `ui-egui`, `assist.ask` / `assist.plan`, screenshots, 5 pane tests |
| 2026-11-02 | minor | Step 3.7: `providers` module shared by the pane and the CLI; the example program gave way to `maluos-slides-cli ask` |
| 2026-11-02 | major | Step 4.1: `outline`, `build_deck`, `call_tool` with extras, `mcp_ext::Server` |
| 2026-11-02 | major | Step 4.2: images to the model, the five recipes, quick actions |
| 2026-11-02 | major | Step 4.4: `timing`, `deck_timing`, presenter view hints, Q&A prep |
| 2026-11-03 | major | Step 4.5: `office`, the `office_picture` tool, the Add a picture quick action |
