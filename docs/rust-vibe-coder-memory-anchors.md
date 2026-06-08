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

After every completed implementation step, leave:

```text
Completed:
Verified:
Next pointer:
Risk:
```

Pointer:
`ordo-studio/user-files/skills/rust-vibe-coder/references/examples.md#memory-anchor-after-step`
