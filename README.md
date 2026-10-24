<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/ai.storyteller.deckcraft.png" width="128" height="128" alt="App icon: a presentation screen with a bar chart">
</p>

<h1 align="center">MaluOS Slides</h1>

<p align="center">
  <b>The MaluOS presentation app: slides, slide shows and PowerPoint files, in pure Rust.</b>
</p>

<p align="center">
  Build decks with themes, layouts, shapes, pictures, tables, charts, transitions and animations,
  open and save .pptx files, and present them full screen. Every button is also a command that the
  CLI and AI agents (through MCP) can drive. Built for MaluOS and Linux.
</p>

<p align="center">
  <img alt="Status: early development" src="https://img.shields.io/badge/status-early%20development-f26b1d?style=flat-square">
  <img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square">
  <img alt="Written in Rust" src="https://img.shields.io/badge/written%20in-Rust-b7410e?style=flat-square">
  <img alt="Platform: Linux" src="https://img.shields.io/badge/platform-Linux%20%C2%B7%20MaluOS-555?style=flat-square">
</p>

MaluOS Slides is a fork of [DeckCraft](https://github.com/storytold/deckcraft), the open-source,
clean-room presentation app by the ArtCraft team. It keeps DeckCraft's engine, renderer, PPTX
support and agent surface, and adds the MaluOS fit: a MaluOS package, the MaluOS desktop, and, step
by step, an LLM assistant that works through the MaluOS gateway. Upstream is tracked as the
`upstream` remote and merged regularly; the plan is in
[`docs/maluos-slides/plan.md`](docs/maluos-slides/plan.md) and progress in
[`docs/maluos-slides/status.md`](docs/maluos-slides/status.md).

> **Naming.** Until the rename step in the plan lands, the crates, binaries and app id still carry
> DeckCraft's names (`deckcraft`, `deckcraft-cli`, `ai.storyteller.deckcraft`). The commands below
> use the current names.

<p align="center">
  <img alt="Editing a slide: ribbon, slide thumbnails with sections, a process diagram on the canvas, notes and status bar" src="docs/images/editor.png" width="900">
</p>

## What works today

The app is in alpha, heading for beta (see upstream's [roadmap](ROADMAP.md)). Today it can:

- **Make decks**: slide masters and eleven layouts with placeholders that inherit position and
  formatting, eight original themes with colour and font schemes, sections, notes, comments,
  headers and footers, custom slide sizes.
- **Draw**: 150+ preset shapes with adjust handles, connectors that stay glued to shapes, freeform,
  curve and scribble tools, merge shapes, gradient, picture and pattern fills, outlines with dashes
  and arrowheads, shadows, glow, soft edges and reflections.
- **Write**: in-place text editing with full font and paragraph formatting, bullets and numbering,
  columns, autofit, vertical text, fields and hyperlinks.
- **Add content**: pictures with crop and corrections, tables with styles, charts, SmartArt basics,
  audio and video (MP3, AAC, FLAC, ALAC, Ogg, Opus, WAV, AIFF; H.264, HEVC, VP9, AV1).
- **Present**: transitions (including Morph), entrance, emphasis, exit and motion-path animations,
  by-paragraph builds, full-screen slide show, presenter view, pen, rehearse timings, media playback.
- **Exchange files**: open and save .pptx presentations, export PDF (slides, notes pages, handouts
  with selectable text), PNG/JPEG, an animated GIF of the deck and outlines. AutoRecover keeps
  unsaved work.
- **Automate**: 320 commands with undo, all scriptable from the CLI, a JSON control channel and an
  MCP server for AI agents.

<table>
  <tr>
    <td width="50%"><img alt="A selected shape with the Shape Format tab" src="docs/images/shape-format.png"><br><sub>Shape Format contextual tab and selection handles</sub></td>
    <td width="50%"><img alt="The Format Shape pane editing a gradient fill" src="docs/images/format-pane.png"><br><sub>Format Shape pane: gradient type, direction and stops</sub></td>
  </tr>
  <tr>
    <td width="50%"><img alt="Slide Sorter view with transitions under each slide" src="docs/images/slide-sorter.png"><br><sub>Slide Sorter with each slide's transition</sub></td>
    <td width="50%"><img alt="Dark mode showing a chart slide" src="docs/images/dark-mode.png"><br><sub>Dark mode, with a native chart</sub></td>
  </tr>
</table>

## Building on MaluOS or Ubuntu

Rust 1.95 or newer (rustup), plus the Linux build dependencies:

```sh
sudo apt install build-essential pkg-config cmake libasound2-dev libxkbcommon-dev
```

Fonts come from [craft-fonts](https://github.com/storytold/craft-fonts): clone it next to this
repository (`../craft-fonts`) and the build embeds its presentation fonts; without it the app uses
the system fonts. PDF export uses the shared `craft-pdf` writer; see
[`crates/pdf/README.md`](crates/pdf/README.md).

```sh
git clone https://github.com/maludb/maluos-slides
cd maluos-slides
cargo build --release --locked -p deckcraft -p deckcraft-cli
target/release/deckcraft-cli render --sample --all --scale 1 out/   # the sample deck as PNGs
target/release/deckcraft                                            # the desktop app
```

On the MaluOS dev VM, run cargo under a memory cap; the exact recipe is in
[`docs/maluos-slides/status.md`](docs/maluos-slides/status.md).

### The Debian package

```sh
packaging/linux/package.sh --skip-build --formats "deb tar"   # needs nfpm on PATH
sudo apt install ./dist/release/deckcraft-<ver>-linux-x86_64.deb
```

The package installs the desktop app and the CLI into `/usr/bin`, a desktop entry, the MIME types
for `.pptx` and the native format, and the icons. It depends on `libxkbcommon0`, `libasound2` and
`libvulkan1` or `libegl1`; the window-system and GPU libraries are opened at run time, so
`packaging/linux/nfpm.yaml` lists them as dependencies and recommendations. In a VM or without a
GPU, Mesa's software Vulkan driver (`mesa-vulkan-drivers`) is enough. Publishing through the MaluOS
apt repository is a plan step.

### Logs

The desktop app writes its `log` records to standard error and to `logs/deckcraft.log` in the
settings directory, beside `ui.json` and `prefs.json` (`~/.config/deckcraft/`, or
`$XDG_CONFIG_HOME/deckcraft/`). A start from the desktop grid has no terminal, so this file is what
to attach to a bug report: the crash guard's panic report, a failed crash-recovery save, parts
skipped on import and audio output problems land there. Each launch moves the previous log to
`deckcraft.1.log` (and that one to `deckcraft.2.log`). The file stops growing at 16 MiB.

| Variable | Effect |
|---|---|
| `RUST_LOG` | Log levels for standard error and the log file. Default: `info` for the app's own crates, `warn` for everything else. env_logger-style directives replace that, e.g. `RUST_LOG=debug` or `RUST_LOG=warn,deckcraft_pptx=trace`. |

## Agents, CLI and MCP

Every action is a command with a stable id, so people, scripts and AI agents use the same verbs:

```sh
deckcraft-cli commands format.          # list commands
deckcraft-cli run --sample --cmd 'slide.new={"layout":"titleOnly","title":"Hello"}' --save hello.deckcraft
deckcraft-cli mcp                       # MCP server over stdio (headless)
deckcraft-cli mcp --connect 7990        # drive the running app (deckcraft --control 7990)
deckcraft-cli mcp --automation-read-root DIR --automation-write-root DIR   # confine the headless agent to folders
```

The control channel is documented in [`docs/control-protocol.md`](docs/control-protocol.md) and
the MCP server in [`docs/mcp.md`](docs/mcp.md).

## Roadmap

MaluOS Slides work is planned in [`docs/maluos-slides/plan.md`](docs/maluos-slides/plan.md).
Feature parity with PowerPoint is upstream's work, tracked in [ROADMAP.md](ROADMAP.md),
[`docs/target-app-parity.md`](docs/target-app-parity.md) and [`docs/gaps.md`](docs/gaps.md); we
merge it rather than redo it, and send fixes upstream.

## License and credits

MaluOS Slides is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option, like DeckCraft. Copyright (c) 2026 ArtCraft Team and the DeckCraft contributors for the
upstream code; MaluOS additions copyright (c) 2026 MaluOS. Required notices are in
[NOTICE](NOTICE).

Bundled fonts, icons, images and other assets keep their own open licenses; each one is listed
with its author, source and license in [ATTRIBUTION.md](ATTRIBUTION.md). Themes, layouts, shape
geometry, icons and sample decks are original work, drawn or generated in code; no proprietary
artwork, fonts, themes or templates are used.

The ArtCraft name, wordmark and logos are trademarks of the ArtCraft Team. Their brand files are
not part of this fork, as their licence requires; DeckCraft is credited here and in the About box.
