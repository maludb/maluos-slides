# The rename: DeckCraft to MaluOS Slides

> **Created:** 2026-10-24 · plan step 2.4

Every crate, binary and identifier is renamed, not only the apps (decision D2 in
[plan.md](plan.md)). One script holds the whole mapping, `scripts/rename-upstream.sh` (what the
fork drops, path moves, `cargo fmt`) with the text rules in `scripts/rename-upstream.py`
(`--list` prints them). It is idempotent.

## Mapping

| Upstream | MaluOS Slides | Where |
|---|---|---|
| `deckcraft-geom`, `deckcraft-engine`, … | `maluos-slides-geom`, `maluos-slides-engine`, … | crate names, `Cargo.toml`, `Cargo.lock` |
| `deckcraft_geom::`, … | `maluos_slides_geom::`, … | Rust paths |
| `deckcraft`, `deckcraft-cli`, `deckcraft-web` | `maluos-slides`, `maluos-slides-cli`, `maluos-slides-web` | packages, binaries, `apps/` directories |
| `ai.storyteller.deckcraft`, developer id `ai.storyteller` | `org.maluos.slides`, `org.maluos` | app id: desktop file, metainfo, MIME, flatpak manifest, icon file names, `StartupWMClass` |
| `DECKCRAFT_*` | `MALUOS_SLIDES_*` | environment variables (`MALUOS_SLIDES_NO_PREFS`, `MALUOS_SLIDES_SOAK_STEPS`, `MALUOS_SLIDES_BUILD_SHA`, …) |
| `DeckCraft` | `MaluOS Slides` | product name: window title, About, metainfo, PPTX `<Application>`, PDF producer, MCP server title |
| `deckcraft` as a log target (`warn,deckcraft*=info`, `"deckcraft::audio"`) | `maluos_slides` | the desktop binary's crate path has an underscore; `apps/maluos-slides/src/logging.rs` |
| `deckcraft://commands` | `maluos-slides://commands` | MCP resource URIs; MCP `serverInfo.name` is `maluos-slides` |
| `~/.config/deckcraft`, `deckcraft.log`, `~/.local/share/deckcraft/recovery` | `~/.config/maluos-slides`, `maluos-slides.log`, `~/.local/share/maluos-slides/recovery` | settings, logs, AutoRecover |
| `Learning Machines LLC`, `DeckCraft maintainers <deckcraft@storyteller.ai>` | `MaluOS`, `Edward Honour <ed@maludb.org>` | package vendor and maintainer (nfpm, metainfo) |
| `getartcraft.com/apps/deckcraft`, `discord.gg/artcraft` | `github.com/maludb/maluos-slides`, its Discussions | `crates/engine/src/links.rs`, About, Backstage, metainfo |
| `storytold/deckcraft` (bare repo path) | `maludb/maluos-slides` | `repository` in `Cargo.toml`, bug tracker, AppImage update info |

**Kept verbatim** (protected in the script): the native format, `.deckcraft` files, the format
id `"deckcraft"` inside `presentation.json` and the commands' `format` parameter, and the MIME
type `application/x-deckcraft` (decision D3: decks made with upstream keep opening and the format
is upstream's); `DeckCraft contributors` and `DeckCraft authors` (credits); links to upstream
issues, pull requests, releases and files (`storytold/deckcraft#123`, `/issues/`, `/pull/`,
`/releases`, `/blob/`); `contributors/contributors.json`'s repo (upstream's contributor list);
`craft-fonts` and `craft-libs` (separate upstream repositories we depend on). `README.md`, the
licences, `docs/maluos-slides/` and the fork's own workflows (`.github/workflows/maluos-*.yml`)
are never rewritten.

**Dropped by the script** (decision D1, Linux and MaluOS only): `docs/brand/` (ArtCraft
trademarks; their licence requires forks to remove them, and `ATTRIBUTION.md`, `AGENTS.md` and
`NOTICE` say so), `packaging/macos`, `packaging/windows`, `packaging/freebsd`,
`packaging/icons.sh` with the `.icns` and `.ico` icons (only `packaging/macos/Info.plist.in` and
`packaging/windows/maluos-slides.wxs` stay, as fixtures of the desktop app's test
`every_desktop_offers_the_same_file_types`; nothing builds them), upstream's workflows (they call
storytold's shared gate and release workflows, which build for every platform), `.coderabbit.yaml`,
`sonar-project.properties` and `omastore.toml`. The fork's own CI, release and packaging-lint
workflows live in `.github/workflows/maluos-*.yml`.

Not renamed on purpose: the Windows-only `apps/maluos-slides/build.rs` resource strings and the
flatpak manifest's runtime details (we publish a deb); the `.slidecraft` legacy extension upstream
still migrates from.

## Branches

- `upstream-renamed`: `upstream/main` plus the script plus one commit that ran it. Nothing else.
- `main`: branches from `upstream-renamed` and carries all MaluOS Slides work.

## Syncing upstream

```sh
git fetch upstream
git checkout upstream-renamed
git merge upstream/main            # git follows the moved files; conflicts are lines that
                                   # upstream changed and the rename also touched
./scripts/rename-upstream.sh       # rewrites what upstream added, resolves most conflicts
git add -A && git commit           # "sync upstream <sha>"
git checkout main
git merge upstream-renamed
cargo xtask ci
```

If a merge conflict remains after the script, resolve it by hand keeping the MaluOS Slides names,
and add a rule to the script if the same pattern will come back. Upstream lands ~40 commits a day,
so sync weekly (decision D7); step 2.9 measures the first one.
