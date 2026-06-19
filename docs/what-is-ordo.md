# What Is Ordo?

Ordo is a local-first AI runtime and operator studio. It gives a user a local
assistant that can work through approved capabilities, persistent memory,
retrieval, automation, model providers, and agent teams without handing
unbounded control to a remote platform.

## Who It Is For

Ordo is for users who want a capable AI operator on their own machine:

- everyday users who want a friendly assistant with visible controls
- technical users who want local models, cloud models, automation, and logs
- non-technical users who need a Tech Specialist mode to install, configure,
  diagnose, and troubleshoot Ordo safely
- builders who want a bus-first local runtime they can extend

Ordo is not currently a SEO, CMS, or creative-workflow product. Those old lanes
were removed from the active direction.

## What Makes It Different

- Local-first runtime with local control API.
- Embedded Servo desktop shell instead of a browser tab or Tauri shell.
- Provider layer for local and cloud models.
- Model lifecycle protection for switching between local providers.
- Mode-scoped instructions, memory, tools, and skills.
- Global and per-mode skills.
- Agent Teams with visible activity and role-specific skills.
- Tech Specialist mode for diagnostics, setup, installs, MCP/plugin upkeep,
  automation, provider setup, local computer tasks, and avatar setup.
- Remote Communication setup for email and future approved channels.
- Artifact side view for files, PDFs, docs, spreadsheets, email views, and
  generated outputs.
- RAG and persistent memory with hashing fallback when no embedding model is
  present.
- MCP security and audit logging.
- Explicit user approval for sensitive work.

## Core Runtime

At a high level:

- `ordo-cli` starts Ordo.
- `ordo-runtime` boots the peers.
- `ordo-bus` moves typed events between components.
- `ordo-control` exposes the local API and serves the Studio bundle.
- `ordo-studio` provides the React UXI.
- `ordo-servo-shell` renders the Studio bundle in a custom app window.
- `ordo-assistant` runs assistant sessions and tool loops.
- `ordo-cloud` talks to providers and manages model lifecycle behavior.
- `ordo-rag` and `ordo-memory` provide retrieval and durable memory.
- `ordo-mcp-host`, `ordo-security`, and `ordo-review` keep tools gated,
  inspectable, and approval-aware.

## Local-First Boundary

Local-first does not mean local-only. Ordo can use local models and cloud
models, but the boundary should be explicit:

- provider configuration is visible
- secrets are stored outside model-visible state
- outbound calls are routed through provider capabilities
- model switching is logged
- local computer access is denied by default
- sensitive actions require explicit approval

## Agent Teams

Agent Teams let Ordo split work across roles such as planner, builder,
researcher, reviewer, or critic. Teams must stay bounded and visible. Smaller
models should get smaller, simpler teams; larger models can handle richer team
plans.

Each role can have its own skills and instructions.

## Tech Specialist

Tech Specialist is the mode users can ask for help when they do not know how to
set something up.

It can guide or perform approved maintenance tasks such as:

- installing or repairing MCP servers
- managing plugins, skills, apps, and webhooks
- setting up automation and hooks
- configuring providers and models
- configuring Agent Teams
- configuring the Avatar surface
- setting up SSH and API keys through secure UI/vault paths
- reading or writing local files only after explicit allow/deny permission

Tech Specialist should not bypass secrets policy or permission UI.

## Memory And RAG

Ordo uses memory and retrieval to avoid starting from scratch every turn.

If no embedding model is available, Ordo uses a hashing fallback so retrieval
still works. If an embedding model is configured, Ordo can use it for better
retrieval quality and should tell the user when that upgrade is available.

Dreaming can propose lessons from logs, corrections, and recurring issues, but
the operator remains in control of durable memory.

## Remote Communication

Remote Communication is the surface for email and future approved channels.

User email accounts should expose simple access choices:

- none
- read
- write

Ordo can also have its own communication identity for remote communication if
the operator chooses to configure one. Signal and Telegram are planned
directions. SMS is intentionally excluded.

## Current Validation

The standard validation command is:

```powershell
.\Test-Ordo-Functions.ps1 -Suite standard -KeepGoing
```

This is the expected pre-push check for beta work that touches runtime, Studio,
Servo, model/provider behavior, or control API behavior.
