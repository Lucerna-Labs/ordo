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
| `providers` | Local/cloud model setup and lifecycle | Includes LM Studio and Ollama guidance. |
| `memory` | Pinned/working memory and dreaming proposals | Operator-curated. |
| `security` | MCP/plugin trust, review, secrets, permissions | No secrets in memory. |
| `artifacts` | Files, PDFs, docs, spreadsheets, email views | Used by side artifact view. |

## Retired Nodes

The old `creative`, `seo`, and `cms` nodes are retired and should not be seeded
by default.

## Retrieval Defaults

- Use hashing fallback if no embedding model is configured.
- Prefer embedding-backed retrieval when a model is available.
- Tell the user when retrieval is operating in fallback mode.
- Prefer focused collection hits over large mixed context.

## Memory Rules

- Pinned memory should be operator-manageable.
- Working memory should be prunable.
- Dreaming can propose lessons but should not silently rewrite durable truths.
- Tech Specialist can inspect memory health but should not expose secrets.
