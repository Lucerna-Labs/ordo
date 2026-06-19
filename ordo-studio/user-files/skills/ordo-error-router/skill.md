---
name: ordo-error-router
description: "The failure handler for the Ordo build pipeline. Use this skill WHENEVER any step gate returns a Fail. It routes the failure on one axis — can the fix be named and is it bounded? — into either bounded autonomous correction (the Debugger loop, capped at 3, each retry re-gated) or a hard halt that surfaces to the user. It is gated by the global autonomous_correction flag, it never touches the deferred-unverifiable case, and it logs every attempt to the retry ledger. Trigger it on every GateResult::Fail. Do NOT route on compiler-vs-architectural — route on bounded-vs-unbounded. Do NOT let the autonomous loop pass a fix that compiles but violates the architecture. Do NOT route a deferred wiring here — that is not an error."
category:
  - build
  - pipeline
  - debugging
  - coding
available_to_modes:
  - coding
  - rust_vibe_coder
risk_level: medium
requires_tools: true
---

# Ordo Build — Error Router

When a step gate Fails, this skill decides what happens. The decision is not "compiler error vs architectural error" — that axis is wrong, because some compiler errors are unbounded thrash and some architectural errors are one-line fixes. The correct axis is: **can the fix be named, and is it bounded?**

## Why this skill exists

Models handle build failures in two bad ways: they either give up and dump a raw trace, or they thrash — "fixing" a borrow error by cloning everything and leaking `Arc`s until it compiles, producing working-but-wrong code that sails through the gate. This router replaces both. Bounded, nameable failures get a capped, re-gated autonomous loop. Unbounded or doctrine-violating failures halt with a trail. And a fix that compiles but breaks the architecture counts as a failed retry, not a success.

## The autonomy flag gates the whole autonomous branch

Read `autonomous_correction`. If `false` (the tuning default), **every** Fail hard-halts and surfaces to the user — zero wasted compute while you learn how the coder behaves against the real primitives. If `true`, bounded failures take the autonomous loop and only unbounded ones halt. The router is identical either way; the flag only decides whether the autonomous branch is live.

## First: is this even an error?

A `GateResult::Deferred` from the couple step is NOT a failure and must never reach this router. It is an expected, tracked deferral that goes to the deferred-debt list. If a deferred wiring lands here, the upstream step mislabeled it — send it back. Only a demonstrable Fail routes.

## The routing axis: bounded vs. unbounded

**Bounded — the fix can be named and is mechanical → autonomous loop:**
- missing import, unused result that should be handled, wrong function signature, missing match arm
- a shared type placed on the bus that belongs in the primitive crate (a one-move relocation)
- a field that needs adding to an existing frozen message (a bounded blueprint amendment)
- a type mismatch with an obvious, contract-preserving resolution

**Unbounded or doctrine-violating → hard halt:**
- a tokio channel panic during linking — this is a wrong message contract, not a syntax slip; a model will not reliably reason back from a panic trace
- the coder reaching for subprocess, webview, Tauri, a port, or IPC between crates
- a direct inter-crate call or a shared mutable reference bypassing the bus
- broken orchestrator topology, or a change that violates Ordo, Nodus, or spiderweb-bus doctrine
- borrow-checker / lifetime errors where the only compiling fix distorts ownership (clone-everything, leaked `Arc`, restructure that breaks bus discipline). These *look* bounded and are not — they are the classic thrash trap.

The test is not which subsystem complained. It is whether a named, bounded, contract-preserving fix exists. If naming the fix already requires distorting the architecture, it is a halt.

## The autonomous loop (the Debugger)

When a bounded failure routes here and the flag is `true`:

1. **Feed the Debugger the full picture, batched.** Run one `cargo check` and hand the Debugger the *complete* error-and-warning output from that pass, plus the relevant doctrine snippet retrieved from knowledge RAG, plus this crate's ledger slice. Do not peel errors off one at a time — fixing one-at-a-time is how a model burns all three retries on a single file. Fix the batch, then re-run.
2. **Re-gate every retry against the SAME gate it is trying to pass.** A retry that compiles but trips the step's architectural or anti-stub gate is a **failed** retry, not a success. This is the rule that stops clone-everything and stub-it-out from sneaking a green build past the gate.
3. **Cap at 3.** Three attempts. If the third does not produce a genuine Pass, escalate to a hard halt.
4. **Log every attempt to the retry ledger** (persistent memory): the error, the attempted fix, the diff, the gate result. So when it does escalate, the user sees the three things already tried and why each failed — the halt is a trail, not a fresh mystery.

The Debugger does NOT evict the current step's skill — a retry is not a step advance, and the skill body is needed to re-gate. The skill stays until the step actually clears.

## The hard halt

When a failure is unbounded, doctrine-violating, or has exhausted three bounded retries:

- Suspend the build. Do not advance, do not try further.
- Surface to the user: the error trace, the specific crate and code, **which doctrine it violated** (name it — "reached for a subprocess, violates ordo-runtime"), and the retry trail from the ledger if the autonomous loop ran.
- Wait. The user decides.

## Blueprint amendments through the router

A discovered structural need surfaced as a Fail is routed by amendment class, consistent with `ordo-build-blueprint`: adding a field to an existing message is a bounded amendment (autonomous-eligible under the flag); adding, removing, or restructuring crates is a major amendment that always halts. Every amendment bumps the blueprint version in the ledger.

## Router self-check

1. Is this actually a Fail, or a misrouted Deferred? Deferred goes to the debt list, not here.
2. Is `autonomous_correction` false? Then halt — no autonomous branch.
3. Can I *name* a bounded, contract-preserving fix? If naming it requires distorting ownership or the architecture, it is a halt, not a loop.
4. If looping: did I feed the full batched cargo output, cap at 3, re-gate each retry against the real gate, and log every attempt?
5. If halting: did I name the violated doctrine and attach the retry trail?
