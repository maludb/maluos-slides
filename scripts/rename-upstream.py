#!/usr/bin/env python3
"""Text part of the DeckCraft -> MaluOS Slides rename (see rename-upstream.sh).

Usage: rename-upstream.py [--list] FILE...     rewrites the files in place; --list prints the mapping.

Three ordered rule sets. PRE rules are exact phrases that need a non-generic answer and run first.
PROTECT phrases are then hidden so the generic MAP rules leave them alone: the native file format
(`.deckcraft`, the `deckcraft` format id, its MIME type), credits to upstream and links into
upstream's issues, pull requests and files. MAP runs last, specific tokens before the generic
case variants. Idempotent: running it twice changes nothing.
"""
import sys

# Exact phrases rewritten before anything else.
#  - log targets follow the Rust crate path of the desktop binary (`maluos_slides`, underscore)
#  - the MCP server advertises the new name
#  - the settings directory and xtask's package table name the binary
PRE = [
    # the app id first: `.deckcraft` below would otherwise shield its last segment
    ("ai.storyteller.deckcraft", "org.maluos.slides"),
    ('<developer id="ai.storyteller">', '<developer id="org.maluos">'),
    # `..deckcraft_model::Line::solid(..)` (struct update syntax): a path, not the `.deckcraft`
    # extension, which is never followed by an underscore
    (".deckcraft_", ".maluos_slides_"),
    ('level_for("deckcraft")', 'level_for("maluos_slides")'),
    ('level_for("deckcraft::', 'level_for("maluos_slides::'),
    ('log::Level::Info, "deckcraft"', 'log::Level::Info, "maluos_slides"'),
    ('log::Level::Debug, "deckcraft"', 'log::Level::Debug, "maluos_slides"'),
    ('for target in ["deckcraft", ""', 'for target in ["maluos_slides", ""'),
    ('ends_with("deckcraft: ', 'ends_with("maluos_slides: '),
    ('"warn,deckcraft*=info"', '"warn,maluos_slides*=info"'),
    ('deckcraft*=debug', 'maluos_slides*=debug'),
    ('deckcraft*=info', 'maluos_slides*=info'),
    ('(`deckcraft*=debug`)', '(`maluos_slides*=debug`)'),
    ('"deckcraft=",', '"maluos_slides=",'),
    (',deckcraft=DEBUG', ',maluos_slides=DEBUG'),
    ('"serverInfo": {"name": "deckcraft"', '"serverInfo": {"name": "maluos-slides"'),
    ('["serverInfo"]["name"], "deckcraft"', '["serverInfo"]["name"], "maluos-slides"'),
    ('c.join("deckcraft")', 'c.join("maluos-slides")'),
    ('("deckcraft", Class::Exempt)', '("maluos-slides", Class::Exempt)'),
    ('for app in ["deckcraft", "deckcraft-cli"', 'for app in ["maluos-slides", "deckcraft-cli"'),
    # the About box line under the product name
    ('"Presentations and slide shows, rebuilt from scratch in pure Rust."',
     '"The MaluOS presentation app, a fork of DeckCraft by the ArtCraft team."'),
    ("DeckCraft's own format", "the app's own format"),
    ("DeckCraft's", "MaluOS Slides'"),
]

# Phrases kept verbatim.
PROTECT = [
    # the native file format is upstream's and stays compatible
    ".deckcraft",
    "x-deckcraft",
    "<string>deckcraft</string>",
    # test fonts' family names (crates/{fonts,render}/tests/fixtures; their name tables are binary)
    "DeckCraft Colr Squares",
    "DeckCraft Legacy",
    '"format": "deckcraft"',
    "deckcraft|pptx",
    "`deckcraft`, `pptx`",
    "pptx, deckcraft, outline",
    # credits
    "DeckCraft contributors",
    "DeckCraft authors",
    "DeckCraft by the ArtCraft team",
    "upstream DeckCraft",
    "Upstream DeckCraft",
    "fork of DeckCraft",
    "forked from DeckCraft",
    "storytold_deckcraft",
    # upstream issue, PR, release and file links stay upstream's; the bare repo path becomes ours
    "storytold/deckcraft#",
    "storytold/deckcraft/issues/",
    "storytold/deckcraft/pull/",
    "storytold/deckcraft/releases",
    "storytold/deckcraft/blob/",
    "storytold/deckcraft/tree/",
    "storytold/deckcraft/commit",
    "storytold/deckcraft/discussions",
    '"repo": "storytold/deckcraft"',
]

# In Rust sources a quoted "deckcraft" is the format id, a file extension or a dialog filter;
# the cases that are not (log targets, the MCP name, xtask's table) are handled by PRE above.
PROTECT_RS = [
    '"deckcraft"',
    # test data: a word the spelling test flags and the user dictionary it lands in
    'hallo deckcraft"',
    '"deckcraft\\n"',
]

# Ordered replacements. Earlier entries win where they overlap with later ones.
MAP = [
    # identity strings that are not a plain word swap
    ("https://getartcraft.com/apps/deckcraft", "https://github.com/maludb/maluos-slides"),
    ("getartcraft.com/apps/deckcraft", "github.com/maludb/maluos-slides"),
    ("https://getartcraft.com/apps", "https://github.com/maludb"),
    ("https://getartcraft.com/", "https://maluos.com/"),
    ("https://discord.gg/artcraft", "https://github.com/maludb/maluos-slides/discussions"),
    ("Join the ArtCraft community on Discord", "MaluOS Slides discussions on GitHub"),
    ("Join our community on Discord", "Discussions on GitHub"),
    ("Learning Machines LLC", "MaluOS"),
    ("DeckCraft maintainers <deckcraft@storyteller.ai>", "Edward Honour <ed@maludb.org>"),
    ("storytold/deckcraft", "maludb/maluos-slides"),
    ("Library/Application Support/DeckCraft", "Library/Application Support/MaluOS Slides"),
    # the generic case variants
    ("DECKCRAFT", "MALUOS_SLIDES"),
    ("DeckCraft", "MaluOS Slides"),
    ("Deckcraft", "MaluOS Slides"),
    ("deckcraft_", "maluos_slides_"),
    ("deckcraft", "maluos-slides"),
]


def rewrite(text: str, is_rust: bool) -> str:
    for old, new in PRE:
        text = text.replace(old, new)
    holes = {}
    protect = PROTECT + (PROTECT_RS if is_rust else [])
    for i, p in enumerate(protect):
        key = f"\x00P{i}\x00"
        if p in text:
            text = text.replace(p, key)
            holes[key] = p
    for old, new in MAP:
        text = text.replace(old, new)
    for key, p in holes.items():
        text = text.replace(key, p)
    return text


def main(argv):
    if argv and argv[0] == "--list":
        for old, new in PRE:
            print(f"pre      {old} -> {new}")
        for p in PROTECT + PROTECT_RS:
            print(f"protect  {p}")
        for old, new in MAP:
            print(f"map      {old} -> {new}")
        return 0
    changed = 0
    for path in argv:
        try:
            raw = open(path, "rb").read()
            text = raw.decode("utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        out = rewrite(text, path.endswith(".rs"))
        if out != text:
            open(path, "wb").write(out.encode("utf-8"))
            changed += 1
    print(f"rename-upstream.py: {changed} file(s) rewritten")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
