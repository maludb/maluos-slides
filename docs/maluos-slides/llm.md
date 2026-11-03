# maluos-llm: the LLM client

> **Created:** 2026-11-02 · plan step 3.1 · decisions D6 (one shared crate), D10 (providers)

`crates/maluos-llm` is the one place MaluOS Slides (and later Malu123) talks to a model. It is a
small synchronous crate at layer 5: no async runtime, no vendor SDK, one trait, two wire formats.
It starts inside this repository and moves to its own repository (`maludb/maluos-llm`, D6) once a
second app consumes it.

## Shape

```
LlmClient::complete(&Request, on_event) -> Result<Completion, LlmError>
```

- **`Request`**: model, optional system text, messages of `ContentBlock`s (text, thinking, tool
  use, tool result, provider-raw), tools as JSON Schema, `max_tokens`, optional `Effort`.
- **Events** arrive on the calling thread while the answer streams: `TextDelta`,
  `ThinkingDelta`, `ToolUseStart`, `ToolInputDelta`, `Usage`. The callback returns `Flow::Stop`
  to cancel (the call ends with `LlmError::Cancelled`).
- **`Completion`**: the model that answered, the whole content, the `StopReason` (`EndTurn`,
  `ToolUse`, `MaxTokens`, `Refusal`, `Other`), usage. `as_message()` is the assistant turn to
  append before the next request; `tool_uses()` is what the harness must answer.
- **`LlmError`** is typed (`NoApiKey`, `Auth`, `RateLimited { retry_after }`, `Overloaded`,
  `BadRequest`, `NotFound`, `Server`, `Transport`, `Protocol`, `Cancelled`) and `retryable()`
  says whether waiting helps.
- **`Transport`** is the HTTP seam: a POST that returns the status, headers and the body as a
  `Read`. `UreqTransport` (ureq 3, rustls, connect and per-read timeouts, no whole-request
  timeout) is the native one; `mock::MockTransport` answers with scripted replies; a wasm host
  supplies its own. The crate compiles for wasm32 with the `native` feature off.

The assistant runs `complete` on a worker thread and forwards events to the pane through a
channel; egui polls them each frame (step 3.6).

## Backends

| Backend | Endpoint | Notes |
|---|---|---|
| `AnthropicClient` | `POST {base}/v1/messages`, `anthropic-version: 2026-10-21` | Streaming always. Thinking is adaptive by default (no `thinking` field unless a display mode is set; `with_thinking_display("summarized")` streams readable reasoning). `output_config.effort` from `Request::effort`. Thinking blocks are passed back with their signature; unsigned ones (from another provider) and foreign `Raw` blocks are dropped. **Refusal fallbacks are on by default**: `fallbacks: "default"` under the `server-side-fallback-2026-07-01` beta, so a safety decline is re-run on another model inside the same call; `with_refusal_fallbacks(false)` turns it off. Default model `claude-opus-5-5`. |
| `OpenAiClient::openai` | `POST https://api.openai.com/v1/chat/completions` | `max_completion_tokens`, `reasoning_effort` (`xhigh`/`max` map to `high`), `stream_options.include_usage`. Default model `gpt-5.1`. |
| `OpenAiClient::compatible` / `::ollama` | `POST {base}/chat/completions` | For Ollama (`http://127.0.0.1:11434/v1`, no key), the MaluOS gateway, vLLM, LM Studio: `max_tokens`, no effort field, bearer token only when a key is given. `reasoning_content`/`reasoning` deltas become `ThinkingDelta`. A tool call reported with `finish_reason: stop` still yields `StopReason::ToolUse`. |
| `mock::MockClient` | none | Scripted completions replayed as events; records requests. For the harness tests of step 3.4. |

Message mapping for OpenAI: system text becomes the first `system` message; a user turn's text
blocks become one `user` message and each tool result its own `tool` message, in block order; an
assistant turn becomes `content` plus `tool_calls` with the arguments serialised.

## Keys

`keys::api_key_from_env(Provider)` reads `MALUOS_SLIDES_ANTHROPIC_API_KEY` then
`ANTHROPIC_API_KEY` (same pattern for OpenAI); blank values count as unset. Keys never come from a
settings file or a deck. The MaluOS vault (`/run/maluos/vault.sock`) is the next source: its
protocol is not documented in this tree yet, so `keys::api_key_with` takes the lookup as a
parameter and a host plugs the vault client in. The gateway socket (`/run/maluos/gateway.sock`)
is a Unix socket; `UreqTransport` speaks TCP only, so the gateway needs either a TCP listener or a
Unix-socket `Transport` (small, not written yet).

## Tests

`cargo nextest run -p maluos-llm`: 16 tests over the SSE parser (joined data lines, comments,
CRLF, size cap, cancel), both backends' request bodies and headers, streamed text, thinking and
parallel tool calls, HTTP and in-stream errors with `retry-after`, truncated streams, refusal
fallback blocks, message mapping in both directions, key lookup precedence and the mock client.
Nothing touches the network.

`examples/live.rs` is the live check: `cargo run -p maluos-llm --example live -- ollama <model>` (also
`anthropic [model]`, `openai [model]` with the key in the environment, `compatible <base_url> <model>`).
It sends one prompt with one tool, streams the answer, answers the tool call and prints the stop
reason and usage. Cloud models on Ollama need the daemon's key linked to an account (`ollama signin`
prints the link); until then the daemon answers 401 and the example reports `LlmError::Auth`. Verified live on
2026-11-02 with `glm-5.3-flash:cloud`: reasoning deltas, a tool call, the tool result and the final
answer, usage on both turns.

## Not yet

- The vault key source and a Unix-socket transport for the gateway: protocols now known, see [maluos-integration.md](maluos-integration.md).
- Prompt caching breakpoints (`cache_control`) on the Anthropic request: step 3.2 decides where
  the stable prefix ends (system text and tools first, the deck outline after).
- Retry with backoff on `retryable()` errors: the harness loop (step 3.4) owns the policy.
- (done in step 4.2) Images in requests: `Image`, `ContentBlock::Image`, `images` on tool results, both backends.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | major | Created with the crate: trait, types, SSE parser, ureq transport, Anthropic and OpenAI-compatible backends, env keys, mocks, 16 tests |
| 2026-11-02 | minor | `examples/live.rs`, the live check against Ollama / a key |
