# MaluOS Slides status

> **Updated:** 2026-11-02 (08:10 UTC)

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

### Build recipe (dev VM)

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
in this clone yet. The VM now reports 31 GB RAM; the build cap in `CLAUDE.md` still applies.

**`cargo xtask ci` on this host:** fmt, pdf-deps, clippy, doctest, assets (34 files), layers
(28 crates) and wasm (23 crates) pass; tests: 1,788 of 1,789 pass, 6 skipped. The one failure is
environment-only: `maluos-slides-pdf tests::unembeddable_font_returns_error_instead_of_a_pdf_missing_text`
builds its damaged font from `FontDb::global().face("Ubuntu", "Regular")`; craft-fonts ships no
Ubuntu, so that resolves to MaluOS's system `Ubuntu[wdth,wght].ttf` (fonts-ubuntu 0.869), a
variable font whose 12 named instances make `add_font` return 12 instead of 1. Fix to send
upstream: build the fixture from an embedded craft font (Inter) instead of a system family.
Smoke: `maluos-slides-cli render --sample --all` draws all 8 slides; `run --sample --save x.pptx`
writes a 58-part package with real `ppt/slides/slideN.xml`; the MCP handshake above.
