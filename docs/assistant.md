# Assistant

The Assistant is the top layer of the Ordo platform â€” the
"lawyer on top of the RAGs." Every interaction the operator has with
the platform is supposed to route through it. It holds durable memory
about who the operator is, keeps the conversation thread, decides
what to pull from the specialized RAG collections, and is the single
gate through which the cloud LLM is called.

Everything else in the workspace â€” plugins, UI extensions, cloud
credentials, review, security, RAG collections, individual capability
lanes â€” is **infrastructure the Assistant draws from**, not a parallel
way to talk to the platform.

## Architecture

```
  â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
  â”‚  ASSISTANT (ordo-assistant + AssistantProvider in ordo-mcp-host)      â”‚
  â”‚                                                                  â”‚
  â”‚  â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”  â”‚
  â”‚  â”‚  FactStore (semantic)                                      â”‚  â”‚
  â”‚  â”‚    SQLite: assistant_facts (subject/predicate/object +     â”‚  â”‚
  â”‚  â”‚    source + confidence + embedding BLOB)                   â”‚  â”‚
  â”‚  â”‚    Recall: cosine similarity against the shared embedder.  â”‚  â”‚
  â”‚  â”œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¤  â”‚
  â”‚  â”‚  Sessions + Turns                                          â”‚  â”‚
  â”‚  â”‚    SQLite: assistant_sessions, assistant_turns             â”‚  â”‚
  â”‚  â”‚    Every turn persists: user msg, reply, retrieved context â”‚  â”‚
  â”‚  â”œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¤  â”‚
  â”‚  â”‚  Progressive-disclosure turn loop (push 3)                 â”‚  â”‚
  â”‚  â”‚    L0 bootstrap system prompt + meta-tool menu             â”‚  â”‚
  â”‚  â”‚    L1 assistant.recall_memory   â†’ FactStore                â”‚  â”‚
  â”‚  â”‚    L2 assistant.knowledge_lookup â†’ KnowledgeStore          â”‚  â”‚
  â”‚  â”‚    L3 assistant.plan_routing    â†’ RouterPlanner (LLM)      â”‚  â”‚
  â”‚  â”‚    L4 bus-backed domain + interface capabilities           â”‚  â”‚
  â”‚  â”‚  Each tool result carries a read-only preamble that tells  â”‚  â”‚
  â”‚  â”‚  the LLM how to use the layer â€” keeps drift down without   â”‚  â”‚
  â”‚  â”‚  re-sending the system prompt.                             â”‚  â”‚
  â”‚  â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜  â”‚
  â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”¼â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
                         â”‚  â”‚
                         â”‚  â””â”€â”€â–º LLM (ordo-cloud.openai / anthropic)
                         â”‚
                         â””â”€â”€â–º RAG (main, creative, workflow, seo, cms, brandâ€¦)
```

## What push 2 adds

- **Autonomous tool use.** The Assistant now sees every registered
  capability (filtered by the `tools::DEFAULT_ALLOWED_LANES` allow
  list) as an OpenAI function-calling tool. When the LLM responds
  with `tool_calls`, the bus-backed `ToolGateway` invokes them,
  feeds results back, and re-calls the LLM until it stops asking
  for tools (capped at `max_tool_iterations`, default 6).
- **Per-session event broadcaster** so the studio chat UI can show
  what the Assistant is doing in real time.
- **WebSocket** `GET /ws/assistant/:session` â€” clients subscribe to
  receive `turn_started`, `context_retrieved`, `tool_call_started`,
  `tool_call_completed`, `tool_call_failed`, `turn_completed`, and
  `turn_failed` events as the turn unfolds.
- **Studio Assistant tab** â€” chat UI + live-progress side rail +
  profile-facts panel with operator-side "forget" buttons. The
  Assistant tab is now the studio's **default**; everything else
  becomes inspection / admin surface.
- **Auto-extraction pass.** Every 10 minutes the runtime walks
  recent turns, asks the LLM to extract durable facts (strict JSON
  schema, dropping anything ambiguous), and upserts them with
  `source: "auto:<session>:<turn>"` and a starting confidence of
  0.5. Operator-entered facts default to 1.0 so they always
  outrank auto-extracted ones in recall.
- **Tool-use safety**: an always-on blocklist keeps `cloud.*`,
  `runtime.update*`, `assistant.*` (recursion), `review.*`, and
  destructive `self_heal.*` operations off the LLM's table even if
  the operator widens the allow list.

## What push 1 delivers

- SQLite-persisted sessions, turns, and facts (new `review_requests`-
  style migration inside `data/ordo.db`)
- Semantic recall over `assistant_facts` using the shared embedder
  (`HashingEmbedder` by default, `LlamaCppEmbedder` when a model is
  configured â€” same embedder the RAG lane uses, so both share a
  vector space)
- Deterministic router that combines fact recall + bus-driven RAG
  query + history window
- PromptAssembler that stacks persona â†’ facts â†’ RAG â†’ history â†’ user
- AssistantService orchestrating the turn loop (one LLM call per
  turn)
- `assistant.*` capability lane on the bus (Core tier, Eager
  activation â€” the Assistant is part of the platform's identity)
- Control API: `/api/assistant/sessions`, `/api/assistant/turn`,
  `/api/assistant/facts`, `/api/assistant/recall`
- `ordo chat` CLI â€” one-shot and interactive REPL modes
- End-to-end wiremock test covering the three things that matter:
  facts actually land in the prompt, history persists across turns
  in the same session, and a missing credential fails cleanly

## What push 3 adds â€” progressive disclosure

Push 2 shipped autonomous tool use by dumping everything the assistant
*might* need (facts, RAG hits, full tool inventory) into the prompt on
every turn. That works, but it gives the LLM no agency over what it
pulls and bloats the context window. Push 3 flips the relationship.

The prompt is now a thin **bootstrap** that describes a four-level
progressive-disclosure memory architecture and advertises four
meta-tools. Every deeper level is reached only when the LLM asks for
it, and every tool result is prefixed with a short read-only
**preamble** that reinforces how to use that layer â€” so drift stays
down without having to re-send the system prompt.

```
  L0  Bootstrap system prompt            (ordo-assistant::prompt)
         â”‚  Describes the layout + meta-tools. Always sent.
         â–¼
  L1  Persistent fact memory             assistant.recall_memory
         â”‚  Operator-/auto-taught facts, cosine recall, reinforced on use.
         â–¼
  L2  Assistant self-knowledge RAG       assistant.knowledge_lookup
         â”‚  Skills, personas, tool notes, observations. Chunked + embedded
         â”‚  in the new `assistant_knowledge` SQLite table. Scoped by kind
         â”‚  and/or one of the ten domain slots.
         â–¼
  L3  Routing planner                    assistant.plan_routing
         â”‚  LLM-assisted router. Returns {primary_domain, parallel_domains,
         â”‚  rationale, capability_prefixes}. Falls back to the keyword-
         â”‚  based `infer_rag_collections` if the LLM is unreachable.
         â–¼
  L4  Domain RAGs + capabilities         every other allow-listed tool
```

Tokens are intentionally *not* the bottleneck here: the self-knowledge
RAG caches durable guidance (what works, what doesn't, tool notes),
which is only looked up once per topic and then re-used.

### Ten domain slots

`ordo-protocol` reserves ten domain RAG collections. Four are named
(`creative`, `workflow`, `seo`, `cms`); six are placeholder slots
(`domain_slot_5` â€¦ `domain_slot_10`) that operators can name later
without a schema migration. `RAG_DOMAIN_SLOTS` in `ordo-protocol`
enumerates them in canonical order; the LLM router sees the menu and
picks by name.

### New meta-tools

| Capability | Purpose | Preamble |
|---|---|---|
| `assistant.recall_memory` | L1 semantic recall over facts | `MEMORY_PREAMBLE` |
| `assistant.knowledge_lookup` | L2 semantic recall over self-knowledge | `KNOWLEDGE_PREAMBLE` |
| `assistant.plan_routing` | L3 LLM-assisted router | `ROUTING_PREAMBLE` |

They're dispatched **in-process** by `AssistantService::dispatch_tool`
before the bus-backed `ToolGateway` sees the call, so they don't need
a bus round-trip and their results always include the layer preamble.

### New self-knowledge CRUD lane

| Capability | Purpose |
|---|---|
| `assistant.remember_knowledge` | Add a skill card / persona / tool note / observation |
| `assistant.forget_knowledge` | Delete a knowledge entry by id |
| `assistant.list_knowledge` | Inventory, filtered by kind and/or domain |

### New `assistant_knowledge` table

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT (UUID) | PK |
| `kind` | TEXT | `skill`, `persona`, `tool_note`, `observation`, `note` |
| `domain` | TEXT | optional domain slot tag |
| `title` / `body` | TEXT | |
| `source` / `confidence` | TEXT / REAL | same conventions as facts |
| `created_at` / `updated_at` / `reinforced_at` | TEXT (RFC3339) | |
| `embedding` | BLOB | f32 LE vector of `title + body` |

## What push 4 adds â€” seeded knowledge + parallel lookup

Push 3 wired up the progressive-disclosure loop but left L2 empty â€”
`assistant.knowledge_lookup` returned zero hits on first boot until an
operator added content by hand. Push 4 closes that gap.

- **Boot-time seeder** (`ordo-assistant::KnowledgeSeeder`). Runs once
  a few seconds after the bus is up, sweeps the capability inventory,
  and upserts one `Skill` entry per advertised capability â€” title is
  the capability name, body is its description, domain is the first
  segment (`creative.capture_brief` â†’ domain `creative`). Also upserts
  four static `Note` entries for the named domain slots. Everything is
  keyed by `source` (`auto:capability:<name>`, `auto:domain:<slot>`),
  so restarts refresh rows in place rather than duplicating.
- **Idempotent upsert** (`AssistantStore::upsert_knowledge_by_source`
  + `KnowledgeStore::upsert_by_source`). The key is `source`, which
  mirrors the existing fact-store convention.
- **Parallel fan-out** (`assistant.parallel_lookup`). The router
  already emits `parallel_domains`; this new meta-tool takes a query
  plus a domain list and runs `knowledge_lookup` concurrently against
  each, merging the results into `{domain, hits[]}` entries. Saves the
  LLM from hand-orchestrating N sequential tool calls when the router
  says a goal legitimately spans several domains.

The reserved domain slots 5â€“10 are *not* seeded â€” they stay empty so
operators know a slot is available for claiming.

## What push 5 adds â€” optional operator review

When a turn is client-facing or otherwise high-stakes, the caller can
set `review: true` on the `TurnRequest`. The assistant produces its
draft as usual, then â€” before persisting â€” submits it to the existing
`review.*` queue and blocks for up to `review_wait_secs` (default 300)
until the operator approves, edits, denies, or lets it expire.

- **Approve**: the original draft is persisted verbatim.
- **Edit**: the operator's edited text replaces the draft and gets
  persisted; the turn records `state: edited_and_approved`.
- **Deny**: the persisted assistant message is a short
  `[draft denied by operator] â€” <note>` marker so the session
  history shows what happened.
- **Expire / timeout**: the turn fails cleanly with a
  `review did not resolve in time` error; no turn row is written.

Two new `TurnEvent`s stream over `/ws/assistant/:session` so the
studio can reflect state:

- `ReviewRequested { review_request_id, draft }` â€” fires the moment
  the draft is submitted.
- `ReviewResolved { outcome }` â€” fires once the operator decides.

The assistant uses `ReviewService::wait_for(id, wait)` (new in push 5)
to register its waiter on the id returned by `request()`, so there's
exactly one queue entry per turn â€” the event and the wait share the
same request id.

## What push 6 adds â€” ops polish

- **Security-gated assistant** (`ordo-runtime`). The `AssistantProvider`
  now wraps through `SecurityGatedProvider` the same way plugin
  providers do. Every `assistant.*` call flows through the shared
  classifier pipeline and lands in the shared audit log. Scope is
  `"assistant"`.
- **Cancel a turn in flight**. `AssistantService::cancel_turn(id)`
  flips a per-session `CancelFlag`; the turn loop checks it between
  tool iterations and returns `AssistantError::Cancelled`. Propagated
  three ways:
  - `POST /api/assistant/sessions/:id/cancel` REST shortcut.
  - WS client sends `{"action":"cancel"}` or the bare string
    `"cancel"`.
  - Closing the `/ws/assistant/:session` socket.
- **Anthropic tool use**. `ordo_cloud::anthropic::messages` now
  translates OpenAI-style messages into Anthropic blocks on the way
  in (`tool_calls` â†’ `tool_use` content blocks, `role: tool` â†’
  `role: user` with `tool_result`, system messages extracted into the
  top-level `system` field) and normalizes the response back into
  the same `{assistant_message, tool_calls, finish_reason}` shape the
  loop already knows. Schemas translate too (`{type, function:{name,
  description, parameters}}` â†’ `{name, description, input_schema}`).
  The turn loop no longer short-circuits for Anthropic â€” agentic
  tool loops work on both providers.
- **Token-level streaming**. Opt-in via `stream: true` on the
  `TurnRequest`. When tools are off and the credential is OpenAI,
  `ordo_cloud::openai::chat_stream` drives the SSE endpoint and the
  assistant republishes each chunk as `TurnEvent::TokenDelta`. The
  studio concatenates these to render live typing. Falls back to the
  non-streaming path on upstream error so the turn still completes.
- **Studio routing visualiser**. The Assistant tab's side rail now
  has a "Routing plan" panel â€” whenever `assistant.plan_routing`
  returns, the studio pulls out the plan from the tool-call result
  and renders primary/parallel domains as chips plus the rationale
  and candidate capability lanes.

## What's next (push 7 candidates)

- **Anthropic streaming**. Push 6 did Anthropic tool use but kept
  streaming OpenAI-only. Claude's SSE shape is close but distinct;
  wiring it is mostly plumbing plus another normalization pass.
- **Seeded observation notes**. The auto-extractor learns facts today
  but not observations ("that tool call was slow", "that plan
  worked"). Generating `Observation` knowledge entries from turn
  history would grow L2 automatically instead of relying on
  operators to write notes.
- **Confidence decay**. Facts and knowledge entries reinforce on
  use; nothing decays unused entries yet. A nightly sweep would
  pair well with the reinforce-on-recall behavior.
- **Structured tool outputs**. OpenAI and Anthropic both support
  JSON-schema-constrained responses; using them for the router's
  `RoutingPlan` would eliminate the best-effort parser.
- **Multi-operator sessions**. Sessions are single-operator today.
  Adding an `operator_id` to turns and a per-operator fact scope
  would let a team share an assistant without cross-contaminating
  memory.

## Data model

### `assistant_sessions`

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT (UUID) | Primary key |
| `created_at` / `updated_at` | TEXT (RFC3339) | |
| `title` | TEXT | Promoted from the first user turn if not set |
| `turn_count` | INTEGER | Bumped on every persisted turn |

### `assistant_turns`

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT (UUID) | Primary key |
| `session_id` | TEXT | FK to `assistant_sessions.id` |
| `turn_index` | INTEGER | 0-based position within the session |
| `user_message` / `assistant_response` | TEXT | |
| `context_json` | TEXT | `TurnContext` serialised: recalled facts (by id + summary), RAG hit summaries, history window size |
| `model` | TEXT | Whatever the LLM reported |
| `credential_service` | TEXT | Which credential was used |

### `assistant_facts`

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT (UUID) | Primary key |
| `subject` | TEXT | `user`, `client:Acme`, `brand`, `project:<slug>`, â€¦ |
| `predicate` | TEXT | `prefers`, `avoids`, `location`, `role`, â€¦ |
| `object` | TEXT | Free-form fact content |
| `source` | TEXT | `operator`, `auto:<session>`, `import` |
| `confidence` | REAL | 0.0 â€“ 1.0 |
| `created_at` / `reinforced_at` | TEXT (RFC3339) | Reinforced on recall |
| `embedding` | BLOB | Little-endian f32 vector of `subject + predicate + object` |

## Calling it

### From an agent / plugin / curl

```bash
# New session + first turn in one call
curl -s -X POST http://127.0.0.1:4141/api/assistant/turn \
  -H 'content-type: application/json' \
  -d '{"user_message":"what should our spring campaign sound like?"}'

# Same session, later turn
curl -s -X POST http://127.0.0.1:4141/api/assistant/turn \
  -H 'content-type: application/json' \
  -d '{"session_id":"â€¦","user_message":"draft three lines"}'

# Teach the assistant something durable
curl -s -X POST http://127.0.0.1:4141/api/assistant/facts \
  -H 'content-type: application/json' \
  -d '{
        "subject": "brand",
        "predicate": "avoids",
        "object": "marketing clichÃ©s and exclamation points",
        "confidence": 0.9
      }'

# Inspect what the assistant remembers about you
curl -s "http://127.0.0.1:4141/api/assistant/facts?subject=brand"

# Pure recall, no LLM call â€” useful for debugging routing
curl -s -X POST http://127.0.0.1:4141/api/assistant/recall \
  -H 'content-type: application/json' \
  -d '{"query":"brand voice","top_k":5}'
```

### From the CLI

```bash
# One-shot
ordo chat "draft a spring colorway launch post"

# Interactive
ordo chat
you> what should the hero line say?
assistant> â€¦

# Continue an existing session
ordo chat --session <id> "follow up"

# Disable retrieval (useful for debugging)
ordo chat --no-rag --no-memory "raw test prompt"
```

### From the bus (agent-to-assistant)

Any capability provider can call the Assistant via the capability
bus, same as any other tool. Useful when a workflow wants to "ask
the assistant to decide something" without rolling its own LLM call.

```
brain.invoke_tool("assistant.turn", {
  "session_id": "<optional>",
  "user_message": "should this copy be approved for client Acme?",
})
```

## The assistant.* capability lane

| Capability | Purpose |
|---|---|
| `assistant.turn` | Process a turn; returns the reply + retrieval context |
| `assistant.new_session` | Create an empty session (optional `title`) |
| `assistant.list_sessions` | Recent sessions |
| `assistant.get_session` | Full transcript for a given session id |
| `assistant.remember_fact` | Upsert a fact |
| `assistant.forget_fact` | Delete a fact by id |
| `assistant.list_facts` | Inventory, optionally filtered by subject |
| `assistant.recall` | Semantic recall without calling the LLM |
| `assistant.recall_memory` | L1 meta-tool: memory recall with layer preamble |
| `assistant.knowledge_lookup` | L2 meta-tool: self-knowledge recall |
| `assistant.plan_routing` | L3 meta-tool: LLM-assisted domain router |
| `assistant.parallel_lookup` | L2 fan-out: knowledge_lookup concurrently across multiple domains |
| `assistant.remember_knowledge` | Add a self-knowledge entry |
| `assistant.forget_knowledge` | Delete a self-knowledge entry |
| `assistant.list_knowledge` | List self-knowledge entries |

All registered as `CapabilityTier::Core` / `CapabilityActivation::Eager`
â€” the Assistant is the platform, not an optional add-on.

## Control API surface

| Method | Route | Body |
|---|---|---|
| `GET` | `/api/assistant/sessions?limit=N` | â€” |
| `POST` | `/api/assistant/sessions` | `{title?}` |
| `GET` | `/api/assistant/sessions/:id` | â€” |
| `POST` | `/api/assistant/sessions/:id/cancel` | â€” (push 6) |
| `POST` | `/api/assistant/turn` | `TurnRequest` (see below) |
| `GET` | `/api/assistant/facts?subject=â€¦` | â€” |
| `POST` | `/api/assistant/facts` | `NewFact` |
| `DELETE` | `/api/assistant/facts/:id` | â€” |
| `POST` | `/api/assistant/recall` | `{query, top_k}` |

### `TurnRequest`

```json
{
  "session_id": "<uuid, optional â€” new session if omitted>",
  "user_message": "text",
  "credential": "<optional override; default 'openai'>",
  "use_rag": true,
  "use_memory": true,
  "use_tools": true,
  "review": false,
  "review_wait_secs": 300,
  "stream": false,
  "history_window": 6,
  "fact_top_k": 8,
  "rag_top_k": 3,
  "metadata": {}
}
```

### `TurnResult`

```json
{
  "session_id": "â€¦",
  "turn": {
    "id": "â€¦", "index": 2, "created_at": "â€¦",
    "user_message": "â€¦", "assistant_response": "â€¦",
    "context": {
      "facts": [ { "fact": { â€¦ }, "score": 0.87 } ],
      "rag_hits": [ { "collection": "main", "title": "â€¦", "score": 0.81, â€¦ } ],
      "history_window": 6
    },
    "model": "gpt-4o-mini", "credential_service": "openai"
  },
  "retrieved_facts": [ â€¦ ],   // same as turn.context.facts, for convenience
  "retrieved_rag": [ â€¦ ]      // same as turn.context.rag_hits
}
```

## How the prompt gets built

1. **System persona** â€” fixed lead-in that tells the model it's the
   Ordo Assistant and asks it to be concise, grounded, and
   honest about what it doesn't know.
2. **System facts** â€” only present when the fact store has relevant
   hits. Rendered as a bullet list `(subject) predicate object`.
3. **System RAG snippets** â€” only present when the bus-driven RAG
   query returned hits. Numbered, with collection + doc id + chunk
   index + score so the model can cite in plain language.
4. **History** â€” alternating user / assistant turns from the last
   `history_window` (default 6) turns of the session.
5. **User message** â€” the new turn.

Recalled facts are *reinforced* every time they get used: their
`reinforced_at` bumps and their `confidence` floats up to a ceiling
of 1.0. Unused facts drift, which gives the roadmap's confidence-
decay feature something to work with.

## Extending it: authoring new facts

Anything can write to the fact store:
- The operator, via `assistant.remember_fact` or `POST /api/assistant/facts`
- A background process, by calling the capability from the bus
- Another capability provider (for example `creative.capture_brief`
  could, once push 2 lands, extract brand-voice signals from each
  captured brief and upsert them as facts)

The recommended subject taxonomy:
- `user` â€” the operator themselves
- `brand` â€” brand-wide preferences
- `client:<name>` â€” a single client
- `project:<slug>` â€” a specific project
- `voice:<name>` â€” a named voice profile (e.g. `voice:formal`)

These aren't enforced â€” the schema is intentionally subject/predicate/
object â€” but consistent subjects make recall noticeably sharper.
