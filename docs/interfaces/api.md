# API Interface

The `api.*` interface lane is for generic external service integration that is
not specifically modeled as REST.

## Scope

- service auth and client setup
- SDK-backed integrations
- webhook dispatch and receipt
- generic third-party API orchestration
- integration state and client configuration

## Future capabilities

- `api.configure_client`
- `api.refresh_auth`
- `api.dispatch_webhook`
- `api.invoke_service`

## Boundaries

- keep generic API integrations separate from REST endpoint contracts
- use `rest.*` when the work is specifically about resource routes, HTTP verbs,
  and request/response payloads
- use `ssh.*` when the work is about remote hosts or machine-level execution
