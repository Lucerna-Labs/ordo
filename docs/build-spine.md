# Ordo Build Spine

Date: 2026-06-06

This document records the native build-spine slice for Ordo's Rust vibe coder and autonomous build pipeline.

## Current State

The build spine now exists as compiled Rust crates, protocol contracts, control API routes, persistent build ledgers, and an operator UXI surface.

Implemented pieces:

- `ordo-protocol::build` defines the six fixed build steps, gate outcomes, gate evidence, error classes, build artifact refs, and planner events.
- `OrdoMessage` now carries build step completion, build gate result, and build planner event messages.
- `ordo-router` classifies the new build messages explicitly, with no wildcard fallback.
- `ordo-build-primitives` owns deterministic gate checks for stubs, architectural violations, COUPLE marker discipline, warning-free compile/test output, and launch proof requirements.
- `ordo-build-planner` owns the build ledger, planner transitions, bounded retry eligibility, deferred-debt handling, and persistent ledger storage.
- `ordo-build-planner::BuildPlannerPeer` can live on the builder bus, accept `BuildGateResult` messages, update the planner ledger, and publish `BuildPlannerEvent` messages.
- `ordo-control` exposes `/api/builds`, `/api/builds/:id`, and `/api/builds/:id/gate` so operators and diagnostic tooling can start builds, inspect ledgers, and submit explicit gate results.
- `ordo-studio` surfaces a Builds tab using Ordo's standard UXI language, with one scroll surface, manual gate submission, ledger inspection, and no autonomous write controls.

## Runtime Wiring

The runtime passes its user-files path into the control API. The control API creates a persistent build planner under:

```text
user-files/build-ledgers
```

The coder still does not receive autonomous hands. That remains intentional. The current slice creates the spine, ledger, gate contracts, event publication, and operator surface. Any future autonomous correction must stay bounded, logged, approval-gated, and incapable of bypassing the build gates.

## Build Sequence

1. Intake
2. Blueprint
3. Crate Build
4. Crate Couple
5. Build Test
6. Launch Proof

The planner advances only when the current step receives a real `GateOutcome::Pass`. A model saying a step is done is not enough.

## Failure Rules

- `Pass`: write durable output, publish step advance, release next skill.
- `Fail`: hard halt unless autonomous correction is enabled and the error is bounded.
- `Deferred`: valid only in `CrateCouple`; append deferred debt and do not route to the error router.

Bounded retry eligibility is reported as `BuildPlannerEvent::AutonomousRetryRequested`; it is not an advance and not a halt.

## Verification

The milestone was verified with warnings denied:

```powershell
$env:RUSTFLAGS='-D warnings'; cargo check --workspace
$env:RUSTFLAGS='-D warnings'; cargo test -p ordo-protocol -p ordo-build-primitives -p ordo-build-planner -p ordo-router --tests
$env:RUSTFLAGS='-D warnings'; cargo test -p ordo-control --tests
cd ordo-studio
npm run build
```

All listed gates passed with zero warnings.

## Next Phase

The remaining work is separate from this slice:

- operator approval gates for autonomous correction
- integration with diagnostic mode, logs, and event logger
- fuller coder automation that uses the build spine without gaining uncontrolled write access
