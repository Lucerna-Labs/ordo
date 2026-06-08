# Rust Vibe Coder Memory Anchors

These are synthetic instruction memories for Rust Vibe Coder. They are
operator-authored rules, not claims about historical events.

## Anchor: Ordo App Platform Slice

When building an Ordo app or platform feature, use this sequence:

```text
Intent -> primitive -> adapter -> provider -> orchestrator -> UXI -> event log
```

After each completed step, write a pointer to the next relevant skill or
reference example.

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#app-platform-slice`

## Anchor: Primitive Kit And Orchestrator Combo

If a capability may be reused by multiple engines, modes, jobs, MCP servers,
plugins, providers, devices, or app surfaces, build a primitive kit instead of a
one-off integration.

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#primitive-kit-combo`

## Anchor: Warning-Clean Rust Loop

Rust work is not complete while warnings remain. Use warnings-denied checks for
the affected crate, then broaden only after the focused gate is clean.

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#warning-clean-rust-loop`

## Anchor: No Rust Patches, Native Rebuild

For Ordo Rust, do not make patch-style `.rs` edits. Trace the crate/module
contract, then rebuild the affected module or file natively and coherently.
Keep public API compatibility unless the operator approved a contract change.
Run warning-denied check/test/clippy gates before declaring the milestone done.

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#no-patch-native-rebuild`

## Anchor: Bus-First Platform Feature

Platform features should publish and consume typed bus-visible events. Avoid
hidden side channels where the UXI or a helper mutates runtime state invisibly.

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#bus-first-platform-feature`

## Anchor: Custom Anti Prompt-Injection Strainer

Any Ordo feature that ingests external text, documents, crawled pages, MCP
output, remote messages, app content, clipboard content, or uploaded files must
route that content through Ordo's custom anti prompt-injection strainer before
the planner or tools treat it as context.

Use this pipeline:

```text
untrusted input -> normalize -> strip executable instructions -> taint/provenance
-> boundary wrap -> planner treats as data -> policy gate narrows sensitive tools
-> audited capability result
```

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#custom-anti-prompt-injection-strainer`

## Anchor: Step Completion Pointer

After every completed implementation step or milestone, leave:

```text
Completed:
Verified:
Human-like usage test:
Launched for confirmation:
Next pointer:
Risk:
```

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#memory-anchor-after-step`

## Anchor: Automated Human-Like Usage Testing

For app-facing work, do not rely only on unit tests. Create or run automated
human-like usage that launches the app, waits for the first usable screen,
drives realistic workflows, checks visible/persisted state, covers a failure
and recovery path, and writes a report.

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#automated-human-usage-testing`

## Anchor: Completion Requires Launch Confirmation

Never claim a project is complete until exhaustive testing has passed and the
app has been launched for operator confirmation. Completion remains pending
until the operator can see the app and confirm the result.

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#completion-requires-launch-confirmation`

## Anchor: User-Friendly UXI And Exhaustive Logs

Every app or platform project must include exhaustive logs/events and a
user-friendly UXI. All meaningful controls must be surfaced in grouped,
operator-facing screens. Hidden-only configuration, unstyled coder forms,
overlapping scroll panes, clipped text, missing status, and missing recovery
actions are not complete UXI.

Use Ordo's static UXI snapshot as the design reference:

```text
ordo-studio/static-html-css/index.html
ordo-studio/static-html-css/styles.css
ordo-studio/static-html-css/README.md
```

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#user-friendly-uxi-and-logs`
