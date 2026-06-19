---
name: ordo-build-pipeline
description: "The master sequencing spec for autonomously building an Ordo app end to end. This is the PLANNER's skill, not the coder's — read it WHENEVER you are about to start, resume, or advance a multi-step Ordo build, or whenever you are deciding which step skill to hand the coder next. It defines the six build steps, each step's entry condition and exit gate, the GateResult message contract, the build ledger, the skill-eviction rule, and the three-memory routing. The planner advances the build by checking gates and releasing step skills in increments — the coder never chooses its own skills. If you are running an Ordo build and you are not following this spec, you have drifted. Do not let the coder skip a gate, do not hand out the next skill before the prior gate passes, and do not let procedural skill text pile up in context."
category:
  - build
  - pipeline
  - planning
available_to_modes:
  - coding
  - rust_vibe_coder
risk_level: medium
requires_tools: true
---

# Ordo Build Pipeline

This is how an Ordo app gets built autonomously: a fixed sequence of steps, each gated, each producing durable state in the ledger, each handed to the coder by the planner one at a time. You — the planner — own the sequence. The coder owns the work inside a step. The coder does not pick its own skills; you release them.

## The one idea this whole pipeline rests on

The build pipeline *is* the primitive-crate + orchestrator doctrine applied to the act of building. Each crate is built in isolation as a sealed function primitive. The couple step is the orchestrator wiring those primitives onto the bus. Build-in-isolation, then couple-on-the-bus — the same topology the app itself uses, applied to constructing it. Hold that and every step below makes sense.

## Where the planner lives

The planner runs as a citizen of the **builder** runtime's bus. Gate hooks are claw-protocol messages on that bus. A gate classifier publishes a `GateResult`; the planner subscribes and reacts. "Gate passed → write ledger → evict prior step's procedure → release next step's skill" is a single message handler, not an out-of-band script.

The **app being built has its own bus**, which does not exist for most of the build and only comes alive at the launch-proof step. Never confuse the two. The planner is a citizen of the builder bus; it is never a citizen of the app it is constructing.

## The GateResult contract

Every step boundary is a deterministic gate classifier that emits exactly one `GateResult`:

- `Pass` — evidence satisfied. Planner writes the step's durable output to the ledger, evicts the prior skill's procedure, releases the next skill.
- `Fail { error_class, evidence }` — routes to `ordo-error-router`. Never advances.
- `Deferred { reason }` — **couple step only.** Not a failure. Appends to the deferred-debt list and continues. Never routes to the error router, never halts.

The planner reads the `GateResult`, never the coder's narrative claim of "done." A claim without a `Pass` is not an advance.

## The six steps

| # | Step skill | Entry condition | Exit gate (Pass requires) |
|---|------------|-----------------|---------------------------|
| 1 | `ordo-build-intake` | Build requested | Complete requirements record in ledger |
| 2 | `ordo-build-blueprint` | Requirements record exists | Frozen message contracts + crate list + dependency DAG + build order, independent-review passed, blueprint v1 in long-term + ledger |
| 3 | `ordo-crate-build` | Blueprint frozen | Every blueprint crate built as a sealed primitive, each compiles clean (modulo scoped COUPLE allows) and passes its unit check |
| 4 | `ordo-crate-couple` | All crates built | Every crate wired onto the bus, every wiring Pass or Deferred, every COUPLE marker removed |
| 5 | `ordo-build-test` | Coupling complete, deferred-debt empty | `test-and-verify` + `independent-review` pass with captured evidence |
| 6 | `ordo-launch-proof` | Tests pass | App launches, screenshot captured, one real UiInput round-trip proven, deferred-debt and COUPLE markers at zero |

The order is not negotiable and steps do not overlap. Step 3 does not begin until step 2's gate passes. No crate is coupled until *all* crates are built. "Complete" cannot be claimed before step 6 passes.

## The build ledger (persistent memory)

One ledger per project, keyed to the project identity, living in **persistent memory**. It is the durable spine of the build and the reason the build survives a session boundary. It holds:

- the requirements record (step 1)
- the blueprint, versioned — never overwritten, amended (step 2 and any amendment)
- per-crate status (step 3)
- per-wiring status (step 4)
- the deferred-debt list (step 4, must reach zero by step 6)
- the COUPLE-marker list (step 3, must reach zero by step 4)
- the retry ledger — every autonomous-correction attempt, its diff, its gate result (error router)
- the launch proof (step 6)

## Scoped reads — do not dump the whole ledger

Evicting procedure from context (below) is wasted if every step then reads the entire ledger back in. Each step reads only its **projected slice**: the crate's own contracts, the deferred-debt list, the relevant blueprint section. Never the full ledger. The pile-up problem we solve in context must not reappear inside the ledger reads.

## Skill eviction — evict procedure, keep contract

When a gate passes, the planner drops the **procedural text** of the step skill just completed from the coder's working context. It does NOT drop the step's **output** — that was already written to the ledger. The next skill opens by reading its ledger slice; it inherits results, not the prior skill's body. Context stays roughly flat per step instead of growing across the whole build.

Two hard rules on eviction:

- **Only at a passed gate.** Never mid-step. If the coder is three crates into step 3 and you evict the build skill, it loses the procedure halfway. Eviction is the same transition as releasing the next skill: skill out, ledger written, next skill in.
- **The error router's Debugger is exempt.** A retry is not a step advance. The current step's skill stays present through every retry because the Debugger checks its diffs against that step's gate. Evict only when the step actually clears.

## Three memories

- **Knowledge RAG** — the doctrine the step skills *retrieve* (`ordo-runtime`, `spiderweb-bus`, `nodus-protocol-doctrine`, `ordo-ui-architecture`, `hardware-fleet`). Step skills stay thin and pull what they need rather than carrying it inline.
- **Long-term memory** — the blueprint and the project identity. Survives across the whole build and across sessions.
- **Persistent memory** — the live ledger, above.

None of the three ever loses anything during eviction. Eviction only touches the coder's working context.

## The autonomy flag

The pipeline reads one global config value: `autonomous_correction`. While you are still tuning the pipeline against the real primitives, set it `false` — every gate failure hard-halts and surfaces the trace to the user, zero wasted compute, you watch how the coder behaves. Once the bounded-class routing is trusted, set it `true` to enable the autonomous-correction branch in `ordo-error-router`. The router is built either way; the flag only gates whether its autonomous branch is live.

## Planner self-check before advancing a step

1. Did a `GateResult::Pass` actually arrive for the current step? If not, do not advance.
2. Have I written this step's durable output to the ledger? If not, write it before evicting anything.
3. Am I about to evict mid-step or mid-retry? If yes, STOP — eviction is gate-boundary only.
4. Is the deferred-debt list empty before step 5, and are COUPLE markers zero before step 4? If not, the gate cannot pass.
5. Am I releasing exactly one next skill, with its ledger slice, and nothing the coder didn't earn? If yes, proceed.

## Companion skills

- `ordo-build-intake`, `ordo-build-blueprint`, `ordo-crate-build`, `ordo-crate-couple`, `ordo-build-test`, `ordo-launch-proof` — the six step skills, released in order.
- `ordo-error-router` — consulted on every `GateResult::Fail`.
- `ordo-runtime`, `spiderweb-bus`, `ordo-ui-architecture`, `nodus-protocol-doctrine`, `hardware-fleet` — doctrine the steps retrieve from RAG.
- `test-and-verify`, `independent-review`, `completion-enforcer` — fired by steps 5 and 6 rather than restated.
