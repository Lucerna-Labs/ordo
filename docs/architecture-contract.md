# Architecture Contract

Binding rules for all new work on Ordo. Each rule is derivable
from code already in the workspace, not a preference. Cite violations
back to this document; cite exceptions here too (with the reason
recorded).

**Status:** live. Updated alongside the architecture, not retroactively.

---

## Rule 1 â€” The bus is pub/sub broadcast, not request/response

New crates **subscribe** to bus topics. They do not "call the bus" in
the sense of awaiting a reply on a channel. Request/response shapes are
built on top (oneshot channel in the caller, correlation_id on the
envelope, service pattern-matches the reply) â€” the bus itself is
fire-and-forget broadcast.

**Proof:** [ordo-bus/src/lib.rs](../ordo-bus/src/lib.rs)

## Rule 2 â€” `CapabilityProvider` is the single extension point

Any new capability of the system â€” tool, domain action, external
integration, background task â€” implements
`CapabilityProvider` and is attached via `McpHost::add_provider()`.

The anti-pattern to reject: adding a new HTTP route that contains
business logic, or wiring a one-off direct call between crates. If
you're tempted, the right move is a new provider.

**Proof:** [ordo-mcp-host/src/lib.rs:52](../ordo-mcp-host/src/lib.rs)

## Rule 3 â€” HTTP mirrors the bus, never owns logic

Routes in `ordo-control` are **read-only windows and control knobs**.
They deserialize requests, call into a service or publish to the bus,
serialize the result. If business logic sneaks into a handler, extract
it into a service or a provider before landing.

The bus is the source of truth; HTTP is one view.

**Proof:** [ordo-control/src/lib.rs:219](../ordo-control/src/lib.rs)

## Rule 4 â€” Security wraps providers, not the bus

New providers must be registered through `SecurityStack.gate(provider,
scope)`. Classification and audit happen at the provider boundary, not
inside services. Do not bypass the gate by publishing directly to the
bus from outside a provider.

**Proof:** [ordo-runtime/src/lib.rs:616](../ordo-runtime/src/lib.rs)

## Rule 5 â€” Review is opt-in via the turn flag; destructive ops route through `ReviewProvider`

A turn carrying `review: true` blocks at the `ReviewProvider` until the
operator approves or denies. Destructive operations (publishing,
deleting, charging money, sending externally) **must** be invocable
through a provider the review layer can gate â€” even if the current
turn doesn't set the flag. Review is the escape hatch that lets the
operator tighten the loop without a schema change.

**Proof:** [ordo-review/src/service.rs:40](../ordo-review/src/service.rs)

## Rule 6 â€” One SQLite, migrations added to `ordo-store`'s slice

All persistence lives in the single shared SQLite file. New tables are
added to the migration slice in `ordo-store`, not as separate DB files
per crate. Conventions:

- JSON columns for structured blobs (`metadata_json`, `actions_json`).
- Normalized columns for simple scalars.
- ISO-8601 strings for timestamps.
- BLOB for embeddings.

**New tables from Phase 1.1 onward ship with `workspace_id` from day
one**, defaulted to `"local"`. Retrofitting is Rule 6's hardest case
(see Phase 4.4); avoid it by never omitting the column.

**Proof:** [ordo-store/src/lib.rs:88](../ordo-store/src/lib.rs)

## Rule 7 â€” Services: `new(deps) + .with_X()` builder, Arc-wrapped

Services are constructed by `new()` with required dependencies, then
customized via chained `.with_*()` setters. Runtime owns `Arc<Service>`
and passes clones to handlers. The bus is **injected** via
`.with_bus()`, not stored eagerly inside `new()` (so services are
testable without a live bus).

**Proof:** [ordo-assistant/src/service.rs:69](../ordo-assistant/src/service.rs)

## Rule 8 â€” Progressive disclosure is enforced, not aspirational

**Rule:** Do not pre-fetch context into the system prompt. The LLM
pulls what it needs via meta-tools (`assistant.recall_memory`,
`assistant.knowledge_lookup`, `assistant.plan_routing`, etc.). New
context-bearing subsystems expose themselves as meta-tools the LLM can
call, not as prompt stuffing.

**Rationale â€” the failure mode this prevents:**

The seductive shortcut is "the user asked about brand, so let's dump
the top 10 brand RAG hits into the system prompt before we call the
LLM." This appears to help, then quietly rots the system:

1. **Cost:** every turn pays for 8k+ tokens of context the model
   probably doesn't need. Across a session, this 10Ã—'s the bill.
2. **Signal dilution:** the model gets a blob of retrieved text
   alongside the user's actual question. Its attention splits. Answers
   become vaguer and more hedged, not sharper.
3. **Staleness:** pre-fetched context reflects the state at turn
   start. When the LLM decides mid-turn it needs something different,
   it can't refine â€” it has what it has. Progressive disclosure lets
   the model re-query with a sharper prompt after seeing the first
   hits.
4. **Debuggability:** when answers go wrong, you can't tell whether
   the LLM reasoned poorly or was misled by junk context it didn't
   ask for. Pull-based retrieval puts the model's queries in the
   trace, and the trace becomes the debugging artifact.
5. **Lock-in to one retrieval strategy:** once prompts are
   pre-stuffed, you can't add a new retrieval source without
   growing the prompt. Pull-based means new sources = new meta-tools,
   zero change to existing turns.

Pre-fetching is the easiest-to-measure win at turn 1 and the
hardest-to-undo loss by turn 100. The rule exists because every AI
platform without it regressed into that failure mode.

**Proof:** [ordo-assistant/src/service.rs:301](../ordo-assistant/src/service.rs)

## Rule 9 â€” Arguments/responses are free `Value`; schemas are descriptive, not enforcing

**Rule:** Provider inputs and outputs cross the boundary as
`serde_json::Value`. Schemas (`schemars` derives, `input_schema` on
descriptors) are for **advertisement and validation at boundaries
that explicitly opt in**, not for forcing typed decoding through the
whole system.

**Rationale â€” the temptation and why we reject it:**

The temptation: "if we decoded each provider's input into a typed
struct, bugs like a missing field or a wrong type would be caught at
compile time instead of at runtime."

That statement is true in isolation. Here's why we still reject it:

1. **The bus is polyglot by design.** External plugins (subprocess
   MCPs today, other-language bindings tomorrow) publish on the same
   bus. Typed decoding in Rust forces every non-Rust participant to
   ship a generated client tracking Rust's type evolution. `Value` at
   the boundary keeps the wire format honest and independent.
2. **Schema evolution asymmetry.** Adding an optional field to a typed
   struct requires a recompile of every consumer. Adding an optional
   field to a `Value` is zero work for consumers that don't read it.
   In a system with plugins and user-installed capabilities, the
   asymmetry is load-bearing.
3. **Compile-time catches the wrong bugs.** The bugs that actually hurt
   are "the LLM synthesized the wrong arguments" or "the provider
   returned semantically bad output." Neither is caught by typed
   decoding â€” both need runtime validation or eval. Typed decoding
   catches the drone of mismatched field names, which schemas +
   tests catch just as well.
4. **Descriptive schemas serve the LLM first.** The primary consumer of
   tool schemas is the LLM doing tool-use. It needs JSON Schema, not
   Rust types. Deriving schemas from types via `schemars` is fine
   (it avoids double-booking); *enforcing* typed decoding in the
   runtime is the step we reject.

When a typed struct is genuinely useful â€” e.g., a service's internal
state, or the shape of an HTTP request it accepts directly â€” use it.
The rule is specifically about the **bus and provider boundaries**.

The tripwire: if a future PR argues "we should really typecheck tool
arguments at the provider layer," read this rationale before approving.
The counter-argument already exists; don't relitigate without new
evidence.

## Rule 10 â€” Concurrency primitives are fixed

- `tokio::broadcast` for pub/sub (bus, event streams)
- `tokio::sync::oneshot` for waiters (review decisions, single replies)
- `tokio::sync::mpsc::unbounded` for work queues (storage task queue)
- `std::thread::Builder` for blocking storage work
- `tokio::spawn` for async tasks, with explicit cancellation via drop
  or CancellationToken when cross-task lifetime matters

No new concurrency primitives without a recorded reason.

**Proof:** [ordo-bus/src/lib.rs](../ordo-bus/src/lib.rs), [ordo-store/src/lib.rs:57](../ordo-store/src/lib.rs), [ordo-review/src/service.rs:19](../ordo-review/src/service.rs)

## Rule 11 â€” Protocol is versioned and reviewed

**New bus event types, new tool descriptors, new capability envelopes,
and new persisted entity shapes require an entry in `ordo-protocol`
with rationale, not just a struct definition in the crate that emits
them.** Drive-by additions inside a feature phase are the fastest path
to protocol drift.

- Additive changes (new optional field, new enum variant marked
  non-exhaustive): log a line in `ordo-protocol/CHANGELOG.md` with
  the phase and one-sentence purpose.
- Breaking changes (renaming, removing, changing semantics of an
  existing field): version bump on the protocol crate, migration note
  in the changelog. Typically avoid during Phase 0â€“2.
- Wire-format-sensitive types (anything that crosses a process
  boundary: plugins, MCP bridge, webhooks) always live in
  `ordo-protocol`. A service's internal types do not.

**Proof:** [ordo-protocol/src/lib.rs](../ordo-protocol/src/lib.rs)

---

## Scope notes per phase

Notes that fall out of the rules but are worth making explicit:

- **Phase 1.3 multimodal.** Only Anthropic and OpenAI cloud adapters
  exist today. Gemini and Ollama are stubs; if they get added, multi-
  modal support is part of their contribution, not a blocker for 1.3.
- **Phase 2.4 external MCP client.** First call to a newly-configured
  external MCP tool triggers review regardless of its declared scope.
  After approval, the tool flows through the normal `SecurityStack`
  gate. This mirrors how you'd trust a new collaborator: one explicit
  nod, then routine.
- **Phase 2.5 OAuth.** The auth-off configuration is a first-class
  supported mode, with its own integration test. "Auth off" is not
  "the default when you forget to set the env var" â€” it's a
  deliberate configuration that never regresses.
- **Phase 4.4 workspace_id retrofit.** Every new table from Phase 1.1
  onward has `workspace_id` from day one. Phase 4.4 only retrofits
  tables that predate Phase 1.

---

## Meta

This document is cited, not negotiated. Proposals to change a rule
come with a worked argument for why the existing rationale no longer
holds. If you're about to violate a rule "just for this phase," the
right move is either to update the rule with a recorded exception or
to find the design that doesn't require the violation.
