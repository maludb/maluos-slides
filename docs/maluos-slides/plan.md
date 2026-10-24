# MaluOS Slides plan

> **Created:** 2026-10-24 · **Status:** Phase 1 done, Phase 2 planned · **Upstream:** [storytold/deckcraft](https://github.com/storytold/deckcraft) at `9aef21e` (v0.10.0 + 158)

MaluOS Slides is the MaluOS presentation app: a PowerPoint-compatible desktop app that an LLM can
drive, forked from DeckCraft the way [Malu123](https://github.com/maludb/malu123) was forked from
GridCraft. DeckCraft gives us the engine (320 commands with undo), the renderer, PPTX import and
export, PDF export, media codecs, and an agent surface (MCP server, CLI, JSON control channel).
MaluOS Slides adds the MaluOS fit: a MaluOS package on the desktop image, MaluOS identity, and an
assistant that works on the open deck through the MaluOS gateway. We get there in slices; every
slice leaves an installable app.

## Why DeckCraft

| What we need | What DeckCraft gives us |
|---|---|
| PowerPoint compatibility | `.pptx` open and save that PowerPoint opens without repair; lenient reader keeps unknown parts; PDF, PNG/JPEG, GIF, outline export |
| A real editor | Masters, layouts, themes, 150+ shapes, connectors, tables, charts, SmartArt basics, 48 transitions, 64 animation effects, presenter view, media playback |
| An LLM-ready surface | Every ribbon button is a command (`slide.new`, `shape.insert`, `file.export`…); all 320 reachable over MCP, CLI and a TCP control channel; `ui.screenshot` and `render_slide` exist |
| Fits MaluOS | Pure Rust, two static binaries, no telemetry, no licence server, deb packaging via nfpm, MIT OR Apache-2.0; built and installed on MaluOS 26.04 in Phase 1 |
| A safe base to extend | Enforced crate layering (`cargo xtask layers`), never-crash policy with a panic guard, command and UI soak tests, 202k lines with 1,641 tests |

What it does not have: any LLM client, any chat UI, any MaluOS integration (gateway, vault, agents,
the office), or MaluOS branding. It also carries macOS, Windows, FreeBSD and web packaging that
MaluOS does not need.

## Principles

1. **Stay mergeable with upstream.** DeckCraft moves fast: 1,146 commits in the 30 days before the
   fork. Our value is the MaluOS fit and the assistant, not a better slide engine. Upstream crates
   stay untouched in substance except for merges and bug fixes we send upstream. Our code lives in
   new crates and in clearly fenced UI hooks. The rename goes through a vendor branch,
   `upstream-renamed` = `upstream/main` + one scripted rename commit; `main` branches from it. Sync
   weekly (see step 2.9) because of the velocity.
2. **Linux and MaluOS only** (owner, 2026-10-24). We drop the macOS, Windows and FreeBSD packaging,
   CI and documentation from our tree, and build MaluOS-specific features in their place. Platform
   `cfg` branches inside upstream crates stay as they are, so merges stay clean; we only remove
   what is ours to remove: packaging directories, workflows, README sections.
3. **Everything the assistant does is a command.** It calls the command catalog a click calls, so
   undo, history, the control channel and tests cover it for free.
4. **Local first, provider agnostic.** The MaluOS gateway and local agents are the default LLM
   provider; cloud providers behind the vault. Keys never live in decks or settings files.
5. **Never crash, never silently change a slide.** The assistant proposes, the engine executes, the
   person sees what changed and can undo. DeckCraft's never-crash policy stays in force.
6. **Rust only; wasm keeps compiling** (`cargo xtask wasm` is upstream's gate too). Whether the
   web build ships as a MaluOS web app is decision D4.
7. **One slice per commit set, boot-tested on the MaluOS desktop image before release.**

## Architecture after the fork

```
L0  geom · color · metafile; bitstream · matroska · ogg · opus         (upstream, untouched)
L1  model · fonts; h264 · hevc · vp9 · av1 · isobmff                   (upstream, untouched)
L2  text · anim · format · media                                        (upstream, untouched)
L3  render · pptx                                                       (upstream, untouched)
L4  pdf                                                                 (upstream, untouched)
L5  engine                                                              (upstream)
L5  maluos-llm          SHARED  LLM client trait + backends (gateway, local agent, Claude API,
                                OpenAI-compatible); no egui; one crate for Malu123 and Slides (D6)
L5  maluos-slides-assist NEW    the assistant: deck → prompt context, tool schema from the command
                                catalog, plan/act loop, change log, safety rails; engine + maluos-llm
L6  ui-egui · mcp                                                       (upstream + fenced assistant pane, settings page, identity hooks)
L7  apps/maluos-slides · apps/maluos-slides-cli · apps/maluos-slides-web
```

The new crates register in `xtask/src/layers.rs` (`TABLE`) at L5 so nothing below L6 links egui.
`wasm_set()` includes every L0–L5 crate, so the assist crate compiles for wasm32 (HTTP via `ehttp`
there, `ureq` natively) or is listed as an exception. The MCP server (12 tools today) and the
assistant share one source of truth: the command catalog in `crates/engine/src/cmd/*`.

## Phases

Estimates are agent-hours on this host (one agent; a release build takes ~6 min, `cargo xtask ci`
longer; the VM is memory-capped).

### Phase 1: build and install (done 2026-10-24)

Repository, release build, `.deb` via nfpm, install and smoke test on MaluOS 26.04. Record in
[status.md](status.md).

### Phase 2: MaluOS Slides identity (the rename and the Linux-only tree)

| Step | Work | Done when | Est. |
|---|---|---|---|
| 2.1 | **Baseline.** `cargo xtask ci` on this host before any change (fmt, pdf-deps, clippy, test, doctest, assets, layers, wasm); record skips that are environment-only (fonts, display) | green, skips listed in status.md | 1–2 h |
| 2.2 | **Fork hygiene.** `CLAUDE.md` gets a MaluOS Slides preamble (plan location, mergeability rules, build cap) above upstream's notes; `AGENTS.md` notes the brand-file removal; `docs/brand/` removed with its `ATTRIBUTION.md` rows (their licence requires it of forks; `xtask assets` still passes); `crates/engine/src/links.rs` points at MaluOS (GitHub repo, maluos.com) and the About box and Backstage credit DeckCraft; README already rewritten (this commit) | `cargo xtask ci` green; About box shows the credit | 2–3 h |
| 2.3 | **Linux-only trim.** Delete `packaging/macos`, `packaging/windows`, `packaging/freebsd` and the workflows `freebsd.yml`, `windows-arm64.yml`, `nix.yml`, `sonarcloud.yml`; `ci.yml` and `release.yml` run on Ubuntu only and build deb + tar (+ web per D4); `.coderabbit.yaml`, `sonar-project.properties` and `omastore.toml` go; `packaging/linux` keeps AppImage/rpm scripts but we publish only the deb. Upstream `docs/*.md` stay untouched (they are merged, not ours) | the fork's CI runs on GitHub and is green on push | 2–3 h |
| 2.4 | **The rename script.** Port `scripts/rename-upstream.sh` + `.py` from Malu123 with the mapping below; protect the native extension `.deckcraft`, its MIME type and upstream credits and links; run it on upstream `9aef21e` to create `upstream-renamed`, merge into `main`; `cargo fmt`; `docs/maluos-slides/rename.md` documents mapping and sync | `cargo xtask ci` green on the renamed tree; `maluos-slides-cli --version`, MCP `serverInfo.name`, control channel verified; zero `deckcraft` outside credits | 4–6 h |
| 2.5 | **App identity.** Original app icon (SVG drawn in code or by hand, MaluOS palette, hicolor sizes via `packaging/icons.sh`), window title, About box, metainfo description and screenshots, sample deck's "made in DeckCraft" note, `StartupWMClass`; settings dir `~/.config/maluos-slides`, log `maluos-slides.log`; `ATTRIBUTION.md` row for the icon | app runs as MaluOS Slides; screenshot in status.md | 3–4 h |
| 2.6 | **Packaging the MaluOS way.** nfpm produces `maluos-slides_<ver>_amd64.deb` (vendor MaluOS, maintainer MaluOS, homepage GitHub); the maluos repo gets it either vendored like the maludb debs (`vendor/maluos-slides/`, a `scripts/vendor-maluos-slides.sh`) or as `packages/maluos-slides` wrapping the built binaries, whichever `build-packages.sh` handles with less change (recommendation: vendor, as for maludb-core); desktop entry in the Office folder of the MaluOS grid; `maluos-desktop` recommends it | installs from the MaluOS archive on a desktop image; launches from the grid; opens `.pptx` by double-click | 3–5 h |
| 2.7 | **Boot test and release.** On the Phase 12 desktop ISO: launch, open and save `.pptx`, present full screen, presenter view on one display, PDF export, Wayland and X11 sessions, HiDPI, dark theme follows the desk | results in status.md; shipped in a MaluOS release | 2–3 h |
| 2.8 | **Fonts decision applied (D5).** Either keep craft-fonts embedded (today, +41 MB) or rely on MaluOS system fonts with the Office-metric substitutes (Carlito, Caladea, Liberation) installed by `maluos-desktop`; verify Latin, CJK and Arabic rendering on the image | screenshots in status.md | 1–3 h |
| 2.9 | **Upstream sync rehearsal.** Merge `upstream/main` through `upstream-renamed` once; measure conflicts and time; set the cadence (recommendation: weekly, Mondays, after the MaluOS release) | procedure in rename.md with the measured cost | 1–2 h |

**Phase 2 total: ~19–31 h.** Steps 2.1–2.3 can land before the rename; 2.4 is the pivot; 2.5–2.9
follow in order.

#### The rename mapping (step 2.4)

| Upstream | MaluOS Slides | Where |
|---|---|---|
| `deckcraft-geom`, `deckcraft-engine`, … | `maluos-slides-geom`, `maluos-slides-engine`, … | crate names, `Cargo.toml`, `Cargo.lock` |
| `deckcraft_geom::`, … | `maluos_slides_geom::`, … | Rust paths |
| `deckcraft`, `deckcraft-cli`, `deckcraft-web` | `maluos-slides`, `maluos-slides-cli`, `maluos-slides-web` | packages, binaries, `apps/` directories |
| `ai.storyteller.deckcraft` | `org.maluos.slides` | app id: desktop file, metainfo, MIME, flatpak manifest, icon file names, `StartupWMClass` |
| `DECKCRAFT_*` | `MALUOS_SLIDES_*` | env vars (`MALUOS_SLIDES_NO_PREFS`, `MALUOS_SLIDES_SOAK_STEPS`, `MALUOS_SLIDES_BUILD_SHA`, …) |
| `DeckCraft` | `MaluOS Slides` | product name: window title, About, metainfo, PPTX `<Application>`, PDF producer, MCP server title |
| `deckcraft://` | `maluos-slides://` | MCP resource URIs |
| `~/.config/deckcraft`, `deckcraft.log` | `~/.config/maluos-slides`, `maluos-slides.log` | settings and logs |
| `Learning Machines LLC`, `deckcraft@storyteller.ai` | `MaluOS`, the MaluOS maintainer address | package vendor and maintainer |
| `getartcraft.com/apps/deckcraft`, `discord.gg/artcraft` | `github.com/maludb/maluos-slides`, MaluOS links | `links.rs`, menus, packaging |
| `storytold/deckcraft` (bare repo path) | `maludb/maluos-slides` | `repository` in `Cargo.toml`, bug tracker, release scripts, AppImage update info |

Kept verbatim (protected in the script): the native format extension `.deckcraft` and MIME type
`application/x-deckcraft` (decision D3: decks made with upstream keep opening; the format is
upstream's and still migrates from `.slidecraft`), `DeckCraft contributors`, `DeckCraft authors`,
upstream issue, PR, release and file links (`storytold/deckcraft#123`, `/issues/`, `/pull/`,
`/releases`, `/blob/`), `contributors/` (upstream's list), `craft-fonts` and `craft-libs`
(separate upstream repos we depend on). `README.md`, `NOTICE`, the licences and
`docs/maluos-slides/` are never rewritten by the script.

### Phase 3: the assistant

| Step | Work | Done when |
|---|---|---|
| 3.1 | `maluos-llm` (D6): the `LlmClient` trait (messages, system prompt, tool definitions, streaming text and tool-call events, usage; typed errors) with the gateway and local-agent backends first, Claude API and OpenAI-compatible behind the vault. Shared with Malu123; if Malu123 writes it first, Slides consumes it as a git dependency | mock-backend unit tests; works against the demo agent on the dev VM |
| 3.2 | Context builder: deck outline for the prompt (sections, slide titles, placeholder text, notes, the selection's shapes and text, theme), size-capped with truncation markers; cache keyed on document revision | snapshot tests on the sample deck |
| 3.3 | Tool schema generated from the command catalog, the same mapping the MCP server uses (`document`, `set_text`, `add_shape`, `insert_chart`, `go_to_slide`, `render_slide`, `run_command`, `export`…); one table drives MCP and the assistant | MCP tests still pass; schema snapshot |
| 3.4 | The loop: message → prompt → model → tool calls through `Session::execute` as one undoable history group per turn → results → answer; limits on turns, tool calls and tokens | headless test with the mock backend: "add a closing slide that thanks the audience" produces `slide.new` + `set_text` |
| 3.5 | Safety rails: answer-only, confirm-before-change (shows the planned commands), autopilot (applies, shows the slides it touched); destructive commands (delete slide or section, replace all text, file.new) always confirm | tests per mode |
| 3.6 | Assistant pane in the UI (a right-hand pane like Format Shape): conversation, streaming answer, the commands it ran with Undo per turn, mode selector, provider status; "Ask Malu" ribbon button and `Ctrl+Shift+A` | screenshots in status.md |
| 3.7 | `maluos-slides-cli ask "…" --in deck.pptx --out deck.pptx` for scripts and tests | documented in `docs/maluos-slides/cli.md` |

### Phase 4: slides-specific depth

| Step | Work |
|---|---|
| 4.1 | Deck from a document: "make a 10-slide deck from this outline / Markdown / Malu123 sheet"; section and layout choice by the model, themes from the built-in eight |
| 4.2 | Slide-level help: rewrite bullets, tighten a title, generate speaker notes, suggest a chart for a table, fix a layout (align, distribute, autofit) |
| 4.3 | Data slides from MaluDB: a chart or table slide bound to a MaluDB query through the gateway, refreshed on command |
| 4.4 | Presenter assistant: notes summary and timing hints in presenter view; Q&A prep from the deck |
| 4.5 | Pictures from the office's image agent when one is configured (MaluOS Phase 13 delegation envelope); nothing fetched from the public web by default |
| 4.6 | Memory across sessions: per-deck notes stored in the `.deckcraft` package (`presentation.json` extension field) or a PPTX custom XML part, nothing outside the file |

### Phase 5: parity through upstream

Track upstream's beta gates (real-deck PPTX corpus, round-trip losses, EMF/SVG, printing,
localization) by merging upstream weekly; contribute our fixes upstream. Our MaluOS-only trim
(2.3) must never touch upstream crates, so these merges stay cheap.

## Repository conventions

- `docs/maluos-slides/` holds everything fork-specific: this plan, `status.md`, `rename.md`,
  later `llm.md`, `assistant.md`, `packaging.md`. Upstream docs stay as they are.
- Commits: one step id per commit (`P2.4: scripted rename, vendor branch`), co-author lines as the
  MaluOS repos use them. Branches: `main` is ours; `upstream-renamed` is the vendor branch; merges
  land as merge commits.
- Quality gate before every push: `cargo xtask ci`. New crates carry upstream's clippy denies
  (`unwrap_used`, `expect_used`, `panic`, `todo`, `unreachable`).
- Build on the dev VM under `systemd-run --user --scope -p MemoryHigh=4500M -p MemoryMax=6G` with
  `-j 6`; the recipe is in status.md.
- Never commit: fonts, PowerPoint-authored files, screenshots of Microsoft software, API keys,
  `dist/`, `target/`.

## Decisions

Recorded owner decisions and the ones this plan needs. Recommendations are the defaults the work
proceeds with unless the owner says otherwise.

| # | Decision | Status | Consequence |
|---|---|---|---|
| D1 | **Linux and MaluOS only**; non-Linux docs, packaging and CI go; MaluOS-specific features take their place | **owner, 2026-10-24** | principle 2, step 2.3, README rewritten |
| D2 | Names: product "MaluOS Slides", binaries `maluos-slides` and `maluos-slides-cli`, crates `maluos-slides-*`, app id `org.maluos.slides` | recommended | step 2.4 mapping; consistent with `org.maluos.malu123` |
| D3 | Native format keeps `.deckcraft` and `application/x-deckcraft` in Phase 2 | recommended | zero file-compat risk; revisit if the format diverges from upstream |
| D4 | Web build: keep `cargo xtask wasm` green, do not ship a web app until Malu123's web package (its step 1.6) exists; then reuse that packaging | recommended | `packaging/web` stays, `release.yml` skips it for now |
| D5 | Fonts: keep craft-fonts embedded for Phase 2 (binaries +41 MB) so decks render the same everywhere; reconsider when `maluos-desktop` ships the Office-metric fonts | recommended | step 2.8 |
| D6 | One shared `maluos-llm` crate for Malu123 and Slides, in its own repo (`maludb/maluos-llm`), consumed as a git dependency | recommended | Malu123's planned `malu123-llm` becomes it; avoids two LLM clients |
| D7 | Upstream sync weekly through the vendor branch | recommended | step 2.9 measures the real cost |
| D8 | Where the MaluOS deb lives: vendored prebuilt deb in the maluos repo (like maludb-core) rather than a `packages/` source build | recommended | step 2.6; a Rust build inside `build-packages.sh` would need the toolchain and ~6 min per release |
