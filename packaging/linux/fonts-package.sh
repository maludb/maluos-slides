#!/usr/bin/env bash
# Build the maluos-fonts .deb from a craft-fonts checkout: the faces the three manifests list
# (fonts/latin-manifest.txt, fonts/manifest.txt, fonts/simplified-chinese-manifest.txt), each
# checked against its manifest sha256, installed under /usr/share/fonts/truetype/maluos-fonts/
# with the licence files and a copyright file. The MaluOS office apps load them from there at
# run time instead of embedding them (MaluOS Slides plan step 2.8).
#
#   packaging/linux/fonts-package.sh [--out DIR]
#   env: CRAFT_FONTS_DIR (default ../craft-fonts), MALUOS_SLIDES_MAINTAINER
#
# Output: $DIST/maluos-fonts_<version>-1_all.deb, version = the checkout's commit date
# (YYYY.M.D) so a newer catalogue sorts later.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/packaging/linux"
DIST="${DIST:-$ROOT/dist/release}"
FONTS="${CRAFT_FONTS_DIR:-$ROOT/../craft-fonts}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) DIST="$2"; shift 2 ;;
    *) echo "usage: $0 [--out DIR]" >&2; exit 2 ;;
  esac
done
[[ -f "$FONTS/fonts/manifest.txt" ]] || { echo "error: no craft-fonts checkout at $FONTS (set CRAFT_FONTS_DIR)" >&2; exit 1; }
command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
export MALUOS_SLIDES_MAINTAINER="${MALUOS_SLIDES_MAINTAINER:-Edward Honour <ed@maludb.org>}"

COMMIT="$(git -C "$FONTS" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"
VERSION="${MALUOS_FONTS_VERSION:-$(git -C "$FONTS" log -1 --format=%cd --date=format:%Y.%-m.%-d 2>/dev/null || date +%Y.%-m.%-d)}"
export VERSION
WORK="${CARGO_TARGET_DIR:-$ROOT/target}/fonts-package"
STAGE="$WORK/root"
rm -rf "$WORK"
FONTDIR="$STAGE/usr/share/fonts/truetype/maluos-fonts"
DOC="$STAGE/usr/share/doc/maluos-fonts"
mkdir -p "$FONTDIR" "$DOC/licenses" "$DIST"
{
  echo "# The MaluOS fonts catalogue, in fallback order: family | style | file | ISO 15924 scripts."
  echo "# Read by the MaluOS office apps at run time (maluos-slides-fonts::craft_fonts). craft-fonts $COMMIT."
} >"$FONTDIR/manifest.txt"

echo "==> maluos-fonts $VERSION from craft-fonts $COMMIT"
{
  echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/"
  echo "Upstream-Name: craft-fonts"
  echo "Source: https://github.com/storytold/craft-fonts (commit $COMMIT)"
  echo "Comment: Every face is under the SIL Open Font License 1.1; each family's licence"
  echo " file is in licenses/. Family, style, file, scripts, licence and source per face:"
  echo
} >"$DOC/copyright"
n=0
for manifest in latin-manifest manifest simplified-chinese-manifest; do
  while IFS='|' read -r family style file scripts licence licence_file sha source; do
    file="$(echo "$file" | xargs)"; [[ -z "$file" ]] && continue
    family="$(echo "$family" | xargs)"; style="$(echo "$style" | xargs)"; sha="$(echo "$sha" | xargs)"
    licence="$(echo "$licence" | xargs)"; licence_file="$(echo "$licence_file" | xargs)"; source="$(echo "$source" | xargs)"
    # The same exclusions as crates/fonts/build.rs (AGENTS.md §1: no Adobe-authored faces, no
    # Noto CJK rebrand of Source Han).
    case "$family" in
      "Source Sans 3"|"Source Serif 4"|"Source Han Sans"*|"Source Han Serif"*|"Noto Sans CJK"*|"Noto Serif CJK"*|"Noto Sans Mono CJK"*) continue ;;
    esac
    src="$FONTS/$file"
    [[ -f "$src" ]] || { echo "error: $file listed in $manifest.txt is missing" >&2; exit 1; }
    echo "$sha  $src" | sha256sum -c --quiet - || { echo "error: $file does not match its manifest sha256" >&2; exit 1; }
    sub="$(basename "$(dirname "$file")")"
    install -Dm644 "$src" "$FONTDIR/$sub/$(basename "$file")"
    if [[ -f "$FONTS/$licence_file" ]]; then
      install -Dm644 "$FONTS/$licence_file" "$DOC/licenses/$sub-$(basename "$licence_file")"
    fi
    printf 'Files: fonts/truetype/maluos-fonts/%s/%s\n Family: %s %s\n Scripts: %s\nLicense: %s (licenses/%s-%s)\nSource: %s\n\n' \
      "$sub" "$(basename "$file")" "$family" "$style" "$(echo "$scripts" | xargs)" "$licence" "$sub" "$(basename "$licence_file")" "$source" >>"$DOC/copyright"
    # The run-time catalogue the apps read (maluos-slides-fonts::craft_fonts): the same four
    # fields the build script embeds, in the same order.
    printf '%s | %s | %s/%s | %s\n' "$family" "$style" "$sub" "$(basename "$file")" "$(echo "$scripts" | xargs)" >>"$FONTDIR/manifest.txt"
    n=$((n + 1))
  done < <(grep -v '^#' "$FONTS/fonts/$manifest.txt")
done
echo "==> $n faces staged under /usr/share/fonts/truetype/maluos-fonts"

sed "s|\${STAGE}|$STAGE|g" "$HERE/nfpm-fonts.yaml" >"$WORK/nfpm.yaml"
(cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p deb -t "$DIST/")
ls -1 "$DIST"/maluos-fonts_*.deb
