# Fixbook

This file records concrete problems we hit while building Codex Claw and the
repair that actually stabilized the platform.

## Fix Pattern 1: Keep One Canonical Repo

### Symptom
- Progress felt lost because work was spread across duplicate folders and
  inspection copies.

### Repair
- Keep one canonical repo only: `codex-ordo-project`.
- Push meaningful milestones to GitHub instead of leaving them local for too
  long.
- Record finished slices in `docs/dones.md`.

### Why it worked
- The project stopped depending on human memory and folder spelunking.

## Fix Pattern 2: Prefer Explicit Contracts Over Hidden Runtime Coupling

### Symptom
- Demo flows worked, but execution logic was too implicit and too tied to
  provider-specific behavior.

### Repair
- Add explicit protocol messages, run lifecycle events, and planner-produced
  execution plans.

### Why it worked
- Execution became observable, testable, and reusable across providers.

## Fix Pattern 3: Avoid Rebuilding the Gateway Indirectly

### Symptom
- A no-gateway goal can quietly collapse into a hidden gateway if routing,
  transport, handshake, and orchestration are not clearly separated.

### Repair
- Keep separate crates for classification, discovery, transport, handshake,
  router, provider execution, and orchestration.

### Why it worked
- Each layer keeps one job, and later P2P or relay work can slot in without a
  new control-plane monolith.

## Fix Pattern 4: Use SQLite Directly Before Reaching for a Heavier Databank

### Symptom
- JSONL storage stopped being enough once richer retrieval metadata, budgets,
  and recurring repair memory were added.
- `tokio-rusqlite` was not attractive because its `rusqlite` line lagged behind
  the versions we wanted.

### Repair
- Use `rusqlite` directly in a shared `ordo-store` crate.
- Keep migrations append-only and let multiple subsystems share the same local
  database file.

### Why it worked
- The runtime got a simple, embedded local databank without delaying progress on
  transport, RAG, and self-heal.

## Fix Pattern 5: Keep Migrations Append-Only

### Symptom
- Schema iteration becomes dangerous if existing migrations are rewritten after
  the database has already been created.

### Repair
- Only add new migrations to the end of the list.
- Treat the migration stream as permanent history once it has shipped locally.

### Why it worked
- Existing local databases remain upgradable instead of silently diverging.

## Fix Pattern 6: Keep User Files Out of Runtime State Paths

### Symptom
- Without a rooted filesystem provider, file reads and writes can bleed into the
  runtime state area or escape the intended path.

### Repair
- Root filesystem access under `user-files`.
- Normalize paths and reject escapes above the configured root.
- Mount user files and runtime data on separate container volumes.

### Why it worked
- The platform became safer to operate and easier to containerize.

## Fix Pattern 7: Protect Important Memory From Normal Pruning

### Symptom
- Important platform knowledge can be lost if all memory is treated the same.

### Repair
- Add separate budgets for:
  - RAG storage
  - working memory
  - pinned memory
  - self-heal history
- Keep official platform truths in pinned memory.

### Why it worked
- The platform can retain core truths even while normal working memory churns.

## Fix Pattern 8: New Protocol Variants Must Update Every Match Table

### Symptom
- Adding new `OrdoMessage` variants caused compile failures in classifier and
  router match expressions.

### Repair
- Treat protocol changes as cross-cutting changes.
- Update classifiers, routers, logs, and memory archiving whenever a new message
  type is added.

### Why it worked
- The compiler becomes a safety net for protocol drift instead of a surprise at
  the end.

## Fix Pattern 9: Never Carry SQLite Handles Across Async Awaits

### Symptom
- The self-heal peer failed to spawn because the SQLite connection made the
  future non-`Send` when a live store reference crossed an `await`.

### Repair
- Read the necessary store state first.
- Drop the store borrow before awaiting a model call.
- Then write the result back after the plan is ready.

### Why it worked
- The peer became safe to run under the Tokio task model.

## Fix Pattern 10: Self-Heal Must Work Even Without a Local Model

### Symptom
- A repair subsystem that depends on a configured local model can fail exactly
  when a user most needs help.

### Repair
- Make `llama.cpp` optional.
- Keep a deterministic fallback planner that produces repair actions anyway.

### Why it worked
- Installation and recovery stay available even before model configuration.

## Fix Pattern 11: Repeated Incidents Should Reuse Previous Fixes

### Symptom
- Re-solving the same platform issue from scratch wastes time and creates
  inconsistent repair behavior.

### Repair
- Normalize incidents into stable fingerprints.
- Persist the successful repair plan and reason.
- Reuse the stored repair when the same fingerprint appears again.

### Why it worked
- The second occurrence becomes faster, calmer, and more consistent than the
  first.

## Fix Pattern 12: Capability Growth Needs Activation Rules

### Symptom
- As the platform grows, a flat list of capabilities can tempt the runtime into
  booting too much by default.

### Repair
- Mark capabilities as core, optional, or heavy.
- Mark activation as eager or lazy.
- Use runtime profiles so a lean install can stay lean.
- Put retrieval behind a lazy runtime lane so the default profile does not pay
  the indexing/query peer cost before retrieval is used.
- Store desired runtime profile and storage budgets in SQLite so a future UI has
  a real control plane instead of only environment variables.
- Add explicit pinned-memory capabilities so important user memories are not
  trapped behind internal-only storage logic.
- Add a thin local HTTP layer over the existing tool/capability surface so a
  future UI can integrate without introducing a new orchestration model.

### Why it worked
- Capability count stops being the same thing as hardware cost.
- The default install stays lighter without hiding retrieval from planning or
  introspection.
- Budget and profile controls now have a stable persisted home that the UI can
  manage later.
- The always-available memory lane can now be managed explicitly instead of
  relying only on startup seeding.
- The UI now has a natural backend seam that reuses the runtime's existing
  contracts instead of splitting the architecture.

## Fix Pattern 13: Put User Controls On Top Of Official Endpoints

### Symptom
- Budget and memory-lane controls existed in code, but normal users still had to
  think like API clients or developers to use them.
- Showing only pinned memory through the operator surface hid half of the memory
  model.

### Repair
- Serve a built-in dashboard from the same local control API bind.
- Make the dashboard call the same official endpoints that automation and future
  external UIs will use.
- Add working-memory list support so the UI can show both the working lane and
  the always-available pinned lane.
- Add an official unpin path so pinned memory can be curated instead of only
  growing.

### Why it worked
- Users get a real control surface immediately without inventing a second
  architecture.
- The UI becomes a live test of the official control contracts instead of a
  special-case path.
- The memory model becomes easier to understand because both lanes are visible
  side by side.
- Important pinned context stays editable instead of ossifying into stale
  always-on clutter.

## Fix Pattern 14: Make Self-Heal Inspectable And Configurable

### Symptom
- The repair lane could remember recurring fixes, but operators could not
  easily inspect or curate that remembered state.
- Local self-heal model configuration depended too much on environment
  variables.

### Repair
- Persist optional self-heal model settings in the same runtime settings store
  as profiles and storage budgets.
- Expose remembered self-heal cases through official capabilities and the local
  control API.
- Add an official forget path so stale remembered fixes can be removed.

### Why it worked
- The maintenance lane stops feeling magical or opaque.
- Users can see what the platform thinks it knows about past incidents.
- Repair-model configuration becomes manageable from the same operator surface
  as the rest of the runtime.

## Fix Pattern 15: Promote Proven Repairs Through The Memory Lane

### Symptom
- A remembered repair can be valuable enough to stay always available, but
  self-heal history alone is not the same thing as pinned memory.
- The first promotion pass surfaced a hidden quality problem: repeated reuse had
  been recursively growing the stored `why` text.

### Repair
- Add a `self_heal.pin_case` capability and a matching control API endpoint.
- Promote remembered repairs through the existing pinned-memory lane instead of
  inventing a separate important-fix store.
- Normalize memory-reuse rationale so repeated incidents do not bloat promoted
  repair notes.

### Why it worked
- Important repairs now live in the same always-available context model as
  other pinned truths.
- The dashboard can promote proven fixes without bypassing the architecture.
- Promotion became a quality check that exposed and fixed noisy retained repair
  text.

## Fix Pattern 16: Put SQLite Work Behind Dedicated Storage Workers

### Symptom
- Memory, self-heal, RAG, and runtime settings all persisted locally, but
  operator-facing providers could still open SQLite directly on the main async
  path.
- That split the persistence story between worker-backed peers and direct
  provider access.

### Repair
- Add a generic storage-worker abstraction in `ordo-store`.
- Route runtime settings through `RuntimeSettingsTask`.
- Share `SelfHealStorageTask` between the self-heal peer and self-heal tools.
- Keep memory and RAG peers on worker-backed storage tasks too.

### Why it worked
- SQLite-heavy work now has a stable execution lane that does not depend on the
  main async scheduler staying unblocked.
- The persistence story became consistent across peers and operator surfaces.
- Runtime/control features can reuse the same state without inventing direct DB
  shortcuts.

## Fix Pattern 17: Make Remembered Repairs Operational, Not Just Visible

### Symptom
- Operators could inspect, forget, and pin remembered self-heal cases, but they
  still could not re-run a known-good fix or export it as a reusable repair
  pack.

### Repair
- Add `self_heal.replay_case` to push a remembered fingerprint back through the
  live self-heal request/response flow.
- Add `self_heal.export_case` to render a remembered repair as both markdown
  and structured JSON.
- Surface both actions through the control API and dashboard.

### Why it worked
- Replay reuses the same repair contracts the runtime already trusts instead of
  inventing a second maintenance path.
- Export gives the platform and the operator a stable repair artifact that can
  be pinned, shared, or preloaded.
- Remembered fixes become active operational tools instead of passive history.

## Fix Pattern 18: Make One Transport Kind Real Before Chasing Them All

### Symptom
- The transport architecture had good seams, but every delivery path was still
  simulated, which made it harder to prove the router/session model against
  actual sockets.

### Repair
- Add framed envelope helpers for network transport.
- Implement a real TCP adapter behind `TransportKind::TcpNoise`.
- Keep QUIC and relay on the simulated fallback until their real adapters
  exist.
- Demonstrate the real path in both unit tests and the CLI.

### Why it worked
- The platform now has a concrete, verifiable network delivery path without
  having to finish every transport at once.
- We can validate message framing and peer endpoint handling separately from
  future QUIC and relay work.
- The docs stay honest about scope instead of turning one real adapter into an
  implied claim that all transport kinds are done.

## Fix Pattern 19: Make Direct QUIC Honest, Local, And Deterministic First

### Symptom
- QUIC was the next obvious transport target, but a full secure peer-identity
  path plus relay support would have been too much for one slice.
- The first direct QUIC attempt also hit two real implementation problems:
  rustls provider setup and racey stream shutdown.

### Repair
- Add a real direct QUIC path only for explicit
  `quic+insecure://host:port` endpoints.
- Install the rustls ring crypto provider explicitly before constructing QUIC
  configs.
- Reuse the existing framed-envelope codec over QUIC.
- Switch local QUIC demo/test traffic to a request/ack exchange so delivery is
  confirmed before teardown.
- Keep symbolic secure QUIC and relay on the simulated fallback.

### Why it worked
- The platform gained a second honest real transport path without pretending
  peer identity pinning or relay are already complete.
- QUIC delivery became deterministic enough for tests and the operator demo.
- The architecture stays truthful: local/dev QUIC is real, relay is not yet.

## Fix Pattern 20: Catch Every Shutdown Signal, Not Just Ctrl+C

(Full postmortem: `docs/incidents/2026-06-07-runtime-exit-minus1-runaway-credentials.md`.)

### Symptom
- `ordo serve` terminated with exit code `0xFFFFFFFF` (−1) and no panic text,
  leaving a 3.6 MB uncheckpointed `ordo.db-wal`.
- `run_serve` awaited only `tokio::signal::ctrl_c()`, which on Windows catches
  `CTRL_C`/`CTRL_BREAK` but NOT `CTRL_CLOSE`/`CTRL_LOGOFF`/`CTRL_SHUTDOWN`, so
  closing the minimized launcher console hard-killed the runtime.

### Repair
- Wait on a `wait_for_shutdown_signal()` that covers all five Windows console
  events (and `SIGINT`/`SIGTERM` on Unix) in `ordo-cli/src/main.rs`.
- On shutdown, fold the WAL with `ordo_store::checkpoint_wal` (a fresh
  connection running `PRAGMA wal_checkpoint(TRUNCATE)`), retrying briefly while
  the detached `StorageTask` threads finish closing their connections.

### Why it worked
- A console close now runs the same graceful path as Ctrl+C instead of an
  exit `−1`, and the WAL is folded deterministically rather than orphaned.
- WAL mode already kept committed data safe across a kill; this makes the exit
  clean and observable. The guaranteed fix for console-close (don't run in a
  closeable console) landed in Fix Pattern 22.

## Fix Pattern 22: Run The Runtime Detached, Not In A Closeable Console

(Full postmortem: same incident doc as Fix Pattern 20.)

### Symptom
- Even with graceful signal handling (Fix Pattern 20), the runtime still ran
  inside a *minimized* console window via `Start-Process cargo run -- serve`;
  closing that window delivered `CTRL_CLOSE` and the OS only grants ~5s before
  it force-terminates, so a clean shutdown was not guaranteed.

### Repair
- In `Launch-Ordo-Portable.ps1`: build the runtime as a separate foreground
  step (`cargo build --bin ordo`), then launch the built `target\debug\ordo.exe
  serve` in its OWN HIDDEN console (`Start-Process -WindowStyle Hidden`) — no
  visible/closeable window, decoupled from the launcher's shell.
- Enable a WER LocalDump for `ordo.exe` (`HKCU\…\Windows Error Reporting\
  LocalDumps\ordo.exe`, `DumpType=2`) so the next termination is provable: a
  native crash leaves a dump, an external kill leaves none.

### Why it worked
- There is no longer a window for a user to close, and the runtime's console is
  separate from the launcher's, so closing the launcher cannot orphan-kill it
  (verified: the hidden/detached runtime survives its spawning shell's exit and
  reaches `/health`). Logoff/shutdown are still handled gracefully by Fix
  Pattern 20. The dump folder makes "native crash vs external kill" decidable.

## Fix Pattern 21: Cap And Surface Runaway Tool Calls At The Brain

(Full postmortem: same incident doc as Fix Pattern 20.)

### Symptom
- An external client drove `cloud.credentials.list` 62× via
  `GET /api/cloud/credentials`; `Brain::invoke_tool` (the control-API tool path)
  had no rate cap, no dedup, and no loop-break — only a 300s per-call timeout.
- The bounded turn loop (`ordo-assistant`) is a different, unused path, so it
  offered no protection here.

### Repair
- Add a `Mutex<ToolCallGuard>` consulted at the top of `invoke_tool`
  (`ordo-brain/src/lib.rs`): a generous per-capability sliding-window rate cap
  that rejects fast runaways before any bus traffic, plus a warn-only
  consecutive-identical detector that makes a slow runaway visible in the logs.
- Never cache tool results — caching would corrupt non-idempotent tools like
  `assistant.new_session`. Hold the lock only for synchronous bookkeeping, never
  across an `.await` (see Fix Pattern 9).

### Why it worked
- A misbehaving client can no longer drive unbounded native + bus work through
  the HTTP tool path, and the next runaway is obvious in real time instead of
  needing reconstruction from raw logs. The client-side fixation that triggers
  such loops remains a P2 follow-up in the UI agent.

## Fix Pattern 23: Resolve Credentials The Same Way On Every Path

### Symptom
- The assistant chatted fine but "never remembered" anything across sessions.
- The fact auto-extractor (the only writer of durable facts/notes/preferences)
  failed every turn with `no cloud credential configured: openai`, because it
  did a single exact-name lookup for `default_service` ("openai") while the
  operator only had an `ollama-cloud-api` credential.

### Repair
- Give the extractor the SAME credential resolution the chat/speech paths use
  (`ordo-assistant/src/extractor.rs::resolve_credential`): operator default
  (`get_default()`) → `default_service` → any listed credential, via a pure,
  unit-tested `candidate_service_names()` helper.

### Why it worked
- The asymmetry was the whole bug: chat had failover, extraction didn't. Once
  the extractor resolves like chat, it uses whatever credential the operator
  actually configured — so facts start being written and recall has something to
  return. Lesson: credential resolution must be one shared behavior, not
  re-implemented per call site.

## Fix Pattern 24: Match The Embedder To The Operator's Inference Stack

### Symptom
- Memory/RAG recall was semantically weak: the embedder was the non-neural
  `hashing` fallback (synonyms/paraphrases scored ~0).
- The only neural option (`LlamaCppEmbedder`) shells out to a standalone
  `llama-embedding` CLI on a loose GGUF and reloads the model EVERY call — a poor
  fit for an Ollama-centric stack where embed models live as Ollama blobs.

### Repair
- Add an `OllamaEmbedder` (`ordo-models`) — a sync `EmbeddingClient` that calls
  the running Ollama server's `/api/embed` via `ureq` (pure-sync, so it doesn't
  panic on a Tokio worker like `reqwest::blocking` would), with a tolerant parser
  for Ollama's three response shapes. Wire it into `build_embedding_client`
  (precedence llama.cpp → ollama → hashing) behind `ORDO_EMBEDDING_OLLAMA_MODEL`.

### Why it worked
- It uses what the operator already runs (nomic-embed-text, 768d), keeps the
  model warm (no per-call reload), stays local-first, and replaces the weak
  hashing floor. Verified live: the runtime reports `embedding_backend: ollama`.
