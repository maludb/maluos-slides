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

## Snapshots

`snapshots/tools.json` is the tool table (every tool, app-only included),
`snapshots/commands.json` the catalog with each command's generated schema, and
`snapshots/context-sample.md` the outline of the sample deck (`engine::sample`) on slide 2. The tests compare
against them; `MALUOS_SLIDES_UPDATE_SNAPSHOTS=1 cargo nextest run -p maluos-slides-assist`
rewrites them after a deliberate change (an upstream merge that adds or re-documents a command
shows up here as a diff to read, not a failure to silence).

## Tests

`cargo nextest run -p maluos-slides-assist`: 17 tests. The table mirrors the server definition
for definition; the headless table drops the app-only tools; every one of the engine's params
docs parses into a schema (a guard against upstream drift in the grammar); schema rules on hand-
written docs; validation of unknown commands, keys and non-object params; the prompt listing's
filter and cut; and, on the headless engine, tool calls that add and inspect slides, a
`run_command` that pushes one undo step, refusals that leave the revision untouched, image and
long-result shaping. For the context: the sample-deck snapshot, no document, the selection and an edited text range, a 121-slide deck cut at a small budget with the current slide whole and every marker present, and the cache rebuilding only on revision, selection or document change. The MCP crate's 25 tests still pass.

## Next

- 3.4 the loop over `LlmClient` with one undo group per turn: the engine pushes one history
  entry per undoable command (`History.undo` is public), so the loop folds the entries a turn
  added into one labelled "Assistant: …".
- 3.5 safety rails use `ToolTable::is_read_only` and `CommandEntry::undoable`; the always-confirm
  list (`slide.delete`, `section.remove`, `file.new`, replace-all) lives there.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | major | Created with step 3.3: tool table, catalog, generated schemas, validation, `run_tool`, snapshots, 12 tests |
| 2026-11-02 | major | Step 3.2: `Context`, `Budget`, `ContextCache`, the sample-deck snapshot, 5 more tests |
