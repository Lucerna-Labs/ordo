# Build History

This file records how the inherited Codex Claw runtime was built, what worked,
what did not work at first, and what made each slice stable.

Ordo starts from that baseline and redirects the product toward
creative pipelines, workflows, SEO packaging, and CMS operations.

## 2026-04-24 (PM): Concerns 1 + 2 Landing + Mapping Homework

### Goal
- Land the two coding items from the post-push concern review
  (memory-log health visibility; turn_id as a first-class grouping
  primitive) in one session so the next router-integration session
  opens on solid substrate.
- Produce `docs/memory-tree-mapping.md` as the homework artifact
  the blueprint called for â€” the "paper exercise before any
  provider registration code" step.
- Resolve the Dependabot alerts from the morning push before
  touching anything else.

### What worked
- **Dependabot triage first.** Upgraded wasmtime 26â†’36 (closed 12
  Winch advisories; we use Cranelift so they weren't reachable but
  scanners matched on version) and rustls-webpki 0.103.10â†’0.103.13
  (closed the high-sev CRL panic + 2 name-constraint lows). Left
  rand 0.8 via tungstenite as documented-not-reachable (WebSocket
  masking, no custom `rand::rng` logger) and flagged glib as stale
  (not in `cargo tree`).
- **Concern 1 (health visibility).** Added `MemoryLogHealth` +
  `MemoryLogIntegrityReport` protocol types, `SystemHealthProbe`
  event type (auto-pinned), `MemoryLogHealthTask` spawnable with
  `tick_once()` test hook, in-memory counters on the service
  (cumulative + 1h-rolling failures), bus-published `ok` /
  `degraded` events on every canary, 24h canary soft-delete sweep.
  Proof that rescue can subscribe: integration test simulates a
  write-path failure by dropping the `memory_events` table,
  confirms `LOG_HEALTH_DEGRADED` fires with the failure reason and
  updated counters. Startup integrity sweep promoted from a
  `tracing` line to a first-class `IntegrityResult` bus event;
  protocol violation auto-pinned on mismatch.
- **Concern 2 (turn_id).** Optional `turn_id: Option<Ulid>` added
  to `MemoryEvent` with a partial index
  (`WHERE turn_id IS NOT NULL`). Turn loop generates a ULID at
  `run_turn` entry and stamps every event emitted during the turn
  with it. `memory.log.query.by_turn` capability returns all
  events for a turn in timestamp order. Null `turn_id` tolerated
  on query for events predating the field â€” retrofit-safe.
  Including the turn_id in the user-message and agent-response
  payload hashes means identical text across two turns doesn't
  collapse under the dedupe window.
- **The mapping doc as real work, not filler.** Pre-filled a
  template for each of the current 8 RAGs (not 10; the blueprint's
  estimate was high) with my best per-column read of the current
  code, explicit TBDs where the answer requires operator decision,
  a "tree additions required" section listing the 16 nodes
  registration would imply, and three open questions flagged for
  decision before the next coding session. Saved as
  `docs/memory-tree-mapping.md`.

### What did not work
- First `replace_all` on `MemoryTier â†’ RetentionTier` earlier in
  the day had over-applied; a leftover issue surfaced when the
  turn loop's log_agent_response referenced `turn_id` before the
  helper signature was updated. Fixed by making
  `log_user_message(...) -> Option<String>` take the turn_id
  parameter and threading it into both helpers.
- Integration test for the degraded event initially failed because
  the test relied on `#[cfg(test)]`-gated store access, which
  integration-test binaries don't see (they link against the
  non-test lib build). Fix: flipped the store's access helper to
  `pub(crate)` without the cfg guard; `MemoryLogService`
  exposes a `#[doc(hidden)] pub drop_events_table_for_tests`
  convenience that integration tests can reach.
- The turn-id grouping test initially produced 3 events instead
  of 4 because the mock LLM returned identical text, and the
  agent-response payload hash matched across turns, triggering
  dedupe. Fixed by adding the turn_id to the payload â€” correct
  semantics (same text in a different turn is a legitimate
  distinct event) AND makes the test deterministic.

### Successful fix
- Runtime wires two new components: `memory-log-health` (periodic
  canary + counters + degraded emission) and
  `memory-log-integrity-sweep` (one-shot on boot after a 3s grace
  window so subscribers can attach before the result fires).
- All event-constructor sites updated to include
  `turn_id: None` (for non-turn events: protocol violations,
  canaries, test fixtures) or `turn_id: Some(...)` (turn-scoped).
- Protocol CHANGELOG has keyed entries for both concerns; the
  mapping doc gets its own section in the architecture reference.

### Verification
- `cargo test --workspace` (340 passed, 0 failed; +10 from
  morning push)
- `cargo test -p ordo-memory-log` (18 unit + 3 integration)
- `cargo test -p ordo-assistant --test turn_loop turn_events_share_a_turn_id_and_two_turns_have_distinct_ids`
- Dependabot alert count: 17 â†’ 3 (remaining flagged as
  not-reachable or stale)

### Deferred
- The `memory-tree-mapping.md` checklist (see the doc). Next
  session opens on a completed checklist, not an empty one.
- Router + projection integration in the assistant turn loop â€”
  still waiting on the mapping-doc decisions before wiring.
- `rand` 0.8 via tungstenite: needs axum 0.8 migration.

## 2026-04-24: Hierarchical Memory Architecture + Follow-Up Wiring

### Goal
- Replace the existing 4-stage pipeline (assist â†’ persistent â†’
  long-term â†’ 10-RAG) with a bus-first hierarchical memory system
  structured as three decoupled crates, each with its own
  responsibility and acceptance criteria. See
  [memory-architecture.md](memory-architecture.md).
- Close the five honest gaps flagged during the Phase 0â€“4 build:
  call-time LLM failover, EmbeddingStore adoption path, provider
  registration with `McpHost`, live WASM integration test,
  LLM-backed context compaction.
- Land the work without violating the architecture contract
  (`docs/architecture-contract.md`) â€” in particular Rule 1 (bus
  stays pub/sub) and Rule 11 (protocol changes require review).

### What worked
- **Pre-flight bus audit before any handler code.** Confirmed the
  bus supports pub-sub + wildcards, lacks correlation req/resp,
  scatter-gather, provider registry, structured error envelopes.
  Extended ordo-bus with *layered* helpers (`BusCorrelator`,
  `ProviderRegistry`) rather than adding new trait methods â€” Rule
  1 stays intact, memory crates get the patterns they need.
- **Protocol first as a deliberate block.** Added 23 wire-shared
  types + 15 `OrdoMessage` variants + `memory_topics` module as a
  single reviewed change with CHANGELOG entries. Renamed the new
  retention-tier enum to `RetentionTier` to avoid collision with
  the legacy `MemoryTier{Working,Pinned}` â€” the kind of overlap
  that bites silently when two subsystems both use "memory".
- **Three memory crates, each shippable standalone.**
  `ordo-memory-log` (append-only DPM substrate, 12 tests),
  `ordo-memory-router` (tree + fast/classify modes with injectable
  classifier, 9 tests), `ordo-memory-projection` (deterministic
  context assembler with replay + identity-over-budget fail-loud,
  8 tests). Plus a 2-test end-to-end integration proving all three
  cooperate on a single turn.
- **Negative-space tests became load-bearing.** Every blueprint
  invariant got a negative test: payload-hash mismatch, non-ULID
  id, parent-reference invalid, classifier hallucination,
  classifier low confidence, cross-tier without flag, identity
  over budget, replay degraded without cached classifier output,
  retrieved-item missing provenance. If a future refactor drops a
  guard, the test yells before the bug does.
- **Call-time LLM failover (Follow-up 1) without a refactor.**
  Candidate list reused from resolution-time failover; on LLM
  transport error or timeout, the assistant advances to the next
  credential mid-turn. Verified with a two-mock test: primary
  returns 500, secondary returns 200, turn succeeds under the
  secondary's credential.
- **Pluggable vector index (Follow-up 2) via `SqliteEmbeddingStore`.**
  Adapter implements the `EmbeddingStore` trait with namespaced
  SQLite-backed storage. New consumers can adopt the trait
  immediately; existing RAG/Fact/Knowledge stores keep their
  inline SQL until migration is scheduled.
- **FilesProvider + AppsProvider bridged into McpHost
  (Follow-up 3).** Adapters live in ordo-mcp-host so the file/apps
  crates don't need to depend on ordo-mcp-host. Both are wrapped by
  `SecurityStack.gate` so every call goes through the same
  classifier pipeline as plugin providers â€” Rule 4.
- **Live WASM + fuel-limit tests (Follow-up 4).** Hand-rolled
  minimal echo module in WAT, parsed at test time, executed under
  `WasmtimeSandbox`. The `InvalidModule` structural test runs on
  all platforms; the two happy-path tests are `#[ignore]` on
  Windows with an explanatory note about the wasmtime/Cranelift/
  Windows trap-handler interaction.
- **`Summarizer` trait for compaction (Follow-up 5).** Default
  `MechanicalSummarizer` preserves existing behavior; a
  `ScriptedSummarizer` covers tests. LLM-backed impls plug in at
  the runtime-wiring layer, keeping ordo-assistant free of cloud
  deps.

### What did not work
- Initial `replace_all` on `MemoryTier â†’ RetentionTier` over-
  applied to the legacy memory variants in `OrdoMessage`
  (`MemoryStored`, `MemoryStoreRequested`, etc.). Broke 12 files
  transiently. Surgery revert of just the legacy variants fixed
  it; reminder why `replace_all` is a scalpel, not a hammer.
- Wasmtime 26 removed `Config::wasm_reference_types` and changed
  the error surface away from `anyhow::Error`. Compile errors
  only surfaced under the `wasmtime` feature, which isn't in the
  default build path.
- Happy-path live WASM execution on Windows hits a wasmtime
  host-trap interaction that crashes the process. `#[ignore]` on
  Windows with a documented workaround (fiber-based async or run
  on Linux/macOS) is honest about the platform constraint rather
  than silently skipping.

### Successful fix
- Bus extensions layered, not baked: `BusCorrelator` +
  `ProviderRegistry` are free-standing types that *use* the
  existing pub-sub. The bus trait is unchanged; every existing
  consumer keeps working without edits.
- Protocol changes went in as a dedicated submodule
  (`ordo-protocol/src/memory.rs`) with CHANGELOG entries keyed to
  each addition. Exhaustive-match sites in `ordo-classify` and
  `ordo-router` updated in the same commit.
- Migrations added for `memory_events`, `memory_tree_nodes`, and
  `vector_index` â€” all follow Rule 6 (one SQLite, migrations in
  ordo-store, `workspace_id` from day one).
- Runtime wiring constructs the three memory services at boot
  time and shares the bus with them. They are alive on the bus
  but don't yet drive the assistant turn loop â€” deliberate next
  step; see deferred work below.

### Verification
- `cargo check --workspace`
- `cargo test --workspace` (329 passed, 0 failed)
- `cargo test -p ordo-sandbox --features wasmtime --test live_wasmtime`
  (1 passed, 2 ignored with platform note)

### Deferred as clear next-step work
- Wire the memory log/router/projection into the assistant turn
  loop. Today they are constructed and reachable on the bus but
  no component calls them.
- Migrate existing RAG/Fact/Knowledge stores to the
  `EmbeddingStore` trait. Trait + `SqliteEmbeddingStore` exist;
  consumers choose when to adopt.
- Promote `Summarizer` plug-in into the prompt builder so
  compaction can optionally call an LLM-backed summarizer.
- Wasmtime JIT live tests on non-Windows hosts; or enable
  fiber-based async execution to work around the Windows trap
  interaction.
- Full "10 RAGs as bus providers" wrapping exercise â€” the tree
  structure + provider registry pattern is in place; the mapping
  from existing RAG instances to tree paths is the remaining
  one-time step.

## 2026-04-02: Canonical Project Direction

### Goal
- Choose one canonical Rust project and stop losing progress across duplicate folders.

### What worked
- Promoting the Rust/Tokio workspace into the only surviving repo gave the
  project a clear center of gravity.
- Keeping the architecture bus-first instead of gateway-first made the crate
  layout cleaner and easier to reason about.

### What did not work
- Earlier work was hard to locate because it lived under inspection copies and
  duplicate folders.
- The first Rust scaffold was incomplete and did not compile cleanly.

### Successful fix
- Keep one repo only: `codex-ordo-project`.
- Preserve the upstream OpenClaw ideas as reference docs, but make the Rust
  workspace the canonical implementation.
- Record progress in `docs/dones.md` so the project can be resumed without
  archaeology.

### Verification
- `cargo check`
- GitHub backup created for the canonical repo

## 2026-04-02: Runtime Foundation

### Goal
- Build a real local-first runtime spine with explicit contracts.

### What worked
- Splitting protocol, bus, runtime, brain, MCP host, memory, router, transport,
  handshake, discovery, and planner into crates made responsibilities clearer.
- The in-process Tokio bus was fast to iterate on and easy to test.
- Explicit envelopes, sender IDs, timestamps, and correlation IDs made request
  and response flows observable.

### What did not work
- Demo-only capability matching was not enough to support real task execution.
- Plannerless execution hid too much inside provider-specific logic.

### Successful fix
- Add explicit run lifecycle messages and an explicit planner surface.
- Keep providers responsible for execution, but keep orchestration in the brain
  and plan generation in the planner crate.

### Verification
- `cargo test --workspace`
- `cargo run`

## 2026-04-02: P2P and Session Direction

### Goal
- Avoid rebuilding a gateway while still supporting peer-aware routing.

### What worked
- Separating classification, discovery, transport, handshake, and routing into
  distinct crates kept the no-gateway design honest.
- Route planning plus transcripted session establishment made transport decisions
  inspectable.

### What did not work
- Treating transport as a monolithic runtime concern would have recreated a
  hidden gateway in practice.
- Keeping routing decisions implicit would have made later QUIC and relay work
  hard to debug.

### Successful fix
- Let the classifier produce directives.
- Let transport choose direct versus relay.
- Let handshake choose PQ-first versus fallback.
- Let the router record every choice in a transcript.

### Verification
- Route and handshake tests
- CLI session transcript demo

## 2026-04-02 to 2026-04-03: RAG and Knowledge Execution

### Goal
- Give the platform a real retrieval layer and use it during execution.

### What worked
- Treating RAG as a peer with its own request/response topics fit the bus model.
- Seeding the core docs at boot gave retrieval useful context immediately.
- Feeding retrieved context into knowledge-style runs worked well.

### What did not work
- A pure lexical baseline was useful but too brittle to be the final retrieval
  shape.
- Knowledge tasks were initially too demo-like until they flowed through the
  planner and capability surface.

### Successful fix
- Add a deterministic embedding baseline and hybrid scoring.
- Keep embeddings in the same SQLite store instead of reaching for a vector DB
  too early.
- Use explicit `knowledge.summarize` plans backed by the same provider model as
  other tools.

### Verification
- RAG ingest/query tests
- CLI RAG retrieval demo
- knowledge-run demo using retrieved context

## 2026-04-03: Storage Consolidation

### Goal
- Replace temporary JSONL persistence with a real local databank.

### What worked
- A shared `ordo-store` crate with migrations made state evolution much easier.
- One SQLite database for memory and RAG simplified local persistence.
- Legacy JSONL import preserved earlier state instead of throwing it away.

### What did not work
- Relying on append-only JSONL stores was too limiting once retention, hybrid
  retrieval, and richer metadata were added.
- The ecosystem path through `tokio-rusqlite` was not attractive because of the
  crate-version mismatch with the current `rusqlite` line.

### Successful fix
- Use `rusqlite` directly with shared migrations.
- Keep SQLite local for now and defer the storage rethink until the rest of the
  runtime stabilizes.

### Verification
- migration-backed store tests
- `cargo test --workspace`
- `cargo run`

## 2026-04-03: Storage Budgets and Separation

### Goal
- Make retention controllable and keep runtime state separate from user files.

### What worked
- Splitting budgets into RAG, working memory, pinned memory, and later
  self-heal history created a clean mental model for UI controls.
- Rooting filesystem operations under `user-files` reduced accidental bleed
  between runtime state and user content.
- Container volumes map cleanly to that split.

### What did not work
- Treating all stored memory the same made it impossible to protect important
  platform truths from normal pruning.

### Successful fix
- Add `MemoryTier`.
- Give pinned memory its own reserved budget.
- Keep filesystem operations rooted under `user-files`.

### Verification
- memory retention tests
- filesystem provider demo using `user-files`
- container files and env wiring

## 2026-04-03: Self-Heal Lane

### Goal
- Give the platform a dedicated repair brain that can remember recurring fixes
  and optionally use a local model without making model setup mandatory.

### What worked
- Treating self-heal as its own crate preserved separation from normal
  orchestration.
- Keeping incident fingerprints and successful repairs in SQLite made repeated
  incidents much cheaper to handle.
- A deterministic fallback kept the feature usable when no local model was
  configured.

### What did not work
- Carrying a live SQLite handle across async `await` points made the self-heal
  peer fail `Send` requirements at compile time.
- Adding new protocol variants without updating every classifier/router match
  table caused exhaustiveness failures.

### Successful fix
- Load self-heal history from SQLite before awaiting the model call.
- Move repeat-incident logic onto stable incident fingerprints.
- Update classifier and router message labeling whenever new protocol variants
  are introduced.

### Verification
- self-heal unit tests
- CLI demo showing first-time planning and second-time reuse
- `cargo test --workspace`
- `cargo run`

## 2026-04-03: Official Memory and Fixbook

### Goal
- Stop making the platform guess about its own architecture and repair history.

### What worked
- Writing canonical docs lets the runtime seed authoritative knowledge at boot.
- Pinned memory is the right place for concise official truths.
- RAG is the right place for more detailed build and repair history.

### What did not work
- Relying only on conversational memory or ad hoc notes made the project too
  easy to forget or misremember.

### Successful fix
- Add `docs/official-memory.md` for concise canonical truths.
- Add `docs/build-history.md` for implementation chronology.
- Add `docs/fixbook.md` for failed attempts and successful fixes.
- Seed those docs into boot-time retrieval and pin the official memory bullets
  into always-available memory.

### Verification
- runtime boot now reports pinned memory bootstrap count
- CLI demo runs with the new docs seeded into retrieval

## 2026-04-03: Capability Scaling Without Hardware Bloat

### Goal
- Let the platform gain more capabilities without forcing every install to boot
  every optional subsystem.

### What worked
- Adding explicit capability metadata makes it possible to reason about core,
  optional, and heavy lanes instead of treating everything the same.
- Runtime profiles provide a stable contract for lean versus richer installs.

### What did not work
- A flat capability list alone does not tell the platform or the user which
  capabilities are expensive.
- Growing the capability surface without activation rules would eventually make
  hardware cost track feature count too closely.

### Successful fix
- Add structured capability descriptors to inventory responses.
- Add `minimal`, `standard`, and `full` runtime profiles.
- Keep the default profile practical while reserving room for future heavier
  capability sets.
- Move retrieval in the default profile behind a lazy activation lane so the
  heavy peer only boots on the first retrieval request.
- Add a persisted runtime settings table so future UI controls have an official
  place to store profile and storage choices.
- Add explicit pinned-memory control capabilities so the always-available memory
  lane is not only an internal bootstrap path.
- Add a thin local control API so the future UI has stable endpoints without
  bypassing the capability system.

### Verification
- capability inventory now returns descriptors
- CLI demo prints tier and activation metadata
- optional demos skip themselves when the selected profile keeps those providers offline
- `standard` now boots with `rag-peer-lazy`, and the real RAG peer only comes
  online after the first retrieval call
- persisted runtime settings can be queried and updated through runtime
  capabilities
- pinned memories can now be listed and explicitly added through the capability
  surface
- the runtime now boots a working local API server for UI-facing control paths

## 2026-04-03: Built-In Control Dashboard

### Goal
- Make storage budgets and memory-lane controls usable without requiring a
  separate app or manual API calls.

### What worked
- Reusing the existing control API kept the dashboard thin and honest.
- Serving the dashboard from the same local runtime bind made it easy to
  discover and containerize.
- Adding working-memory list support completed the operator picture so the UI
  can show both memory lanes instead of only pinned notes.
- Letting the same path remove pinned entries kept the memory lane manageable
  instead of turning it into an append-only bucket.

### What did not work
- Backend endpoints alone still left too much friction for normal users.
- A pinned-memory-only view hid half of the memory model.

### Successful fix
- Serve a built-in dashboard from `GET /` in `ordo-control`.
- Keep the dashboard on top of official API endpoints rather than giving it
  direct database access.
- Add `memory.list_working` plus `GET /api/memory/working?limit=...`.
- Add `memory.unpin_note` plus `DELETE /api/memory/pinned`.
- Show runtime profile, budgets, capability inventory, pinned memory, and
  working memory in one local operator surface.

### Verification
- `cargo test --workspace`
- `cargo run`
- control dashboard route test
- runtime demo now prints the dashboard URL

## 2026-04-03: Self-Heal Operator Surface

### Goal
- Make the maintenance lane inspectable and configurable through the same
  official dashboard and settings path as the rest of the runtime.

### What worked
- Reusing the persisted runtime settings table kept self-heal model
  configuration aligned with profile and storage settings.
- Treating remembered fixes as first-class capability results made it easy to
  expose them through both the bus and the local HTTP layer.
- Letting operators forget stale remembered fixes keeps self-heal memory useful
  instead of turning into an untouchable archive.

### What did not work
- Self-heal memory was initially visible only to the internal repair lane, not
  to operators trying to understand or curate it.
- Local model settings still depended too much on environment variables, which
  is awkward for normal users.

### Successful fix
- Persist optional `llama.cpp` binary and model settings in the runtime
  settings store.
- Expose `self_heal.list_cases` and `self_heal.forget_case` through the MCP
  host.
- Add `GET /api/self-heal/cases` and `DELETE /api/self-heal/cases`.
- Extend the built-in dashboard to show remembered repairs and self-heal model
  controls.

### Verification
- self-heal store history tests
- control API self-heal endpoint test
- `cargo test --workspace`
- `cargo run`

## 2026-04-03: Promote Remembered Repairs Into Pinned Memory

### Goal
- Let the operator surface turn a proven remembered repair into always-available
  pinned memory instead of leaving it stranded only inside self-heal history.

### What worked
- Reusing the existing pinned-memory lane kept important repair knowledge inside
  the same budgeted memory model as other always-available context.
- Exposing promotion through the self-heal capability surface kept the
  dashboard thin and consistent with the rest of the runtime.

### What did not work
- Listing and forgetting remembered repairs was not enough once we wanted the
  platform to retain especially important fixes as first-class guidance.
- The first promotion pass surfaced a real bug: repeated memory-reuse planning
  kept bloating the stored `why` text, which only became obvious when we pinned
  and displayed a remembered fix.

### Successful fix
- Add `self_heal.pin_case` plus `POST /api/self-heal/cases/pin`.
- Route promotion through the memory peer so pinned-memory budgets still apply.
- Extend the dashboard with a `Pin Fix` action for each remembered repair.
- Normalize memory-reuse `why` text so repeated incidents do not recursively
  bloat promoted repair notes.

### Verification
- self-heal store summary test
- control API self-heal pin integration test
- `cargo test --workspace`
- `cargo run`

## 2026-04-03: Storage Workers And Replayable Repair History

### Goal
- Stop opening SQLite directly from operator-facing providers and make remembered
  self-heal fixes replayable and exportable.

### What worked
- The generic storage-worker seam was enough to move runtime settings and
  self-heal history off the shared async path without changing the bus
  contracts.
- Sharing the self-heal storage worker between the repair peer and the MCP
  provider kept the operator surface aligned with the live repair memory.
- Replaying a remembered fix through the normal self-heal request/response flow
  reused the same recurrence logic the platform already trusts.

### What did not work
- The first refactor pass left async archive/storage call sites behind in the
  memory peer and surfaced test fallout where old direct store calls were still
  assumed.
- Review/pin alone still left remembered fixes too passive for operators who
  wanted to actively retry or package a known-good repair.

### Successful fix
- Add generic `StorageTask` worker primitives in `ordo-store`.
- Route runtime settings through `RuntimeSettingsTask`.
- Share `SelfHealStorageTask` between `ordo-heal` and the self-heal tools.
- Expose `self_heal.replay_case` and `self_heal.export_case`.
- Add `POST /api/self-heal/cases/replay` and
  `POST /api/self-heal/cases/export`.
- Extend the built-in dashboard with replay/export actions and an operator
  detail panel.

### Verification
- `cargo test --workspace`
- `cargo run`
- control API self-heal replay/export test

## 2026-04-03: Real TCP Delivery Under The Transport Seam

### Goal
- Replace one simulated transport path with a real socket-backed adapter
  without pretending QUIC and relay are already finished.

### What worked
- The existing envelope serialization was enough to build a framed TCP delivery
  path without changing message contracts.
- Keeping a mixed adapter let the router preserve simulated behavior for QUIC
  and relay while using a real path for `TcpNoise`.
- A tiny CLI listener demo made the new transport path visible without needing
  a second full runtime.

### What did not work
- The first new route-selection test accidentally reused a symmetric-NAT peer,
  which the planner correctly upgraded to relay instead of `TcpNoise`.
- Treating every transport kind as equally real would have overclaimed the
  architecture, so the docs had to be explicit about what is still simulated.

### Successful fix
- Add framed envelope read/write helpers in `ordo-transport`.
- Add a real TCP adapter for `TransportKind::TcpNoise`.
- Use the mixed/default adapter in the router.
- Add a live TCP delivery demonstration to the CLI.
- Document that QUIC and relay remain simulated.

### Verification
- `cargo fmt`
- `cargo test --workspace`
- `cargo run`

## 2026-04-03: Real Direct QUIC Delivery For Explicit Local/Dev Endpoints

### Goal
- Add one honest real QUIC path without pretending relay or secure peer
  identity are already finished.

### What worked
- Keeping direct QUIC opt-in behind explicit `quic+insecure://` endpoints let
  the adapter become real without breaking existing symbolic peer demos.
- Reusing the framed-envelope contract meant TCP and QUIC could share the same
  payload shape instead of inventing a parallel network codec.
- A tiny request/ack exchange over QUIC made the transport test deterministic
  enough for both unit tests and the CLI demo.

### What did not work
- The first QUIC pass failed because rustls needed an explicit process-level
  crypto provider installation for the test/runtime process.
- The first shutdown flow closed the QUIC connection too aggressively, which
  raced the server before it had accepted the stream.

### Successful fix
- Install the rustls ring crypto provider explicitly before building local QUIC
  client and server configs.
- Restrict the real QUIC path to explicit `quic+insecure://host:port`
  endpoints for now.
- Switch the demo/test QUIC flow to a bidirectional request/ack exchange so
  delivery completes before local teardown.
- Keep symbolic secure QUIC and relay on the simulated adapter until the
  identity and relay layers are real.

### Verification
- `cargo fmt`
- `cargo test --workspace`
- `cargo run`
