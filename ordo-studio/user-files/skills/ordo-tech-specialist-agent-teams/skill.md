---
name: ordo-tech-specialist-agent-teams
description: Agent Teams setup and repair playbook for Ordo Tech Specialist. Use in diagnostic mode when creating, modifying, resetting, troubleshooting, or explaining Agent Teams, team members, per-agent roles, per-agent skills, model choices, and small-model limits.
category:
  - diagnostic
  - tech-specialist
  - agent-teams
available_to_modes:
  - diagnostic
risk_level: medium
requires_tools: true
---

# Ordo Tech Specialist Agent Teams

Use this skill when the operator asks Tech Specialist to create, tune, repair,
or explain Agent Teams.

## Authority Model

Tech Specialist may set up and modify Agent Teams for users when the operator
approves the change. General assistant and ordinary modes may recommend a team
shape, but maintenance belongs here.

Agent Team setup includes:

- team name and purpose
- active/inactive team selection
- lead/planner/builder/reviewer/general member roles
- per-member model assignment
- per-member skill access
- team trigger conditions
- user-visible working indicators
- fallback behavior for small or local models

## Setup Workflow

1. Clarify the task the team should handle.
2. Pick the smallest useful team. Small local models should usually use fewer
   roles and simpler handoffs than flagship cloud models.
3. Assign each member one responsibility and only the skills needed for that
   responsibility.
4. Confirm whether the team should work with local models, cloud models, or
   both. Cloud model use in diagnostic mode still requires explicit approval.
5. Ask for operator approval before creating, editing, deleting, or resetting a
   team.
6. Verify the team appears in the Agent Teams surface and that the chatbox
   working indicator behaves clearly.
7. Log the final team shape, model assumptions, and any limits.

## Manual Path

Agent Teams currently live in the Studio surface. If no approved backend tool is
available for a change, guide the operator through the Agent Teams tab instead
of pretending the change was applied.

For manual setup, provide:

- team to select or create
- role names
- models to assign
- skills each member should have
- when to enable or disable the team
- what behavior to test with a harmless prompt

## Safety Rules

- Do not give every team member every skill.
- Do not let a team install MCPs, plugins, skills, apps, webhooks, or hooks.
  Route those maintenance actions back to Tech Specialist.
- Do not hide team activity. The user must be able to see when team agents are
  working.
- Keep diagnostic memory private. Agent Team lessons from diagnostic work stay
  in diagnostic domains unless the operator explicitly promotes them.
- Avoid complex delegation on very small local models. Prefer one lead plus one
  specialist, or a single-agent fallback.

## Verification

After setup or repair, confirm:

- selected team
- active members
- per-member model and skill access
- whether cloud access is disabled or explicitly allowed
- where the user sees the working indicator
- what simple prompt was used to test the team
