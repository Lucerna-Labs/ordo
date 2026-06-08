# Ordo User Guide

This guide explains how to use Ordo from the operator point of view.

## Start Ordo

Start the runtime:

```powershell
cargo run -p ordo-cli -- serve
```

Start the desktop UXI:

```powershell
cd ordo-studio
npm install
npm run tauri:dev
```

The runtime control API runs locally at:

```text
http://127.0.0.1:4141
```

## Main Tabs

## What Makes Ordo Different

Ordo is designed so the model is useful without being given uncontrolled hands.

Important concepts:

- **Gateway with fallback**: Ordo routes provider/model access through its own
  local gateway and fallback model/profile logic.
- **P2P and NAT/cloud connection layer**: Ordo is designed to connect directly
  to local devices, apps, NVRs, and companion services when possible, then fall
  back through ICE/STUN/TURN-style traversal or relay paths when needed.
- **Post-quantum handshakes**: direct device/app connections are planned around
  post-quantum handshake support, giving Ordo a safer foundation for local and
  hybrid network communication.
- **Encrypted secrets**: API keys and app credentials should be stored locally
  through the secrets/credential system, not typed into prompts.
- **Agent has no hands**: the assistant cannot directly operate the machine. It
  requests capability calls, and Ordo applies mode rules, hooks, review gates,
  and runtime policies.
- **Planner**: the Planner turns a request into structured work before tools
  run.
- **Strainer**: untrusted text and web/tool output can be normalized, stripped,
  classified, and taint-tracked to reduce prompt-injection risk.
- **Self-learning tree**: Dreaming, diagnostic findings, corrections, and
  approved lessons are treated as reviewable branches of memory.
- **Always-on diagnostic mode**: diagnostic work stays local-only and isolated
  from general assistant memory.
- **Cross-mode consultation**: one mode can ask another mode's agent for help
  without directly reading that mode's private RAG.
- **Swarm capability**: bounded subagents can participate in a task, but their
  work remains scoped and logged.
- **Rust Vibe Coder**: Rust project work gets its own coding mode and skill so
  architecture tracing, warning cleanup, tests, and public-release hygiene are
  handled consistently.

### Assistant

Use Assistant for normal conversation and task steering.

The assistant surface includes:

- mode selector
- workspace selector
- cross-mode consultation selector
- model selector
- thinking level selector
- context usage indicator
- chat input
- file/image/folder upload controls
- export controls
- stop/interrupt behavior while a task is running

Ordo's UXI is meant to keep controls visible. If a workflow can be paused,
tested, edited, deleted, refreshed, inspected, uploaded to, exported from, or
logged, the operator should be able to find that control in the relevant tab
without using a hidden CLI path.

### Provider

Use Provider to configure local and cloud models.

Ordo is designed to prefer:

- local provider detection
- OpenAI-compatible APIs
- environment-backed credentials
- custom provider templates

Local providers should not require users to paste API keys into Ordo.

For Ollama Cloud models, use the `Ollama Cloud Models` provider only after
signing in to Ollama locally and pulling/selecting a `*-cloud` model such as
`gpt-oss:120b-cloud`. This template intentionally talks to the local
OpenAI-compatible Ollama endpoint at `http://localhost:11434/v1`. Do not use
`https://ollama.com/api` in that template; that is Ollama's native cloud API
surface. The separate `Ollama Cloud API` provider talks directly to Ollama's
OpenAI-compatible cloud endpoint at `https://ollama.com/v1` using your
`OLLAMA_API_KEY`.

### Modes

Modes are scoped work environments. Each mode can have its own memory, RAG,
tool access, storage limits, and behavior.

Common mode types:

- General
- Rust Vibe Coder
- Coding
- Research
- Security
- Diagnostic
- Dreaming
- Writing
- Business

Modes can consult one another, but one mode should not directly read another
mode's private RAG.

### Hooks

Hooks are lifecycle guardrails. They can run before or after important events
such as tool use, permission requests, session starts, subagent starts, and
compaction.

Use hooks for rules like:

- deny risky file edits
- warn before dependency changes
- inject project-specific guidance
- enforce review boundaries

### Automation

Automation is where scheduled and event-driven Ordo work is managed.

Supported automation types:

- cron-style schedules
- heartbeats
- routines
- webhooks
- local events
- dreaming reviews
- diagnostic sweeps
- coding automation

Coding automation is approval-gated. It can inspect a project, propose fixes,
or prepare work plans. Risky writes, commits, and dependency changes require
operator approval.

### Dreaming

Dreaming is Ordo's self-learning review mode.

It reviews signals such as:

- failed tasks
- corrections
- rejected outputs
- logs
- completed work
- recurring issues

Dreaming proposes lessons, but does not silently change the system.

### Diagnostic

Diagnostic mode is for local-only system inspection.

It can inspect:

- runtime profile
- storage budgets
- logs
- MCP servers
- skills
- plugins
- automation state
- provider status
- memory/RAG health

When you ask it to, diagnostic mode can also maintain peripheral components
through approved tools. That includes installing, deleting, repairing, trusting,
quarantining, or re-authorizing MCP servers, skills, plugins, provider profiles,
and related integrations when those maintenance tools are available.

Diagnostic mode is not allowed to silently rewrite Ordo's core runtime,
security boundaries, hooks, or UXI. Those remain explicit operator-approved
engineering work.

Diagnostic knowledge is isolated from normal assistant work.

### Skills, Plugins, And MCP

These are separate surfaces:

- Skills are instruction/workflow packs.
- Plugins are installable provider packages.
- MCP servers are external tool servers with trust state.

Keep them separate to avoid tool catalog confusion.

Ordo treats MCP servers and plugins as untrusted until they are inspected and
graduated. The MCP surface uses research-informed defense-in-depth measures:
signed lockfiles, capability drift checks, trust states, quarantine,
re-authorization, sandboxed workers, provenance tracking, pre/post-call
scanning, redacted findings, and audit logs. In normal use, this means a new or
changed MCP should be reviewed before it is trusted.

### Review

Review is the human approval lane. Sensitive actions can require approval
before they complete.

### Settings

Settings holds general runtime, UI, connection, notification, hook, and local
configuration controls.

### Logs And Events

Ordo surfaces logs and events so operators can understand what happened during
a workflow. Logs should show user actions, capability calls, provider choices,
policy denials, retries, errors, and recovery information where relevant.

### Docs And Dev Docs

Docs are for operator instructions. Dev Docs are for architecture, build, and
extension guidance.

## Sessions

Ordo supports multiple sessions. The session selector lets you move between
new and older conversations.

Use a new session when switching projects or mode scopes.

## Workspaces

Workspace selection lets Ordo operate against a selected local project instead
of only its internal runtime memory.

Local workspaces should be sandboxed to the selected project folder.

## Exporting Work

Chat and session exports are intended for markdown summaries, work logs, and
handoff records.

## Voice

Ordo includes hooks for speech output through compatible providers. Voice
features require provider support and local configuration.

## Pre-Ship Health Check

Run the operator simulator to make sure Ordo is healthy:

```powershell
cargo run -p ordo-operator-sim -- --origin http://127.0.0.1:4141
```

The report will show whether runtime, modes, MCP, automation, sessions, and
assistant turns are working.
