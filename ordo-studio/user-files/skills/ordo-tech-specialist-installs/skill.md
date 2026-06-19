---
name: ordo-tech-specialist-installs
description: Installation and maintenance workflow for Tech Specialist. Use in diagnostic mode when installing, updating, disabling, repairing, or documenting Ordo skills, MCPs, plugins, apps, webhooks, Agent Teams, provider profiles, UI extensions, and manual setup paths.
category:
  - diagnostic
  - tech-specialist
  - installs
  - maintenance
available_to_modes:
  - diagnostic
risk_level: medium
requires_tools: true
---

# Ordo Tech Specialist Installs

Use this skill when the operator asks Tech Specialist to install, update, repair,
or explain Ordo capabilities.

## Authority Model

Only Tech Specialist should perform capability maintenance. General assistant,
agent teams, and ordinary modes may describe or request maintenance, but should
route the actual install/repair task here.

Maintenance includes:

- skills
- MCP servers
- plugins
- apps
- webhooks
- Agent Teams
- UI extensions
- provider profiles
- SSH/API descriptors
- automation and Dreaming setup

## Guided And Manual Paths

Offer two paths:

- Guided: Tech Specialist inspects, explains, prepares the repair, asks for
  approval, performs approved steps, verifies, and logs.
- Manual: show the operator the exact surface, fields, manifest shape, or
  command sequence so they can do it themselves.

Do not hide manual controls from users who prefer to manage their own setup.

## Safe Install Workflow

1. Clarify the target and source.
2. Inspect the current installed state.
3. Check compatibility and required credentials.
4. For MCP/plugins, run the MCP security checklist before enabling.
5. For skills, check `available_to_modes`, tags, risk, and routing audit.
6. For webhooks/apps, check scope, callback target, event type, and secret
   handling.
7. Ask for explicit approval before mutation.
8. Apply the smallest change.
9. Verify by listing the capability and, when safe, testing the non-destructive
   path.
10. Log symptom, evidence, action, result, verification, and follow-up.

## Skill Routing Repairs

Use `skills.audit_routing` to find:

- orphaned skills
- phantom mode names
- declared-but-vetoed contradictions
- undeclared skills relying on permissive defaults

Apply only safe skill-frontmatter repairs. Mode policy changes are deferred
operator decisions.

## Secret Handling

Secrets belong in the credential/vault flow or approved setup UI, never in chat,
logs, skill text, plugin manifests, or RAG notes.
