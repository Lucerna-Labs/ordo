---
name: ordo-crate-couple
description: "Step 4 of the Ordo build pipeline. Use this skill to wire the already-built crates onto the Tokio bus, one wiring at a time, verifying after each. This is the orchestrator phase — the crates are sealed primitives and coupling composes them on the bus. It handles the deferred-unverifiable case (a wiring you cannot confirm until more is built) as tracked debt, NOT as a failure, and it clears the COUPLE markers left by the build step. Trigger it whenever the planner releases step 4, after every blueprint crate has passed its build gate. Do NOT couple before all crates are built. Do NOT wire everything at once and check at the end. Do NOT treat 'can't verify yet' as either a pass or a failure — it is deferred debt."
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

# Ordo Build — Couple

The crates are built and sealed. This step wires them onto the bus and verifies each connection as it is made. Pull `spiderweb-bus` and `ordo-runtime` from RAG and match how an existing crate subscribes and emits. Read this step's ledger slice — the coupling order, the frozen contracts, the COUPLE-marker list.

## Why this skill exists

A model that wires all crates at once and runs the app at the end gets a tangle: when something doesn't flow, the bug could be in any of a dozen connections. Verifying one wiring at a time means a wiring bug is caught against one message, not five. This is the same protocol-before-bus, one-message-verified-before-the-next sequencing the runtime architecture mandates — applied to assembling the finished app.

## Wire one connection at a time

Follow the blueprint's coupling order. For each wiring: connect the crate's subscriptions and emissions onto the bus, start it from `main`, then push one real message end to end through the new connection and confirm it arrives and is handled. Then the next. `main` starts the bus, starts the store, starts each subsystem — and wires nothing else; the wiring *is* the subscriptions each crate declares.

## The three gate outcomes

Each wiring produces exactly one result:

- **Pass** — a message flowed end to end through the new connection and was handled correctly. Captured evidence (log line, observed output). Recorded in the ledger.
- **Fail** — the wiring is wrong and you can prove it: wrong message type, a panic, a handler that errored. Routes to `ordo-error-router`. A tokio channel panic during linking is a Fail and a hard-halt class — it means the contract is wrong, not that a retry will fix it.
- **Deferred** — you genuinely cannot verify this wiring until a later crate is also wired (a round-trip that needs three crates present to close). This is NOT a failure. Append it to the deferred-debt list with the reason, and continue. It never routes to the error router and never halts.

The distinction matters: routing a deferred wiring to the Debugger makes it try to "fix" something that isn't broken yet. Only an actual, demonstrable failure routes.

## Clear the COUPLE markers

The build step left `#[allow(dead_code)] // COUPLE: <crate>` markers wherever a crate exposed a seam whose consumer didn't exist yet. Now the consumer exists. As each marked seam gets wired, remove its allow and confirm the warning genuinely clears under `-Dwarnings`. Update the COUPLE-marker list in the ledger. The step does not pass while any COUPLE marker remains.

## The exit gate

Pass requires: every crate wired in coupling order, every wiring resolved as Pass or Deferred (no open Fail), and the COUPLE-marker list at zero. The deferred-debt list may be non-empty here — it is the next gate (step 5 entry) and step 6 that force it to zero. Wiring statuses written to the ledger. On Pass the planner releases `ordo-build-test`.

## Forbidden in this step

- Do NOT begin coupling before all crates have passed the build gate.
- Do NOT introduce a direct inter-crate call, a shared mutable reference, a port, or a subprocess to make a wiring "work." If a wiring needs one of those, the wiring is wrong — route it as a Fail to the error router.
- Do NOT mark a wiring Pass without a message actually flowing through it. "It should connect" is not a Pass.
- Do NOT close out the step with COUPLE markers still in the tree.
- Do NOT silently leave a wiring unverified — it is either Pass, Fail, or explicitly Deferred with a reason.
