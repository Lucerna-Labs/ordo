# Interface Map

Ordo should also keep interface surfaces separate. Users should be
able to identify the difference between SSH work, generic API work, and REST
API work without digging through the whole doc set.

## Interface prefixes

- `ssh.*`
  - remote shell execution, remote file movement, deployment hops, machine
    access, operator diagnostics
- `api.*`
  - generic external service integrations, auth, SDK-backed clients, webhooks,
    non-REST service orchestration
- `rest.*`
  - resource-oriented HTTP endpoints, route contracts, request/response bodies,
    CRUD-style service integrations
- `cloud.*`
  - real outbound HTTP to named cloud providers (OpenAI, Anthropic, generic
    REST vendors), driven by stored credentials. This is the local-first
    escape hatch â€” Ordo is local-first, not local-only, and this
    lane is where the boundary lives.

## Separation rules

- SSH should stay distinct from application API integrations.
- Generic API tooling should stay distinct from REST-specific endpoint tooling.
- REST is a specific HTTP integration style, not the catch-all bucket for every
  external service.
- If an integration talks in resources, routes, verbs, and payload contracts,
  it belongs under `rest.*`.
- If an integration is not REST-specific, it should stay under `api.*`.
- Remote host access, remote commands, and deployment hops belong under
  `ssh.*`, even if they eventually trigger API or REST workflows elsewhere.
- Stays strict: `rest.*` stays pure-data helpers (endpoint description,
  request scaffolding, response validation). The moment a capability reaches
  for real credentials and makes a real outbound call, it belongs under
  `cloud.*`. This keeps the in-memory REST helpers safe to use in tests and
  keeps the credential-bearing network path auditable in one place.

## Example capability layout

- `ssh.connect_host`
- `ssh.run_remote_command`
- `ssh.sync_workspace`
- `api.configure_client`
- `api.refresh_auth`
- `api.dispatch_webhook`
- `rest.describe_endpoint`
- `rest.prepare_request`
- `rest.validate_response`
- `rest.sync_resource`
- `cloud.openai.chat`
- `cloud.openai.embed`
- `cloud.anthropic.messages`
- `cloud.rest.request`
- `cloud.credentials.list`
- `cloud.credentials.upsert`
- `cloud.credentials.delete`

## Quick reference

- SSH surface docs:
  - `docs/interfaces/ssh.md`
- Generic API surface docs:
  - `docs/interfaces/api.md`
- REST API surface docs:
  - `docs/interfaces/rest-api.md`
- Cloud surface docs:
  - `docs/interfaces/cloud.md`

## Why this matters

- Users can see where a tool belongs before it is implemented.
- Provider naming stays understandable as the platform grows.
- Retrieval can pull the correct integration context more reliably.
- Future interface-specific providers can be added without collapsing into one
  overloaded "API" bucket.
