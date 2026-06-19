lane: Ordo Reasoning

# Ordo Mathematical Primitive Reconstruction Skill

## Purpose

Ordo Mathematical Primitive Reconstruction teaches a model the operator's
technique for breaking systems down into mathematical primitives and rebuilding
them from the original primitive vocabulary instead of copying the surface form.

It applies to anything that can be decomposed into structure, constraints,
signals, transformations, state, or equations:

- RF and synthetic signal behavior
- protocols
- algorithms
- UI layout behavior
- motion and animation
- routing functions
- scoring functions
- parsing rules
- scheduling behavior
- control systems
- filters
- geometry
- data transforms
- workflows with measurable state transitions

The goal is to understand the underlying function deeply enough to rebuild it
cleanly from primitives.

## Loader Hook

Apply Ordo Mathematical Primitive Reconstruction when the user wants to break
down a primitive, function, signal, protocol, behavior, system, workflow, UI
layout, algorithm, or process into math or structural primitives, then rebuild
it by replacing surface guesses with the original required primitives. Produce
decomposition, primitive mapping, reconstruction plan, and verification tests.

## Activation Triggers

Use this skill when the user says or implies:

- break it down into math
- decompose the function
- find the primitives
- rebuild from primitives
- reconstruct the original behavior
- replace the needed primitives
- model this signal
- derive the formula
- reverse the behavior
- reduce it to constraints
- rebuild it correctly
- like we did for RF signals
- identify the underlying mechanism

Do not activate for ordinary summaries unless the user wants the underlying
structure, math, or primitive reconstruction.

## Core Rule

Do not copy the surface behavior. Reconstruct the function.

Always separate:

```text
Observed surface:
Underlying variables:
Primitive operations:
Constraints:
Invariants:
Unknowns:
Original primitive candidates:
Replacement/rebuild primitives:
Verification signals:
```

If the math is uncertain, label it as a hypothesis and test it.

## Method Overview

The technique has five moves:

```text
1. Observe:
   Capture what the system appears to do.

2. Decompose:
   Break the behavior into variables, equations, constraints, states, and
   transformations.

3. Identify primitives:
   Find the smallest original building blocks that explain the behavior.

4. Reconstruct:
   Rebuild the behavior using those primitives, not surface mimicry.

5. Verify:
   Compare reconstructed behavior against observed behavior and revise.
```

The model should think like a systems engineer, not a copier.

## Stage 1 - Observation

Collect the observable behavior.

```text
Inputs:
Outputs:
Timing:
State changes:
Thresholds:
Boundaries:
Noise:
Failure modes:
Examples:
Counterexamples:
```

For signal-like systems, also capture:

```text
Frequency or rate:
Amplitude or scale:
Phase or offset:
Sampling assumptions:
Modulation or encoding:
Noise floor:
Windowing:
Aliasing risk:
```

For UI/layout systems, capture:

```text
Coordinate system:
Anchors:
Constraints:
Breakpoints:
Spacing:
Typography metrics:
State transitions:
```

## Stage 2 - Mathematical Decomposition

Reduce the behavior into formal pieces.

Possible primitives:

```text
Scalars
Vectors
Matrices
Complex numbers
Intervals
Bounds
Ratios
Transforms
Filters
Kernels
State machines
Graphs
Queues
Probability distributions
Threshold functions
Piecewise functions
Recurrence relations
Constraints
Invariants
```

Map each observed behavior to the smallest primitive that explains it.

Use this format:

```text
Behavior:
Primitive:
Variables:
Equation or rule:
Evidence:
Confidence:
```

## Stage 3 - Primitive Candidate Scan

Identify candidate original primitives.

Ask:

```text
What is the smallest primitive that could produce this behavior?
Is this behavior continuous, discrete, symbolic, probabilistic, or hybrid?
Is this a transform, filter, state transition, routing rule, or constraint?
Is the apparent complexity just composition of simpler primitives?
Which primitive is load-bearing?
Which primitive is decorative or incidental?
Which primitive can be replaced without changing the function?
Which primitive must be preserved exactly?
```

Do not treat labels, names, UI text, or incidental implementation details as
primitives unless they affect the function.

## Stage 4 - Rebuild Plan

Rebuild from the primitive vocabulary.

For each component:

```text
Original behavior:
Needed primitive:
Replacement primitive:
Why this primitive is sufficient:
What must remain invariant:
What can change:
Test case:
```

The rebuild should preserve the function, not necessarily the old
implementation.

## Stage 5 - Verification

Verification must compare behavior, not aesthetics or assumptions.

Use:

```text
Known input/output pairs
Boundary cases
Noise cases
Timing cases
State transition cases
Property tests
Round-trip tests
Conservation or invariant tests
Error bounds
Visual or signal plots when relevant
```

For numerical systems, define acceptable tolerance.

For state machines, verify allowed and forbidden transitions.

For signal systems, verify spectral/temporal properties rather than only sample
text or labels.

## Reconstruction Modes

### Signal Reconstruction

Use for lawful synthetic, owned, lab, or test signal analysis.

```text
Observed signal:
Sampling model:
Primitive waveforms:
Transforms:
Filters:
Encoding/modulation:
Noise model:
Reconstruction:
Verification:
```

Safety boundary:

Do not help intercept, decode, bypass, or misuse real third-party RF systems.
Keep RF work scoped to lawful, synthetic, owned, educational, or authorized test
signals.

### Function Reconstruction

Use for algorithms and transformations.

```text
Input domain:
Output domain:
Primitive operations:
Composition:
Invariants:
Complexity:
Equivalent implementation:
Tests:
```

### Protocol Reconstruction

Use for message flows and structured exchanges.

```text
Actors:
Messages:
State machine:
Fields:
Timing:
Trust boundary:
Failure states:
Equivalent protocol:
```

Keep security and authorization boundaries explicit.

### UI/Layout Reconstruction

Use for visual or interaction systems.

```text
Coordinate system:
Layout primitives:
Constraints:
State:
Input handling:
Rendering invariants:
Responsive behavior:
Verification screenshots/tests:
```

### Workflow Reconstruction

Use for human or agent process flows.

```text
Actors:
Inputs:
Decision points:
State:
Transitions:
Artifacts:
Permissions:
Equivalent workflow:
```

## Replacement Rule

When replacing a primitive, prove equivalence at the right level.

```text
Exact replacement:
  Same primitive, same behavior.

Equivalent replacement:
  Different implementation, same externally observable function.

Approximate replacement:
  Similar behavior within declared tolerance.

Invalid replacement:
  Surface looks similar but core invariants fail.
```

Never call an approximate replacement exact.

## Relation To Ordo Primitive Kit

Use this skill before `ordo_primitive_orchestrator` when the system is not yet
understood.

Flow:

```text
1. Use this skill to decompose and reconstruct the underlying function.
2. Use `ordo_primitive_orchestrator` to turn the reconstructed primitive into a
   reusable Ordo capability kit.
3. Use `ordo_rust_architecture` when implementing the kit in Rust.
```

This skill discovers the primitive. The primitive/orchestrator skill packages
it for reuse.

## Output Modes

### Quick Decomposition

```text
Observed behavior:
Likely primitives:
Load-bearing invariant:
Unknowns:
Best reconstruction path:
Test:
```

### Full Primitive Reconstruction

```text
Observed surface:
Input/output model:
Variables:
Primitive map:
Equations/rules:
Invariants:
Replacement plan:
Verification plan:
Risks:
```

### Equivalence Review

```text
Original function:
Replacement function:
Matched primitives:
Changed primitives:
Equivalent:
Approximate:
Failed invariants:
Required fixes:
```

### Signal Analysis Note

```text
Signal scope:
Sampling assumptions:
Primitive components:
Transform/filter model:
Encoding hypothesis:
Noise/error model:
Reconstruction:
Verification:
Safety boundary:
```

## Quality Rules

Good reconstruction:

- identifies variables before equations
- separates observations from hypotheses
- preserves invariants
- keeps tolerances explicit
- tests edge cases
- uses simpler primitives when possible
- avoids copying incidental surface details

Bad reconstruction:

- matches appearance only
- invents hidden mechanisms without evidence
- ignores boundary cases
- treats labels as primitives
- claims exact equivalence without tests
- skips the unknowns

## Safety Constraints

Never:

- claim a primitive is original without evidence
- hide uncertainty
- use this method to bypass access controls
- assist unauthorized RF interception, decoding, spoofing, or jamming
- treat a surface clone as a functional reconstruction
- skip verification when the result affects engineering, safety, security, or
  data integrity

Prefer:

- lawful synthetic examples
- explicit assumptions
- testable equations
- reversible derivations
- independent verification
- conservative confidence labels

## Installation Metadata

```yaml
id: ordo_math_primitive_reconstruction
recommended_path: ordo-studio/user-files/skills/ordo_math_primitive_reconstruction/skill.md
category:
  - reasoning
  - mathematics
  - primitives
  - reconstruction
  - systems
risk_level: medium
requires_tools: false
persistent_memory_access: optional
available_to_modes:
  - coding
  - research
```
