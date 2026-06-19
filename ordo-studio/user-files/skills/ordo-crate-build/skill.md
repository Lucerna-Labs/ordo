---
name: ordo-crate-build
description: "Step 3 of the Ordo build pipeline. Use this skill to build the blueprint's crates ONE AT A TIME, each as a sealed function primitive, in topo order, before any coupling begins. It enforces the anti-stub gate (no todo!/unimplemented!/placeholder bodies), the warnings-as-errors policy with scoped COUPLE markers for legitimate isolation warnings, and the per-crate compile-clean + unit-check gate. Trigger it whenever the planner releases step 3, or whenever you are writing an individual Ordo crate. Do NOT couple crates here — building and coupling are separate phases. Do NOT leave a crate half-implemented and move on. Do NOT silence a warning to make a crate pass. A crate that compiles but stubs its real work is not built."
category:
  - build
  - pipeline
  - coding
  - rust
available_to_modes:
  - coding
  - rust_vibe_coder
risk_level: high
requires_tools: true
---

# Ordo Build — Crate by Crate

Build each blueprint crate as a complete, sealed function primitive, in the topo order the blueprint set, one at a time. A crate is finished when it compiles clean and its unit check passes — not when it parses. Pull `ordo-runtime` from RAG and match the shape of an existing subsystem crate. Read only this crate's ledger slice — its frozen contracts and its place in the DAG — not the whole ledger.

## Why this skill exists

Crate-by-crate building is exactly where the temptation to stub peaks: "build crate A, leave the part that needs B for later." That produces a workspace that compiles and silently does nothing. Models also reflexively silence warnings to get a green build, but in an Ordo codebase the warnings are architectural smells — an unhandled `Result` on a bus send is a dropped message, not a lint nit. This skill makes stubbing and warning-silencing structurally fail the gate.

## Build one crate at a time

Follow the topo build order. Build the crate, get it compiling clean, get its unit check passing, then move to the next. Do not start crate N+1 while crate N is unfinished. Each crate is sealed: it subscribes to its frozen inputs, emits its frozen outputs, and exposes its capability at its contract seam. It does not reach into another crate, does not share mutable state, does not call another subsystem directly. Off-bus shared types live in the primitive crate; the crate honors the protocol contract for everything on the bus.

## The anti-stub gate

A crate does not pass while it contains any of:

- `todo!()`, `unimplemented!()`, `unreachable!()` standing in for real logic
- a placeholder return (hardcoded `Ok(())`, `Default::default()`, an empty `Vec`) where real work belongs
- a commented-out body, or a function that silently does nothing it was meant to do

This is grep-able and the gate treats it as deterministic: a match is a Fail. If a crate genuinely cannot do its full job until a later crate exists, that is a wiring concern for the couple step — expose the seam and mark it (below), do not fake the behavior with a stub.

## The warnings policy

Warnings are failures, with one narrow, tracked exception.

**Deny by default, globally.** Set `RUSTFLAGS="-Dwarnings"` (or the workspace lint equivalent) so `cargo check` itself fails on any warning and the compile gate inherits it for free — no separate warning parser. `unused_must_use` on a bus send, an unhandled match arm, unreachable code: these mean a message is being dropped or a contract is half-implemented. They are never allowed, at any phase, full stop.

**The one scoped exception.** A crate built in isolation legitimately throws `dead_code`/`unused` because its consumer does not exist yet — the function it exposes has no caller until the couple step. Only for `dead_code` and `unused`, and only during this build step, the coder may place a scoped allow with a tracking marker:

```rust
#[allow(dead_code)] // COUPLE: <consumer-crate-name>
```

Every such marker is written to the COUPLE-marker list in the ledger as tracked debt. The couple step requires each marker to be removed and the warning to genuinely clear once the consumer exists. A leftover COUPLE marker at launch-proof is a hard fail. The marker turns "ignore this warning" into a debt that a later gate forces to zero — same discipline as the deferred-debt list.

Do NOT use the scoped allow for anything but `dead_code`/`unused`. Do NOT use it without the `// COUPLE:` marker. Do NOT use a blanket crate-level `#![allow(...)]` to escape the policy.

## The per-crate exit gate

A crate passes when: it compiles clean under `-Dwarnings` (modulo only the marked COUPLE allows), the anti-stub gate finds nothing, and its unit check passes. The crate's status and any COUPLE markers are written to the ledger. The planner does NOT advance to coupling until *every* blueprint crate has passed this gate.

## The phase boundary

This is a hard wall: all crates built and passing before any coupling starts. Do not wire crate A to crate B because they happen to both be done. Building and coupling are separate phases for the same reason the architecture separates primitives from the orchestrator — the wiring is its own concern, verified on its own, in step 4.

## If a crate fails its gate

The failure routes to `ordo-error-router`, which decides bounded autonomous correction vs. hard halt. Do not silence, stub, or work around a failure to force a Pass — that defeats every gate downstream.
