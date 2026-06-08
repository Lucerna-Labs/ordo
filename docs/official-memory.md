# Official Memory

This document is the canonical memory pack for Ordo.
It is meant to be:
- pinned into always-available memory
- seeded into local retrieval
- treated as the official reference before the platform guesses

## Canonical Facts
- Ordo is the creative-operations fork of the Codex Claw runtime.
- Ordo is a local-first Rust AI runtime built around an in-process Tokio bus.
- The current runtime surface is inherited from the Codex Claw backend while the
  product direction pivots toward creative pipelines, workflows, SEO, and CMS
  operations.
- Domain skills and tools should stay separated by prefix:
  - `creative.*`
  - `workflow.*`
  - `seo.*`
  - `cms.*`
- Interface skills and tools should also stay separated by prefix:
  - `ssh.*`
  - `api.*`
  - `rest.*`
- Cross-component communication should happen over explicit bus messages, not hidden shared state.
- The project intentionally avoids a heavy central gateway and instead splits discovery, routing, transport, handshake, and execution into separate crates.
- `ordo-protocol` is the source of truth for message types, topics, IDs, and shared runtime contracts.
- `ordo-bus` is the canonical event fabric during development.
- `ordo-classify` decides whether work stays local, targets a peer, or broadcasts.
- `ordo-discovery` tracks peers and capability availability.
- `ordo-transport` plans direct versus relay-backed delivery paths.
- `ordo-handshake` owns pairing and crypto negotiation, including PQ-first policy.
- `ordo-router` establishes sessions and records why a route and handshake were chosen.
- `ordo-mcp-host` owns provider-backed capability execution.
- `ordo-brain` owns orchestration, plan preparation, run submission, and run result collection.
- `ordo-planner` turns goals into explicit capability-backed execution plans.
- `ordo-rag` owns retrieval and document indexing.
- `ordo-memory` owns archived memory and memory queries.
- `ordo-heal` is a separate maintenance lane for platform repair and recurring-fix reuse.
- `ordo-runtime` boots and supervises the default local component set.
- Creative pipeline state should be modeled as explicit workflow stages such as
  intake, brief, production, review, SEO packaging, CMS preparation, publish,
  and feedback.
- SEO metadata and CMS publishing readiness are product concerns for this fork,
  even where the exact providers have not been implemented yet.
- Brand rules, content models, channel constraints, taxonomy rules, and publish
  checklists should be treated as authoritative retrieval context.
- User files and runtime state must stay on separate paths.
- Filesystem access must remain rooted under the configured user-files directory.
- Capability providers should remain the only place that actually executes tools.
- Runs should finish with explicit terminal events instead of implicit completion.
- Prepared goals should be reusable so preview and execution share the same context and plan.
- Official project documentation should be seeded into retrieval at boot so the system has authoritative context.
- Retrieval should be split into a small `main` collection plus focused domain
  and interface collections instead of one mixed index.
- The `main` retrieval collection is the right home for compact cross-domain
  creative guidance such as design basics, marketing foundations, writing
  modes, typography, and color theory.
- Important platform truths should also be pinned into always-available memory.
- SQLite is the current local databank because it is embedded, practical, and easy to migrate.
- SQLite is not treated as the final philosophical storage choice and should be revisited later.
- RAG storage, working memory, pinned memory, and self-heal history each need separate budgets.
- Pinned memory is reserved for information the platform should always have available.
- The current retrieval baseline is hybrid lexical plus local embeddings stored in SQLite.
- A separate vector database is not required yet.
- The self-heal lane may use a user-supplied local `llama.cpp` model.
- The self-heal lane must still work without a local model by falling back to deterministic repair guidance.
- A recurring incident should reuse a previously successful fix when its fingerprint matches.
- The platform should remember how successful fixes were made so the next occurrence starts from known-good guidance instead of rediscovery.
- Remembered self-heal cases should be inspectable and removable through
  official capabilities and the local control API.
- Proven remembered self-heal cases should be promotable into pinned memory
  when a repair deserves to stay always available.
- Remembered self-heal cases should also be replayable through the live repair
  lane and exportable as reusable official repair packs.
- Optional self-heal model settings should persist in the local databank so the
  dashboard can manage them without hand-editing environment files.
- Many capabilities are acceptable as long as they are not all expensive and always-on.
- Capability metadata should distinguish core, optional, and heavy lanes.
- Runtime profiles should decide what boots eagerly so modest hardware can still run the platform.
- Capability growth should prefer lazy activation and profile gating over permanently higher hardware requirements.
- The default `standard` runtime profile should lazy-activate retrieval instead of booting the full RAG peer immediately.
- Runtime policy should be queryable through official capabilities instead of hidden config assumptions.
- Persisted runtime settings should live in the local databank so a UI can manage budgets and profile choices without rewriting env files.
- Explicit environment variables should still override persisted runtime settings when an operator needs a hard override.
- Important pinned memories should be manageable through first-class capabilities so the UI can control the always-available lane directly.
- Important pinned memories should also be removable through that same official control surface so stale permanent context does not accumulate forever.
- A thin local control API is acceptable when it is only an operator/UI surface over the bus and not a replacement for the distributed architecture.
- The local dashboard should stay on top of those official control endpoints instead of bypassing the bus or reading the databank directly.
- The operator surface should show both pinned memory and working memory so users can understand what is always available versus what is disposable.
- The operator surface should also show remembered self-heal fixes so platform
  repair memory is reviewable instead of hidden.
- Promoting a remembered repair should reuse the pinned-memory lane instead of
  inventing a second store for important fixes.
- SQLite-backed memory, RAG, self-heal history, and persisted runtime settings
  should run behind dedicated storage-worker threads so local persistence does
  not steal time from the main async execution path.
- `TcpNoise` now has a real framed TCP delivery path.
- Direct QUIC also has a real local/dev delivery path when a peer advertises an
  explicit `quic+insecure://host:port` endpoint.
- Symbolic secure QUIC and relay routes are still simulated and should be
  described that way.
- The built-in docs are part of the product, not just developer notes.
- Ordo should eventually grow first-class provider lanes for creative
  workflow routing, SEO packaging, and CMS publishing, but it should not
  overclaim those capabilities before they are implemented.
