# MaluOS Slides status

> **Updated:** 2026-11-02 (10:30 UTC)

MaluOS Slides is the MaluOS presentation app, forked from
[storytold/deckcraft](https://github.com/storytold/deckcraft) on 2026-10-24 at upstream
`9aef21e` (v0.10.0 + 158 commits). Remotes: `origin` = github.com/maludb/maluos-slides,
`upstream` = github.com/storytold/deckcraft. Upstream code is untouched so far; this file and the
`docs/maluos-slides/` directory are the only additions.

## Phase 1: build and install on MaluOS (Ubuntu 26.04)

| Step | State | Notes |
|---|---|---|
| 1.1 repository | done 2026-10-24 | `maludb/maluos-slides` created (public), upstream history kept, pushed to `origin main` |
| 1.2 release build | done 2026-10-24 | `cargo build --release --locked -p deckcraft -p deckcraft-cli -j 6` under a memory-capped cgroup; 5m46s cold, 6m11s again with craft-fonts embedded; rustc 1.99.1 (upstream requires 1.95) |
| 1.3 packaging | done 2026-10-24 | `packaging/linux/package.sh --skip-build --formats "deb tar"` with nfpm 2.48.0; desktop file and AppStream metainfo validate; `deckcraft-0.10.0-linux-x86_64.deb` (79 MB) |
| 1.4 install | done 2026-10-24 | `apt-get install ./dist/release/deckcraft-0.10.0-linux-x86_64.deb` on the dev VM; binaries in `/usr/bin`, desktop entry, MIME types and icons registered |
| 1.5 smoke test | done 2026-10-24 | `deckcraft-cli render --sample --all` draws all 8 sample slides at 1920×1080; `deckcraft --sample --control 7990` under Xvfb answers `ui.screenshot` and `shape.insert` (software Vulkan/EGL, embedded Inter font) |

### Build recipe (dev host)

System packages beyond the Rust toolchain: `libxkbcommon-dev`, `libasound2-dev` (the first build
failed on the missing `alsa.pc`), `pkg-config`, `cmake`; for packaging `nfpm`
(release binary in `~/.local/bin`), `desktop-file-utils`, `appstream`; for the headless GUI test
`xvfb`.

Fonts: `.cargo/config.toml` embeds `../craft-fonts` when it exists. The dev VM has
`storytold/craft-fonts` checked out at `43913056` (the commit upstream's release.yml pins) in
`~/craft-fonts`; without it the build warns and the app falls back to system fonts.

```sh
systemd-run --user --scope -p MemoryHigh=4500M -p MemoryMax=6G -- \
  env CRAFT_FONTS_REQUIRED=1 cargo build --release --locked -p deckcraft -p deckcraft-cli -j 6
PATH=$HOME/.local/bin:$PATH CRAFT_FONTS_DIR=$HOME/craft-fonts GITHUB_REPOSITORY=maludb/maluos-slides \
  packaging/linux/package.sh --skip-build --formats "deb tar"
sudo apt-get install ./dist/release/deckcraft-0.10.0-linux-x86_64.deb
```

The memory cap matters: the dev VM has 9 GB and is killed by the hypervisor under page-cache
pressure. Peak RSS of the build cgroup was 2.3 GB at `-j 6`.

### Still upstream-branded

Everything still says DeckCraft: crate names, binaries, the app id `ai.storyteller.deckcraft`,
the desktop entry, the package name and the ArtCraft brand assets under `docs/brand/`. Renaming
and rebranding is Phase 2 work and should follow the malu123 approach (scripted rename of
upstream through a vendor branch) so upstream can still be merged.

## Phase 2: MaluOS Slides identity

Planned 2026-10-24 in [plan.md](plan.md) (steps 2.1–2.9, ~19–31 h). Owner decision D1 recorded the
same day: Linux and MaluOS only; non-Linux documentation, packaging and CI leave the tree. The
README was rewritten accordingly (Linux build and package instructions, DeckCraft credited, brand
files no longer referenced).

| Step | State | Notes |
|---|---|---|
| 2.1 baseline `cargo xtask ci` | done 2026-10-24 | all 8 steps pass (fmt, pdf-deps, clippy, test, doctest, assets, layers, wasm); 1,792 tests pass, 6 ignored. Environment note: with the default test parallelism the `ui-egui` unit-test binary segfaults inside `libvulkan.so` (egui_kittest's wgpu renderer on the VM's software Vulkan, a loader race, not a code defect); `RUST_TEST_THREADS=4` is stable, single-threaded too. Run ci as `RUST_TEST_THREADS=4 cargo xtask ci` on this host. ~45 min cold under the memory cap, peak 4.7 GB |
| 2.2 fork hygiene | done 2026-10-24 | README rewritten (Linux only, DeckCraft credited); `CLAUDE.md` carries the MaluOS Slides preamble; the rename script removes `docs/brand/` and rewrites `ATTRIBUTION.md`, `AGENTS.md` and `NOTICE` to say so; `links.rs`, the About box and Backstage point at GitHub (`maludb/maluos-slides`, Discussions) and credit DeckCraft; `docs/claude-log/` ignored |
| 2.3 Linux-only trim | done 2026-10-24 | the rename script drops `packaging/{macos,windows,freebsd}`, `icons.sh`, the `.icns`/`.ico` icons, upstream's seven workflows, `.coderabbit.yaml`, `sonar-project.properties`, `omastore.toml`; `packaging/macos/Info.plist.in` and `packaging/windows/maluos-slides.wxs` stay as fixtures of `every_desktop_offers_the_same_file_types`; the fork's own `maluos-ci.yml`, `maluos-release.yml`, `maluos-packaging-lint.yml` (Ubuntu only) |
| 2.4 rename script, vendor branch | done 2026-10-24 | `scripts/rename-upstream.sh` + `.py` (see [rename.md](rename.md)) run on upstream `9aef21e` → branch `upstream-renamed`, merged into `main`: 39 paths moved, 348 files rewritten, 28 packages `maluos-slides-*`, app id `org.maluos.slides`, `Cargo.lock` re-sorted so `--locked` holds. Seven script rules came out of CI runs: struct-update paths (`..deckcraft_model::`), log targets (`maluos_slides`, underscore), the MCP server name, the plist's `<string>deckcraft</string>` extension, the spelling test's `hallo deckcraft` and `deckcraft\n`, the test fonts `DeckCraft Legacy` and `DeckCraft Colr Squares`. `cargo xtask ci` green on the renamed tree: 8 steps, 1,792 tests, 6 ignored. Release build 5m54s; `maluos-slides-0.10.0-linux-x86_64.deb` (79 MB) built, installed over the Phase 1 package and verified: `maluos-slides-cli --version`, MCP `serverInfo.name` = `maluos-slides`, GUI under Xvfb with `--control`, sample deck's note reads "made in MaluOS Slides", settings in `~/.config/maluos-slides/`, desktop entry `org.maluos.slides` registered |
| 2.5 app identity | planned | |
| 2.6 MaluOS packaging | planned | D8: vendored deb |
| 2.7 boot test and release | planned | |
| 2.8 fonts decision applied | planned | D5 |
| 2.9 upstream sync rehearsal | planned | D7 |

## Session 2026-11-02: owner direction, host tooling, review

**Owner direction (D9 in [plan.md](plan.md)):** MaluOS Slides becomes an AI harness that creates
real PowerPoint-compatible presentations (native shapes, text, tables and charts in `.pptx`, never
one rendered image per slide), with the tool at the centre and every function callable from both
an API and MCP.

**Where the agent surface stands today** (measured on `main` at `df93917`): 321 engine commands
(`maluos-slides-cli commands`); the MCP server (`maluos-slides-cli mcp`) answers `initialize` with
`serverInfo.name = maluos-slides`, protocol 2025-06-18, and lists 25 tools (`run_command` and
`batch` reach every command, `call_app` every control-channel method); the CLI (`run`, `render`,
`convert`, `info`, `describe`, `app`) and the loopback JSON-lines control channel
(`--control PORT`) dispatch through the same `Session::execute`. There is no HTTP API: the control
server rejects HTTP request lines by design (`apps/maluos-slides/src/control_server.rs`). A
network API (HTTP/JSON over the command catalog, the same table that drives MCP, plan step 3.3) is
the gap between today and "every function from an API".

**Host tooling installed this session** (the toolchain had only rustc and cargo): rustup
components `rustfmt` and `clippy`, target `wasm32-unknown-unknown`, `cargo-nextest` 0.9.150,
`trunk` 0.22.1, `nfpm` 2.48.0 (`~/.local/bin`, checksum verified), `~/craft-fonts` cloned and
checked out at `43913056` (the commit `maluos-ci.yml` pins). Still missing because they need sudo
on this host: `xvfb` (headless GUI screenshots), `bc`, `gh`, and the optional CI dev packages
`libwayland-dev libx11-dev libxrandr-dev libxi-dev libgl1-mesa-dev` (the build does not need
them). Note: `~/src/maluos-slides` and `~/src/craft-fonts` are stale checkouts from October; the
live repository is `~/maluos-slides`. The `upstream` remote (storytold/deckcraft) is not configured
in this clone yet. The host reports 31 GB RAM; the build cap in `CLAUDE.md` still applies as a habit.

**`cargo xtask ci` on this host:** fmt, pdf-deps, clippy, doctest, assets (34 files), layers
(28 crates) and wasm (23 crates) pass; tests: 1,788 of 1,789 pass, 6 skipped. The one failure is
environment-only: `maluos-slides-pdf tests::unembeddable_font_returns_error_instead_of_a_pdf_missing_text`
builds its damaged font from `FontDb::global().face("Ubuntu", "Regular")`; craft-fonts ships no
Ubuntu, so that resolves to MaluOS's system `Ubuntu[wdth,wght].ttf` (fonts-ubuntu 0.869), a
variable font whose 12 named instances make `add_font` return 12 instead of 1. Fix to send
upstream: build the fixture from an embedded craft font (Inter) instead of a system family.
Smoke: `maluos-slides-cli render --sample --all` draws all 8 slides; `run --sample --save x.pptx`
writes a 58-part package with real `ppt/slides/slideN.xml`; the MCP handshake above.

**Owner vision (later the same day):** "Cursor for presentations". The assistant has full control
of the editor the way Cursor has of its IDE, from a chat sidebar like the Selection Pane; the
decks it makes stay easy to edit by hand (one paragraph's text is one command on one shape);
providers are Anthropic and OpenAI first, then Ollama. Written up as the Vision section and
decisions D10 and D11 in [plan.md](plan.md).

### The host (owner, 2026-11-02)

This machine is the owner's MaluOS desktop: Ubuntu 26.04 base, GNOME 50 on Wayland, 12 cores,
31 GB RAM, Ollama 0.40 as a system service. It is not a VM; the "dev VM" of the October entries
above was the earlier 9 GB machine. The `systemd-run` memory scope and `-j 6` stay as a habit so
a build never starves the desktop. The app can be run on the desktop directly
(`target/debug/maluos-slides --sample`); `xvfb` is only for headless screenshots in scripts.

### MaluOS vault and gateway (2026-11-02)

The vault and gateway protocols are not undocumented after all: their sources are installed
(`/usr/lib/maluos/maluos_vault.py`, `/usr/lib/maluos/maluos_gateway/`). Both are HTTP + JSON over Unix sockets; the vault
identifies callers by `SO_PEERCRED` and the owner (`user:maludb`) is enrolled; the gateway has OpenAI and Anthropic faces
with tools and streaming. What Slides builds and what it asks of the MaluOS build is in
[maluos-integration.md](maluos-integration.md).

## Phase 4: slides-specific depth

| Step | State | Notes |
|---|---|---|
| 4.1 deck from a document | done 2026-11-02 | `maluos-slides-assist::outline`: Markdown (`#` title + subtitle, `##` sections, `###` slides, bullets with levels, `Notes:`, pipe tables; `##`-only documents are flat) or a JSON slide list → an `Outline` → built with engine commands through any `Backend` (`slide.new`, `section.add`, `slide.notes`, `insert.table`, `design.theme`, `file.new` when replacing). Tool `build_deck {markdown | slides, title?, subtitle?, theme?, replace?, after?}` in the shared table: the assistant offers it (instructions say to prefer it for whole decks), the HTTP API serves it, and `maluos-slides-cli mcp` now runs `assist::mcp_ext::Server`, a wrapper that appends the fork's tools to upstream's MCP server without editing it; `replace: true` is destructive (confirmed). CLI: `ask --from DOC` attaches a document (200 KiB cap), `outline DOC --out DECK [--theme] [--in BASE]` builds without a model. 4 tests in assist (parser, apply, the tool through run_tool/policy/MCP wrapper, the mock model building a 6-slide deck in one call with one undo step) + API and CLI tests. Live: `outline talk.md --theme Nocturne` made a 7-slide deck (2 sections, a table, notes); `ask --from talk.md` through Ollama `glm-5.3-flash:cloud` called `build_deck` once (with `replace`) and produced the same 7 slides in 2 rounds |

## Phase 3: the assistant

| Step | State | Notes |
|---|---|---|
| 3.1 `maluos-llm` | done 2026-11-02 | `crates/maluos-llm` (L5, in this repo until a second app consumes it, D6): `LlmClient` trait, provider-neutral `Request`/`Completion`/`ContentBlock`, streaming events with cancel, typed errors; SSE parser; `Transport` seam with the ureq 3 native transport (wasm32 compiles without it); `AnthropicClient` (Messages API 2026-10-21, adaptive thinking, effort, signed thinking blocks round-trip, refusal fallbacks on by default) and `OpenAiClient` (OpenAI flavour and the compatible flavour for Ollama, the gateway, vLLM); env keys; `MockTransport` and `MockClient`. 16 tests, clippy and wasm clean, layering table updated (29 crates). Doc: [llm.md](llm.md). Not done from the step's "done when": a run against the demo agent on the dev VM, because the gateway is a Unix socket with an undocumented protocol; the mocks stand in until a key or a local model exists. 2026-11-02 later: Ollama 0.40.13 is installed as a system service (user `ollama`) and the owner asked for a check with `glm-5.3-flash:cloud`; `examples/live.rs` (`cargo run -p maluos-llm --example live -- ollama <model>`) sends a prompt with one tool and answers the tool call. Against the daemon it maps the reply to `LlmError::Auth` (401 `Unauthorized`) as it should, and `ollama run` says the same: the daemon's key (`/usr/share/ollama/.ollama/id_ed25519`, not the owner's) is not linked to an ollama.com account yet; `ollama signin` prints the connect URL for it. The owner linked the key; the live run then passed: `ollama run` answers, and the example against the daemon's `/v1/chat/completions` streams reasoning (`ThinkingDelta`), gets a `slide_count` tool call (stop `ToolUse`), sends the tool result and gets the one-sentence answer (stop `EndTurn`), with usage incl. cached tokens on both turns. Step 3.1 "done when" is now met for the Ollama path; Anthropic and OpenAI remain mock-verified until a key is exported |
| 3.2 context builder | done 2026-11-02 | `maluos-slides-assist::context`: the deck as prompt text read from the engine state (header with theme fonts and colours; the selection with the edited text range and each selected shape on one line; the current slide whole with placeholder paragraphs as indented bullets and other shapes as one summary line each; the outline with section headings; notes), `Budget` enforced slide by slide with actionable markers (`inspect_slide N`, `inspect_document`), `cut_slides` reported; `ContextCache` keyed on document uid, revision and selection. Snapshot `snapshots/context-sample.md` on the sample deck; 5 tests (17 in the crate). No upstream crate edited; doc: [assistant.md](assistant.md) |
| 3.3 tool schema from the command catalog | done 2026-11-02 | `crates/maluos-slides-assist` (L6 beside `mcp`, intra-layer order mcp → assist → ui-egui; 30 crates in the layer table): `ToolTable` is `maluos_slides_mcp::tool_definitions()` as `maluos_llm::ToolDef`s (app-only tools dropped without a window, `readOnlyHint` kept); `Catalog` is every engine command with a JSON Schema generated from its params doc by the engine's own `parse_params` grammar, `validate` refuses unknown commands (did-you-mean within two edits) and undocumented keys before anything runs; `run_tool` executes through an MCP `Backend` (`Headless` or `Remote`) and shapes the result as the `ContentBlock::ToolResult` of the next request (64 KiB cap, images kept aside). Snapshots `snapshots/tools.json` and `snapshots/commands.json` (all 300+ commands with schemas). 12 tests; MCP's 25 still pass; clippy, fmt, layers, wasm (assist is L6, outside the wasm set until ui-egui pulls it in), assets green. No upstream crate edited. Doc: [assistant.md](assistant.md) |
| 3.4 the loop | done 2026-11-02 | `maluos-slides-assist::agent`: `Assistant::ask` over any `LlmClient`; system text = instructions + command catalog + deck outline rebuilt each round; tool calls through `run_tool` (so `Session::execute`) on a `Host` (`Headless`, later the app); limits on rounds (12), tool calls (40), output tokens (8k), conversation length; retries with backoff on retryable errors; cancel from the event callback; dangling tool calls closed so the conversation stays valid; the turn's undo entries folded into one `Assistant: <request>` step (history is public, no engine edit). Done-when met: the mock-backend test "add a closing slide that thanks the audience" produces `slide.new` + `text.set`, one undo step. 8 tests (25 in the crate, 66 with mcp and llm). `examples/ask.rs` runs a turn against a real model; live on 2026-11-02 with Ollama `glm-5.3-flash:cloud` on the sample deck: 3 rounds, `slide.new {layout: title, at: 8, title, body}` then `inspect_document`, a 9-slide `.pptx` saved with "Thank you!" last, one undo step, 38.8k input / 1.1k output tokens. Seen: the model took its own new slide in the rebuilt outline for pre-existing content; the instructions now say the outline includes the turn's edits |
| 3.5 safety rails | done 2026-11-02 | `maluos-slides-assist::policy`: `Mode` answer-only / confirm (default) / autopilot; calls classified read-only / change / destructive from the MCP `readOnlyHint`, the catalog's query flag and the destructive lists (slide and section deletion, layout deletion, file.new/open/close/revert); one `Plan` per round naming tools and the engine commands behind them, answered through `Confirm` (approve / decline with a reason the model hears / cancel); answer-only offers only reading tools; destructive always confirmed; report counts plans, declined, refused and lists touched and removed slides. 6 tests (31 in the crate, 72 with mcp and llm). Live with Ollama `glm-5.3-flash:cloud` in autopilot: "delete the last slide" produced two destructive plans, each shown with its command and flagged; the model deleted by index, then again by a now-stale id, and the engine accepted the stale id as a no-op success (undo entry, journal line, nothing deleted). Fixed in the engine as an upstream bug fix: `slide.delete {ids}` drops ids the deck does not have and fails with "no such slide" when none is left |
| 3.6 assistant pane | done 2026-11-02 | `crates/ui-egui/src/malu_assistant.rs` (fork file) plus fenced hooks in `lib.rs`, `panes.rs`, `ribbon.rs`, `menus.rs`, `icons.rs`: pane "Malu" with provider / model / mode pickers and key status, the transcript with streaming text and reasoning, plan cards with Approve / Decline / Stop, tool lines, changed slides, Undo per turn, token counts; title-bar spark button, `Ctrl+Shift+A`, commands `assist.ask {text?, send?, toggle?}` and `assist.plan {verdict}` (scriptable through the control channel). The loop became a step machine (`Assistant::begin/on_completion/on_verdict`, `ModelCall` on a worker thread) so the window never blocks; `ask` is now a thin driver over it. Screenshots: [open](screenshots/3.6-pane-open.png), [plan](screenshots/3.6-pane-plan.png), [done](screenshots/3.6-pane-done.png), taken live with Ollama `glm-5.3-flash:cloud` on the sample deck through the control channel. 5 kittest tests with the mock model; 331 tests across assist, ui-egui, mcp and llm pass; clippy, fmt, layers and the wasm check (ui-egui now pulls assist and mcp; both compile for wasm32) green |
| 3.7 `maluos-slides-cli ask` | done 2026-11-02 | `apps/maluos-slides-cli/src/ask.rs` (fork module, fenced hooks in `main.rs` and `help.rs`): `ask "REQUEST" [--in FILE | --sample] [--out FILE] [--mode answer|confirm|autopilot] [--yes] [--provider P] [--model M] [--json] [--quiet] [--max-rounds N] [--stdin]`; the answer streams to stdout, tools and plans to stderr, `--json` prints one report object; plans ask `y/N/q` on a terminal and are declined in scripts without `--yes`. Provider selection (`providers::Choice`, `default_choice`) moved into `maluos-slides-assist` and is shared with the pane; the preview example is gone. Live with Ollama `glm-5.3-flash:cloud`: 4 rounds, `slide.new` + `transition.set`, a 9-slide `.pptx` saved, JSON parsed by a script. 4 tests (30 in the CLI binary). Doc: [cli.md](cli.md) |
| 3.8 HTTP/JSON API | done 2026-11-02 | `crates/maluos-slides-api` (L6 after assist; 31 crates in the layer table) + `maluos-slides-cli api [--listen] [--token] [--in|--sample] [--connect]`: hand-written HTTP/1.1 (no async runtime, no new dependency), loopback only unless a bearer token is set; routes `GET /v1/tools` = `tool_definitions()` unchanged, `POST /v1/tools/{name}` = `call_tool`, `GET|POST /v1/commands[/{id}]`, `GET /v1/document`, `GET /v1/slides/{i}[/render.png]`, `POST /v1/ask` (stateless turn; plans declined unless `yes`); automation roots as for mcp. D9's API half is now in place beside MCP. 5 tests (routes, token and off-loopback rule, ask with the mock, the wire over a real socket, percent decoding) + a curl smoke test on the sample deck. Doc: [api.md](api.md). Not yet: served from inside the app, streaming ask, conversation ids |
