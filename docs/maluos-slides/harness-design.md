# The MaluOS office harness: design decisions shared by Slides, Malu123 and MaluDocs

The owner's direction (2026-11-03): the same process that turns MaluOS Slides into an AI harness
will be run for **Malu123** (a spreadsheet, like Excel) and **MaluDocs** (a document editor,
like Word). This document records every design decision taken in Slides so the three
applications end up consistent in design and function. Each decision says what was decided, why,
how it shows in Slides, and what the other two should mirror. Slides-specific detail lives in
[assistant.md](assistant.md), [llm.md](llm.md), [api.md](api.md), [cli.md](cli.md) and
[maluos-integration.md](maluos-integration.md); this file is the cross-application contract.

Numbering: `H<n>` (harness). Owner decisions from the plan keep their `D<n>` ids.

## 1. Shape of the work

| Id | Decision | Why | Mirror in Malu123 / MaluDocs |
|---|---|---|---|
| H1 | **Keep the layering.** App-specific AI code lives in its own crates (`maluos-<app>-assist`, `maluos-<app>-api`) and the shared `maluos-llm`; hooks inside the inherited editor crates are fenced (`// MaluOS:` comments, one `pub mod` per feature). Originally "keep upstream mergeable"; since the owner cut the link to DeckCraft (D12, 2026-11-03) inherited crates may be edited directly when a feature needs it, with tests, but the layering stays so the harness is one recognisable shape across the apps. | A reader finds the harness in the same places in every app; an app that still tracks its upstream keeps the stricter form. | Same crate names with the app's prefix; same fence style. Whether an app tracks an upstream is that app's decision; the layering holds either way. |
| H2 | **Plan and status files.** `docs/<app>/plan.md` holds phases and steps with "done when"; `docs/<app>/status.md` records what landed, with verification and live runs; commit ids are plan steps (`P4.5: …`). | The owner follows progress by stopping points, not by diffs. | Identical layout and wording; one row per step with the evidence. |
| H3 | **Linux and MaluOS only** (owner D1). No other platform packaging or docs; platform `cfg` branches inside upstream crates stay as upstream has them. | One target, one image. | Same. |
| H4 | **Gates before a push**: fmt, clippy `-D warnings`, nextest, doctests, asset attribution, layering, wasm check of the wasm-safe crates. Build under a memory scope so the desktop never starves. | A green tree at every stopping point. | Same xtask set; the layering table lists the assist crate at the UI layer and the API crate above it. |

## 2. The engine is the only door

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H5 | **The assistant acts only through engine commands** (`Session::execute`), never through the model types. Every change is a command with params documented in a fixed grammar, parsed into a JSON Schema for the model. | Commands are the audited, undoable, scriptable surface; the model gets exactly what the user has. | Malu123: cell, range, sheet and formula commands; MaluDocs: paragraph, run, style, section commands. The params-doc grammar and the schema generator are shared code. |
| H6 | **One undoable history group per turn.** Everything a turn did folds into one entry named after the request (`Assistant: add a closing slide…`); the pane shows Undo per turn. | The user judges a turn as a whole. | Same fold over the app's history type. |
| H7 | **One tool table for the assistant, the HTTP API and MCP** (owner D9). The MCP server's definitions are the model's tools; the fork's extra tools are appended last and dispatched before upstream's `call_tool`; the API publishes the same table; an MCP wrapper adds the extras without editing upstream's server. | Three callers, one behaviour, one set of tests. | Same `ToolTable` / `extra_definitions` / `is_extra` / `mcp_ext::Server` pattern. |
| H8 | **Tool conventions.** `snake_case` verb names; `readOnlyHint` decides the read-only class; a description marker (`Desktop app only`) hides tools a headless backend cannot serve; unknown arguments are refused by name with the allowed list; results are JSON text, errors are `is_error` text the model can act on; results are cut at 64 KiB with a marker. | The model learns one dialect across the apps. | Same markers, same error phrasing, same cut. |
| H9 | **Validation before action.** `run_command` and `batch` calls are checked against the catalog (unknown command → did-you-mean by edit distance ≤ 2; undocumented keys refused) before anything runs. | A typo must not become a stray change. | Same catalog type. |

## 3. Models and keys

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H10 | **`maluos-llm` is the one client**: a `Transport` trait with no async runtime, Anthropic Messages and OpenAI-compatible backends (OpenAI, Ollama, the gateway), streaming, tool use, images in tool results and user messages, a mock client and transport for tests, a `live` example for a real check. | One crate, shared by every app, fixed once. | Depend on it; add nothing app-specific to it. |
| H11 | **Provider choice**: the first provider with a key in the environment, else Ollama at `127.0.0.1:11434`; the user may switch in the pane; the choice (provider, model, mode) is saved in `~/.config/maluos-<app>/assistant.json`, **never a key**. | Works out of the box on a MaluOS desktop with Ollama; cloud when the owner adds a key. | Same `providers::default_choice`, same config shape and path pattern. |
| H12 | **Keys live in the MaluOS vault or an env var**, never in settings files, decks, documents or the repo. The vault is HTTP over its Unix socket with peer-credential identity; reads are `mode=read` with a `purpose`. | Platform rule. | Same `vault_value` code; the same purpose string pattern `"<App>: <what for>"`. |
| H13 | **Limits per turn**: rounds 12, tool calls 40, output tokens 8000, catalog 32 KiB, retries 3 on rate-limit/overload (none on auth), messages trimmed at user turns to 60, images 6 per turn and dropped the turn after. | Bounded cost and a conversation that stays valid. | Same `Limits` defaults unless an app proves it needs others. |

## 4. Context the model sees

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H14 | **System text = instructions + catalog + document outline.** The outline is rebuilt after every tool call (so the turn's own edits show), cut to a byte budget with explicit markers (`… [cut]`), the current unit kept whole, cached on (document id, revision, selection). | The model reasons on the real state; cost is bounded. | Slides: sections, slides, placeholders, notes, selection, theme. Malu123: sheets, used ranges, headers, the selected range's values and formulas, named ranges. MaluDocs: headings tree, the current section's paragraphs, styles, the selection. Same cut and cache mechanics. |
| H15 | **Pictures to the model once per request**: a rendered slide rides with its `render_slide` result (both backends), and old pictures are stripped when a new turn starts. | Visual checks after visual changes without paying for them forever. | Malu123: a rendered range or chart; MaluDocs: a rendered page. Same stripping rule. |
| H16 | **Indexes and ids**: prose numbers from 1, tools take 0-based indexes and shape ids; the outline shows both. | Models mix them up otherwise. | Same sentence in every app's instructions. |

## 5. Safety rails

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H17 | **Three modes**: AnswerOnly (reads only, refuses changes with a sentence the model relays), Confirm (every change shown as a plan the user approves, declines with a reason the model hears, or stops), Autopilot (changes applied; destructive calls still confirmed). | The owner picks the trust level per session. | Same `Mode`, `Class`, `Decision`, `Plan`, `Confirm` trait. |
| H18 | **Destructive is a list, not a guess**: named commands (delete, remove-all, file new/open/close/revert…) and tools (new/open, `build_deck` with `replace`). | Explicit, testable; an upstream merge that adds a command shows up in a snapshot diff. | Each app keeps its own list in `policy.rs`; the test asserts every name is a real command. |
| H19 | **Nothing from the public web by default.** The assistant never fetches URLs; pictures come from the office's image agent only; an artifact given by URL is read only under the office's own URL. | Platform trust boundary; the owner's data stays in the estate. | Same rule and the same tool text ("look nowhere else, never the web"). |

## 6. The pane and the app

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H20 | **The assistant is a side pane named "Malu"**, opened from a spark button in the title bar and `Cmd+Shift+A`, with provider, model and mode pickers, a status line (endpoint, key needed or not), a transcript with tool calls folded, a plan card (Approve / Decline / Stop), Undo per turn, and a request box (Enter sends). | "Cursor for presentations": the assistant beside the work, never a modal. | Same name, button, shortcut, layout and strings; the pane module is `malu_assistant.rs` in each app's UI crate. |
| H21 | **Quick actions** for the usual requests on the current unit ("This slide: Tighten title, Rewrite bullets, Write notes, Chart from table, Fix layout, Add a picture, Q&A prep"): each fills the box with a fixed sentence and sends. | One click for the common cases; the sentences double as the recipes' test prompts. | Malu123: "This sheet/range: Explain formula, Fill down, Clean data, Chart this, Add totals, Add a picture". MaluDocs: "This section: Tighten, Summarise, Fix headings, Write abstract, Table from text, Add a picture". |
| H22 | **Scriptable UI commands** `assist.ask {text?, send?, toggle?}` and `assist.plan {verdict, reason?}` through the app's control channel, so a test or the CLI drives the pane exactly as a user does. | Every user-visible action is a command with tests. | Same two commands. |
| H23 | **Instructions are recipes**, one line each, naming the tool and the shape of the result (counts, limits, what never to touch). The persona is Malu; the answer is brief: what changed, where. | Short, testable, cheap. | Same opening paragraph and "How to work" block; recipes per app. |

## 7. Surfaces: CLI, HTTP API, MCP

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H24 | **CLI verbs** `ask` (a turn against a file or the sample, `--mode`, `--yes`, `--provider`, `--model`, `--json`, `--from DOC`), `api` (serve), `mcp` (stdio server with the extras), plus app-specific builders (`outline` in Slides). | Headless use and CI. | Same verbs and flags; the builder verb is the app's natural input (`outline` → `csv`/`table` for Malu123, `markdown` for MaluDocs). |
| H25 | **HTTP API** `/v1`: `health`, `tools` (the table), `tools/{name}` (one call), `commands[/{id}]`, `document`, the unit routes (`slides/{i}[/render.png]`), `ask`; loopback only unless a bearer token is set; hand-written HTTP/1.1, no async. | One API shape for every app's integrations and for the office's agents. | Same routes with the unit renamed (`sheets/{name}/ranges/{a1}`, `sections/{i}`), same token rule. |
| H26 | **MCP** is upstream's server wrapped: the fork's tools appended to `tools/list` and answered on `tools/call`. | Keeps the upstream server untouched. | Same wrapper. |

## 8. The platform: vault, gateway, office

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H27 | **Integration follows the installed sources**, never guesses: `/usr/lib/maluos/*.py` and `maluctl` define the vault, gateway, desk and task protocols. What Slides needs and the platform lacks is written as numbered asks in `maluos-integration.md` for the MaluOS build. | The platform and the apps are built in step. | One shared asks file per platform topic; each app adds its own rows rather than a new file. |
| H28 | **The office and its agents**: the desk's enrolment (`/etc/maluos/office.yaml`, no secret in it; the desk token in the vault as `office-desk-token`, or `MALUOS_OFFICE_TOKEN`), the agents directory at `endpoints.agents`, agents chosen **by capability** (`image*`, `picture*`) never by id, tasks over the MaluOS task protocol (`tasks/send`, `tasks/get` polled while `working`, artifacts in the output), the delegation envelope added when the platform ships it. | One way to reach an office agent from any desk app. | Reuse `office.rs` as is (it belongs in a shared crate once a second app needs it); each app adds its own tool on top (`office_picture` in Slides). |
| H29 | **Dev doubles**: the shipped mock office (`maluos_desk.mock_office`) plus a scratch override for agents the mock lacks; tests carry their own mock office, mock vault socket and mock model, so the suite needs no platform. | The suite runs in CI; the desk run proves the real path. | Same test doubles, same `Office::set_override` seam. |

## 9. Verification and record

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H30 | **Three levels of proof per step**: unit and loop tests with the mock model; a window test (kittest) that drives the pane and can render a screenshot; a live run on the desktop through the control channel with a real model (Ollama cloud here), screenshot saved under `docs/<app>/screenshots/` and attributed in `ATTRIBUTION.md`. | The owner wants to see the app, not a transcript. | Same three levels; screenshots named `<step>-<what>.png`. |
| H31 | **Snapshots** of the tool table, the command catalog and a sample context are committed; a deliberate change rewrites them with an env flag; an upstream merge shows up as a diff to read. | Drift is visible. | Same three snapshots. |
| H32 | **Owner decisions are recorded, not asked**: deferrals and choices go into plan.md/status.md with the date. | The owner is not in the loop mid-step. | Same. |

## 10. Added at step 4.6

| Id | Decision | Why | Mirror |
|---|---|---|---|
| H33 | **One engine addition is allowed when it is upstream-shaped and unavoidable.** Slides added `document.tags` to `crates/engine` (the add-in tags the model already keeps for round trip, with no model or format change) because the assistant may only write through commands and none existed for the field. It is written as upstream would (a `cmd!` entry, params doc, test) and offered upstream. | Rule H1 forbids fork code in upstream crates; a command the format already supports is the smallest honest exception, recorded here and in status.md. | Prefer an existing command; if one must be added, make it the generic one upstream would want (not "assistant memory") and record it. |
| H34 | **Per-document memory lives in the document**, as one add-in/custom property the format round-trips, as a small versioned JSON of dated notes; capped (24 notes, 240 chars), de-duplicated, undoable through the engine, shown to the model as "Remembered about this <document>" with indexes, and to the user in the pane with Forget. Tools `remember`, `forget`, `recall`. Nothing is written outside the file; secrets never. | The user's facts about a deck belong to the deck, travel with it, and are visible and removable. | Malu123: the workbook's custom document property (`docProps/custom.xml`) or an equivalent the format keeps; MaluDocs: the document's custom property. Same tag name pattern `maluos.assistant.memory`, same JSON, same tools and pane section. |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-03 | major | Created at the owner's request with every decision from Phases 1–4.5 of Slides, each with its mirror for Malu123 and MaluDocs |
| 2026-11-03 | minor | H33 (an upstream-shaped engine addition) and H34 (memory in the document) from step 4.6 |
| 2026-11-03 | minor | H1 reworded after D12 (the link to DeckCraft cut; keep the layering, not mergeability) |
