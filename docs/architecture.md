# Ordo Architecture

Ordo is a local-first AI runtime with a desktop operator UXI. The runtime is
Rust/Tokio, bus-first, and capability-scoped. The desktop app is now rendered by
a custom embedded Servo shell pointed at Ordo's own localhost server.

## Product Direction

Ordo is not a creative-ops, SEO, or CMS workflow product. Those older lanes were
abandoned and should not be rebuilt unless a future operator decision explicitly
revives them.

Current direction:

- Local-first assistant runtime.
- Mode-scoped agents.
- Agent Teams for bounded multi-agent work.
- Persistent memory and RAG.
- Provider/model management for local and cloud models.
- Ordo Tech Specialist for user-friendly diagnostics and setup help.
- Automation, hooks, routines, webhooks, dreaming reviews, and bounded coding
  automation.
- Remote communication setup for email and future approved channels.
- MCP, plugin, skill, app, webhook, SSH, API-key, and local computer access
  maintenance behind explicit operator approval.

## Runtime Shape

- `ordo-cli` boots the runtime.
- `ordo-runtime` supervises component startup and shutdown.
- `ordo-bus` is the shared event fabric.
- `ordo-protocol` owns envelopes, topics, typed messages, IDs, and route types.
- `ordo-control` exposes the local HTTP API and serves the built Studio bundle.
- `ordo-studio` is the React UXI.
- `ordo-servo-shell` embeds Servo as a library and renders the Ordo UI without
  browser chrome.
- `ordo-assistant` owns sessions, turns, memory recall, skill routing, tool use,
  subagent consultation, and model-facing prompt assembly.
- `ordo-cloud` owns provider calls and model lifecycle handling.
- `ordo-rag` indexes local documents and answers retrieval requests.
- `ordo-memory` owns working and pinned memory.
- `ordo-mcp-host` advertises and executes capability providers.
- `ordo-security` gates provider calls with scanning and policy.
- `ordo-review` owns human approval queues.
- `ordo-jobs`, `ordo-automation`, and related crates own scheduled and
  event-driven work.

## Desktop Rendering

Servo does not load the Studio bundle from `file://`. Ordo serves the built
frontend over localhost:

- `GET /`
- `GET /index.html`
- `GET /assets/*`
- `GET /api/*`

The Servo shell opens:

```text
http://127.0.0.1:4141/
```

The old Tauri shell and `src-tauri` tree are gone. Vite is only a development
tool, not a runtime dependency.

## Model Lifecycle

Provider switching must be explicit and bounded:

- Only one active local model/provider path should be considered current.
- Switching from LM Studio to Ollama, or from Ollama to LM Studio, should eject
  the previous active model before selecting the next one.
- Cloud and local providers share the same high-level model-choice surface, but
  local unload/eject behavior only applies where the provider supports it.
- Model lifecycle actions should be logged.
- Small local models should receive narrower instructions and simpler Agent
  Team structures than flagship cloud or large local models.

## Memory And Retrieval

Ordo has three relevant memory lanes:

- Assistant/session memory for conversational continuity.
- Working and pinned memory for durable operator facts.
- RAG collections for local documents and self-knowledge.

RAG must work without shipping an embedding model. The default fallback is a
hashing embedder. If an embedding model is configured, Ordo can use it for
better retrieval quality and should make that improvement visible to the user.

Persistent memory must stay operator-manageable. Dreaming and diagnostics can
propose lessons, but they should not silently rewrite durable truths.

## Skills And Modes

Skills are intentionally scoped:

- Global skills are available broadly.
- Mode skills are only loaded for relevant modes.
- Tech Specialist skills describe maintenance capabilities and safety rules.
- Agent Team skills describe how to configure and modify teams.

General mode should be the default mode on launch.

The general assistant should not be responsible for installing or modifying
skills, MCP servers, plugins, apps, webhooks, SSH keys, or API keys. Those are
Tech Specialist responsibilities with explicit operator approval.

## Agent Teams

Agent Teams are Ordo's bounded multi-agent surface.

Design rules:

- Teams can be used with local or cloud models.
- Team size and role complexity should fit the selected model.
- Each team role should have its own instructions and allowed skills.
- Team activity should be visible in the assistant composer or its surrounding
  status area.
- Team work must remain logged, bounded, and interruptible.

Direct backend editing of agent-team configuration is expected to live behind
official capabilities. The UXI should not be the only place where team state
exists.

## Tech Specialist

Ordo Tech Specialist replaces the old scattered operating-system specialist
idea. It is the user-friendly maintenance agent for non-technical users and
technical users who want setup help.

It may help with:

- diagnostics
- logs
- provider setup
- model setup
- skills
- MCP servers
- plugins
- apps
- webhooks
- automation and hooks
- Agent Teams
- avatar setup
- SSH keys
- API keys
- local computer file read/write, when explicitly allowed

It must not see secrets directly. Secret entry, reveal, and storage remain
behind dedicated UI and vault paths.

## Remote Communication

Remote Communication owns user-controlled external communication surfaces.

Current direction:

- Email client support for one or more user-selected providers.
- Per-email-account access levels: none, read, or write.
- A separate Ordo-controlled remote communication identity can be configured
  when the operator wants Ordo to receive remote instructions.
- Signal and Telegram are future approved channels.
- SMS is intentionally excluded.

## Artifact Side View

Agents need a way to show artifacts without replacing chat. The artifact side
view can display generated documents, PDFs, spreadsheets, email views, saved
files, and other user-visible outputs while the conversation stays available
for modification instructions.

## Invariants

- Cross-component communication happens over the bus or official control
  endpoints, not hidden shared state.
- The local control API is a thin operator surface over the runtime, not a
  second orchestration system.
- Secrets do not enter prompts, logs, model-visible memory, or screenshots.
- Local computer access is denied by default.
- Permissions should use explicit allow/deny UI.
- MCPs and plugins are untrusted until inspected and graduated.
- Runs and automations must be logged, bounded, and recoverable.
- Retrieval, memory, provider decisions, model switching, tool calls, denials,
  retries, failures, and recovery actions should be visible in logs.
- Old unused code paths should be removed instead of kept as confusing
  fallback surfaces.

## Validation

The standard validation entrypoint is:

```powershell
.\Test-Ordo-Functions.ps1 -Suite standard -KeepGoing
```

It should pass before public pushes that affect runtime, UXI, Servo, or
capability behavior.
