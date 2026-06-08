# Hierarchical Memory Architecture

The three-crate replacement for the old 4-stage pipeline
(assist â†’ persistent â†’ long-term â†’ 10-RAG). Based on the
`ordo-memory-blueprint-v2` design doc.

Status: **crates shipped and tested; assistant turn-loop integration
still to do**.

## The crates

### `ordo-memory-log` â€” append-only event log

The DPM (Deterministic Projection Memory) substrate. Source of truth.

Owns:
- Event ingestion (append-only, idempotent on `payload_hash` within a
  5-second window for bus-retry dedupe)
- Storage (SQLite via `ordo-store`)
- Retrieval by id / range / parent chain
- Pin / soft-delete (never hard-delete)
- `snapshot_hash` for replay verification
- JSONL export for backup / migration

Does NOT own interpretation of events, embedding, routing decisions, or
projection logic.

Key invariants enforced:
- Event ids MUST be ULIDs (append rejects non-ULIDs as
  `InvalidEventId` violations).
- `payload_hash` is recomputed on append and MUST match
  (`PayloadHashInvalid` violation otherwise).
- `parent_id`, when set, MUST reference an existing event
  (`ParentReferenceInvalid` violation otherwise).
- `identity.assertion` and `system.protocol_violation` auto-pin.
- `workflow.checkpoint` auto-lands in the warm tier.
- Soft-delete is a flag, never a DELETE. Queries exclude soft-deleted
  rows; the physical row stays for replay.

Tables: `memory_events` (Rule 6: workspace_id from day one).

12 unit tests covering positive and negative cases.

### `ordo-memory-router` â€” tree-routed retrieval

Given a query, decide which providers to invoke.

Owns:
- The memory tree (mutable at runtime; tombstone-based soft-delete so
  replays can reconstruct past structure)
- Two routing modes: Fast (deterministic BM25-style + domain hint) and
  Classify (injectable LLM classifier with output cached on the
  decision event for DPM-correct replay)
- Auto mode that picks between them based on Fast-mode confidence
- Provider registry lookups (delegated to `ordo_bus::ProviderRegistry`)

Does NOT own retrieval execution itself (providers do that) or context
assembly (projection does that).

Key invariants enforced:
- Classifier output is validated against the live tree â€” hallucinated
  paths are rejected as `ClassifierHallucination` violations.
- When no classifier node reports confidence â‰¥ 0.6, the decision falls
  back to top-3 Fast-mode results and emits `ROUTE_LOW_CONFIDENCE`.
- Classify-mode decisions CACHE the classifier's full output so replay
  never re-calls the LLM (DPM: "capture non-deterministic outputs,
  never recompute them").
- Tree mutations emit `ordo.memory.tree.change` on the bus *before*
  the mutation â€” intent is audit-worthy even if the mutation later
  fails.
- Tombstoning preserves historical tree state so
  `list_at_timestamp(ts)` can reconstruct the tree-as-of-then.

The `Classifier` trait is injectable:
- `ScriptedClassifier` for deterministic tests.
- LLM-backed impls live at the runtime-wiring layer (not in this crate)
  to keep the router free of cloud deps.

9 unit tests covering tree + routing modes + negative-space cases.

Tables: `memory_tree_nodes` (workspace_id from day one).

### `ordo-memory-projection` â€” deterministic context assembly

Given (query, event log slice, tree state, routing decision, retrieved
items, pin set), produce the LLM context window.

Owns:
- Pure projection function: same inputs â†’ byte-identical output.
- Token budget priority ordering: pinned identity â†’ workflow checkpoint
  â†’ other pinned â†’ current query â†’ retrieved â†’ recent log.
- Replay: uses the cached routing decision output, never re-calls the
  LLM. Replay failure modes are first-class outputs
  (`MissingClassifierOutput`, `HashMismatch`, `Impossible`) rather than
  exceptions.
- `output_hash`: blake3 of context + provenance for replay verification.

Does NOT own the log, the router, or retrieval execution.

Key invariants enforced:
- Identity assertions exceeding budget FAIL LOUDLY with
  `IdentityOverBudget` â€” never silently truncated â€” unless the caller
  explicitly opts in via `allow_identity_truncation: true`.
- Retrieved items with null provenance are DROPPED and emit
  `ProvenanceMissing` violations. There is never a provenance-less
  result in the final context.
- Token budget is respected exactly â€” the final context never exceeds
  it.
- Replay of a Classify-mode decision without a cached classifier
  output returns `ReplayMissingClassifierCache`. The service never
  re-calls the LLM.

8 unit tests + 2 end-to-end integration tests (in
`ordo-memory-projection/tests/end_to_end.rs`).

## Bus extensions

The blueprint required request/response, scatter-gather, provider
registry, and structured errors. None existed on the pub-sub `Bus`
trait. Resolution: extend with *layered helpers* that use the existing
pub-sub under the hood, so Rule 1 (bus is pub-sub) stays intact.

- `ordo_bus::BusCorrelator` â€” `call(request_topic, reply_topic,
  envelope, timeout)` for req/resp; `scatter_gather(...)` for N-way.
  Works by subscribing to the reply topic and filtering by
  `correlation_id`.
- `ordo_bus::ProviderRegistry` â€” in-memory `HashMap<path, Vec<Entry>>`
  with heartbeat-based expiry. `register`, `deregister`, `heartbeat`,
  `for_path`, `sweep_expired`, `all`.

11 unit tests in ordo-bus.

## Protocol additions

All in `ordo-protocol/src/memory.rs`. See
`ordo-protocol/CHANGELOG.md` for the keyed-by-addition record.

Wire types (23):
`MemoryEvent`, `MemoryEventType`, `RetentionTier`, `MemoryLogFilter`,
`MemoryLogQueryByRange`, `MemoryLogQueryResult`, `FeedbackSignal`,
`FeedbackPolarity`, `FeedbackSource`, `RetrievalSemantics`, `CostHint`,
`RouteMode`, `ProviderRegistration`, `TreeNode`, `ClassifierOutput`,
`ClassifierNodeChoice`, `RouteDecided`, `ProjectionRequest`,
`ProjectionBuilt`, `ReplayDegradedReason`, `ProtocolViolation`,
`ProtocolViolationType`, `Severity`.

`OrdoMessage` variants (15): log ops, router ops, projection ops,
feedback, protocol violation, tree change.

Bus topics (single source of truth): `memory_topics` module.

## What's wired today

At runtime boot:
- Three services constructed from the shared SQLite.
- Both wired with the shared bus.
- The router's registry is empty (no providers register themselves as
  memory providers yet â€” that's the "10 RAGs as bus providers"
  exercise).
- No component calls the services yet â€” they're reachable on the bus,
  but the assistant turn loop still uses the legacy retrieval path.

## What's deferred (honestly)

- **Assistant turn-loop integration.** The turn should:
  1. Append `user.message` to the log before the LLM call.
  2. Ask the router for tree nodes + providers.
  3. Dispatch to providers (scatter-gather) to gather retrieved items.
  4. Build a projection from the log + tree + decision + retrieved.
  5. Feed the projection into the LLM call.
  6. Append `agent.response` to the log after.
  Today none of steps 1â€“4 or 6 are wired. The plumbing exists; the
  assistant service needs to call it.
- **10 RAGs as bus providers.** Each existing RAG gets wrapped as a
  `ProviderRegistration` with `serves_paths` â†’ tree node mappings.
  One-time mapping exercise; the wrapper pattern is documented in the
  blueprint's "Provider wrapping" section.
- **Consolidation / sleep phase.** Not built. Static tree is adequate
  for v1.
- **Cold tier.** The `include_cold` flag on range queries is in the
  protocol; the actual `ATTACH DATABASE` plumbing is deferred.
- **Automatic threshold recalibration.** Feedback signals are
  collected from day one; automatic recalibration of the 0.75 fast
  threshold is phase-2 work.

## How to verify

```bash
cargo test -p ordo-bus                # bus extensions (11)
cargo test -p ordo-memory-log         # log (12)
cargo test -p ordo-memory-router      # router (9)
cargo test -p ordo-memory-projection  # projection (8) + e2e (2)
cargo test --workspace                # all of it (329 passing)
```

## Next stepchoice: where the memory crates first get called

Two equal-quality options for threading memory into the turn:

1. **Assistant turn loop first.** The fastest path to "the operator sees
   the benefit" â€” every turn becomes DPM-replayable. Lands one place,
   ships one behavioral change.

2. **RAG wrapping first.** Populates the router with real providers, so
   Classify-mode calls can actually discriminate. Takes longer, but
   makes the router visibly useful before the assistant changes.

Either can go first. (1) is probably the right call for a single-
operator local-first build: visible benefit + a real integration test
that exercises the full stack on every turn.
