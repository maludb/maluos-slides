#!/usr/bin/env bash
# DeckCraft -> MaluOS Slides. The single source of the name mapping (docs/maluos-slides/rename.md).
#
# Run from the repository root on a clean tree. Idempotent. Does, in order:
#   1. removes what the fork drops: upstream's ArtCraft brand files (their licence requires it of
#      forks), the macOS, Windows and FreeBSD packaging and icons (two metadata files stay as
#      test fixtures), upstream's workflows and the
#      configs of services the fork does not use (decision D1 in docs/maluos-slides/plan.md)
#   2. git-moves every tracked path whose name contains "deckcraft" (app id first; the native
#      format extension `.deckcraft` stays)
#   3. rewrites the text of every tracked file with scripts/rename-upstream.py
#   4. cargo fmt (longer identifiers make rustfmt rewrap lines) and re-sorts Cargo.lock
#
# Upstream sync (vendor branch): on `upstream-renamed`, `git merge upstream/main`, resolve any
# conflicts by re-running this script, commit, then `git merge upstream-renamed` on main.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Files this script never rewrites: they describe the fork itself.
SKIP='^(README\.md|LICENSE-MIT|LICENSE-APACHE|docs/maluos-slides/.*|scripts/rename-upstream\..*|\.github/workflows/maluos-.*)$'

# 1. what the fork drops
# (`packaging/macos/Info.plist.in` and `packaging/windows/deckcraft.wxs` stay: the desktop app's test
# `every_desktop_offers_the_same_file_types` reads them as fixtures; they are not built.)
git ls-files packaging/macos packaging/windows | grep -vE '^packaging/(macos/Info\.plist\.in|windows/(deckcraft|maluos-slides)\.wxs)$' | xargs -r -d '\n' git rm -q
git rm -rq --ignore-unmatch \
  docs/brand \
  packaging/freebsd packaging/icons.sh \
  assets/app-icon/deckcraft.icns assets/app-icon/deckcraft.ico \
  .github/workflows/ci.yml .github/workflows/release.yml .github/workflows/packaging-lint.yml \
  .github/workflows/freebsd.yml .github/workflows/nix.yml .github/workflows/sonarcloud.yml \
  .github/workflows/windows-arm64.yml \
  .github/dependabot.yml .coderabbit.yaml sonar-project.properties omastore.toml
# ATTRIBUTION rows of the removed assets, and the three places that call the brand files "the one exception"
sed -i -e '/^| `docs\/brand\//d' -e '/^| `packaging\/macos\//d' -e '/^| `assets\/app-icon\/deckcraft\.icns`/d' ATTRIBUTION.md
sed -i 's/fails if a file under `assets\/`, `docs\/images\/` or `docs\/brand\/` is missing from this table\./fails if a file under `assets\/` or `docs\/images\/` is missing from this table. The ArtCraft brand files of upstream DeckCraft (`docs\/brand\/`) are trademarks and are removed from this fork, as their licence requires./' ATTRIBUTION.md
python3 - <<'EOF'
import re
p = "AGENTS.md"
s = open(p).read()
s = re.sub(r"One narrow exception: the ArtCraft name, wordmark and logos in `docs/brand/` are trademarks of the\nArtCraft Team \(not open source, terms in `docs/brand/LICENSE-brand.txt`\), used only unmodified to\nidentify the project\.",
           "The ArtCraft brand files of upstream DeckCraft (`docs/brand/`) are trademarks and are removed from\nthis fork, as their licence requires; upstream DeckCraft is credited in the About box and the README.", s)
s = s.replace("fails if a file under `assets/`, `docs/images/`\n  or `docs/brand/` has no row.", "fails if a file under `assets/` or `docs/images/`\n  has no row.")
open(p, "w").write(s)
p = "NOTICE"
s = open(p).read()
s = re.sub(r"The ArtCraft name, wordmark and logos in docs/brand/ are trademarks of the ArtCraft Team\. They are\nnot open source and are not covered by the license above; their terms are in\ndocs/brand/LICENSE-brand\.txt\.",
           "MaluOS Slides is a fork of DeckCraft. The ArtCraft name, wordmark and logos are trademarks of the\nArtCraft Team; upstream's brand files (docs/brand/) are not included in this fork, as their\nlicence requires. MaluOS additions are Copyright (c) 2026 MaluOS, under the same licenses.", s)
open(p, "w").write(s)
EOF

# 2. paths (the native format extension `.deckcraft` is not a name)
{ git ls-files | grep -i deckcraft || true; } | sort -r | while read -r old; do
  new=$(printf '%s' "$old" | sed -e 's/ai\.storyteller\.deckcraft/org.maluos.slides/g' -e 's/\.deckcraft$/@@EXT@@/' -e 's/deckcraft/maluos-slides/g' -e 's/@@EXT@@/.deckcraft/')
  [ "$old" = "$new" ] && continue
  mkdir -p "$(dirname "$new")"
  git mv -k "$old" "$new"
done
find apps crates packaging assets -type d -empty -delete 2>/dev/null || true

# 3. text
git ls-files | grep -Ev "$SKIP" | xargs -d '\n' python3 scripts/rename-upstream.py

# 4. fmt, and Cargo.lock in cargo's canonical order (the renamed packages sort differently;
#    `cargo update --workspace --offline` rewrites the file without changing any version)
if command -v cargo >/dev/null; then
  cargo fmt --all
  cargo update --workspace --offline --quiet
fi

left=$(git ls-files | grep -Ev "$SKIP" | xargs -d '\n' grep -Il -i deckcraft 2>/dev/null | wc -l)
echo "rename-upstream.sh: done; $left file(s) still mention deckcraft (expected: the .deckcraft format, upstream credits and links)"
