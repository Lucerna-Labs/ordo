# Ordo Servo Beta

Lucerna Labs, a division of Lucerna Media.

Ordo is now a local-first AI runtime with a self-rendered Servo desktop shell.
The Rust runtime serves the Studio bundle and the control API from
`127.0.0.1:4141`; the embedded Servo shell opens that local URL without browser
tabs, an address bar, or a separate web-server dependency.

## Current Beta Shape

- Runtime: `ordo-cli`, booted as the local Ordo process.
- UI: `ordo-studio`, built to `ordo-studio/dist`.
- Renderer: `ordo-servo-shell`, a custom Servo embedder with a raw app window.
- Local server: `ordo-control`, serving `/`, `/index.html`, `/assets/*`, and
  `/api/*` from the same localhost origin.
- Launcher: `Launch-Ordo-Servo.ps1` or `Launch-Ordo-Servo.cmd`.
- Removed runtime dependency: Tauri. The old `ordo-studio/src-tauri` tree and
  old Studio/Portable launchers have been removed.

## Run On Windows

```powershell
git clone https://github.com/Lucerna-Labs/ordo
cd ordo
.\Launch-Ordo-Servo.ps1
```

The launcher:

1. Builds the Studio bundle.
2. Starts the Ordo runtime.
3. Waits for the runtime health check.
4. Serves the Studio bundle from Ordo's own localhost server.
5. Opens the embedded Servo shell.

Use `Launch-Ordo-Servo.cmd` when double-clicking from Explorer.

## What This Beta Includes

- Servo self-rendering app window with no browser chrome.
- Ordo-owned localhost asset serving; Vite is only a development convenience.
- Provider management for local and cloud models.
- Model lifecycle protection so switching providers unloads/ejects the previous
  active local model before the next model is selected.
- LM Studio and Ollama-compatible local model paths.
- Cloud and OpenAI-compatible provider paths.
- Agent Teams with per-team role setup and visible team-working indicators in
  the chat composer.
- Ordo Tech Specialist mode for user-friendly diagnostics, installs, MCP/plugin
  maintenance, automation setup, provider setup, avatar setup, and local
  computer tasks when the operator explicitly allows access.
- Skills separated by global and mode-specific scope so small models are not
  flooded with irrelevant instructions.
- Automation surface for routines, hooks, cron-style jobs, heartbeats,
  webhooks, local events, dreaming reviews, and bounded coding automation.
- Remote Communication surface for email and future operator-controlled
  channels such as Signal and Telegram. SMS is intentionally not included.
- Artifact side view so agents can show generated files, PDFs, docs,
  spreadsheets, email views, and other artifacts without replacing the chat.
- RAG and persistent memory with hashing fallback when no embedding model is
  configured; embedding models can still be used for better retrieval quality.
- MCP security posture with trust states, quarantine, drift checks,
  provenance, redaction, and audit logging.
- A single standard validation script: `Test-Ordo-Functions.ps1`.

## Lightweight Copy Rules

For laptop or portable source copies, keep the source and built Studio bundle
but exclude heavyweight/generated folders:

- `.git`
- `target`
- `node_modules`
- `ordo-servo-shell/target`
- `bin/servo-nightly`
- crash dumps and transient logs

The current local laptop copy made during beta prep is named `Jesse--Beta`.

## Validate The Build

```powershell
.\Test-Ordo-Functions.ps1 -Suite standard -KeepGoing
```

The standard suite checks:

- required tools
- Rust formatting
- core Rust crates
- runtime smoke test
- Studio production build
- Servo shell compile check
- runtime full harness

The beta commit shipped after a clean standard run:

```text
10 passed, 0 failed, 0 skipped
```

## Important Boundaries

- Servo should load only Ordo's local URL. It is not meant to be a general web
  browser.
- Secrets stay out of prompts, logs, and model-visible state.
- The general assistant should not install skills, MCPs, plugins, apps, or
  webhooks. That belongs to Ordo Tech Specialist with explicit operator
  approval.
- Local computer read/write access is denied by default and should be granted
  through explicit allow/deny UI, not natural-language interpretation.
- Agent Teams should work with both local and cloud models, but smaller local
  models need narrower tasks and simpler team structures than flagship models.
