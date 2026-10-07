# Cloud Interface (Retired for Models)

The original Ordo exposed `cloud.*` model-provider calls. The combined Ordo
build does not register that provider. Its only LLM is the supervised resident
`ordo-bonsai-27b-lightcone` service.

This file records the retired boundary so old capability names are not mistaken
for supported configuration. Non-LLM external integrations may still use their
own scoped API, REST, MCP, or remote-communication surfaces; they may not be
used as an alternate language-model route.

## Runtime Contract

- `CloudOpsProvider` is not registered.
- `cloud.openai.chat`, `cloud.anthropic.messages`, and other cloud LLM calls are
  unavailable.
- Studio has no Provider tab or model-provider credential controls.
- Assistant, logic, modes, teams, and avatar cognition are pinned to the
  resident service.
- Request-supplied service, endpoint, and model overrides cannot redirect the
  resident path.

## Retired Capability Names

- `cloud.openai.chat`
- `cloud.openai.embed`
- `cloud.anthropic.messages`
- `cloud.rest.request`
- model-provider credential list/upsert/delete operations

The resident server exposes an OpenAI-shaped protocol only as a private local
compatibility boundary. That protocol shape does not make it a configurable
provider and does not re-enable any of the names above.
