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

- Built (step 4.5): the office client `maluos-slides-assist::office` (enrolment file, token from env or the vault over its Unix socket, agents by capability, the task protocol) and the `office_picture` tool. Shared with Malu123 and MaluDocs per [harness-design.md](harness-design.md) H28.
- A Unix-socket `Transport` in `maluos-llm` (HTTP/1.1 over `AF_UNIX`, the same `post` seam).
- A gateway provider in `maluos-slides-assist::providers` (Anthropic face; tier names as models).
- A vault key source (`keys::api_key_with` lookup): `GET /v1/secrets`, pick by `provider`, then
  `…/value?mode=read&purpose=maluos-slides%20assistant`; the env var overrides; a 403 with a
  pending request id becomes "waiting for the owner's approval" in the pane.

## The office and its agents (for step 4.5; found 2026-11-03)

Step 4.5 ("pictures from the office's image agent") needs no image library and no web fetch on
the Slides side. It needs an **office**: a MaluDB Business OS install whose agents this desk may
use. What is installed here (`maluos-desk` 0.4.0, Phase 12 slices 6–9):

- **Enrolment**: `sudo maluctl office enroll URL --code CODE` (root once; the owner must be an
  enrolled vault principal, which `user:maludb` is). The desk token goes into the owner's vault
  compartment as `office-desk-token` (never a file); `/etc/maluos/office.yaml` and
  `/etc/maluos/office.env` (`MALUOS_OFFICE_*`) name the endpoints; the office's models become a
  gateway rung (`--model office`). Status: `maluctl office status`; leave: `sudo maluctl office leave`.
- **The agents directory**: `GET <endpoints.agents>` with the desk token, answered as
  `{agents: [{id, name, description, capabilities: [...], tasks: URL, activity: URL, page: URL}]}`,
  cached per user by `maluctl office agents`. Slides picks an agent **by capability** (an
  `image.*` / `picture` capability when the office publishes one), never by a hard-coded id.
- **The task interface** (`/usr/lib/maluos/maluos_agent_tasks.py`, protocol `maluos-tasks/0.1`):
  JSON-RPC 2.0 over HTTP at the agent's `tasks` URL, Bearer desk token. `agent/card`,
  `tasks/send {message: {parts: [{text}]}}` → `{id, state, output: {text, artifacts: [...]}}`,
  `tasks/get {id}` to poll while `state` is `submitted`/`working`, `tasks/list`. A picture comes
  back as an artifact of the answer. Local agents serve the same methods on
  `/run/maluos/agents/NAME/tasks.sock` (peer credentials, no token).
- **The delegation envelope** (Phase 13 slice 2) is referenced by the installed code but not
  present yet: today the desk token itself authorises the task. Slides sends through the task
  interface as it exists and adopts the envelope when it lands.
- **A mock office ships with the desk package**: `PYTHONPATH=/usr/lib/maluos python3 -m
  maluos_desk.mock_office --port 18080 --code CODE` serves the whole contract (enrol, heartbeat,
  two agents `helpdesk` and `research`, tasks, models, MCP stubs, `/mock/state`). Its agents
  answer text only with `artifacts: []` and none claims an image capability, so the 4.5 tests
  carry their own mock agent (same protocol, one artifact), and the live check runs the shipped
  mock with an `illustrator` agent patched in (`maluos-mock-image-office`, port 18081).

The owner's `maludb/maludb-os-docker` repository is the **office-side** installer (the Business
OS kernel plus Help Desk and Spaces in one container on a MaluDB hosting VM). Evaluated
2026-11-03 and not installed here: its `host-install.sh` refuses any host that is not a hosting
VM (Ubuntu 24.04, PostgreSQL 17, the MaluDB API on :8000, `/var/www/config/database.php`, a
public FQDN; this desk is MaluOS 26.04 with PostgreSQL 18 and none of the rest), it stops the
host's Apache and edits `pg_hba.conf`, and Slides needs the office only as a URL to enrol with.
Its Claude Code plugin covers Business OS application development, not this app.

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

7. **An image agent for desks** (step 4.5): an office agent that publishes an `image.generate`
   (or similar, named here so Slides can match it) capability in `endpoints.agents`, answers a
   `tasks/send` with the picture as an artifact (`{kind: "image", media_type, data | url}`;
   state the shape), and the Phase 13 delegation envelope once it exists. Until an office is
   reachable from this desk, 4.5 is verified against a mock agent only.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | major | Created from the installed sources: what exists, two integration paths, six asks |
| 2026-11-03 | minor | The office, its agents directory and task protocol, the shipped mock office; `maludb-os-docker` evaluated (office-side, not for this desk); ask 7 (an image agent for desks) |
| 2026-11-03 | minor | Step 4.5 built against the office contract; the mock with an image agent for the live check |
