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

## Snapshots

`snapshots/tools.json` is the tool table (every tool, app-only included) and
`snapshots/commands.json` the catalog with each command's generated schema. The tests compare
against them; `MALUOS_SLIDES_UPDATE_SNAPSHOTS=1 cargo nextest run -p maluos-slides-assist`
rewrites them after a deliberate change (an upstream merge that adds or re-documents a command
shows up here as a diff to read, not a failure to silence).

## Tests

`cargo nextest run -p maluos-slides-assist`: 12 tests. The table mirrors the server definition
for definition; the headless table drops the app-only tools; every one of the engine's params
docs parses into a schema (a guard against upstream drift in the grammar); schema rules on hand-
written docs; validation of unknown commands, keys and non-object params; the prompt listing's
filter and cut; and, on the headless engine, tool calls that add and inspect slides, a
`run_command` that pushes one undo step, refusals that leave the revision untouched, image and
long-result shaping. The MCP crate's 25 tests still pass.

## Next

- 3.2 context builder (deck outline, selection, theme) using `document.inspect` and the
  selection, size-capped and cached on the revision.
- 3.4 the loop over `LlmClient` with one undo group per turn: the engine pushes one history
  entry per undoable command (`History.undo` is public), so the loop folds the entries a turn
  added into one labelled "Assistant: …".
- 3.5 safety rails use `ToolTable::is_read_only` and `CommandEntry::undoable`; the always-confirm
  list (`slide.delete`, `section.remove`, `file.new`, replace-all) lives there.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | major | Created with step 3.3: tool table, catalog, generated schemas, validation, `run_tool`, snapshots, 12 tests |
