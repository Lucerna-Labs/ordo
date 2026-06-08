# Ordo Developer Guide

This guide explains how to work on Ordo as a developer.

## Architecture

Ordo is built around a Rust/Tokio message bus. Subsystems communicate through
typed capabilities and explicit runtime surfaces rather than hidden direct
control paths.

Core ideas:

- local-first runtime
- bus-first subsystem communication
- capability inventory
- mode-scoped tools and memory
- local SQLite persistence
- explicit human approval gates
- provider and model abstraction
- diagnostic and operator tooling

## Unique Runtime Commitments

These are core Ordo design rules, not optional UI language.

### Gateway With Fallback

Provider access should pass through Ordo's gateway/provider layer. Local models
and OpenAI-compatible APIs are preferred where possible, with fallback profiles
for compatible providers. Do not scatter provider-specific calls throughout the
runtime.

### P2P And NAT/Cloud Device Paths

Ordo's connection layer should support direct local app and device
communication first, then use NAT traversal and cloud relay fallback only when
direct connection is unavailable. This is for Ordo-to-device and Ordo-to-app
communication; provider handshakes and cloud model access still go through the
gateway/provider layer.

Post-quantum handshakes belong in this connection layer. Treat them as part of
the peer/device trust establishment path, not as a reason to expose secrets to
models or bypass the gateway/provider boundary.

### Encrypted Secrets Store

Secrets belong in the local secrets/credential system. The model should receive
handles, status, or redacted metadata, never raw provider keys or tokens.

### Agent Has No Hands

Models do not directly operate the machine. They request capability calls.
Capabilities are filtered by mode, hooks, policy, review state, workspace scope,
and runtime security boundaries.

### Planner-First Execution

The Planner should decompose work into bounded steps before tools run. Avoid
features that let raw model text become direct state mutation.

### Strainer Against Prompt Injection

Untrusted content should move through the strainer path where appropriate:
normalization, stripping, classification, taint tracking, and sensitive-action
gating. Treat web/tool output as untrusted by default.

### Self-Learning Tree

Dreaming, diagnostic findings, corrections, repeated failures, and approved
lessons should be organized as a learning tree with review gates. Avoid a
single global memory pile that every mode can read.

### Always-On Diagnostic Mode

Diagnostic mode is local-only, isolated, and intended to inspect Ordo without
leaking diagnostic memory into general assistant work.

Diagnostic mode may also perform requested peripheral maintenance through
approved tools: MCP install/uninstall/trust/quarantine/re-authorization, skill
install/delete/repair, plugin install/delete/enable/disable, provider-profile
maintenance, and related integration repair. This permission is not a core
mutation bypass. Core runtime, security, hook, credential-custody, and UXI
boundary changes still require explicit operator-approved engineering work.

### Modes And Cross Consultation

Modes are containment boundaries. Cross-domain expertise should use explicit
consultation between mode agents, not direct RAG sharing.

The Rust Vibe Coder mode is the Rust-first development surface. It should
inspect existing crate boundaries before changing code, keep writes
approval-gated, treat warnings as failures, and verify affected crates with
`cargo check`, `cargo test`, and `cargo clippy` using warnings denied.

### Swarm Capabilities

Subagents and multi-agent workflows must be bounded by depth, budget, scope,
logs, and approval gates. Swarms should be surgical, not uncontrolled.

### MCP Security Model

MCP servers and plugins must be treated as untrusted external capability
providers unless they are first-party and explicitly trusted. Ordo's MCP layer
should preserve the modern defense-in-depth posture adopted from current MCP
security research and practice:

- signed lockfiles for installed server identity and declared capabilities
- trust states with explicit graduation instead of implicit trust
- quarantine for suspicious, changed, or unverified servers
- re-authorization when a server's tool catalog or capability shape drifts
- sandboxed workers and subprocess isolation where available
- provenance records for installed artifacts, tool catalogs, and invocations
- pre-call and post-call scanning around plugin/MCP payloads
- redacted audit findings so security diagnostics do not leak secrets

Do not add MCP installation, discovery, or auto-update paths that bypass these
states. Convenience is allowed only when the trust and audit model remains
visible to the operator.

## Development Setup

Install:

- Rust stable
- Node.js and npm
- Tauri prerequisites for your OS

Check Rust:

```powershell
rustc --version
cargo --version
```

Install studio dependencies:

```powershell
cd ordo-studio
npm install
```

## Run The Runtime

```powershell
cargo run -p ordo-cli -- serve
```

The control API defaults to:

```text
http://127.0.0.1:4141
```

## Run The Studio

```powershell
cd ordo-studio
npm run tauri:dev
```

## Required Checks

Warnings should be treated as failures.

```powershell
$env:RUSTFLAGS='-D warnings'
cargo check --workspace
cargo test --workspace
```

Run focused checks when touching automation:

```powershell
$env:RUSTFLAGS='-D warnings'
cargo test -p ordo-automation-primitives -p ordo-automation -p ordo-jobs --tests
cargo clippy -p ordo-automation-primitives -p ordo-automation -p ordo-jobs --tests
```

Run studio build:

```powershell
cd ordo-studio
npm run build
```

Run the operator simulator:

```powershell
cargo run -p ordo-operator-sim -- --origin http://127.0.0.1:4141
```

Run the Rust Vibe Coder preflight harness:

```powershell
.\scripts\ordo-preflight.ps1 -Origin http://127.0.0.1:4141 -Strict
```

This validates the mode, required skills, persistent memory anchors, a
no-write coder response, and a generated lite Rust app that must pass
`cargo check`, `cargo test`, and `cargo clippy` with warnings denied.

## Crate Map

- `ordo-protocol` - shared message schemas and topics
- `ordo-bus` - in-process Tokio bus
- `ordo-runtime` - runtime lifecycle wiring
- `ordo-control` - HTTP control API
- `ordo-studio` - desktop UXI
- `ordo-assistant` - assistant sessions and turn loop
- `ordo-modes` - mode registry and manifests
- `ordo-automation-primitives` - automation data model
- `ordo-automation` - automation orchestration
- `ordo-jobs` - scheduler primitives
- `ordo-agents` - agent/subagent primitives
- `ordo-operator-sim` - human-like pre-ship simulator
- `ordo-mcp-host` - capability host
- `ordo-mcp-registry` - MCP lockfiles and trust state
- `ordo-mcp-sandbox` - isolated MCP execution
- `ordo-plugins` - plugin manifests and stdio provider loading
- `ordo-cloud` - OpenAI/Anthropic/OpenAI-compatible provider boundary
- `ordo-connections` - external connection metadata and tests
- `ordo-memory-log`, `ordo-memory-router`, `ordo-memory-projection` - memory tree
- `ordo-rag` - retrieval indexing and preview
- `ordo-security` - policy and classifier-gated security stack
- `ordo-secrets-*` - local secret custody

## Adding A Capability

Prefer this order:

1. Define the data shape in the owning crate.
2. Add validation near the primitive data model.
3. Expose it through the capability host or control API only when needed.
4. Add focused tests.
5. Surface it in the UXI only after the runtime path exists.
6. Add operator documentation.

Avoid creating parallel control paths that bypass the bus or control API.

## Adding A Mode

A mode should define:

- id
- label
- description
- memory scopes
- RAG domains
- allowed tool lanes
- blocked tool capabilities
- policies
- planner bias
- persona/instruction layer

Modes should not silently share private RAG with other modes. Use explicit
cross-mode consultation when expertise is needed.

## Adding Automation

Automation belongs in two layers:

- `ordo-automation-primitives` for intent, risk, validation, and serializable
  shape
- `ordo-automation` for converting automation specs into jobs and task args

High-risk automation should require approval.

Coding automation must include:

- workspace path
- mode
- goal
- max subagents
- write policy
- commit policy
- dependency policy
- risk

Core runtime mutation should not be available as an automation action.

## UXI Rules

The UXI should:

- match Ordo's existing dark operator aesthetic
- avoid overlapping scroll regions
- keep tabs clear and separated
- expose important controls directly
- surface every meaningful user control in a grouped, understandable place
- include readable status, error, recovery, and refresh/test/inspect paths
- include exhaustive logs/events for app and platform workflows
- expose logs in the UXI when they explain normal operator-facing behavior
- use the live Ordo Studio UXI (`ordo-studio/src/OrdoShell.tsx`) and
  `ordo-studio/UXI_DEV_NOTES.md` as the design reference for user-friendly Ordo
  surfaces (the `ordo-studio/static-html-css/` snapshot is stale legacy — not a
  baseline)
- reject bland coder UI: unstyled forms, hidden-only settings, giant empty
  tables, clipped text, and missing empty/error states are not acceptable final
  UXI
- keep risky actions explicit
- avoid leaking secrets into model-visible state
- use the local control API rather than reading runtime files directly

For app-facing work, completion requires the app to be launched, inspected like
a human operator would use it, and verified for visible controls, persisted
state, logs/events, and no layout overlap.

## Data That Must Not Be Committed

Do not commit:

- `target`
- `node_modules`
- `dist`
- `user-files`
- runtime databases
- local logs
- `.env`
- model files
- local skills or personal data
- secrets or provider keys

## Public Release Notes

Before publishing a public copy:

1. Remove local runtime data.
2. Remove personal skills and modes.
3. Remove secrets and environment files.
4. Remove build artifacts.
5. Scan for local absolute paths.
6. Run warning-denied Rust checks.
7. Run studio build.
8. Run the operator simulator.
