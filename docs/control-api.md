# Control API

The Ordo control API is the local operator surface for the runtime and the
Studio UXI.

It is not a heavy central gateway and it is not a second orchestration path. It
wraps the runtime, bus, provider, memory, review, security, and capability
systems behind local HTTP endpoints.

## Default Binding

```text
http://127.0.0.1:4141
```

The bind can be changed with:

```text
ORDO_CONTROL_API_BIND
```

Set it to an empty value to disable the API.

## Static Studio Serving

The control API serves the built Studio bundle so Servo can load Ordo over a
localhost HTTP origin:

- `GET /`
- `GET /index.html`
- `GET /assets/*`

This replaces the old Vite-at-runtime workaround. Vite remains useful for
frontend development, but the production/beta app is served by Ordo itself.

The built-in dashboard remains available at:

- `GET /dashboard`

## Core Runtime Endpoints

### Health

- `GET /health`

Returns the local runtime health payload.

### Capability Inventory

- `GET /api/capabilities`

Returns the live capability descriptors currently exposed by the runtime.

### Runtime Profile

- `GET /api/runtime/profile`

Returns the effective runtime profile and activation state.

### Runtime Storage

- `GET /api/runtime/storage`

Returns storage budgets and memory/RAG storage state.

### Runtime Settings

- `GET /api/runtime/settings`
- `POST /api/runtime/settings`

Reads or updates persisted runtime settings. Explicit environment variables
remain the final override layer.

## Assistant

The assistant API owns sessions, turns, cancellation, and session-visible
events.

Representative surfaces:

- create/list assistant sessions
- submit turns
- cancel active turns
- stream assistant progress events
- recall or manage assistant facts
- export conversation state through the UXI

Assistant requests must preserve mode scope, skill scope, memory policy, review
policy, and security gates.

## Providers And Model Lifecycle

Provider endpoints should expose:

- provider templates
- credential create/update/delete/list
- local provider detection
- model discovery
- active provider/model selection
- model lifecycle results

When switching local providers or models, Ordo should unload/eject the previous
active local model where the provider supports it. The lifecycle result should
be returned to the UI and logged.

## RAG And Memory

### RAG Collections

- `GET /api/rag/collections`

Returns collection name, label, group, document count, chunk count, and sample
document titles.

### RAG Preview

- `GET /api/rag/preview?query=...`
- `GET /api/rag/preview?query=...&collections=main,providers`

Runs a lightweight retrieval preview.

RAG must work in hash-fallback mode when no embedding model is present. If an
Ollama or llama.cpp embedding model is configured, the API reports the active
embedding backend and improved semantic retrieval is available after restart.

### Pinned Memory

- `GET /api/memory/pinned?limit=10`
- `POST /api/memory/pinned`
- `DELETE /api/memory/pinned`

Lists, stores, or removes pinned memory.

### Working Memory

- `GET /api/memory/working?limit=10`
- `POST /api/memory/working`

Lists or stores working memory.

## Agent Teams

Agent Team API/capability support should allow official backend management of:

- team definitions
- role definitions
- role instructions
- role-specific skills
- model/provider suitability hints
- activation state
- activity/status events

The UXI should not be the only authority for Agent Teams. Tech Specialist needs
official capability-backed ways to create, inspect, and modify teams for users.

## Tech Specialist

Tech Specialist capabilities should expose safe maintenance operations for:

- diagnostics
- logs
- skills
- MCP servers
- plugins
- apps
- webhooks
- automation
- hooks
- Agent Teams
- providers and models
- avatar setup
- SSH keys
- API keys
- local computer access, after explicit approval

Secrets must remain behind vault/UI paths and should not be returned through
diagnostic or Tech Specialist responses.

## Automation

Automation endpoints/capabilities should cover:

- routines
- hooks
- cron-style jobs
- heartbeats
- webhooks
- local events
- dreaming reviews
- bounded coding automation

Automation operations should be logged and approval-gated where needed.

## Remote Communication

Remote Communication owns email and future approved communication channels.

Expected configuration shape:

- multiple user email providers/accounts
- per-account access level: none, read, or write
- optional Ordo-owned communication identity
- future Signal and Telegram support

SMS is intentionally excluded.

## Artifacts And Files

Artifact APIs should let agents present user-visible outputs in the side view
without replacing chat:

- generated files
- saved PDFs
- docs
- spreadsheets
- email views
- local files selected by the user

File access remains permissioned and sandboxed. Local computer read/write is
denied by default.

## Review And Self-Heal

Review remains the human approval lane.

Self-heal and diagnostics can inspect and suggest repairs, but durable repair
memory and pinned lessons should remain operator-manageable.

## Design Notes

- The API is local-first and should bind to localhost by default.
- Endpoint handlers should use official runtime services and capabilities.
- The API should not read secrets into ordinary JSON responses.
- The API should not bypass bus, review, memory, provider, or security policy.
- Logs should explain provider decisions, model lifecycle decisions, tool calls,
  denials, retries, failures, and recovery actions.
