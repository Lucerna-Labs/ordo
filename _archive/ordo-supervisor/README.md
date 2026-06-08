# ordo-supervisor — ARCHIVED (orphaned, never wired into the runtime)

> ⚠️ **This crate is not part of the build.** It is parked here for possible
> later revival. It is **not** a `[workspace]` member (it is listed under
> `[workspace] exclude`), nothing depends on it, and `cargo -p ordo-supervisor`
> will not resolve it from this archived location. Do not treat it as live code.

## What this crate is

The **supervisor**: a single in-process tokio task that derives a system-wide
rollup state from bus signals and publishes it on transitions.

- Subscribes to the bus (`ordo.*` wildcard).
- Records the inputs that matter — heartbeats, `*.degraded` events
  (memory-log, memory-projection, secrets seal-tier, MCP client-auth),
  self-heal urgency, and run lifecycle — into a pure `SystemModel`
  (`src/state.rs`, fully unit-tested, no bus/clock dependency).
- On a fixed interval (default 1s) derives a `(HealthState, ActivityState,
  reason)` triple and publishes `OrdoMessage::SystemStateChanged` on the
  `ordo.system.state` topic **only when the derived state changes**.

All derivation policy (thresholds, self-heal→health mapping, TTLs) lives in
this crate, never on the protocol wire — the protocol carries state, the
supervisor decides when to transition.

## Why it is archived

It was **never wired into the workspace or the runtime** — orphaned, not
deprecated. Specifically:

- It is **not listed** in the root `Cargo.toml` `[workspace] members` (an
  explicit ~56-entry list, not a glob), so cargo never built it — it does not
  even appear in `Cargo.lock`.
- **Nothing depends on it.** Its own doc-comment references a boot integration
  (`ordo-runtime` spawning it behind `RuntimeConfig::enable_supervisor`) that
  **does not exist** in the runtime — no `enable_supervisor` flag, no spawn
  call. It was groundwork that never landed.
- `ordo-protocol` still ships the `ordo.system.state` topic constant and the
  `SystemStateChanged` / `HealthState` / `ActivityState` types, and notes that
  the publisher (`ordo-supervisor`, "separate PR") never arrived — this crate
  *is* that missing publisher.

The code itself is coherent and tested; it is archived because it has no
call site, not because it is broken.

## Revival steps

The crate is self-contained and its only deps are in-workspace path crates,
so revival is mechanical:

1. **Move it back** to the workspace root:
   `git mv _archive/ordo-supervisor ordo-supervisor`
   (it sits as a sibling of `ordo-bus`, `ordo-protocol`, etc.), then remove
   this archive note so it doesn't travel into the live crate:
   `git rm ordo-supervisor/README.md`.
2. **Re-add it** to `[workspace] members` in the root `Cargo.toml`
   (e.g. next to `ordo-heal`), and **delete** the `"_archive/ordo-supervisor"`
   line from `[workspace] exclude`. The path deps `ordo-bus` and `ordo-protocol`
   (`{ path = "../ordo-bus" }`, `{ path = "../ordo-protocol" }`) and the
   `{ workspace = true }` deps (tokio, futures, tracing, uuid) all resolve again
   automatically once the crate is back under the workspace tree.
3. **Verify** it still compiles against current protocol/bus:
   `cargo check -p ordo-supervisor` and `cargo test -p ordo-supervisor`
   (the `ingest` match in `src/lib.rs` is the likeliest drift point if
   `OrdoMessage` variants or topic constants changed while it was parked).
4. **Wire a call site** to actually run it — add the spawn in `ordo-runtime`
   (gated, e.g. behind a `RuntimeConfig::enable_supervisor` flag) so the task
   is started during boot. Without this step it will compile but never run.
