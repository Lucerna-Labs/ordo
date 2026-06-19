# Ordo Architecture

Ordo is the creative-operations fork of the Codex Claw runtime. It is
still being built as a Tokio-bus runtime first, but the product direction now
centers on creative pipelines, SEO workflow, CMS packaging, and operator-facing
publishing orchestration.

## Product direction

- Keep the existing local-first runtime and bus contracts as the stable core.
- Treat briefs, asset pipelines, approval routing, SEO metadata, and CMS-ready
  publish packages as the next domain-specific workflow lanes.
- Seed the runtime with creative-ops guidance so retrieval can answer workflow
  questions before custom providers exist.

## Domain lanes

- `creative.*`
  - brief intake, campaign planning, asset packaging
- `workflow.*`
  - stage routing, approvals, revisions, scheduling
- `seo.*`
  - metadata packaging, readiness audits, search guidance
- `cms.*`
  - field mapping, taxonomy, entry preparation, publish/export

These lanes should stay separated so future providers, docs, and heuristics do
not collapse into one mixed creative bucket.

## Interface lanes

- `ssh.*`
  - remote host access, shell execution, deployment hops
- `api.*`
  - generic service integrations, auth, SDK clients, webhooks
- `rest.*`
  - REST endpoint contracts, HTTP payloads, resource synchronization

These lanes should also stay separated so integration work does not disappear
into one overloaded API bucket.

## Current shape

- `ordo-cli` boots peers inside one Tokio runtime.
- `ordo-bus` is the shared event fabric. The first implementation uses
  `tokio::sync::broadcast` plus simple wildcard topic matching.
- `ordo-protocol` owns message envelopes, topic names, stable IDs, and
  correlation IDs, plus peer, NAT, and route types.
- `ordo-discovery` maintains peer snapshots and capability lookups.
- `ordo-classify` decides whether traffic should stay local, target a peer, or
  broadcast.
- `ordo-transport` turns route directives into concrete transport and handshake
  plans, with relay fallback, plus a transport-adapter seam for delivery.
- `ordo-transport` now has one real TCP path and one real local/dev direct QUIC
  path, while still leaving relay and symbolic QUIC routing on the simulated
  fallback.
- `ordo-handshake` performs hello exchange, transport compatibility checks,
  crypto-suite negotiation, and pairing requirements.
- `ordo-planner` turns high-level goals plus retrieved context into explicit
  tool-backed execution plans.
- `ordo-store` owns shared SQLite connection bootstrap and schema migrations for
  local persisted state.
- `ordo-heal` owns the platform self-heal lane: recurring-incident memory,
  repair planning, and optional local-model-backed maintenance reasoning.
- `ordo-control` exposes a thin local HTTP surface for UI and operator tooling,
  backed by the same bus and capability system as the rest of the runtime.
- `ordo-control` now also serves a built-in dashboard so profile, storage, and
  memory-lane controls are usable without waiting for a separate frontend.
- The desktop renderer direction has pivoted toward a Servo-backed
  self-rendering shell while the current Tauri/WebView Studio remains the
  compatibility host. See
  `docs/plans/2026-06-17-servo-self-rendering-shell.md`.
- `ordo-control` now also exposes self-heal history review, remembered-fix
  cleanup, and persisted local-model configuration through that same official
  operator surface.
- `ordo-control` now also lets operators promote remembered self-heal fixes
  into pinned memory through the same official dashboard and API surface.
- `ordo-rag` indexes chunked local documents, persists them, and answers
  retrieval queries over the same bus, now with a hybrid lexical + local
  embedding scoring baseline.
- `ordo-router` turns route directives plus peer state into an actual managed
  session with transcript events, adapter-backed delivery, and session-local
  message flow.
- `ordo-brain` publishes requirements, queries live capability inventory,
  enriches knowledge-style runs with RAG context, generates explicit plans,
  submits run requests, and collects lifecycle events into run summaries.
- `ordo-mcp-host` hosts providers, advertises capabilities, and emits heartbeats.
- `ordo-mcp-host` can also accept tool calls, answer capability inventory requests,
  execute explicit plans, and emit run lifecycle events.
- `ordo-mcp-host` now also exposes runtime-policy introspection so other parts of the
  platform can ask what profile and storage rules are active.
- `ordo-mcp-host` now also exposes persisted runtime settings so a future UI can
  inspect and save profile/storage policy without editing environment variables
  directly.
- `ordo-mcp-host` now also exposes self-heal history listing and case removal so the
  operator surface can curate remembered repairs without bypassing the bus.
- `ordo-mcp-host` now also exposes a self-heal case promotion tool so proven repairs
  can become always-available pinned memory without bypassing the memory peer.
- `ordo-memory` shadows bus traffic and responds to memory queries over the same
  bus, with SQLite-backed persistence.
- `ordo-memory` now also supports explicit pinned-memory store/list requests so
  the always-available memory lane can be managed through the same bus model.
- `ordo-memory` now also supports explicit pinned-memory removal so the
  operator surface can curate always-available context instead of only growing
  it.
- `ordo-rag` and `ordo-memory` currently share one local SQLite database file so
  state can evolve behind a single migration stream.
- `docs/official-memory.md` is the canonical pinned-memory source for official
  platform truths.
- `docs/creative-ops.md` captures the Ordo workflow, SEO, and CMS
  product direction that now gets seeded into retrieval.
- `docs/build-history.md` records how the platform was built and stabilized.
- `docs/fixbook.md` records problems, failed approaches, and the fix that
  actually worked.
- Retention is split into four storage classes:
  - RAG budget
  - working memory budget
  - pinned memory budget
  - self-heal incident history budget
- Filesystem access can be rooted under a dedicated user-files path so container
  deployments can keep runtime state and user files on separate volumes.
- `ordo-runtime` boots and supervises the default local component set.
- The runtime now has explicit profiles so core installs can stay lean while
  optional capabilities scale upward later.
- The runtime can also boot a local control API so future UI work does not need
  to bypass the bus or mutate environment files directly.
- The local dashboard should stay a thin shell over official control endpoints,
  not grow into a parallel orchestration path.
- The local dashboard should surface self-heal memory and repair-model settings
  through the same official API instead of inventing a second maintenance path.
- Promoting a remembered repair into always-available memory should reuse the
  memory peer and pinned-memory budget instead of inventing a parallel store.

## Invariants

- Cross-component communication happens over the bus, not via direct shared
  state.
- Messages use explicit envelopes with sender identity, timestamp, and optional
  correlation ID.
- Long-running work should be surfaced as explicit run lifecycle events on the
  bus instead of hidden inside transport-specific control paths.
- Runs should finish with an explicit terminal event so multi-step execution is
  observable without guessing from the last step event.
- Retrieval should be exposed as an explicit peer capability with request and
  response topics, not hidden inside memory or the planner.
- Persistent local state should evolve through shared migrations rather than
  ad hoc file formats per component.
- Persisted runtime settings should be treated as boot defaults, while explicit
  environment variables remain the final override layer for operators.
- A separate vector database is not required yet; embeddings can live in the
  local SQLite store until scale or latency demands otherwise.
- Pinned memory should be protected from working-memory pruning and have its own
  reserved budget so important context remains available to the LLM.
- Pinned memory should be user-manageable through first-class capabilities, not
  only through internal bootstrap logic.
- Pinned memory should not be append-only; operators need an official way to
  remove stale pinned entries.
- The local control API should remain a thin operator surface over the bus, not
  grow into a heavy central gateway.
- Self-heal should run as a separate maintenance lane with its own retained
  incident memory so platform repair work does not pollute normal user memory.
- A recurring incident should reuse a previously successful fix when the
  fingerprint matches, instead of re-planning from scratch every time.
- A local `llama.cpp` adapter may guide self-heal planning, but the platform
  must still offer deterministic repair guidance when no local model is
  configured.
- Local self-heal model settings should be persisted through the same runtime
  settings path as other operator-controlled runtime defaults.
- Remembered self-heal cases should be inspectable and removable through
  official capabilities so stale repair memory does not accumulate forever.
- Remembered self-heal cases should also be promotable into pinned memory when
  a repair is important enough to stay always available.
- Remembered self-heal cases should also be replayable and exportable through
  official capability surfaces so operators can reuse or preload a proven fix.
- Capability inventory should be queryable over the bus so planning can reflect
  the live runtime surface instead of hard-coded assumptions.
- Run requests should be able to carry retrieved context so execution can use
  knowledge hits without coupling directly to the RAG store.
- Run requests may carry explicit execution plans so orchestration and tool
  execution stay separated.
- Tool execution should remain behind capability providers even when a run is
  explicitly planned.
- Prepared goals should be reusable so plan preview and execution can share the
  same retrieved context and capability snapshot.
- The runtime may seed stable local documents into RAG at boot so retrieval is
  useful before any ad hoc ingestion happens.
- The runtime may seed a built-in self-heal skill/playbook so maintenance logic
  has a stable "what and why" reference even before user docs are added.
- The runtime should seed official docs into retrieval and pin concise canonical
  truths into pinned memory so the platform can reason from official material
  instead of guesswork.
- Capability growth should be governed by metadata and runtime profiles, not by
  forcing every capability to be always-on.
- Core capabilities may boot eagerly, while optional or heavy capabilities
  should default toward lazy or profile-gated activation.
- The default `standard` profile should prefer lazy activation for retrieval so
  the platform stays useful on modest hardware without hiding the capability.
- The in-process Tokio bus is the canonical development transport.
- External transports should be adapters behind the same routing model, not
  alternate control planes.
- P2P routing should prefer direct links, then relay assistance, without
  reintroducing a heavy central gateway.
- Hybrid PQ + Noise should be preferred when both peers support it, with
  classical Noise fallback when they do not.
- Session establishment should be explicit and inspectable; transcript events
  should explain why a route and handshake were chosen.
- Router delivery should depend on swappable transport adapters so future QUIC
  and relay transports can slot in without changing classifier or handshake
  policy code.
- `TcpNoise` already uses a real framed TCP adapter.
- Direct `Quic` may use a real local/dev adapter when a peer advertises an
  explicit `quic+insecure://host:port` endpoint.
- Symbolic secure QUIC and relay remain behind simulated adapters until peer
  identity pinning and relay services are implemented for real.
- SQLite-backed peers and operator providers should use dedicated storage
  workers instead of opening the databank directly on shared async lanes.

## Near-term milestones

- Add first-class creative workflow providers for intake, review, approval, and
  handoff stages.
- Add SEO-oriented provider surfaces for metadata packaging, content checks,
  and publication readiness.
- Add CMS-oriented provider surfaces for entry preparation, taxonomy mapping,
  and publish/export workflows.
- Turn the transport planner into secure QUIC/Noise session establishment with
  peer identity pinning.
- Replace the simulated relay adapter with a real relay transport.
- Replace the deterministic local embedder with a real embedding model once the
  model adapter surface is ready.
- Revisit SQLite versus alternative databank designs after the local-first
  storage contracts stabilize.
- Feed retrieved context into more providers than the current knowledge task
  family.
- Add explicit run cancellation and richer terminal states beyond success/fail.
- Flesh out `ordo-models` with provider adapters.
- Use `ordo-runtime` for supervised component startup and shutdown.
- Surface runtime profile selection and capability-tier controls through the
  same dashboard/settings surface.
- Add richer discovery and peer trust enrichment beyond heartbeat defaults.
