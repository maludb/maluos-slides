# maluos-slides-cli: MaluOS additions

> **Created:** 2026-11-02 · plan step 3.7

The CLI is upstream's (`apps/maluos-slides-cli`): `info`, `render`, `convert`, `run`, `commands`,
`describe`, `app`, `mcp`, `bench`. This page covers what the fork adds. Everything else is in
`maluos-slides-cli help`.

## `ask`: one assistant turn, headless

```
maluos-slides-cli ask "REQUEST" [--in FILE | --sample] [--out FILE]
                      [--mode answer|confirm|autopilot] [--yes]
                      [--provider anthropic|openai|ollama] [--model NAME]
                      [--json] [--quiet] [--max-rounds N] [--stdin]
```

Opens FILE (`.deckcraft` or `.pptx`), the sample deck, or a new presentation; runs one turn of
the same assistant the app's pane uses (`maluos-slides-assist`); saves to `--out`. The answer
streams to stdout, tool calls and plans to stderr, so `ask … > answer.txt` keeps the answer and
the terminal shows the work.

- **Modes** are the pane's: `answer` reads and explains; `confirm` (default) shows each change
  as a plan and, on a terminal, asks `y/N/q`; in a script (stdin not a terminal) plans are
  declined unless `--yes`; `autopilot` applies changes at once and still asks for deletions and
  new or opened presentations.
- **Provider**: `--provider`, else the first with a key in the environment
  (`MALUOS_SLIDES_ANTHROPIC_API_KEY` / `ANTHROPIC_API_KEY`, the same for OpenAI), else Ollama at
  `http://127.0.0.1:11434/v1` (`MALUOS_SLIDES_OLLAMA_URL` overrides). `--model` names the model;
  each provider has a default.
- **`--json`** prints one object at the end instead of the streamed answer:

```json
{"answer": "…", "rounds": 3, "toolCalls": [{"name": "run_command", "input": {…}, "ok": true, "summary": "{\"id\":291,\"index\":8}"}],
 "commands": ["slide.new"], "touchedSlides": [8], "removedSlides": 0, "plans": 1, "declined": 0, "refused": 0,
 "usage": {"inputTokens": 25332, "outputTokens": 984, "cacheReadInputTokens": 0},
 "undo": "Assistant: add a closing slide that thanks the audience", "stopped": null, "error": null, "saved": "out.pptx"}
```

- **Exit status**: 0 when the turn ended with an answer (even if plans were declined); 1 when the
  model call failed, the deck could not be opened or saved, or an option was wrong.

Examples:

```
maluos-slides-cli ask "add a closing slide that thanks the audience" --sample --mode autopilot --out out.pptx
maluos-slides-cli ask "tighten every title to five words" --in deck.pptx --out deck.pptx --yes --json
echo "what is this deck about?" | maluos-slides-cli ask --stdin --in deck.pptx --mode answer
```

Tests: `cargo nextest run -p maluos-slides-cli ask` (argument parsing; a scripted turn with the
mock model that edits the sample deck and saves a `.pptx` the engine reopens with nine slides;
plans declined without `--yes` and approved with it; a model error reported).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | major | Created with step 3.7: the `ask` subcommand |
