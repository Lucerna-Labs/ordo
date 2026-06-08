---
name: ordo-build-blueprint
description: "Step 2 of the Ordo build pipeline. Use this skill after the requirements record exists and BEFORE any crate is built. It produces the build blueprint: the crate list where each crate is one sealed responsibility, the FROZEN claw-protocol message contracts, the dependency DAG, and the topo-sorted build order. It also defines the only legal way to amend the blueprint later. Trigger it whenever the planner releases step 2, or whenever you are about to start writing Ordo crates without a frozen message schema and an ordered build plan. Do NOT begin coding crates here. Do NOT leave message types to be 'figured out per crate' — unfrozen contracts are the number-one cause of the tokio channel panics this pipeline exists to prevent."
---

# Ordo Build — Blueprint

This step turns the requirements record into a buildable plan and freezes the contracts everything else depends on. No crate gets built until this gate passes. Pull `ordo-runtime` and `spiderweb-bus` from RAG before you start — the blueprint must match that architecture exactly.

## Why this skill exists

Two failures originate here when this step is skipped or rushed. First, message types discovered crate-by-crate desync two crates and surface as a tokio channel panic during the couple step — late, confusing, and expensive. Second, an architectural mistake made in the blueprint gets built *correctly and wrongly* across every crate. Freezing the contracts up front kills the first. Running independent review before the first crate kills the second. Both gates live in this step for that reason.

## What the blueprint contains

1. **The crate list.** Each crate is exactly one responsibility — one sealed function primitive. If a crate "does X and also Y," it is two crates. The protocol crate (`claw-protocol` pattern) and the store crate (`claw-store` pattern, only if the requirements record says state persists) are explicit entries. The UI, if any, is a subsystem crate like any other.

2. **The frozen message contracts.** Every claw-protocol message type the app needs, defined in full, in the protocol crate, before any crate is built. This is the constitution. Once frozen, a crate may not invent or alter a message type — it honors the contract. Changing a contract after freeze is an amendment (below), not a casual edit.

3. **The dependency DAG.** Which crate depends on which. The protocol crate and any shared primitive crate sit at the root — everything imports them. This graph is the input to the build order.

4. **The topo-sorted build order.** The actual sequence the build step follows, derived from the DAG — primitive/protocol crates first, then consumers. Do NOT emit the blueprint's list order as the build order; emit the topological order. Building a consumer before its dependency forces the coder to stub the dependency, which the build step's anti-stub gate will then reject — wasted work.

5. **The coupling order.** The order crates get wired onto the bus in step 4, also derived from the DAG.

## Freeze the contracts — what that means

After this step, the set of message types is fixed. A crate built in step 3 is built against a known, unchanging schema. This is your contract-shaped-primitive thinking applied to the build: the schema is the sealed contract; crates conform to it. A discovered need to change the schema is a blueprint event, handled by the amendment protocol — never a silent per-crate edit.

## The amendment protocol

The build step *will* discover the blueprint was incomplete — that is guaranteed by "you can't tell until you build further." Forward-only would force the coder to either drift silently (the worst outcome) or halt unproductively. So amendments are legal, sanctioned, and versioned:

- A discovered structural need is written to the ledger as a **proposed amendment**, never applied silently.
- **Add a field to an existing frozen message** is a bounded amendment. Under `autonomous_correction: true` it may auto-apply; under `false` it surfaces to the user.
- **Add a crate, remove a crate, or restructure the DAG** is a major amendment. It always halts for the user — adding a crate changes the shape of the build.
- Every amendment bumps the blueprint version in the ledger. The blueprint has history, not just a current state. Never overwrite; append a version.

## The independent-review gate

Before this step can pass, run `independent-review` on the blueprint itself — a second model that did not author it checks the crate boundaries, the message contracts, and the DAG against Ordo doctrine. This is the highest-leverage review point in the whole pipeline: catching an architectural mistake here costs a conversation; catching it at launch-proof costs the build. Resolve its findings before the gate passes.

## The exit gate

Pass requires all of: crate list (each one responsibility), message contracts frozen in the protocol crate, dependency DAG, topo build order, coupling order, independent-review passed and findings resolved. The blueprint is written to **long-term memory** and the ledger as version 1. On Pass the planner releases `ordo-crate-build`.

## Forbidden in this step

- Do NOT write crate implementation code. This step plans; it does not build.
- Do NOT leave any message type undefined or "TBD per crate." Freeze them all or you have not finished.
- Do NOT emit list order as build order. Topo-sort it.
- Do NOT collapse two responsibilities into one crate to save crates. One crate, one responsibility.
- Do NOT skip independent review because the blueprint "looks right." Looking right is exactly when a built-in mistake is most expensive.
