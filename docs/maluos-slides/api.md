# The HTTP/JSON API

> **Created:** 2026-11-02 · plan step 3.8 · decision D9 (every function callable from an API and MCP)

`crates/maluos-slides-api` serves MaluOS Slides over HTTP/JSON; `maluos-slides-cli api` runs it.
It is the same table the MCP server uses: `GET /v1/tools` returns `tool_definitions()` unchanged
and `POST /v1/tools/{name}` is `call_tool`, so an HTTP client and an MCP client can never see
different tools. The command catalog, the document and the slides come from the engine through
the same `Backend`; `POST /v1/ask` is one turn of the assistant.

## Running it

```
maluos-slides-cli api [--listen ADDR:PORT] [--token T] [--in FILE | --sample] [--connect PORT]
                      [--automation-read-root DIR] [--automation-write-root DIR]
```

- Default `127.0.0.1:7980`. The server is hand-written HTTP/1.1 (no async runtime), one request
  per connection, answered in full; bodies need `Content-Length` (chunked is refused, 411) and
  are capped at 16 MiB.
- **Loopback only** unless `--token` (or `MALUOS_SLIDES_API_TOKEN`) is set; then every request
  needs `Authorization: Bearer T` (or `X-Api-Token: T`) and the server may listen elsewhere.
- `--in FILE` or `--sample` opens the presentation it works on; otherwise a new one.
  `--connect PORT` drives a running app instead (started with `--control PORT`); `/v1/ask` then
  answers 501, because the loop needs the in-process session.
- The automation roots confine the files the in-process presentation reads and writes, as for
  `mcp`.

## Routes

| Route | Body | Answer |
|---|---|---|
| `GET /v1` | | the route list |
| `GET /v1/health` | | `{ok, server, version, backend}` |
| `GET /v1/tools` | | `{tools: [{name, title, description, inputSchema, annotations}]}` |
| `POST /v1/tools/{name}` | the tool's arguments | `{ok, result}` when the tool's one text block is JSON, `{ok, text}` for plain text, `{ok, content}` for several blocks (an image comes as MCP content); 404 for an unknown tool, 400 when the tool failed |
| `GET /v1/commands?filter=S` | | `{commands: [{id, label, menu, shortcut, params, enabled}]}` |
| `POST /v1/commands/{id}` | the command's params | `{ok, result}`; 400 with the engine's message when it failed |
| `GET /v1/document` | | `document.inspect` |
| `GET /v1/slides/{i}` | | `slide.inspect` for slide `i` (0-based); 404 past the end |
| `GET /v1/slides/{i}/render.png?scale=S&edit=1` | | `image/png` |
| `POST /v1/ask` | `{text, mode?: answer\|confirm\|autopilot (default autopilot), provider?, model?, yes?}` | the turn report: `answer, rounds, toolCalls, commands, touchedSlides, removedSlides, plans, declined, refused, usage, undo, stopped, error`; 502 when the model call failed |

Errors are `{ok: false, error: {code, message}}` with the HTTP status.

`/v1/ask` is stateless: each call is a fresh conversation on the server's presentation. Without
`yes: true` every plan the mode produces is declined (there is no one to ask), so `autopilot`
applies ordinary changes and declines deletions, and `confirm` changes nothing. The provider
defaults to the first with a key in the environment, else Ollama, as the CLI and the pane do.

## Examples

```
maluos-slides-cli api --sample &
curl -s localhost:7980/v1/document | jq '.slides[] | .title'
curl -s -X POST localhost:7980/v1/tools/add_slide -d '{"layout":"titleOnly","title":"Agenda"}'
curl -s -X POST localhost:7980/v1/commands/slide.notes -d '{"index":8,"text":"Thank the audience"}'
curl -s localhost:7980/v1/slides/8/render.png?scale=2 -o slide-9.png
curl -s -X POST localhost:7980/v1/ask -d '{"text":"add a closing slide that thanks the audience","yes":true}' | jq .answer
```

## Not yet

- Serving from inside the desktop app (`maluos-slides --api PORT`), so the API drives the live
  window with `/v1/ask` included; the crate is built for it (the app would implement `Host`).
- Streaming `/v1/ask` (server-sent events) and a conversation id across calls.
- CORS headers for browser pages; the API is for scripts and services for now.

## Tests

`cargo nextest run -p maluos-slides-api`: the routes through `Server::handle` (tool list equal
to MCP's, a tool call that adds a slide, the document, a slide, a rendered PNG, the catalog with
a filter, a command, each error shape), the token and the off-loopback rule, `/v1/ask` with the
mock model (autopilot applies, confirm declines), and the wire format over a real socket (a POST
with a body, a GET, a bad request line, a chunked body refused).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | major | Created with step 3.8 |
