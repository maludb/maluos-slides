# MaluOS integration: the vault and the gateway

> **Created:** 2026-11-02 · what MaluOS Slides needs from the MaluOS platform for keys and models
> (rule: keys live in the vault or an env var). Read from the installed sources on the owner's
> desktop (`/usr/lib/maluos/maluos_vault.py`, `/usr/lib/maluos/maluos_gateway/`, `maluctl`).

## What exists on a MaluOS desktop (found 2026-11-02)

**Vault**: `maluos-vault.service` on `/run/maluos/vault.sock`. HTTP/1.1 + JSON over the Unix
socket; the caller is identified by the kernel's `SO_PEERCRED` (uid → principal, e.g. the
desktop user `maludb` is the enrolled human principal `user:maludb`). Routes that matter here:

| Route | What |
|---|---|
| `GET /v1/whoami` | uid, user, principal, enrolled, operator, broker |
| `GET /v1/secrets[?compartment=C]` | the secrets the caller may use: `ref` (`compartment/name`), `kind`, `provider`, `env_var`, access level |
| `GET /v1/secrets/<ref>/value?mode=read&purpose=…` | the value, logged in the ledger; `mode=use` is for brokers only (the gateway), the value then never reaches the caller |
| `POST /v1/secrets` | add (`name`, `value`, `kind`, `provider`, `env_var`, `compartment`, `upsert`) |
| `POST /v1/secrets/<ref>/request` | ask the owner for a grant; a refusal files one too (`forbidden; grant request #N is pending`) |

`maluctl secret add NAME --provider anthropic --env ANTHROPIC_API_KEY` stores a key in the
caller's own compartment (`user:<login>/NAME`); `maluctl secret show`, `exec`, `grant`,
`delegate` (use-only, for agents) exist. On this machine no secret and no compartment exists yet.

**Gateway**: `maluos-gateway.service` on `/run/maluos/gateway.sock` (TCP optional, off). Faces:
`POST /v1/chat/completions` (OpenAI), `POST /v1/messages` (Anthropic), `/v1/embeddings`,
`GET /v1/models`, `/api/chat` (Ollama), plus `/v1/maluos/{health,status,whoami,routes}`. Tools
and tool calls are translated to a canonical body and served by every backend; streaming is
supported; headers `X-MaluOS-Task`, `X-MaluOS-Class`, `X-MaluOS-Max-Wait` in, `X-MaluOS-Route` /
`-Request` out. Models are tiers (`fast`, `standard`, `strong`, `embed`, each a ladder of
backends) or explicit ids; backends are configured with `maluctl gateway backend add NAME --kind
anthropic|openai|ollama_cloud|… --secret REF` so the gateway draws the key from the vault on the
caller's behalf. On this machine only `ollama-local` is live; the anthropic/openai entries are
catalogued without keys, and the catalogue stops at Claude 4.x / GPT-5.

## The two ways Slides can use this

1. **Gateway first**: Slides sends its requests to `/run/maluos/gateway.sock`, Anthropic face
   for Claude, OpenAI face for the rest. The key never leaves the vault; policy, budget and the
   ledger apply; tiers (`strong`) work without naming a model. Cost: whatever the gateway's
   translation drops (see asks 2 and 3).
2. **Direct with a vault key**: Slides reads `user:<login>/anthropic` (`mode=read`, purpose
   `maluos-slides assistant`) and calls the provider itself with the full API (adaptive thinking,
   effort, prompt caching, signed thinking blocks, the newest models).

Recommendation: both, as providers in the picker: "MaluOS gateway" (default on MaluOS) and the
direct providers with the key from the vault, the env var still winning when set.

## What Slides builds on its side (no ask needed)

- A Unix-socket `Transport` in `maluos-llm` (HTTP/1.1 over `AF_UNIX`, the same `post` seam).
- A gateway provider in `maluos-slides-assist::providers` (Anthropic face; tier names as models).
- A vault key source (`keys::api_key_with` lookup): `GET /v1/secrets`, pick by `provider`, then
  `…/value?mode=read&purpose=maluos-slides%20assistant`; the env var overrides; a 403 with a
  pending request id becomes "waiting for the owner's approval" in the pane.

## Asks of the MaluOS build (the gap)

1. **Vault read contract for desktop apps**: confirm an owner's process may read its own
   secret with `mode=read` and no reauth; fix the naming convention apps use to find a provider
   key (proposal: match `provider` = `anthropic` / `openai`, fall back to `env_var`;
   `maluctl secret add` sets both). Confirm `MALUOS_VAULT_SOCKET` as the override for tests.
2. **Gateway field coverage**: which request fields survive the translation to the canonical
   body on each face. Slides sends, on `/v1/messages`: `stream`, `system`, `tools` with
   `input_schema`, `max_tokens`, `thinking: {type: adaptive}`, `output_config.effort`, signed
   `thinking` blocks in the history, `anthropic-version` / `anthropic-beta` headers, and reads
   `usage.cache_read_input_tokens`; on `/v1/chat/completions`: `tools`, `stream`,
   `stream_options.include_usage`, `max_completion_tokens`, `reasoning_effort`. State which are
   passed through, mapped or dropped, so Slides knows when to go direct.
3. **Catalogue**: add the Claude 5 family (`claude-opus-5-5`, `claude-sonnet-5-5`,
   `claude-haiku-5-5`) and `gpt-5.1` as backends/aliases, or document how a desktop adds them
   (`maluctl gateway alias set …`).
4. **Provisioning**: a documented or first-boot step that stores the owner's provider keys
   (`maluctl secret add …`) and registers the cloud backends with `--secret`, and a default
   policy for the owner's principal (`maluctl gateway policy set --principal user:<login> --cloud
   yes`, budgets). Today none of this is done on a fresh desktop.
5. **Docs**: `docs/phase6-slice1-vault.md` and `docs/inference-node-api.md` are cited by the
   sources but not installed; ship them under `/usr/share/doc/maluos-vault/` and
   `/usr/share/doc/maluos-gateway/`, or give the repository path.
6. **Dev mode**: a way to run both services in CI headless (`--kind mock` backend exists for the
   gateway); env overrides for both sockets; a non-root way to enrol a test principal.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | major | Created from the installed sources: what exists, two integration paths, six asks |
