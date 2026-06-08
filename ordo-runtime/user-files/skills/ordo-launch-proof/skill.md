---
name: ordo-launch-proof
description: "Step 6 and the final gate of the Ordo build pipeline. Use this skill as the last step before claiming an Ordo build complete. It launches the app, captures a screenshot, and proves one real UiInput round-trip flowed over the app's bus — window plus proven round-trip, not a screenshot alone. It also confirms the deferred-debt list and COUPLE markers are at zero. Trigger it whenever the planner releases step 6, or whenever you are about to tell the user an Ordo app is done. Do NOT claim complete on a screenshot alone — a window proves it drew once, not that the bus is alive. Do NOT claim complete with any deferred debt or COUPLE marker still open."
---

# Ordo Build — Launch Proof

The build is tested. This step proves it actually runs as an Ordo runtime before anyone says "complete." It overlaps `completion-enforcer` — pull that and let it enforce no-placeholders-no-stubs-no-unfinished-work; this skill adds the Ordo-specific launch proof on top. Read the requirements record's definition of done from the ledger; that is what this step verifies against.

## Why this skill exists

A screenshot proves a window appeared. It does not prove the message bus is alive, that the orchestrator came up, or that input flows and renders. For an Ordo app — where the entire architecture is messages on a bus — a window that drew once and a window that is a live runtime look identical in a screenshot. The difference is a round-trip. This step demands the round-trip so "complete" means "runs," not "drew."

## The launch proof

1. **Launch the app.** Start the binary. Confirm it comes up without panicking. Capture the startup output. The app's own bus comes alive here — this is the only point in the pipeline where that bus exists.

2. **Capture the screenshot.** Capture the running studio UI (the Tauri/React webview). This is necessary but not sufficient.

3. **Prove one real round-trip.** Drive a real interaction in the studio UI — the live proof that distinguishes a running UXI from a static diagram: click or type in the UI, let the studio call the runtime (a Tauri `invoke` command or the control API over HTTP), let the runtime resolve the result, and confirm the studio re-renders the changed result. Observe the state actually change (a tab switch, a content update). Capture evidence of the round-trip: the input emitted, the message flowing, the rendered result changing. A window plus a proven round-trip is "launched." A window alone is "drew once" and does not pass.

4. **Confirm against the definition of done.** The requirements record (step 1) named the observable condition under which this build is finished. Verify it holds, with evidence.

## Clear the debts

The gate cannot pass while either is non-empty:

- **Deferred-debt list at zero.** Every previously-unverifiable wiring is now proven (cleared in step 5; reconfirm here).
- **COUPLE markers at zero.** No `#[allow(dead_code)] // COUPLE:` survives anywhere in the tree, and the app compiles clean under `-Dwarnings` with none of them.

A leftover in either list is a hard fail regardless of how well the app launches.

## The exit gate — the only place "complete" is earned

Pass requires all of: app launched without panic, screenshot captured, one real UiInput round-trip proven with evidence, definition-of-done satisfied, deferred-debt at zero, COUPLE markers at zero, and `completion-enforcer` clean. The launch proof is written to the ledger. Only on this Pass may the build be reported complete to the user.

## Forbidden in this step

- Do NOT claim complete on a screenshot without a proven round-trip.
- Do NOT claim complete with open deferred debt or surviving COUPLE markers.
- Do NOT fabricate or paraphrase launch output or round-trip evidence. If it didn't run, say so.
- Do NOT treat "it launched on my machine once" as the proof — the captured screenshot and the captured round-trip are the proof.
