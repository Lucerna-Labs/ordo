---
name: ordo-build-test
description: "Step 5 of the Ordo build pipeline. Use this skill after coupling is complete to run exhaustive testing before the app can be launched-proofed. It is a thin wrapper: it fires the existing test-and-verify and independent-review skills and gates on their captured evidence — it does not restate their procedures. Trigger it whenever the planner releases step 5, or whenever an Ordo build is coupled and you are about to claim it works. Do NOT declare the build tested from your own assertion. Do NOT advance to launch-proof while the deferred-debt list is non-empty. Compiling and coupling are not testing."
---

# Ordo Build — Test

Coupling is done. Before the app is launch-proofed, it gets exhaustively tested and independently reviewed. This step does not invent a testing procedure — it runs the two skills that already own that, and it gates on their evidence.

## Why this skill exists

The pipeline has verified pieces (each crate's unit check) and connections (each wiring). It has not yet verified the assembled whole under real exercise. This step is the whole-system gate, and it deliberately delegates to the skills built for it rather than duplicating them — a duplicated procedure drifts from the original the moment one is updated.

## Entry condition: deferred-debt must be clearable

Before testing begins, every item on the deferred-debt list from the couple step is now verifiable — the crates it was waiting on exist and are wired. Resolve each: confirm the previously-unverifiable wiring now flows end to end, and clear it from the list. A deferred item that *still* cannot be verified at this stage is no longer "deferred" — it is a coupling gap. Route it back through the couple step, do not carry it forward.

## What this step runs

1. **`test-and-verify`** — pull it and run its full procedure: build clean under `-Dwarnings`, run the automated test suite, launch and exercise the feature end to end, fill in the verification report with real captured evidence. Every claim backed by pasted output or a screenshot, never assertion. For an Ordo app specifically, "exercise the feature" means push real messages through the assembled bus and observe correct handling, not just that the binary starts.

2. **`independent-review`** — pull it and run it on the coupled, tested system. A second model that did not write the code reviews it, independently verifies it works, and surfaces what the author is blind to. This is the second of the two independent-review points (the first was the blueprint); here it reviews the implementation, not the plan. Resolve its findings before the gate passes.

## The exit gate

Pass requires: deferred-debt list at zero, a completed `test-and-verify` report with captured evidence for every step, and `independent-review` passed with findings resolved. Evidence is written to the ledger. On Pass the planner releases `ordo-launch-proof`.

A failed test honestly reported is this gate working. A passed build falsely claimed is exactly what it exists to stop — route any failure to `ordo-error-router`, fix, and re-run from the failed point.

## Forbidden in this step

- Do NOT restate or re-implement the test-and-verify or independent-review procedures — run the skills.
- Do NOT report tested without the captured evidence to prove it.
- Do NOT advance with any deferred-debt item still open.
- Do NOT treat the per-crate unit checks or the per-wiring verifications as a substitute for whole-system testing. They were necessary; they are not sufficient.
