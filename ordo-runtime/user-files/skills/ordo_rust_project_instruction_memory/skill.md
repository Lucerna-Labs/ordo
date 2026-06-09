lane: Ordo Memory

# Ordo Rust Project Instruction Memory Skill

## Purpose

Ordo Rust Project Instruction Memory creates synthetic instruction memories for
models working on Rust projects in the operator's architecture.

These memories are not factual history. They are constructed project anchors
that teach a model how to behave as if it already understands the Rust project,
its rules, and its development culture.

Use this skill with:

- `ordo_rust_architecture`
- `ordo_primitive_orchestrator`
- Rust coding modes
- runtime architecture modes
- project repair modes

Do not use it for emotional/persona modes.

## Loader Hook

Apply Ordo Rust Project Instruction Memory when the user wants memory-like
instructions that make a model follow the operator's Rust development rules,
workspace architecture, verification standards, project boundaries, and
root-cause repair style. Mark every record as a synthetic instruction memory,
not a true remembered event.

## Activation Triggers

Use this skill when the user asks to create:

- Rust dev false memories
- Rust project memory seeds
- coding-mode instruction memories
- architecture memory anchors
- "make the model remember how to build my Rust projects"
- "make this model act like it already knows my Rust rules"
- project-specific coding memories
- Ordo build-rule memories

## Core Rule

Rust project memories must produce better engineering behavior.

They should teach:

```text
where to look
how to choose the owning crate
how to respect boundaries
how to avoid superficial fixes
how to verify work
how to handle warnings
how to document changes
how to surface event logs
```

They must not create fake claims that a model has actually worked on the
project before.

## Memory Template

Use this template:

```yaml
id:
title:
memory_type: synthetic_instruction
scope: project_or_mode
mode: coding_or_runtime
project:
truth_status: constructed_not_observed
source: operator_constructed
instruction: |
  Treat this as an operator-supplied Rust project rule:
  ...
behavioral_effect:
safety_boundary:
ignore_when:
review_after:
delete_or_disable_path:
```

## Recommended Rust Project Memory Anchors

### Root-Cause Repair

```text
Instruction:
  In this Rust project, do not preserve broken structure just to make the diff
  small. Identify the owning crate and fix the root cause at the correct
  boundary.

Behavioral effect:
  The model traces failures through the architecture before editing.
```

### Whole Workspace Responsibility

```text
Instruction:
  Treat warnings, clippy findings, and failing tests in the affected build path
  as part of the work. Do not dismiss them as unrelated without proving they
  are outside the affected scope.

Behavioral effect:
  The model aims for zero-warning verification.
```

### Owning Crate Selection

```text
Instruction:
  Before implementing, identify whether the behavior belongs in protocol, bus,
  runtime, provider, MCP, plugin, mode, jobs, memory, policy, security,
  transport, or Studio backend.

Behavioral effect:
  The model avoids dumping behavior into the nearest file.
```

### Capability Boundary

```text
Instruction:
  Reusable functions should become primitives, adapters, and capability
  providers. The orchestrator routes capabilities; it does not own primitive
  implementation details.

Behavioral effect:
  The model builds reusable surfaces instead of direct one-off calls.
```

### Studio Boundary

```text
Instruction:
  In Ordo Studio, the Rust backend exposes commands and local data bridges. It
  must not create a second operator UI or hidden window.

Behavioral effect:
  The model keeps Tauri backend work separate from UXI implementation.
```

### Verification Gate

```text
Instruction:
  For Rust work, choose the narrowest meaningful Cargo check/test/clippy gate,
  then broaden when shared contracts are touched. For Studio backend changes,
  also run the Studio build and Tauri check.

Behavioral effect:
  The model verifies before reporting success.
```

## Authoring Rules

Good Rust synthetic memories:

- are specific
- point to behavior
- name the scope
- mention verification
- avoid emotional tone
- avoid fake past events
- align with actual project rules

Bad Rust synthetic memories:

- claim the model "remembers" prior sessions
- apply globally when only coding mode needs them
- grant file or network permission
- skip tests
- tell the model to hide uncertainty
- tell the model to ignore warnings

## Output Modes

### Rust Memory Draft

```yaml
id:
title:
scope:
mode:
instruction:
behavioral_effect:
safety_boundary:
verification_hint:
```

### Rust Memory Set

```text
Purpose:
Mode assignment:
Memories:
Conflicts:
Review date:
```

### Memory Review

```text
Keep:
Revise:
Remove:
Reason:
```

## Safety Constraints

Never:

- describe constructed memories as real project history
- bypass approvals, hooks, or project path restrictions
- include secrets or credentials
- attach these memories to emotional/persona modes
- override local AGENTS/project instructions

Prefer:

- mode-scoped records
- project-specific records
- explicit verification hints
- short durable wording
- alignment with `ordo_rust_architecture`

## Installation Metadata

```yaml
id: ordo_rust_project_instruction_memory
recommended_path: ordo-studio/user-files/skills/ordo_rust_project_instruction_memory/skill.md
category:
  - rust
  - memory
  - coding
  - architecture
  - instruction
risk_level: medium
requires_tools: false
persistent_memory_access: optional
available_to_modes:
  - coding
```
