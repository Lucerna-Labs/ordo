# Memory Tree Mapping

This document maps Ordo memory/RAG areas to the current product direction.

## Active Tree Nodes

| Node | Purpose | Notes |
|---|---|---|
| `main` | Core Ordo docs and operator guidance | Keep compact. |
| `assistant` | Assistant turn-loop, skill-use, and behavior guidance | Used by normal chat. |
| `modes` | Mode-specific instructions and boundaries | General mode is default. |
| `skills` | Global and per-mode skill metadata | Supports scoped skill loading. |
| `agent_teams` | Team roles, role skills, and team setup | Must support Tech Specialist management. |
| `tech_specialist` | Diagnostics, installs, setup, and safety rules | Read-wide, write-narrow. |
| `automation` | Routines, hooks, cron, webhooks, dreaming reviews | Bounded and logged. |
| `remote_communication` | Email and future approved channels | SMS excluded. |
| `providers` | Resident cognition health and lifecycle | Provider switching is retired; preserve the node name for stored-data compatibility. |
| `memory` | Pinned/working memory and dreaming proposals | Operator-curated. |
| `security` | MCP/plugin trust, review, secrets, permissions | No secrets in memory. |
| `artifacts` | Files, PDFs, docs, spreadsheets, email views | Used by side artifact view. |

## Retired Nodes

The old `creative`, `seo`, and `cms` nodes are retired and should not be seeded
by default.

## Retrieval Defaults

- Store neutral source chunks in AtomDB's general RAG slice.
- Store feedback and learned query-cue bridges in the separate self-learning
  slice; never rewrite general facts from feedback.
- Use AtomDB's bounded multi-cue retrieval field when no embedding model is
  configured.
- Treat embedding-backed retrieval as an additional channel when available.
- Tell the user when retrieval is operating in fallback mode.
- Prefer focused collection hits over large mixed context.

## Memory Rules

- Pinned memory should be operator-manageable.
- Working memory should be prunable.
- Dreaming can propose lessons but should not silently rewrite durable truths.
- Tech Specialist can inspect memory health but should not expose secrets.
