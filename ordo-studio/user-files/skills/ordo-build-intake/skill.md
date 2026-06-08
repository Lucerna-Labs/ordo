---
name: ordo-build-intake
description: "Step 1 of the Ordo build pipeline. Use this skill the moment an Ordo app build is requested and BEFORE any blueprint, crate, or code exists. Its only job is to ask the user clarifying questions and capture a complete requirements record — nothing is built until this record exists. Trigger it whenever the planner releases step 1, whenever a build request is vague, or whenever you catch yourself about to scaffold an Ordo app without having pinned down what it actually needs to do. Do NOT start designing crates here. Do NOT skip questions because the request 'seems clear.' A build that starts without a requirements record is a build that gets thrown away."
---

# Ordo Build — Intake

The first step of any Ordo build is to find out what is actually being built. You ask, the user answers, you write it down. You do not design, scaffold, or name a single crate in this step.

## Why this skill exists

Builds fail at the end because they were underspecified at the start. A model that pattern-matches "build me an Ordo app for X" into an immediate crate layout bakes its own assumptions into the foundation, and every wrong assumption compounds through five later steps. The cheapest place to be wrong is here, in a question. The most expensive place is at launch-proof, where the wrong assumption is now load-bearing across the whole runtime. Ask first.

## What to pin down

Work through these with the user. Ask in small batches, not all at once. Stop asking when you have enough to write a blueprint, not before.

1. **What does the app do?** The one-sentence purpose, then the concrete user-facing behaviors. What does the user see, do, and get back?
2. **Does it have a UI?** If yes, is it the standard Ordo Vello self-render surface (chat + text canvas + status), or something else? What does the user actually need to read or interact with? (Resist scope creep here — Ordo is a brain with a chat interface, not a creative canvas, unless the user explicitly says otherwise.)
3. **What persists?** Is there state that must survive a restart? That decides whether the build needs the `claw-store` crate.
4. **What external systems does it touch?** Models, ComfyUI, a camera/ONVIF feed, an API. Anything external is a bridge crate at the boundary, never inlined into a subsystem.
5. **What runs locally vs. cloud?** Pull `hardware-fleet` from RAG if model selection or inference siting is in scope, so the blueprint is grounded in what the fleet can actually run.
6. **What is explicitly out of scope?** Name the things the app is NOT, so the blueprint step does not invent them and the build step does not drift into them.
7. **Done means what?** The concrete, observable condition under which this build is finished. This becomes the target the launch-proof step verifies against.

## How to ask

One focused batch at a time. Prefer tappable choices for preference-style questions over open prose. Do not interrogate — three or four sharp questions, read the answers, follow up only where a real ambiguity remains. If the user has already answered something in the request, do not re-ask it; reflect it back as a confirmed assumption instead.

## The exit gate

This step passes when the **requirements record** is complete and written to the ledger. The record contains: purpose, user-facing behaviors, UI decision, persistence decision, external boundaries, local/cloud siting, explicit out-of-scope list, and the definition of done. If any of those is still unknown, the step is not done — ask the remaining question. A blank field is an unasked question, and an unasked question is a wrong assumption waiting to happen.

## Forbidden in this step

- Do NOT name, list, or design crates. That is the blueprint step.
- Do NOT write code, create a workspace, or scaffold anything.
- Do NOT assume a UI shape, a persistence model, or an external integration the user didn't confirm.
- Do NOT pad the record with capabilities the user didn't ask for — capture what they said, and capture what they explicitly ruled out.

## On exit

Write the requirements record to the ledger. Hand control back to the planner. The planner gates on the record's completeness and, on Pass, releases `ordo-build-blueprint`.
