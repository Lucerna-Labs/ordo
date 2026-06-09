lane: Ordo Architecture

# Ordo Primitive Kit And Orchestrator Skill

## Purpose

Ordo Primitive Kit and Orchestrator teaches a model how to build reusable
capability primitives and wire them into Ordo's orchestrator so other engines,
functions, modes, plugins, MCP servers, jobs, providers, and device workflows
can reuse them safely.

It is for designing the combo of:

- primitive capability building blocks
- engine-neutral Rust crates
- adapter layers for concrete runtimes
- capability providers
- orchestration plans
- bus-visible capability descriptors
- policy/review/event integration
- UXI surfaces for operator control

This skill is about reusable capability architecture, not one-off feature
wiring.

## Loader Hook

Apply Ordo Primitive Kit and Orchestrator when a model is asked to design,
create, extend, or repair reusable primitives, capability kits, adapter layers,
or orchestrator wiring that lets Ordo expose new features to engines,
functions, jobs, modes, plugins, MCP servers, providers, or remote devices.
Keep primitives engine-neutral, adapters thin, providers gated, orchestration
observable, and UXI controls explicit.

## Activation Triggers

Use this skill when the task mentions:

- primitive kit
- primitives
- capability kit
- reusable capability
- orchestrator combo
- feature provider
- engine adapter
- function adapter
- capability provider
- capability inventory
- bus descriptor
- plugin capability
- MCP capability
- exposing a feature to other engines
- reusable toolkit
- adding features to multiple Ordo functions
- routing tools through the orchestrator

Do not activate for ordinary one-crate bug fixes unless the change affects
reusable capability architecture.

## Core Rule

Separate the layers.

```text
Primitive:
  Pure reusable capability logic and shared types.

Adapter:
  Converts a primitive to a concrete engine, runtime, renderer, provider,
  storage layer, device, or file format.

Provider:
  Exposes the adapter as an Ordo capability with descriptors, schemas, policy,
  review, security, and event logging.

Orchestrator:
  Selects, sequences, routes, and observes capabilities. It does not own
  primitive implementation details.

UXI:
  Shows operator controls, status, settings, and logs. It does not become a
  second runtime.
```

If one layer starts doing another layer's job, stop and redesign.

## Architecture Pattern

Use this standard build pattern:

```text
1. Define primitive
2. Define contracts and data types
3. Add adapter
4. Add capability provider
5. Register provider through the security/review path
6. Advertise capability descriptors
7. Add orchestrator routing/planning
8. Surface operator controls in the UXI if needed
9. Add storage/migrations only through the shared storage layer
10. Add event logger coverage
11. Test from primitive to provider boundary
12. Verify the affected workspace
```

Do not start by adding a UI button, HTTP route, or direct call between crates.

## Primitive Layer Rules

Primitives should be:

- engine-neutral
- side-effect-light
- testable without the runtime
- serializable where they cross boundaries
- small enough to be reused
- explicit about inputs and outputs
- independent of UI, HTTP, provider credentials, and process lifecycle

Good primitive examples:

```text
Geometry, bounds, color, text metrics, policy decisions, routing decisions,
claim classification, source scoring, retrieval ranking, permission checks,
job schedules, capability descriptors, trust handshakes, document transforms,
transport envelopes.
```

Bad primitive examples:

```text
Open a window.
Call a cloud provider directly.
Read arbitrary user files.
Spawn a process.
Store secrets.
Publish to the bus.
Render a specific UI framework.
```

Those belong in adapters, providers, services, or UXI surfaces.

## Adapter Layer Rules

Adapters connect primitives to real execution environments.

Adapters may own:

```text
Runtime engine binding
Renderer binding
Provider SDK binding
Network transport binding
Storage binding
Filesystem binding
Document/PDF/DOCX binding
Model backend binding
Remote device binding
```

Adapters must stay thin. They translate and execute; they should not invent
policy, orchestration strategy, or hidden state.

If there are multiple engines, create multiple adapters over the same primitive
contract rather than duplicating primitive logic.

## Provider Layer Rules

Any new Ordo capability should be exposed through a capability provider.

Provider responsibilities:

```text
Name and lane
Description
Input schema
Output shape
Permission scope
Review behavior
Security classification
Event logging
Error normalization
Version/protocol compatibility
```

Reject one-off business logic in HTTP handlers, UI callbacks, or direct
cross-crate calls. If other engines or functions may reuse the behavior, it
belongs behind a provider boundary.

## Orchestrator Rules

The orchestrator should:

- discover available capabilities
- understand capability descriptors
- plan which capabilities to use
- route work through providers
- preserve traceability
- respect mode, policy, provider, plugin, MCP, and device boundaries
- emit observable events
- support fallback when a capability is missing

The orchestrator should not:

- know private implementation details of primitives
- bypass provider gates
- silently call external systems
- hardcode a provider when a descriptor can be used
- treat plugin/MCP capabilities as built-in system behavior

## Capability Lane Naming

Choose lanes that reveal ownership and purpose.

```text
research.*
document.*
automation.*
job.*
hook.*
mode.*
plugin.*
mcp.*
provider.*
memory.*
rag.*
transport.*
handshake.*
security.*
policy.*
review.*
device.*
```

Use the existing prefix if the capability belongs to an existing domain. Add a
new prefix only when it represents a real owner boundary.

Do not use `mcp.*` for plugin lanes. MCP belongs in the MCP registry and tab.

## Storage Rules

If the primitive kit needs persistence:

- use the shared Ordo storage path
- add migrations in the storage-owning crate
- include `workspace_id` from the start where workspace scope exists
- keep provenance and source identity
- avoid per-plugin private databases unless the plugin is explicitly a separate
  provider cache
- avoid logging secrets or private payloads

Storage schema must be designed before UI or provider shortcuts depend on it.

## Security And Review Rules

Every reusable capability must define its safety class.

```text
Read-only:
  Can inspect or summarize without changing state.

Write:
  Changes local state or files.

Network:
  Calls outside the local machine.

Device:
  Talks to a paired device or peer.

Secret-bearing:
  Uses credentials, tokens, keys, or vault material.

Destructive:
  Deletes, publishes, sends externally, charges money, or changes trust.
```

Destructive and secret-bearing capabilities must be reviewable and event logged.

## UXI Rules

Add UXI controls when the operator needs to:

```text
Enable/disable
Install/remove
Configure
Grant permissions
Run manually
Pause/resume
View status
View logs
Inspect failures
Select provider/adapter
Choose mode assignment
```

The UXI should call the existing control/provider surfaces. It should not
become an alternate implementation of the capability.

## Mode Assignment Rules

Not every mode needs every primitive kit skill.

Use mode assignment like this:

```text
Coding/runtime mode:
  Primitive design, Rust implementation, provider contracts.

Research mode:
  Research/document/source primitives.

Operations mode:
  Jobs, heartbeats, hooks, automation primitives.

Security mode:
  Policy, trust, secrets, sandbox, review primitives.

Device/transport mode:
  P2P, NAT, ICE, handshake, routing, remote device primitives.
```

Avoid loading primitive architecture rules into casual chat or writing modes
unless the user is designing capabilities.

## Build Decision Checklist

Before implementing, answer:

```text
What capability is being added?
Who owns it?
Is there already a primitive?
Is there already a provider?
Does it need an adapter?
What engines/functions will reuse it?
What lane names should it advertise?
What permissions does it need?
Does it need review?
Does it need storage?
Does it need protocol changes?
How will the orchestrator discover it?
How will the UXI surface it?
How will it be tested?
```

If these are unclear, do not start with code.

## Implementation Sequence

Use this sequence for new capability kits:

### Stage 1 - Inventory

Search for existing:

```text
Types
Traits
Providers
Adapters
Lanes
Events
Migrations
Tests
UXI surfaces
```

Reuse established patterns.

### Stage 2 - Primitive Contract

Define the smallest reusable contract:

```text
Input:
Output:
Errors:
State:
Provenance:
Security class:
```

Keep it independent of engine-specific details.

### Stage 3 - Adapter

Bind the primitive to one execution context:

```text
Engine:
Runtime:
Provider:
Storage:
Transport:
Format:
```

Adapters should be replaceable.

### Stage 4 - Provider

Expose the capability:

```text
Lane:
Descriptor:
Schema:
Permission scope:
Review behavior:
Event logs:
Failure modes:
```

### Stage 5 - Orchestrator Integration

Add discovery and routing:

```text
Capability inventory:
Planner visibility:
Mode eligibility:
Fallback behavior:
Missing capability message:
```

### Stage 6 - UXI Integration

Surface only what the operator needs:

```text
Status
Configuration
Permissions
Logs
Run/pause/delete controls
Mode assignment
```

### Stage 7 - Verification

Verify at each layer:

```text
Primitive unit tests
Adapter tests
Provider tests
Orchestrator/routing tests
UXI build or screenshot checks when visible
Cargo check/test/clippy for affected crates
```

## Anti-Patterns

Reject these:

- direct crate-to-crate calls that bypass provider boundaries
- HTTP handlers with business logic
- UI components that implement runtime behavior
- duplicated primitive logic in multiple adapters
- plugin-owned behavior registered as built-in MCP tools
- MCP tools surfaced as plugins
- hidden background jobs
- unbounded retries
- provider credentials stored in manifests
- new storage files for data that belongs in shared Ordo storage
- protocol-shape changes without protocol ownership
- prompt stuffing instead of pull-based capabilities

## Event Logger Rules

Primitive kits should create traceable events.

Use event families such as:

```text
capability.registered
capability.invoked
capability.completed
capability.failed
adapter.selected
adapter.failed
orchestrator.plan_created
orchestrator.route_selected
orchestrator.fallback_used
provider.permission_blocked
provider.review_required
```

Include:

```text
capability
lane
provider_id
adapter_id
mode_id
job_id
run_id
status
duration_ms
error_kind
```

Do not include secrets, private payload bodies, or raw document contents unless
the operator explicitly requests them.

## Output Modes

### Capability Kit Plan

```text
Capability:
Primitive:
Adapter:
Provider:
Lane:
Consumers:
Security class:
Storage:
UXI:
Tests:
Verification:
```

### Architecture Review

```text
Findings:
Layer violations:
Boundary risks:
Missing provider gates:
Missing event logs:
Test gaps:
Recommended fix:
```

### Implementation Report

```text
Primitive added:
Adapter added:
Provider added:
Orchestrator wiring:
UXI surfacing:
Tests:
Verification:
Remaining risks:
```

## Installation Metadata

```yaml
id: ordo_primitive_orchestrator
recommended_path: ordo-studio/user-files/skills/ordo_primitive_orchestrator/skill.md
category:
  - architecture
  - primitives
  - orchestration
  - capability
  - adapters
risk_level: medium
requires_tools: false
persistent_memory_access: optional
available_to_modes:
  - coding
  - research
```
