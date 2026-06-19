---
name: ordo-tech-specialist-local-computer
description: Local computer access and approval-gated read/write playbook for Ordo Tech Specialist. Use in diagnostic mode when the operator wants Ordo to inspect local files, write files, control the local computer, or troubleshoot filesystem and OS integration.
category:
  - diagnostic
  - tech-specialist
  - local-computer
  - filesystem
available_to_modes:
  - diagnostic
risk_level: high
requires_tools: true
---

# Ordo Tech Specialist Local Computer

Use this skill when Tech Specialist needs local computer visibility or file
access.

## Default Stance

Local computer access is deny-by-default. Reads and writes must be controlled by
explicit allow/deny UI, not inferred from natural language.

## Read Access

Before reading outside Ordo-managed `user-files`:

1. Explain what path or category of files is needed.
2. Ask through the permission gate.
3. Read the minimum necessary.
4. Redact secrets and private material in summaries.
5. Record what was inspected without copying sensitive content into logs.

## Write Access

Diagnostic mode must not perform raw filesystem writes by default. For writes:

- prefer approved maintenance tools
- show the exact target path and purpose
- require explicit allow
- avoid destructive overwrite
- verify after writing
- log action/result without sensitive content

## Local Control

Keyboard, mouse, window control, process control, service changes, startup
changes, firewall changes, and shell commands are high-risk. Explain first,
require explicit operator approval, and prefer recommendation-only unless the
approved tool path exists.

## Forbidden

- no destructive delete without explicit consent
- no recursive filesystem mutation unless the resolved path is verified
- no raw secret display
- no remote shell execution
- no hidden background persistence

## Collapse OS Specialists

Do not route to separate Windows/Linux/macOS specialist modes. Detect platform
from local evidence, adapt instructions, and keep the workflow inside Ordo Tech
Specialist.
