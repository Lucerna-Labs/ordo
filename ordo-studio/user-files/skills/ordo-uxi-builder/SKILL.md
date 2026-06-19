---
name: ordo-uxi-builder
description: Ordo UXI and user-friendly application design discipline. Use when building or modifying any Ordo tab, settings screen, app UXI, workflow surface, logs/events panel, controls, uploads, chat composer, provider setup, automation screen, mode UI, diagnostic UI, or when a coding agent needs to avoid bland developer interfaces and produce operator-friendly, fully surfaced controls with static snapshots and human-like usage verification.
category:
  - uxi
  - interface
  - coding
available_to_modes:
  - coding
  - rust_vibe_coder
  - diagnostic
risk_level: medium
requires_tools: true
---

# Ordo UXI Builder

Use this skill whenever work affects an application surface, even if the change
starts in backend code. Ordo treats the UXI as part of the feature contract.

## Core Rules

1. Build for operators, not for coders. The final screen should feel intentional,
   readable, and controllable, not like a default admin table or unfinished form.
2. Surface every meaningful control in the UXI. If a feature has toggles,
   install/delete/edit/pause actions, limits, trust state, logs, modes, providers,
   uploads, or routing choices, expose them in a grouped and understandable way.
3. Use the LIVE Ordo Studio UXI as the baseline for user-friendly Ordo design:
   the running shell `ordo-studio/src/OrdoShell.tsx` (41 tabs) and the rules in
   `ordo-studio/UXI_DEV_NOTES.md`. The old `ordo-studio/static-html-css/`
   snapshot is a stale legacy copy of the previous shell — do not use it as a
   baseline.
4. Keep one unified screen per tab. Avoid overlapping scroll regions, nested
   scroll traps, duplicate panes, hidden buttons, clipped text, and controls that
   require guessing.
5. Include exhaustive logs for every app or platform feature. Log user actions,
   capability requests, provider decisions, retries, denials, errors, security
   events, and persisted state changes where relevant.
6. Expose logs in a user-friendly place. A log file is not enough when operators
   need to understand why a workflow succeeded, failed, paused, or requested
   approval.
7. Create or maintain a static UXI snapshot for app-facing work. At minimum,
   point to Ordo's static snapshot and explain how the new surface follows it.
8. Verify like a human. Launch the app, drive realistic workflows, inspect the
   visible state, confirm logs/events, and check mobile or narrow layouts when
   applicable.
9. Do not claim completion until the app is launched and the operator can confirm
   the UXI.

## Ordo Visual Contract

- Use Ordo's dark operator shell by default with the existing warm accent, status
  dots, compact labels, solid panels, and restrained borders.
- Use cards for repeated entities and framed tools; do not build cards inside
  cards.
- Put primary navigation on the left and keep the active surface obvious.
- Use icons for common actions and concise labels for high-risk or unfamiliar
  actions.
- Keep settings and management views dense but calm: scan-friendly rows, grouped
  controls, obvious status, and no decorative filler.
- Make failure states visible and useful. Show what failed, why it failed when
  known, and what action the user can take next.

## Workflow

1. Identify the user workflow and the controls required to operate it.
2. Map the feature as:

```text
user intent -> visible controls -> capability call -> event/log record -> visible result
```

3. Compare the planned screen to the static Ordo snapshot before designing new
   styles.
4. Add or update UXI controls, empty states, loading states, error states, and
   logs together.
5. Run automated human-like usage tests or manually launch and inspect the app
   when automation is not available.
6. Record a milestone note with the snapshot/log/test evidence.

For detailed examples, read
`references/ordo-uxi-user-friendly.md`.
