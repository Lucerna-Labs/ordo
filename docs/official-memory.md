# Official Memory

This document records concise Ordo truths that can be pinned or seeded into
memory/RAG.

## Product Truths

- Ordo is a local-first AI runtime and operator studio by Lucerna Labs.
- Ordo is not a SEO, CMS, or creative-workflow product in the current beta.
- The current desktop shell is embedded Servo, not Tauri.
- The supported beta launchers are `Launch-Ordo-Servo.ps1` and
  `Launch-Ordo-Servo.cmd`.
- Ordo serves the built Studio bundle from its own control API on localhost.
- Vite is a development convenience, not a runtime dependency.
- General mode is the default assistant mode on launch.
- Tech Specialist is the user-friendly maintenance mode for diagnostics,
  installs, provider setup, MCP/plugin upkeep, automation, Agent Teams, avatar
  setup, SSH/API-key setup, and local computer tasks with explicit approval.
- The general assistant should not install skills, MCPs, plugins, apps,
  webhooks, SSH keys, API keys, or local computer permissions.
- Local computer read/write access is denied by default.
- Permission should be granted through explicit allow/deny UI, not interpreted
  from natural language.
- Secrets must not be exposed to prompts, logs, screenshots, diagnostic exports,
  or model-visible memory.
- Agent Teams are bounded multi-agent configurations with role-specific
  instructions and skills.
- Agent Teams should work with local and cloud models, but smaller models need
  smaller teams and narrower tasks.
- Skills are scoped globally and per mode to keep prompts focused.
- RAG works with a hashing fallback when no embedding model is configured.
- An embedding model can improve retrieval quality when configured.
- Dreaming can propose lessons but should not silently rewrite durable memory.
- Remote Communication owns email and future approved channels such as Signal
  and Telegram. SMS is intentionally excluded.
- User email accounts should have access controls: none, read, or write.
- Artifact side view lets agents show files, PDFs, docs, spreadsheets, email
  views, and other artifacts without replacing chat.
- MCPs and plugins are untrusted until inspected and graduated.
- Logs should capture provider decisions, model lifecycle, tool calls, denials,
  retries, failures, recovery steps, automation, and Agent Team activity.

## Validation Truths

- The standard validation command is:

```powershell
.\Test-Ordo-Functions.ps1 -Suite standard -KeepGoing
```

- A release/push that touches runtime, Studio, Servo, providers, model
  lifecycle, or capability behavior should pass the standard suite.
