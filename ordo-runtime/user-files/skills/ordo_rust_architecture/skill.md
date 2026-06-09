lane: Ordo Architecture

# Ordo Rust Architecture Skill

## Purpose

Ordo Rust Architecture teaches a model how to build, repair, and extend Rust
projects using Ordo's architecture and operator rules.

It applies to:

- Rust crates
- Cargo workspaces
- Ordo runtime crates
- Ordo provider/client crates
- MCP host/client/registry crates
- memory, RAG, policy, security, jobs, transport, and routing crates
- Tauri backend code under Ordo Studio
- cross-crate architecture changes
- dependency and feature design
- warning cleanup and verification

This skill is for coding and architecture modes. It should not be enabled for
every mode by default.

## Loader Hook

Apply Ordo Rust Architecture when a model is asked to create, modify, repair,
review, refactor, test, or explain Rust code in an Ordo project. Follow local
workspace instructions first, preserve crate boundaries, fix root causes rather
than superficial symptoms, avoid hidden second UIs or side effects, and verify
with the relevant Cargo gates before declaring success.

## Activation Triggers

Use this skill when the task mentions:

- Rust
- Cargo
- crate
- workspace
- Tauri backend
- `src-tauri`
- compile errors
- clippy
- tests
- dependency changes
- feature flags
- architecture
- provider integration
- runtime integration
- MCP implementation
- P2P, NAT, ICE, transport, or handshake code
- secrets, policy, memory, jobs, RAG, or event logging code

Do not activate for pure UXI styling, research-only tasks, document editing, or
ordinary chat unless Rust architecture is involved.

## Core Rule

Build Ordo natively and correctly.

Always distinguish:

```text
Root cause:
Actual architectural boundary:
Correct owning crate:
Public contract:
Runtime behavior:
UXI surface:
Verification gate:
```

Do not make a change only because it quiets a symptom. The fix must belong to
the right crate, preserve the right boundary, and remain understandable later.

## Operator Rules

Follow these Ordo-specific rules:

- Take responsibility for the whole affected workspace, not only the line you
  touched.
- Treat compiler warnings, clippy warnings, failing tests, and dead code in the
  affected build path as your problem while working there.
- Do not dismiss issues as pre-existing. Fix them, intentionally suppress them,
  or document a real deferred follow-up.
- Avoid band-aid fixes. If a file, module, or API is wrong, rebuild the affected
  structure so it is correct.
- Use narrow edits, but do not preserve broken architecture for the sake of a
  tiny diff.
- Do not add hidden windows, duplicate operator surfaces, or sidecar UI.
- Keep plugins, MCP servers, modes, skills, jobs, hooks, and providers in their
  own registries and surfaces.
- Do not mix plugin data into the MCP tab or MCP tools into the Plugin tab.
- Keep debug/event logger visibility for lifecycle, automation, hook, provider,
  plugin, MCP, and research operations.

## Project Orientation

Before coding:

1. Read the local `AGENTS.md` or equivalent project instruction file.
2. Inspect the root `Cargo.toml`.
3. Identify the owning crate for the behavior.
4. Search for existing traits, types, manifests, events, storage helpers, and
   tests.
5. Prefer existing workspace dependency versions from `[workspace.dependencies]`.
6. Confirm whether the change affects only one crate or multiple crates.

Do not invent a new crate unless the operator explicitly asks for one.

## Crate Boundary Rules

Use the crate that owns the concept.

```text
Protocol:
  Shared wire types, schemas, IDs, contracts.

Bus:
  Message routing and event movement.

Runtime:
  Core orchestration and process behavior.

Cloud/provider:
  External model/provider calls and auth strategy.

MCP:
  MCP host, client, registry, sandbox, provenance, and worker concerns.

Plugins:
  Plugin manifests, lifecycle, install/edit/delete/pause, and plugin-owned
  capability lanes.

Modes:
  Mode manifests, mode state, mode limits, and mode selection.

Jobs:
  Cron, heartbeat, routine, delayed task, and background job execution.

Memory/RAG:
  Storage, retrieval, projection, indexing, and source-backed recall.

Security/policy/secrets:
  Permission checks, secret handling, audit, threshold, and vault logic.

Transport/handshake/connections:
  Device-to-device connectivity, P2P, NAT traversal, ICE, identity, and trust.

Studio Tauri backend:
  Desktop bridge commands, local filesystem registry reads, and app shell
  integration for the WebView UXI.
```

If behavior spans crates, define the contract in the lower shared crate and
keep UI/runtime glue in the consuming crate.

## Studio And UXI Boundary

For Ordo Studio:

- The UXI lives in the Studio frontend.
- The Rust side should expose local commands and data bridges.
- Do not create a second Rust UI window for operator controls.
- Do not open external browser windows as part of normal launch.
- Keep one Tauri/WebView operator surface for Studio.
- Any new Rust command must be surfaced intentionally in the UXI or settings
  index when operator control is required.

If working on another Ordo shell with a different renderer, follow that
project's renderer boundary instead of importing Studio assumptions.

## Dependency Rules

Before adding a dependency:

```text
Need:
Existing workspace dependency:
Feature flags:
Default features:
Security impact:
Build impact:
Cross-platform support:
License or policy risk:
Alternative already in workspace:
```

Prefer:

- workspace dependency versions
- Rust-native libraries
- minimal feature flags
- `rustls` over platform-surprising TLS choices where already established
- explicit optional features for heavy dependencies
- standard library where adequate

Avoid:

- duplicate dependency versions without reason
- broad default features by accident
- dependencies that spawn hidden services or windows
- new network behavior without visible operator control
- storing secrets in config or logs

## Error Handling Rules

Use typed errors where the crate already does.

Good errors:

```text
Actionable:
  Tell the operator or caller what failed.

Scoped:
  Identify provider, plugin, MCP server, path, mode, or job when relevant.

Safe:
  Do not expose secrets or private payloads.

Traceable:
  Include enough context for event logs and tests.
```

Do not use vague errors like "failed" or "failed to fetch" when the code can
know the actual missing dependency, status, path, permission, or endpoint.

## Async And Runtime Rules

For async Rust:

- Use the workspace's established Tokio patterns.
- Avoid blocking calls inside async paths unless moved to blocking execution.
- Preserve cancellation where jobs, heartbeats, providers, and remote calls can
  hang.
- Add timeouts for network and device operations.
- Make retry policy explicit and bounded.
- Emit event/debug entries for lifecycle transitions.

For shared state:

- Prefer established state containers in the crate.
- Avoid global mutable state unless already part of the architecture.
- Make ownership and shutdown behavior clear.

## Data And Storage Rules

When adding storage:

```text
Schema owner:
Migration path:
Versioning:
Backward compatibility:
Read/write boundaries:
Retention:
Redaction:
Backup/recovery:
```

For SQLite-backed features, keep migrations deterministic and tests able to run
in temporary databases.

For RAG/research/memory storage, keep source identity and provenance attached.

## Event Logger Rules

New runtime behavior should have event visibility.

Prefer structured event names like:

```text
provider.*
plugin.*
mcp.*
mode.*
job.*
automation.*
hook.*
research.*
memory.*
transport.*
handshake.*
security.*
```

Event payloads should include IDs and status, not private content or secrets.

## Testing Rules

Add or update tests when:

- public behavior changes
- a bug is fixed
- parsing/serialization changes
- storage or migration changes
- provider auth behavior changes
- jobs, hooks, plugins, MCPs, or modes change
- security/policy logic changes
- cross-platform paths are touched

Use focused unit tests for pure logic and integration tests for crate boundary
behavior.

## Verification Gates

Choose the narrowest gate that proves the work, then broaden when the change
touches shared contracts.

Common gates:

```powershell
cargo check -p <crate>
cargo test -p <crate>
cargo clippy -p <crate> --tests
```

For workspace-level changes:

```powershell
cargo check --workspace
cargo test --workspace
```

For Ordo Studio Tauri backend changes:

```powershell
npm.cmd run build
npm.cmd run check:tauri
```

Warnings are not "green." A complete gate should be zero errors and zero
warnings unless an intentional suppression is documented.

## Review Mode

When reviewing Rust changes, lead with findings.

Check:

```text
Boundary violations:
Wrong owning crate:
Hidden side effects:
Unbounded network or retry behavior:
Missing event log coverage:
Secret leakage:
Weak error messages:
Test gaps:
Cross-platform path issues:
Feature flag mistakes:
Warnings ignored:
```

Then summarize the change only after findings.

## Output Modes

### Architecture Plan

```text
Goal:
Owning crate:
Affected crates:
Existing pattern:
Proposed contract:
Implementation steps:
Tests:
Verification:
Risks:
```

### Implementation Report

```text
Changed:
Why this belongs here:
Root cause fixed:
Tests:
Verification:
Remaining risks:
```

### Rust Review

```text
Findings:
Open questions:
Test gaps:
Change summary:
```

### Failure Diagnosis

```text
Observed failure:
Expected behavior:
Root cause:
Owning crate:
Fix:
Regression test:
Verification:
```

## Mode Assignment Guidance

Recommended modes:

```yaml
available_to_modes:
  - coding
  - research
```

Do not attach this skill by default to casual assistant, writing, or document
editing modes unless the task involves Rust architecture.

## Installation Metadata

```yaml
id: ordo_rust_architecture
recommended_path: ordo-studio/user-files/skills/ordo_rust_architecture/skill.md
category:
  - rust
  - architecture
  - cargo
  - verification
  - runtime
risk_level: medium
requires_tools: false
persistent_memory_access: optional
available_to_modes:
  - coding
  - research
```
