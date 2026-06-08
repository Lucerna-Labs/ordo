# Review â€” human in the loop

Ordo doesn't ship content blindly. Every capability that
produces something the operator might want to sign off on can route
its output through a **review queue**: the draft lands in a preview,
you approve, deny, or edit it in place, and the agent only sees the
version you signed off on.

## Data flow

```
  agent / plugin tool call
        â”‚   creative.generate_copy { review: true, â€¦ }
        â–¼
  â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
  â”‚ ReviewService                          â”‚
  â”‚   â€¢ persist request â†’ SQLite           â”‚â”€â”€â”€â–º data/ordo.db (review_requests)
  â”‚   â€¢ broadcast event to WebSocket subs  â”‚
  â”‚   â€¢ await oneshot decision             â”‚
  â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
        â”‚
        â”‚  studio sees `opened` on /ws/review
        â–¼
  â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
  â”‚  Studio Review tab                     â”‚
  â”‚    preview (markdown / html / json /   â”‚
  â”‚             image / plain)             â”‚
  â”‚    approve / deny / edit buttons       â”‚
  â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
        â”‚   POST /api/review/:id/{approve,deny,edit}
        â–¼
  â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
  â”‚ ReviewService.decide                   â”‚
  â”‚   â€¢ mark SQLite row terminal           â”‚
  â”‚   â€¢ wake the waiting agent via oneshot â”‚
  â”‚   â€¢ broadcast `resolved` to WS subs    â”‚
  â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
        â”‚
        â–¼
  agent receives approved (possibly edited) content
```

## Calling it from an agent

Any capability that takes a free-form `review` flag will queue its
output before returning. Today that's every LLM-backed creative-llm
capability:

```bash
curl -s -X POST http://127.0.0.1:4141/api/tools/creative.generate_copy \
  -H 'content-type: application/json' \
  -d '{
        "prompt": "spring trail colorway launch post",
        "review": true,
        "review_title": "Spring colorway post (draft 1)"
      }'
```

The call blocks until the operator acts. When they approve, the
response contains the final text plus a `review` block:

```json
{
  "model": "gpt-4o-mini",
  "assistant_message": "Polished final copy after operator edits",
  "review": {
    "state": "edited_and_approved",
    "id": "5f2e6b94-â€¦",
    "edited": true,
    "note": "tightened the second paragraph"
  }
}
```

When the operator denies, the call returns a `Failed` result whose
error message contains the review id, so the agent can cite it when
it retries or falls back:

```
operator denied review 5f2e6b94-â€¦  (creative.generate_copy): off-brand
```

## Review: true is optional

Leaving `review` out (or setting it to `false`) keeps the call
synchronous and unmediated â€” same behaviour as before this lane
existed. Review is an opt-in gate, so agents that don't need
supervision keep working.

Wait-budget defaults to 5 minutes. Override per-call with
`wait_seconds` on `review.request_approval`, or globally with
`CreativeLlmProvider::with_review_wait(â€¦)` at boot.

## REST surface

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/review/pending` | Every request still waiting for action |
| GET | `/api/review/recent?limit=N` | Most recent requests (pending + resolved) |
| GET | `/api/review/:id` | One request, full content included |
| POST | `/api/review/:id/approve` | `{ "note": "optional" }` |
| POST | `/api/review/:id/deny` | `{ "note": "optional" }` |
| POST | `/api/review/:id/edit` | `{ "content": "new text", "note": "optional" }` |

All four decision routes are idempotent-safe â€” a second attempt on a
resolved request returns 400 with a clear `already resolved` error.

## WebSocket surface

`GET ws://127.0.0.1:4141/ws/review` subscribes to the event stream.

On connect, the server sends a **queue_snapshot** so the client
doesn't have to poll:

```json
{ "event": "queue_snapshot", "pending": [...], "total": 3 }
```

Every state transition then pushes one message:

```json
{ "event": "opened",   "request": { "id": "â€¦", "title": "â€¦", "content": "â€¦", "content_type": "text/markdown", â€¦ } }
{ "event": "resolved", "request": { "id": "â€¦", "state": "approved", â€¦ } }
```

If a slow subscriber falls behind the 256-event ring, the server
sends one `{ "event": "lagged", "skipped": N }` marker followed by a
fresh `queue_snapshot` so the client can resync.

Actions (approve / deny / edit) go over REST, not the WebSocket, so
there is exactly one auditable mutation path.

## Bus capability surface

The same functionality is available on the capability bus for
agents and other in-process callers:

| Capability | Purpose |
|---|---|
| `review.request_approval` | Queue a draft. Pass `async: true` or `wait_seconds: 0` to fire-and-forget. |
| `review.list_pending` | Inventory of open requests. |
| `review.approve`, `review.deny`, `review.edit` | Operator-side actions, by id. |

## Content types

The studio switches preview renderers on `content_type`:

| Type | Renderer |
|---|---|
| `text/markdown` | Minimal in-bundle markdown-to-HTML |
| `text/html` | Sandboxed iframe (no scripts, no same-origin) |
| `application/json` | Pretty-printed monospace pre |
| `text/plain` | Monospace pre |
| `image/png` / `image/jpeg` / `image/svg+xml` | `<img>` (accepts raw base64 or a full `data:` URL) |

Anything else falls back to the plaintext renderer.

## Persistence

Review state lives in the shared `data/ordo.db` under
`review_requests`:

| Column | Type |
|---|---|
| `id` | TEXT (UUID) primary key |
| `created_at` / `resolved_at` | RFC3339 strings |
| `origin_capability` / `origin_plugin` | who produced the artifact |
| `title` / `content_type` / `content` | the artifact |
| `metadata_json` | caller-supplied metadata |
| `state` | `open` / `approved` / `edited_and_approved` / `denied` / `expired` |
| `edited_content` | operator's rewrite, if any |
| `decision_note` | operator's free-form note |

A pending request survives a runtime restart. When the studio
re-subscribes to `/ws/review` after the runtime is back, the initial
`queue_snapshot` shows every request still in `open` state. Agents
that were waiting for a decision at crash time don't re-attach
automatically â€” they see the restart as a timeout â€” but the artifact
is still in the queue and the operator can still act on it.

## Expiry

If no decision lands within `wait_seconds` of `request_and_wait`,
the server marks the request `expired` and returns an error to the
caller. Expired requests show up in the recent-list but not the
pending-list.

## Operator CLI

```bash
# Inventory
curl -s http://127.0.0.1:4141/api/review/pending | jq

# Approve
curl -s -X POST http://127.0.0.1:4141/api/review/<id>/approve \
  -H 'content-type: application/json' \
  -d '{"note":"ship it"}'

# Edit + approve in one call
curl -s -X POST http://127.0.0.1:4141/api/review/<id>/edit \
  -H 'content-type: application/json' \
  -d '{"content":"â€¦cleaned copyâ€¦","note":"tightened opener"}'
```

## Extending it to new capabilities

Any capability provider that wants optional review treatment follows
the same pattern `CreativeLlmProvider` uses:

1. Accept a `review: bool` flag on the arguments.
2. After the underlying work produces a draft, call
   `ReviewService::request_and_wait` with the draft as the `content`
   and an appropriate `content_type`.
3. On `Approved` / `EditedAndApproved`, substitute
   `resolved.effective_content()` back into the output so downstream
   agents see the approved version.
4. On `Denied`, return `ToolCallResult::Failed` with the operator's
   note.
5. On `Expired`, fail the call â€” that signals the agent to either
   retry or take a fallback path.

The review machinery is a single `Arc<ReviewService>` on the
runtime; nothing new needs to be registered or discovered.
