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
  - retired in this resident-model build. Cloud-model calls and model-provider
    credential mutation are not registered with the runtime.

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
- `rest.*` stays pure-data helpers (endpoint description, request scaffolding,
  response validation). External application integrations must use a scoped
  non-LLM integration surface and must not create an alternate model path.

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
- The resident-model boundary stays distinct from non-LLM integrations.
- Retrieval can pull the correct integration context more reliably.
- Future interface-specific providers can be added without collapsing into one
  overloaded "API" bucket.
