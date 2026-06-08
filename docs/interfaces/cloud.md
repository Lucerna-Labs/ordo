# Cloud Interface

The `cloud.*` interface lane is where Ordo makes **real outbound
HTTP calls** against named cloud providers. It is the boundary between the
local-first runtime and the wider internet: everything else in the
workspace runs on the in-process bus, but capabilities here reach out to a
third-party service using credentials the operator configured.

Ordo is **local-first, not local-only**. That distinction lives
here.

## Scope

- authenticated outbound HTTP to named providers (OpenAI, Anthropic,
  generic REST vendors)
- typed service clients with sensible default models and endpoints
- a generic authenticated REST helper for any other service the operator
  wants to reach
- per-service credential storage in the local SQLite database
- credential CRUD with secrets redacted on every read path

## Current capabilities

- `cloud.openai.chat` â€” chat completions against `openai`
- `cloud.openai.embed` â€” embeddings against `openai`
- `cloud.anthropic.messages` â€” `POST /v1/messages` against `anthropic`
- `cloud.rest.request` â€” authenticated HTTP against any configured
  service (method, path, query, headers, body)
- `cloud.credentials.list` â€” list stored credentials with secrets redacted
- `cloud.credentials.upsert` â€” create or update a credential (per service)
- `cloud.credentials.delete` â€” remove a credential by service name

## Auth styles

Every credential picks one of:

- `bearer` â€” `Authorization: Bearer <secret>`
- `basic` â€” `Authorization: Basic <base64(user:pass)>` (store `user:pass`
  in `secret`)
- `api_key_header` â€” custom header; set `extras.header_name`
- `api_key_query` â€” query parameter; set `extras.param_name`
- `anthropic` â€” `x-api-key` + `anthropic-version` headers (optional
  `extras.anthropic-version` override)

## Where credentials live

- SQLite table `cloud_credentials` in the shared `data/ordo.db`
- one row per `service` name (primary key)
- secrets are stored in plaintext today; they never leave the machine
  except as an outbound authentication header on a request the operator
  explicitly configured
- **redaction is always on the read path** â€” every response JSON uses
  `has_secret: true/false` instead of the secret itself, and `extras`
  values collapse to `***`

## Boundaries

- `cloud.*` is the only lane allowed to use the network with stored
  credentials. Pure-data REST helpers stay under `rest.*`.
- `cloud.*` never pulls a credential into a response. If a caller needs
  to verify a credential exists, it checks `has_secret`.
- Removing a credential instantly disables the matching `cloud.*`
  capability â€” the provider returns a structured `not_configured` error.

## Operator UI

- Control API routes:
  - `GET /api/cloud/credentials`
  - `POST /api/cloud/credentials`
  - `DELETE /api/cloud/credentials`
- Built-in dashboard at `/` includes a **Cloud Credentials** card.
- Tauri studio includes a **Cloud** tab backed by the same control-API
  routes.

## Future extensions

- Rotate-only flow: a dedicated "rotate secret" path that never accepts
  the existing secret as input.
- Per-capability allowlist so operators can permit `cloud.openai.embed`
  while denying `cloud.openai.chat` on the same credential.
- Encrypted-at-rest credential column (OS keychain / age-encrypted) once
  the rest of the local-first surface has a shared key-management story.
