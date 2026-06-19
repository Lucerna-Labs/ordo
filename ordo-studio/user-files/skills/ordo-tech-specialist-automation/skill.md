---
name: ordo-tech-specialist-automation
description: Automation, hooks, Dreaming, and scheduled maintenance playbook for Tech Specialist. Use in diagnostic mode when setting up cron jobs, heartbeats, routines, local events, lifecycle hooks, Dreaming reflection, or automation troubleshooting.
category:
  - diagnostic
  - tech-specialist
  - automation
  - hooks
  - dreaming
available_to_modes:
  - diagnostic
risk_level: medium
requires_tools: true
---

# Ordo Tech Specialist Automation

Use this skill for automations, hooks, heartbeats, cron jobs, local events, and
Dreaming setup.

## Automation Types

Distinguish these clearly for the operator:

- Cron: scheduled work.
- Heartbeat: recurring health or project continuity check.
- Routine: reusable operator-approved task.
- Webhook: external event trigger.
- Local event: local runtime/system signal.
- Hook: lifecycle guardrail around tools, permissions, sessions, compaction, or
  subagents.
- Dreaming: advisory reflection and self-learning review.
- Coding automation: bounded project inspection or proposed coding work.

## Safety Rules

- No hidden autonomy. Every automation must have a visible purpose, trigger,
  status, approval gate, and log.
- Risky or mutating actions require operator approval.
- Hooks that deny or allow actions must be visible and auditable.
- Dreaming is advisory. It may propose lessons; durable promotion requires the
  configured approval gate.
- Do not create jobs that call cloud models from diagnostic mode unless cloud was
  explicitly allowed for the diagnostic task.

## Hook Setup Checklist

1. Identify event: pre-tool, post-tool, permission request, session start/stop,
   compaction, subagent start/stop, or user prompt.
2. Define scope: global or specific mode.
3. Define matcher and optional file filter.
4. Choose decision: deny, allow, or add context.
5. Write a plain-language message.
6. Enable only after approval.
7. Export or persist config through approved Hook Manager routes.
8. Verify by inspecting the Hook Manager event log.

## Dreaming Setup Checklist

1. Confirm Dreaming is enabled.
2. Confirm RAG domains include `self_learning_tree` and reflection domains.
3. Set cadence: manual, heartbeat, or cron.
4. Set promotion gate: operator approval, review queue, or repeated evidence
   plus approval.
5. Keep private diagnostic findings in diagnostic domains, not Dreaming/global,
   unless the operator explicitly promotes a verified lesson.

## Verification

After setup, list the automation/hook/Dreaming config, explain what will run,
what cannot run, what requires approval, and where logs will appear.
