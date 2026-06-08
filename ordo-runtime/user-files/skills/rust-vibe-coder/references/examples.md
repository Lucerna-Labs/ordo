# Rust Vibe Coder Examples

These examples are persistent anchors for Ordo coding work. They are compact on
purpose: use them as pointers after each completed step, then inspect the real
code before acting.

## app-platform-slice

Build an Ordo app or platform feature as a slice:

```text
Intent -> primitive -> adapter -> provider -> orchestrator -> UXI -> event log
```

Example:

```text
Goal: Add local device pairing.
Primitive: peer identity, device descriptor, handshake transcript.
Adapter: QUIC/WebRTC/native transport implementation.
Provider: connection capability with schema, policy, and review metadata.
Orchestrator: choose direct P2P, NAT traversal, or relay fallback.
UXI: pairing controls, status, logs, trust state.
Verification: unit tests for primitive, integration test for routing decision,
operator simulator pass for visible controls.
```

Step pointer: `rust-vibe-coder/references/examples.md#app-platform-slice`

## primitive-kit-combo

When a feature should help more than one engine, build a primitive kit instead
of one direct integration.

```text
Primitive crate:
  Owns reusable types, validation, and deterministic operations.

Adapter crate:
  Binds the primitive to one concrete backend, provider, file format, device,
  renderer, or runtime service.

Provider:
  Exposes the adapter as an Ordo capability with descriptor, schema, policy,
  review, and event logging.

Orchestrator:
  Selects the provider and sequences work. It does not own primitive logic.
```

Step pointer: `rust-vibe-coder/references/examples.md#primitive-kit-combo`

## warning-clean-rust-loop

Rust work is not complete while warnings remain.

```powershell
$env:RUSTFLAGS='-D warnings'; cargo check -p <crate>
$env:RUSTFLAGS='-D warnings'; cargo test -p <crate> --tests
$env:RUSTFLAGS='-D warnings'; cargo clippy -p <crate> --tests
```

If a warning appears:

```text
1. Identify owning crate/module.
2. Fix the cause, not the diagnostic text.
3. Re-run the same command.
4. Broaden checks only after the focused gate is clean.
```

Step pointer: `rust-vibe-coder/references/examples.md#warning-clean-rust-loop`

## no-patch-native-rebuild

For Ordo Rust, do not make patch-style `.rs` edits. Rebuild the affected
module/file natively after tracing its contract.

```text
1. Read the crate manifest, module entry point, public types, tests, and callers.
2. State the module contract in one or two sentences.
3. Rebuild the affected file/module so it remains coherent as a whole.
4. Keep public API compatibility unless the operator approved a contract change.
5. Run warning-denied check/test/clippy gates.
6. Write a milestone note before continuing.
```

Milestone note shape:

```text
Completed: Rebuilt <module/file> around <contract>.
Verified: <exact command> -> passed with zero warnings.
Next pointer: rust-vibe-coder/references/examples.md#no-patch-native-rebuild
Risk: <none or specific unresolved risk>
```

Step pointer: `rust-vibe-coder/references/examples.md#no-patch-native-rebuild`

## bus-first-platform-feature

Ordo platform features should publish and consume typed bus-visible events
instead of creating hidden side channels.

```text
Input event -> planner/orchestrator decision -> capability call -> provider
result -> event log/debug surface -> UXI state
```

Avoid:

```text
UI calls private helper -> helper mutates runtime state invisibly
```

Step pointer: `rust-vibe-coder/references/examples.md#bus-first-platform-feature`

## custom-anti-prompt-injection-strainer

Any feature that ingests external text, documents, crawled pages, MCP output,
remote messages, app content, or uploaded files must preserve Ordo's custom
anti prompt-injection boundary.

```text
Untrusted input
-> strainer normalization
-> instruction stripping / boundary wrapping
-> taint + provenance record
-> planner sees data, not instructions
-> policy gate narrows sensitive tools
-> capability result logs taint ancestry
```

Build the strainer as a reusable capability path, not a UI-only filter:

```text
Primitive: normalized content, taint marker, provenance hash, boundary wrapper.
Adapter: web, file, MCP, remote-message, app, or clipboard ingestion source.
Provider: strain/fetch/ingest capability with schemas and audit events.
Orchestrator: routes untrusted content through strainer before planner/tool use.
UXI: shows taint state, source, strictness, and blocked-sensitive-tool reason.
Verification: tests for hostile instructions, source tracking, blocked memory
persistence, and gated tool calls after tainted context.
```

Never bypass the strainer because a source seems trusted. If the operator wants
special treatment, add a policy rule or mesh-size rule; do not create a bypass.

Step pointer: `rust-vibe-coder/references/examples.md#custom-anti-prompt-injection-strainer`

## automated-human-usage-testing

Before claiming an app or platform workflow is complete, create or run a
human-like usage test that behaves like an operator, not a unit test.

```text
1. Launch the app or service exactly how the operator will launch it.
2. Wait for the first usable screen, not merely process startup.
3. Drive primary workflows with realistic clicks, keyboard input, file upload,
   mode switches, settings changes, and error recovery.
4. Assert visible outcomes, persisted state, event logs, and no overlapping UI.
5. Exercise at least one failure path and one recovery path.
6. Capture a report, screenshots/log excerpts where useful, and final verdict.
7. Leave completion pending until the operator can confirm the launched app.
```

For Ordo, the operator simulator is the baseline pattern:

```text
health -> runtime profile -> modes -> skills/plugins/MCP -> automations
-> files/apps/connections -> chat session -> assistant turn -> report
```

For a UI app, prefer a small repeatable harness:

```text
start app -> open target tab -> perform realistic task -> verify visual state
-> inspect logs/events -> export report -> stop app cleanly
```

Step pointer: `rust-vibe-coder/references/examples.md#automated-human-usage-testing`

## completion-requires-launch-confirmation

Do not call the project complete just because code compiles. Completion requires
exhaustive testing appropriate to the blast radius and a launched app for
operator confirmation.

```text
Minimum completion gate:
- warning-denied check/test/clippy for affected Rust crates
- broader checks for shared/runtime/security/automation changes
- generated or existing human-like usage test for app workflows
- app launched and visible for operator confirmation
- milestone note with exact commands and remaining risk
```

Completion note shape:

```text
Completed:
Verified:
Human-like usage test:
Launched for confirmation:
Next pointer:
Risk:
```

Step pointer: `rust-vibe-coder/references/examples.md#completion-requires-launch-confirmation`

## user-friendly-uxi-and-logs

Every app-facing feature needs a user-friendly UXI and exhaustive logs. A
backend capability without visible controls is not complete.

```text
Feature contract:
  visible controls
  visible state
  edit/delete/pause/test/refresh actions where relevant
  logs/events for user actions, provider choices, denials, retries, and errors
  static UXI snapshot or explicit reference to Ordo's snapshot
  human-like workflow verification
```

Use Ordo's static snapshot as the baseline:

```text
ordo-studio/static-html-css/index.html
ordo-studio/static-html-css/styles.css
ordo-studio/static-html-css/README.md
```

Reject bland coder UI:

```text
unstyled forms, hidden-only settings, giant empty tables, overlapping scroll
regions, clipped text, no status, no logs, no recovery action
```

Step pointer: `rust-vibe-coder/references/examples.md#user-friendly-uxi-and-logs`

## memory-anchor-after-step

After each completed step, leave a compact note:

```text
Completed: <one sentence>
Verified: <command/check/result>
Next pointer: <skill/reference anchor>
Risk: <none or specific risk>
```

For long-term memory, store only durable lessons:

```text
Synthetic instruction memory, not observed history:
When building Ordo features, use primitive -> adapter -> provider ->
orchestrator -> UXI -> event log. Verify with warnings denied and keep writes
approval-gated.
```

Step pointer: `rust-vibe-coder/references/examples.md#memory-anchor-after-step`
