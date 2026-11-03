pub struct Subcommand {
    pub name: &'static str,
    pub usage: &'static str,
    pub summary: &'static str,
    pub details: &'static str,
}

pub const SUBCOMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "info",
        usage: "info FILE",
        summary: "print a presentation's slides, layouts and theme as JSON",
        details: "Prints FILE (.deckcraft or .pptx) as JSON: its slides (index, id, title, layout, how many shapes,
notes, transition), its layouts, theme fonts, sections and slide size.

The shapes of one slide, with their ids, boxes and text, come from the slide.inspect command:

  maluos-slides-cli run --in deck.deckcraft --print --cmd 'slide.inspect={\"index\":0}'",
    },
    Subcommand {
        name: "render",
        usage: "render (FILE | --sample) [--slide N | --all] [--scale S] [--edit] [--transparent] OUT",
        summary: "draw slides as PNG images",
        details: "Draws slides of FILE, or of the sample presentation with --sample, as PNG images. FILE comes first;
OUT is the image to write. With --all, OUT is a folder that gets one slide-01.png, slide-02.png… per
slide. Prints each image's path and size.

  --slide N        the slide at index N; indices start at 0, and 0 is the default
  --scale S        pixels per point, 2 by default: a 960×540 slide becomes 1920×1080
  --edit           draw what the editor shows: placeholder prompts and their dashed outlines
  --transparent    leave the slide background out, so only what is on the slide is drawn

  maluos-slides-cli render deck.deckcraft --slide 2 slide-3.png",
    },
    Subcommand {
        name: "render-transition",
        usage: "render-transition FILE --slide N --t T[,T...] [--width PX] OUT.png",
        summary: "draw frames of the transition into a slide as PNG images",
        details: "Draws the transition into slide N of FILE at each time T, from 0 (the slide before) to 1 (slide N),
as PNG images. With one T, OUT.png is the image; with several, each frame is written next to it as
OUT-t0.250.png and so on. Prints each image's path, size, transition and time.

  --slide N    the slide the transition leads into; indices start at 0
  --t T,T...   the times to draw, comma-separated
  --width PX   the image width in pixels, twice the slide width in points by default

  maluos-slides-cli render-transition deck.deckcraft --slide 1 --t 0.25,0.5,0.75 fade.png",
    },
    Subcommand {
        name: "convert",
        usage: "convert IN OUT",
        summary: "save a presentation in the format OUT's extension names",
        details: "Opens IN (.deckcraft or .pptx) and writes it to OUT, in the format OUT's extension names:
.deckcraft, .pptx, .pdf, .txt or .md for the outline, .png or .jpg for the first slide, .gif for an
animated GIF with one frame per slide. Other extensions are refused. Prints OUT.

  maluos-slides-cli convert deck.deckcraft deck.pptx",
    },
    Subcommand {
        name: "run",
        usage: "run [--in FILE | --sample] [--cmd COMMAND[=PARAMS]]... [--save OUT] [--export OUT [--export-params JSON]] [--print]",
        summary: "run commands on a presentation, then save or export it",
        details: "Runs commands, in order, on one presentation: FILE with --in, the sample presentation with --sample,
else a new one with an empty title slide. Then --save writes the presentation: .deckcraft keeps
everything; .pptx, .pdf and .md (the outline) are written as convert writes them. --export writes the
current slide as .png or .jpg, or the presentation as .pdf, .pptx or an animated .gif (one frame per
slide). --export-params passes file.export options as one JSON object, such as
'{\"width\": \"large\", \"secondsPerSlide\": 3, \"loop\": false, \"slides\": [0, 1, 2]}' for a GIF
(`maluos-slides-cli describe file.export` lists them).

Each --cmd is one command: COMMAND is a command id, then `=`, then the command's parameters as one
JSON object. Quote it for the shell. A command that needs no parameters is just its id.

  maluos-slides-cli run \\
    --cmd 'slide.new={\"layout\":\"titleOnly\",\"title\":\"Q3 results\"}' \\
    --cmd 'shape.insert={\"preset\":\"roundRect\",\"rect\":[80,160,320,120],\"text\":\"Revenue\"}' \\
    --cmd 'format.size={\"size\":28}' \\
    --save q3.deckcraft

`maluos-slides-cli commands` lists the command ids; `maluos-slides-cli describe COMMAND` explains one and its
parameters.

What the ids don't say:
- Positions and sizes are in points on a 960×540 slide, x to the right and y down; a rect is
  [x, y, width, height].
- Slide indices start at 0. slide.new inserts after the current slide and makes the new one current;
  slide.go {\"index\": N} makes slide N current.
- Shape ids are looked up on the current slide only.
- A command that adds a shape (shape.insert, insert.textBox, insert.table…) selects it. format.* acts
  on the selected text or shapes; edit.select {\"ids\": [...]} selects others. Many shape commands
  take the ids directly; `maluos-slides-cli describe COMMAND` says what a command acts on.

If a command fails, run stops there and saves nothing. --print shows each command's answer, such as
the id of an inserted shape.",
    },
    Subcommand {
        name: "commands",
        usage: "commands [FILTER]",
        summary: "list the commands run can use, one per line",
        details: "Lists the commands that run --cmd, app and the MCP server take, one per line: the id, its name in
the app and its parameters. FILTER keeps the commands whose id or name contains it, ignoring case.

  maluos-slides-cli commands format.

In the parameters, `?` marks an optional one, a spaced ` | ` separates alternatives and `→` shows what
the command answers. `maluos-slides-cli describe COMMAND` spells one command out.",
    },
    Subcommand {
        name: "describe",
        usage: "describe COMMAND",
        summary: "explain one command and its parameters",
        details: "Explains one command: what it is in the app, each of its parameters, what it answers, and how run
calls it.

  maluos-slides-cli describe shape.insert",
    },
    Subcommand {
        name: "app",
        usage: "app [--port PORT] COMMAND [PARAMS] | --method METHOD [PARAMS]",
        summary: "run a command in the MaluOS Slides app started with --control",
        details: "Sends one request to the MaluOS Slides app started with `maluos-slides --control PORT` (port 7979 by default)
and prints its answer as JSON. COMMAND [PARAMS] runs a command, as run --cmd does; --method METHOD
[PARAMS] calls a control-channel method such as ui.screenshot. PARAMS is a JSON object.

  maluos-slides-cli app shape.insert '{\"preset\":\"star5\",\"rect\":[100,100,200,200]}'
  maluos-slides-cli app --method ui.screenshot '{\"path\":\"/tmp/shot.png\"}'",
    },
    // MaluOS: a deck from an outline (docs/maluos-slides/plan.md step 4.1).
    Subcommand {
        name: "outline",
        usage: "outline FILE.md --out DECK [--theme T] [--in BASE] [--print]",
        summary: "build a presentation from a Markdown outline, no model needed",
        details: "Builds the deck a Markdown outline describes and saves it to --out (.pptx or .deckcraft):

  # Title            the deck title; the paragraph under it is the subtitle
  ## Section         a section and its section-header slide (with ## only and no ###, each ## is a slide)
  ### Slide          a slide; bullets (-, *, 1.) with two spaces or a tab per level are its body
  Notes: text        the slide's speaker notes
  | a | b |          a pipe table on the slide

With --in BASE the slides are added after the last slide of BASE instead of a new presentation.
--theme picks one of the built-in themes (MaluOS Slides, Harbor, Ember, Meadow, Nocturne, Paper,
Coral Reef, Slate). The assistant's build_deck tool does the same from a request.

  maluos-slides-cli outline talk.md --out talk.pptx --theme Harbor",
    },
    // MaluOS: the HTTP/JSON API (docs/maluos-slides/plan.md step 3.8).
    Subcommand {
        name: "api",
        usage: "api [--listen ADDR:PORT] [--token T] [--in FILE | --sample] [--connect PORT] [--automation-read-root DIR] [--automation-write-root DIR]",
        summary: "serve the commands and the assistant over HTTP/JSON",
        details: "Serves MaluOS Slides as an HTTP/JSON API, on 127.0.0.1:7980 by default, over the same tool table the MCP
server uses. Routes under /v1 (GET /v1 lists them):

  GET  /v1/health                     the server and its backend
  GET  /v1/tools                      the tool table (name, description, inputSchema), as MCP has it
  POST /v1/tools/NAME                 call a tool; the body is its arguments
  GET  /v1/commands?filter=S          the command catalog (id, label, params, enabled)
  POST /v1/commands/ID                run an engine command; the body is its params
  GET  /v1/document                   the presentation: slides, sections, theme, selection
  GET  /v1/slides/N                   one slide's shapes with ids, boxes and text
  GET  /v1/slides/N/render.png?scale=S   the slide as a PNG image
  POST /v1/ask                        one assistant turn: {text, mode?, provider?, model?, yes?}

Answers are JSON ({ok, result} or {ok: false, error: {code, message}}); render.png is image/png.
The server works on its own presentation (--in FILE, --sample, or a new one) or drives a running
app with --connect PORT (the app started with --control PORT; /v1/ask needs the in-process one).
It listens on the loopback interface only; another --listen address needs --token (or
MALUOS_SLIDES_API_TOKEN), sent as `Authorization: Bearer T`. The automation roots confine the files
the in-process presentation reads and writes, as for mcp.

  maluos-slides-cli api --sample
  curl -s localhost:7980/v1/document | jq .slides
  curl -s -X POST localhost:7980/v1/tools/add_slide -d '{\"layout\":\"titleOnly\",\"title\":\"Agenda\"}'
  curl -s -X POST localhost:7980/v1/ask -d '{\"text\":\"add a closing slide\",\"yes\":true}'",
    },
    // MaluOS: the assistant (docs/maluos-slides/plan.md step 3.7).
    Subcommand {
        name: "ask",
        usage: "ask \"REQUEST\" [--in FILE | --sample] [--from DOC] [--out FILE] [--mode answer|confirm|autopilot] [--yes] [--provider P] [--model M] [--json]",
        summary: "ask the assistant for a change to a presentation, headless",
        details: "Runs one assistant turn on FILE (or the sample presentation, or a new one) and saves the result to
--out. The request is one quoted argument, or comes from stdin with --stdin. The answer streams to
stdout; tool calls and plans go to stderr.

  --mode answer     read and explain only; nothing changes
  --mode confirm    (default) show each change as a plan; on a terminal it asks y/N/q, in a script
                    plans are declined unless --yes
  --mode autopilot  apply changes at once; deletions and new/opened presentations still ask
  --yes             approve every plan
  --provider P      anthropic, openai or ollama; the default is the first one with a key in the
                    environment (MALUOS_SLIDES_ANTHROPIC_API_KEY / ANTHROPIC_API_KEY, the same for
                    OPENAI), else ollama at http://127.0.0.1:11434/v1 (MALUOS_SLIDES_OLLAMA_URL)
  --model M         the model name; each provider has a default
  --json            print one JSON object at the end instead of the streamed answer: answer,
                    rounds, toolCalls, commands, touchedSlides, removedSlides, usage, undo, saved
  --quiet           no tool and plan lines on stderr
  --max-rounds N    model calls allowed for the turn (default 12)
  --from DOC        attach a text or Markdown document to the request (up to 200 KiB), e.g. the
                    notes a deck should be made from

  maluos-slides-cli ask \"add a closing slide that thanks the audience\" --sample --mode autopilot --out out.pptx
  maluos-slides-cli ask \"tighten every title\" --in deck.pptx --out deck.pptx --yes --json",
    },
    Subcommand {
        name: "mcp",
        usage: "mcp [--connect PORT] [--sample] [--automation-read-root DIR] [--automation-write-root DIR]",
        summary: "serve the commands to an agent over MCP on stdio",
        details: "Runs a Model Context Protocol server on stdin and stdout. It works on its own presentation without a
window, or with --connect PORT (or host:port) on the MaluOS Slides app started with --control PORT.
--sample opens the sample presentation.

--automation-read-root DIR and --automation-write-root DIR confine every file the agent's commands read
(open, pictures, audio, video) and write (save, export, PDF) to those folders, links followed; a root left
out grants none of its access. They run the in-process server, so they conflict with --connect (a running
app is not confined by them).",
    },
    Subcommand {
        name: "bench",
        usage: "bench (FILE | --sample) [--runs N] [--json]",
        summary: "time opening, drawing, saving and exporting a presentation",
        details: "Times how long FILE, or the sample presentation with --sample, takes to open, to draw each slide, to
save as .deckcraft and .pptx, to reopen as .pptx and to export as .pdf, and the peak memory. Prints the
median and slowest of N runs (5 by default, 1 to 1000) as a table, or as JSON with --json.

  maluos-slides-cli bench deck.pptx --runs 10",
    },
    Subcommand {
        name: "links",
        usage: "links",
        summary: "print the MaluOS Slides community and project links",
        details: "Prints the MaluOS Slides community and project links.",
    },
    Subcommand { name: "version", usage: "--version", summary: "print the version", details: "Prints the version of maluos-slides-cli." },
];

pub fn page(name: &str) -> Option<String> {
    let name = match name {
        "help" | "--help" | "-h" => return Some(overview()),
        "--version" | "-V" => "version",
        name => name,
    };
    SUBCOMMANDS.iter().find(|s| s.name == name).map(|s| format!("usage: maluos-slides-cli {}\n\n{}", s.usage, s.details))
}

pub fn overview() -> String {
    let width = SUBCOMMANDS.iter().map(|s| s.usage.split(' ').next().unwrap_or_default().len()).max().unwrap_or_default();
    let list: Vec<String> =
        SUBCOMMANDS.iter().map(|s| format!("  {:<width$}  {}", s.usage.split(' ').next().unwrap_or_default(), s.summary)).collect();
    format!(
        "maluos-slides-cli works on MaluOS Slides presentations (.deckcraft, .pptx) without a window.

{}

A presentation is edited by running commands on it:

  maluos-slides-cli run --cmd 'slide.new={{\"title\":\"Q3 results\"}}' --save q3.deckcraft

`maluos-slides-cli help SUBCOMMAND`, or `maluos-slides-cli SUBCOMMAND --help`, explains one subcommand;
`maluos-slides-cli help run` explains how to write a command.",
        list.join("\n")
    )
}
