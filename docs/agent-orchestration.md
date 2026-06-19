# Agent Teams

Agent Teams are Ordo's bounded multi-agent collaboration surface.

They let Ordo coordinate multiple roles for a task while keeping work scoped,
logged, interruptible, and visible to the operator.

## Goals

- Let the operator choose or configure a team.
- Let each role have its own instructions and relevant skills.
- Keep team activity visible in the Assistant surface.
- Support both local and cloud models.
- Scale down gracefully for small local models.
- Keep all work behind existing security, review, memory, and provider rules.

## Team Shape

A team can define:

- team id
- label
- purpose
- mode
- provider/model preference
- roles
- role instructions
- role-specific skills
- maximum rounds
- maximum parallel work
- review requirements

Common roles:

- lead
- planner
- builder
- researcher
- reviewer
- critic
- generalist

## Model Suitability

Agent Teams must account for model size and capability.

Small local models:

- fewer roles
- shorter instructions
- simpler tasks
- more deterministic checks
- more operator confirmation

Large local or flagship cloud models:

- richer teams
- deeper planning
- stronger critic/reviewer role
- broader context windows
- more complex multi-step tasks

## Role Skills

Each team role can have its own skills. This prevents every role from receiving
every skill and keeps small models from drowning in irrelevant instructions.

Examples:

- planner: planning and decomposition skills
- builder: implementation/build skills
- reviewer: test, safety, and quality-gate skills
- Tech Specialist: MCP, plugin, provider, automation, and local computer setup
  skills

## Visibility

When a team is active, the Assistant composer should show a clear visual
indicator. It should identify:

- that team agents are working
- the active team
- active roles
- whether the turn can be stopped or interrupted

This keeps multi-agent behavior from feeling invisible or surprising.

## Tech Specialist Management

Tech Specialist should be able to help users create, inspect, modify, and
troubleshoot Agent Teams through official backend capabilities.

The UXI can provide manual controls, but team configuration should not exist
only as front-end state.

## Safety

Agent Teams inherit Ordo's safety boundaries:

- no secrets in prompts
- local computer access denied by default
- explicit permission UI for local read/write
- MCP/plugin trust enforcement
- review gates for sensitive actions
- bounded rounds and tool calls
- logs for role activity and provider decisions

## Current Beta Notes

The Studio includes Agent Teams setup and chat-surface team activity
indicators. Backend capability coverage should continue expanding so Tech
Specialist can manage team setup without pretending it changed state when it
only guided the user.
