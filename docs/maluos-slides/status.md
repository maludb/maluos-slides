# MaluOS Slides status

> **Updated:** 2026-10-24

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
| 2.1 baseline `cargo xtask ci` | next | |
| 2.2 fork hygiene | README done 2026-10-24 | CLAUDE.md preamble, brand files, links.rs, About box pending |
| 2.3 Linux-only trim | planned | |
| 2.4 rename script, vendor branch | planned | mapping in plan.md |
| 2.5 app identity | planned | |
| 2.6 MaluOS packaging | planned | D8: vendored deb |
| 2.7 boot test and release | planned | |
| 2.8 fonts decision applied | planned | D5 |
| 2.9 upstream sync rehearsal | planned | D7 |
