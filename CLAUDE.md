# MaluOS Slides — instructions for agents (read first)

MaluOS Slides is the MaluOS presentation app, a fork of DeckCraft (`upstream` remote). Everything
below the rule is DeckCraft's own guidance, mechanically renamed, and still applies where it is
not about macOS, Windows or the ArtCraft suite. MaluOS Slides-specific rules:

- **The plan is `docs/maluos-slides/plan.md`; progress is `docs/maluos-slides/status.md`.** Start
  there, not in `plan/` (upstream's local-only folder, which this fork does not have) and not in
  `../../craftrules` (not checked out here).
- **Keep upstream mergeable.** Do not edit `crates/*` except for bug fixes you would send
  upstream. Fork code goes in new crates (`crates/maluos-slides-assist`, the shared `maluos-llm`)
  and fenced hooks in `crates/ui-egui` and `apps/`. Every `deckcraft` name is `maluos-slides` here
  (crates, binaries, env vars `MALUOS_SLIDES_*`, app id `org.maluos.slides`); the native format
  keeps its `.deckcraft` extension. The mapping is `scripts/rename-upstream.sh`, the procedure
  `docs/maluos-slides/rename.md`. Upstream changes arrive through the `upstream-renamed` vendor
  branch, never by hand-editing names.
- **Linux and MaluOS only** (owner decision D1). No macOS, Windows or FreeBSD packaging, CI or
  docs in this tree; platform `cfg` branches inside upstream crates stay as upstream has them.
- **The assistant acts only through engine commands** (`Session::execute`), one undoable history
  group per turn. It never touches the model directly.
- **Keys live in the MaluOS vault** (`/run/maluos/vault.sock`) or an env var; never in settings
  files, decks or the repo.
- **Commit ids** are plan steps: `P2.4: scripted rename, vendor branch`. Run `cargo xtask ci`
  before pushing.
- **This host** is the MaluOS dev VM: it is killed by the hypervisor on page-cache pressure. Run
  cargo under `systemd-run --user --scope -p MemoryHigh=4500M -p MemoryMax=6G` with
  `CARGO_BUILD_JOBS=6` (or `-j 6`). Headless GUI checks: `xvfb-run -a maluos-slides --sample
  --control 7990`, then `maluos-slides-cli app --port 7990 --method ui.screenshot '{"path":…}'`.

---

# MaluOS Slides — working notes for Claude

Read [AGENTS.md](AGENTS.md) first: its rules (asset policy, clean room, never crash) bind you and
win over anything here.

## Start every session
1. Read `plan/STATUS.md`, then the current milestone in `plan/execution-plan.md`.
2. Read `../../craftrules/AGENTS.md` and the standards it lists for your task.
3. Run `cargo xtask ci` before you change anything, so you know the tree is green.

## Habits
- Every user-visible action is a command with tests (`crates/engine/src/cmd/*`).
- Look at UI changes: `maluos-slides --sample --control 7990` + `ui.screenshot`, or
  `maluos-slides-cli render`.
- Commit after each landed arc (`M<n>.<k>: what`) and push to `origin main`.
- Keep `ROADMAP.md`, the progress docs in `docs/` (AGENTS.md §7) and `plan/STATUS.md` current.
- Don't ask the user questions; record genuinely-theirs decisions in `plan/STATUS.md`.
