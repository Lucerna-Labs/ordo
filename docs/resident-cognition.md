# Resident Cognition

Ordo has one permanent cognition route:

`Ordo -> Lightcone admission -> Atom cognitive engine -> Bonsai 27B PQ2_0 weights -> native post-weights hook -> output validation -> verified feedback`

The internal service is `ordo-resident`; its fixed model identity is
`ordo-bonsai-27b-lightcone`. The service listens on `127.0.0.1:4142` by default
and exposes only the small OpenAI-compatible surface required by Ordo.

## Invariants

- The model, endpoint, and unauthenticated localhost route are seeded at boot
  and pinned again in memory at every assistant, orchestration, and logic call.
- Request credentials, saved provider defaults, mode defaults, failover chains,
  Agent Team labels, and avatar settings cannot redirect cognition.
- The resident process loads the model once and remains alive for the Ordo
  runtime lifetime. Ordo terminates a child process it launched.
- A pre-existing server is reused only when its health response identifies the
  exact resident model, the cognitive engine, the two-hop limit, and every
  required membrane lane as operational. An old, partial, or unrelated service
  on the port makes startup fail.
- The GGUF weights are immutable. Lightcone's native hook operates after raw
  model logits; the cognitive engine supplies admitted warrant and routing.
- Output validation is enforceable: ungrounded model prose is replaced at the
  OUT boundary, and only grounded results reinforce workspace-local feedback.
- Boundary-specific cognitive engines remain resident across turns so native
  habituation, novelty, precision, and stigmergic state can accumulate without
  allowing a non-admitted claim into that boundary. The bank is bounded at 128
  boundary signatures; verified reinforcement is also persisted across restarts.
- Fictional-context claims require a named-work anchor and are injected with an
  explicit fictional-work-only scope; they cannot enter a generic real-world
  query through a shared plot term.
- The default world is the content-addressed `atom-universal-foundation-v2`
  pack: 891 claims, all 178 declared subdomains, and at least five generally
  reachable claims per subdomain. Runtime startup verifies all seven manifest
  files before loading any claim.
- Passive admission scales by requiring two shared whole-word surface terms or
  one corpus-rare anchor for multi-term queries. Single-term domain queries
  retain whole-word reachability; the cone still performs no semantic ranking.

## Launch

From the combined workspace root:

```bash
./launch-ordo.sh
```

The launcher verifies the model and knowledge pack, builds `ordo_server` when
missing or stale, exports workspace-local paths, and then starts the Ordo Studio
launcher.

Useful read-only checks while running:

```bash
curl http://127.0.0.1:4142/v1/health
curl http://127.0.0.1:4142/v1/models
curl http://127.0.0.1:4142/v1/cognition/capabilities
curl -X POST http://127.0.0.1:4142/v1/cognition/probe \
  -H 'content-type: application/json' \
  -d '{"query":"Explain why binary search requires ordered data."}'
curl http://127.0.0.1:4142/v1/cognition/formal/primitives
curl -X POST http://127.0.0.1:4142/v1/cognition/formal/execute \
  -H 'content-type: application/json' \
  -d '{"primitive":"algebra_solve_linear","args":{"coefficient":3,"offset":4,"result":19}}'
```

The supported path overrides are `ORDO_COGNITION_BINARY`, `ORDO_MODEL_PATH`,
`ORDO_KNOWLEDGE_PACK`, `ORDO_COGNITION_STATE`, and `ORDO_COGNITION_BIND`.
Disabling resident cognition is reserved for focused tests
(`ORDO_RESIDENT_COGNITION=false`); a normal Ordo session requires it.

`POST /v1/chat/completions` returns an `ordo_receipt` covering geometric
admission, the selected engine route, hop/bond activity, post-weight sampler
passes and deflections, OUT-boundary validation, and the persistent verified
reinforcement count. The typed formal endpoint executes the native engine
runtime and an independent oracle; disagreement fails closed.

The opt-in live integration proof loads the real weights and confirms that a
fake provider override still records the fixed resident service and model:

```bash
cd ordo
cargo test -p ordo-runtime --test resident_live -- --ignored --nocapture
```
