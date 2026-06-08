# Control API

This document describes the local control API that Ordo exposes for a
future UI and operator tooling.

## Purpose

The control API is not a heavy central gateway. It is a thin local operator
surface that talks to the existing bus and capability system.

Its job is to give a UI or local automation a stable way to:
- read runtime profile and storage state
- update persisted runtime settings
- inspect capabilities
- manage pinned memory
- manage working-memory notes
- manage build ledgers for the native Ordo build spine
- inspect remembered self-heal cases
- promote remembered self-heal fixes into pinned memory
- forget stale remembered self-heal fixes
- configure optional local self-heal model settings

It now also serves a built-in dashboard at:
- `GET /`

That dashboard stays intentionally thin. It uses the same local API that future
automation or a separate frontend would use instead of reading SQLite directly
or inventing a second control path.

## Default binding

By default, the runtime binds the control API to:
- `127.0.0.1:4141`

This can be changed with:
- `ORDO_CONTROL_API_BIND`

Set the variable to an empty string to disable the control API.

In containers, the recommended bind is:
- `0.0.0.0:4141`

## Endpoints

### Dashboard
- `GET /`

Serves the built-in operator dashboard for:
- runtime profile and budget inspection
- persisted settings updates
- capability inventory display
- live RAG collection inventory display
- RAG routing preview with inferred or manually forced collections
- pinned-memory management
- working-memory note management
- remembered self-heal case review
- remembered self-heal fix promotion into pinned memory
- remembered self-heal fix replay through the live repair lane
- remembered self-heal fix export into an operator-friendly repair pack
- self-heal model configuration
- native build-spine ledger inspection and gate submission

### Health
- `GET /health`

Returns a simple status payload.

### Capability inventory
- `GET /api/capabilities`

Returns the live capability descriptors so a UI can discover what the runtime
currently supports.

### RAG collections
- `GET /api/rag/collections`

Returns the live retrieval collection inventory with:
- collection name and label
- collection group
- document count
- chunk count
- a small sample of indexed document titles

If retrieval is disabled in the current profile, the endpoint still responds so
the dashboard can stay up and explain that the RAG lane is unavailable.

### RAG preview
- `GET /api/rag/preview?query=...`
- `GET /api/rag/preview?query=...&collections=main,seo`

Runs a lightweight retrieval preview for operator inspection. If `collections`
is omitted, the API uses the same inferred collection routing the runtime uses
for normal goal preparation. If `collections` is supplied, the preview stays
strictly inside those named retrieval lanes.

### Runtime profile
- `GET /api/runtime/profile`

Returns the effective runtime profile and activation state for optional lanes.

### Runtime storage
- `GET /api/runtime/storage`

Returns the effective storage budgets for:
- RAG
- working memory
- pinned memory
- self-heal history

### Runtime settings
- `GET /api/runtime/settings`
- `POST /api/runtime/settings`

Returns or updates persisted runtime settings stored in SQLite. Updates apply on
the next restart. Explicit environment variables still override persisted values.

The persisted settings surface now includes:
- runtime profile
- storage budgets
- optional self-heal `llama.cpp` binary path
- optional self-heal model path
- self-heal context size
- self-heal max tokens
- self-heal temperature

### Pinned memory
- `GET /api/memory/pinned?limit=10`
- `POST /api/memory/pinned`
- `DELETE /api/memory/pinned`

Lists recent pinned memories, pins a new memory, or removes a pinned memory
from the always-available memory lane.

### Working memory
- `GET /api/memory/working?limit=10`
- `POST /api/memory/working`

Lists or stores normal working-memory notes.

### Build spine
- `GET /api/builds`
- `POST /api/builds`
- `GET /api/builds/:id`
- `POST /api/builds/:id/gate`

Lists durable build ledgers, starts a new native build ledger, reads one
ledger, or records an explicit gate result for the current build step.

Builds advance only when the gate result is a real pass for the ledger's
current step. A model saying that work is complete is not enough. Failed
results halt the build unless the ledger explicitly allows a bounded
autonomous retry. Deferred results are only valid for crate coupling debt.

### Self-heal history
- `GET /api/self-heal/cases?limit=10`
- `DELETE /api/self-heal/cases`
- `POST /api/self-heal/cases/pin`
- `POST /api/self-heal/cases/replay`
- `POST /api/self-heal/cases/export`

Lists recent remembered self-heal cases, forgets one by fingerprint when an
operator wants to remove stale repair memory, or promotes a remembered case into
the pinned always-available memory lane.

Replay pushes a remembered fingerprint back through the live self-heal lane so
the runtime can reuse the same repair path it would use for a fresh incident.

Export renders a remembered fix as both:
- structured JSON for tools
- markdown for operators, pinned memory, or future preloaded repair packs

## Design notes

- The control API is a local convenience surface, not a replacement for the bus.
- Endpoint handlers call into the existing brain and capability system instead
  of bypassing orchestration.
- The RAG operator surface now uses the same live retrieval peer that normal
  goal preparation uses instead of maintaining a parallel index browser.
- The built-in dashboard also stays on top of those same endpoints, so UI work
  exercises the same contracts external automation would use.
- Build ledgers live under the runtime user-files area and are exposed through
  the same local control surface as automation, not through ad hoc files.
- SQLite-backed runtime settings and self-heal history now sit behind dedicated
  storage workers instead of being opened directly on the async path.
- Pinned memory remains protected by its own storage budget.
- Runtime settings remain local-first and live in the same SQLite databank.
- Self-heal history stays user-manageable through official endpoints instead of
  requiring direct database edits.
- Promoting a remembered fix should go through the official memory lane instead
  of inventing a side cache for important repairs.
- Replaying or exporting a remembered fix should still go through official
  capability paths so the operator surface does not become a parallel control
  plane.

## Why this exists

Users should not need to hand-edit environment variables or guess internal
store layout just to change storage budgets, review pinned memory, inspect
remembered repairs, or switch self-heal model settings. The control API gives
the future UI a real place to attach.
