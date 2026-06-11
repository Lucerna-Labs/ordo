# Ordo

Ordo is a local-first AI runtime and operator studio by Lucerna Labs (a division of Lucerna Media).

It is built around a Rust/Tokio message bus, explicit capability boundaries,
mode-scoped agents, local memory, retrieval, automation, and a desktop UXI for
operating the system without handing control to a remote platform.

Ordo is currently beta software.

## What Makes Ordo Different

Ordo is not just a chat UI around a model. Its unique pieces are the runtime
rules around the model:

- **Custom gateway with fallback**: Ordo routes local models, cloud models,
  OpenAI-compatible APIs, and custom providers through a local gateway layer
  with fallback profiles.
- **P2P and NAT/cloud connection layer**: Ordo is being built to connect
  directly to other local devices and apps through peer-to-peer paths, NAT
  traversal, ICE/STUN/TURN-style fallback, and cloud relay only when direct
  connection is not possible.
- **Post-quantum handshakes**: device and app connection setup is designed
  around post-quantum handshake support so Ordo's direct communication layer can
  evolve beyond ordinary API-key exchange and legacy TLS assumptions.
- **Direct app/device communication**: Ordo's connection model is designed for
  local apps, NVRs, devices, and companion services to talk to Ordo directly
  without forcing every integration through a public SaaS API.
- **Encrypted secrets store**: provider keys, tokens, and connection secrets
  belong in the local secrets system, not in prompts or model-visible state.
- **Agent has no hands**: the model does not directly control the computer.
  It asks through explicit capabilities, review gates, hooks, and runtime
  policies.
- **Research-informed MCP security**: MCP servers and plugins are treated as
  untrusted capability providers by default, with modern defense-in-depth
  measures such as signed lockfiles, trust graduation, quarantine,
  re-authorization on drift, sandboxed workers, provenance tracking,
  pre/post-call scanning, redaction, and audit logs.
- **Planner-first execution**: the Planner decomposes work into bounded steps
  before tools run, instead of letting raw model output freely mutate state.
- **Prompt-injection strainer**: untrusted text can pass through structural
  normalization, stripping, classification, and taint tracking before it reaches
  sensitive workflows.
- **Self-learning tree**: memory, dreaming, diagnostics, corrections, and
  approved lessons are organized as reviewable learning branches rather than
  one uncontrolled memory pile.
- **Always-on diagnostic mode**: Ordo has a local-only diagnostic mode that can
  inspect runtime health and integration state while keeping its knowledge
  isolated from normal assistant work.
- **Mode containment**: modes have separate memory scopes, RAG domains, tool
  access, and policies.
- **Cross-mode consultation**: a mode can consult another mode's agent for
  expertise without directly reading that mode's private RAG.
- **Swarm-ready agent model**: Ordo can represent bounded subagents and
  multi-agent work while keeping each run logged, scoped, and approval-gated.
- **Operator-first UXI contract**: important controls must be surfaced in the
  desktop UXI with readable state, logs, recovery actions, and a static snapshot
  reference, avoiding hidden-only configuration and bland developer screens.
- **Exhaustive event logging**: app and platform workflows are expected to log
  user actions, capability calls, provider decisions, policy gates, retries,
  failures, and recovery paths.
- **Rust Vibe Coder**: Ordo includes a Rust-first coding mode and skill for
  architecture tracing, warning-free Cargo checks, approval-gated writes, and
  public-release hygiene.

## What Ordo Does

- Runs a local AI assistant through a dedicated operator UXI.
- Supports mode-based workspaces, including general, coding, research,
  security, diagnostic, dreaming, and other scoped modes.
- Keeps mode memory and RAG scoped so one mode does not silently contaminate
  another.
- Connects to local models through Ollama and LM Studio-style OpenAI-compatible
  APIs.
- Supports cloud/provider connections through local environment variables or
  the local credential vault.
- Manages MCP servers, plugins, skills, apps, files, webhooks, connections, and
  review queues from the local control API.
- Provides scheduled automation, heartbeats, dreaming reviews, and bounded
  coding automation.
- Includes an always-local diagnostic mode for inspecting runtime health,
  integrations, logs, MCP servers, settings, and storage.
- Lets diagnostic mode install and maintain peripheral components such as MCP
  servers, skills, plugins, provider profiles, and related integrations on
  behalf of the user when explicitly requested and when approved maintenance
  tools are available.
- Includes an operator simulator that behaves like a user and produces a
  pre-ship health report.
- Keeps app controls, runtime state, logs, and recovery actions surfaced in the
  UXI instead of leaving core features hidden in config files or CLI-only paths.
- Supports chat session management, export, context usage indicators,
  compaction signals, interrupt/stop controls, file upload surfaces, and
  speech output hooks.

## Core Features

### Local Operator Studio

`ordo-studio` is the desktop UXI. It gives operators tabs for:

- Assistant
- Avatar
- Provider setup
- Modes
- Hooks
- Automation
- Dreaming
- Diagnostic
- Skills
- Plugins
- MCP
- Review
- Settings
- Docs and Dev Docs

The UXI talks to the local Ordo control API instead of bypassing the runtime.

### Avatar

Ordo includes an optional **talking companion avatar** — a second assistant you
talk to by voice, rendered as an animated character in the Studio's Avatar tab
or in its own resizable pop-out window.

- **Voice to voice.** Voice-activated (energy-based VAD, starts muted): you
  speak, she listens, works on your answer, and replies aloud. Speech-to-text
  and text-to-speech are provider-agnostic — the browser voice is the
  zero-config default, with OpenAI-compatible and MiniMax endpoints pluggable,
  local or cloud.
- **State-driven presence.** She is a set of looping behavior clips switched by
  what is happening: idle (working at her desk / watching you), listening when
  you speak, "researching" while she works on your answer, speaking when she
  replies — plus emotional reactions: pleased when you thank her, annoyed on a
  failure, rudeness, or a repeated question, and a brief "found it" beat the
  moment she has an answer. Clips are swappable on disk; the renderer never
  hard-codes the character.
- **Its own brain, shared mind.** The avatar can run on its **own model** (a
  local Ollama/llama.cpp server or a cloud endpoint) concurrently with the main
  assistant, while sharing the same memory, RAG, skills, and modes — so it works
  alongside you on a spare monitor without competing with the generalist.
- **Customizable.** Edit her persona (name, tone, spoken style), scope her
  skills (tool lanes), and preview her appearance from the Avatar tab. The
  avatar is its own protected mode, kept out of the chat mode picker.

### Modes

Modes are scoped operating profiles. A mode can have its own:

- visible memory scopes
- RAG domains
- allowed tools
- blocked tools
- planner bias
- persona/instruction layer
- storage budget controls

Modes can consult other modes through explicit cross-mode consultation without
directly reading the other mode's private RAG.

### Automation

Ordo has a primitive plus orchestrator automation model.

Supported automation shapes include:

- manual routines
- cron-style schedules
- heartbeats
- local events
- webhooks
- dreaming reviews
- diagnostic sweeps
- coding automation

Coding automation is deliberately guarded. It carries workspace, mode, goal,
subagent limit, write policy, commit policy, dependency policy, and risk level.
Core mutation is denied as an automation action.

### Dreaming

Dreaming is an advisory self-learning mode. It reviews completed work, failures,
corrections, logs, and recurring patterns, then proposes lessons for operator
approval. It does not silently rewrite Ordo or promote lessons without a gate.

### Diagnostic Mode

Diagnostic mode is designed for local-only runtime inspection. It can inspect
runtime profile, settings, storage, MCP inventory, logs, automation state, and
integration health through approved diagnostic and maintenance tools.

When the user requests it, diagnostic mode can also use approved maintenance
tools to install, remove, repair, trust, quarantine, or re-authorize peripheral
components such as MCP servers, skills, plugins, provider profiles, and related
integrations. It is not a bypass for core runtime, security, hook, or UXI
mutation.

Diagnostic memory is isolated from general chat and other modes.

### MCP, Plugins, And Skills

Ordo separates these concepts:

- MCP servers are external tool servers with trust state, lockfiles, and tool
  catalogs.
- Plugins are installable provider packages.
- Skills are instruction and workflow packs.

The UXI keeps these surfaces separate so the operator can manage them without
catalog confusion.

### Providers And Models

Ordo is local-first but not local-only.

Provider support includes:

- Ollama local
- Ollama Cloud models through signed-in local Ollama using `*-cloud` model names
- LM Studio local
- OpenAI-compatible APIs
- environment-backed credentials
- custom provider templates
- cloud fallback profiles

The default direction is to avoid requiring users to paste keys into Ordo when
the provider can be reached through local environment configuration.

The `Ollama Cloud Models` template uses local Ollama's OpenAI-compatible
`http://localhost:11434/v1` surface after the operator signs in to Ollama and
selects a cloud model. The separate `Ollama Cloud API` provider talks directly
to Ollama's OpenAI-compatible cloud endpoint at `https://ollama.com/v1` with an
`OLLAMA_API_KEY`; the native `https://ollama.com/api` surface is a different,
non-OpenAI shape and is not used for that provider.

### Memory And RAG

Ordo uses local SQLite-backed memory and retrieval:

- working memory
- pinned memory
- mode-scoped memory
- RAG collections
- storage budgets
- retrieval previews
- self-heal history

The retrieval layer has a deterministic hashing embedder by default and optional
model-backed embeddings.

### Security

Ordo's security model is based on capability boundaries, explicit tools,
review gates, trust states, and local-first storage.

Security-related components include:

- classifier-gated providers
- human review queues
- MCP sandboxing
- signed MCP lockfiles
- trust graduation
- drift detection and re-authorization for changed MCP capabilities
- quarantine state for suspicious or unverified MCP servers
- provenance records for MCP tool catalogs and invocations
- pre-call and post-call scanning around MCP/plugin payloads
- secrets vault and broker
- audit trail support
- redacted findings so security reports do not leak matched secrets
- taint tracking for untrusted tool output

### Operator Simulator

`ordo-operator-sim` is a pre-ship simulator. It drives the live local API and
checks the surfaces users depend on:

- health
- runtime profile
- storage
- capabilities
- modes
- sessions
- MCP
- plugins
- skills
- automations
- security
- review
- files
- apps
- connections
- assistant turn

Run it with:

```powershell
cargo run -p ordo-operator-sim -- --origin http://127.0.0.1:4141
```

Reports are written to:

```text
target/operator-sim/operator-sim-report.json
target/operator-sim/operator-sim-report.md
```

### Rust Vibe Coder Preflight

`scripts/ordo-preflight.ps1` extends the pre-ship check for Rust Vibe Coder.
It verifies the mode, required skills, persistent memory anchors, and then
generates a tiny Rust app that must pass `cargo check`, `cargo test`, and
`cargo clippy` with warnings denied.

Run it against a live Ordo runtime:

```powershell
.\scripts\ordo-preflight.ps1 -Origin http://127.0.0.1:4141
```

Use `-SkipCoderTurn` when you want to test the harness and generated lite app
without spending a model turn. Use `-Strict` for release gating.

## Repository Layout

- `ordo-runtime` - runtime boot and lifecycle wiring
- `ordo-control` - local HTTP control API
- `ordo-studio` - Tauri desktop UXI
- `ordo-assistant` - assistant turn loop, sessions, tool use, memory/RAG use
- `ordo-modes` - mode manifests and registry
- `ordo-automation-primitives` - automation data model and validation
- `ordo-automation` - automation orchestrator
- `ordo-jobs` - scheduler primitives
- `ordo-agents` - agent/subagent primitives
- `ordo-operator-sim` - pre-ship operator simulator
- `ordo-mcp-*` - MCP host, registry, client, sandbox, worker, provenance
- `ordo-plugins` - plugin manifest and stdio provider system
- `ordo-cloud` - OpenAI/Anthropic/OpenAI-compatible cloud boundary, including provider-agnostic voice (STT/TTS)
- `ordo-avatar`, `ordo-tts` - talking-companion avatar driver (behavior state) and phoneme/TTS support
- `ordo-connections` - configured external connection records
- `ordo-memory-*` - hierarchical memory log, router, and projection
- `ordo-rag` - retrieval indexing and search
- `ordo-security` - security classifier, policy, and audit ring
- `ordo-secrets-*` - local secret vault, broker, threshold, and audit crates
- `ordo-files`, `ordo-apps`, `ordo-webhooks` - user file, app, and webhook primitives

## Getting Started

### Requirements

- Rust stable
- Node.js and npm
- Tauri prerequisites for your OS

### Run The Runtime

```powershell
cargo run -p ordo-cli -- serve
```

Default local control API:

```text
http://127.0.0.1:4141
```

Health endpoint:

```text
http://127.0.0.1:4141/health
```

The talking-companion avatar driver is gated behind `ORDO_ENABLE_AVATAR=1`
(the desktop launcher sets this for you).

### Run The Studio

```powershell
cd ordo-studio
npm install
npm run tauri:dev
```

### Run Checks

```powershell
$env:RUSTFLAGS='-D warnings'
cargo check --workspace
cargo test --workspace
```

Studio build:

```powershell
cd ordo-studio
npm run build
```

End-to-end harnesses (Python, stdlib only — each self-launches a runtime on
`127.0.0.1:4142` with a mock provider and tears it down):

```powershell
python scripts/ordo_full_test.py     # comprehensive: every subsystem, one PASS/WARN/FAIL verdict
python scripts/ordo_avatar_test.py   # avatar + provider-agnostic voice
```

Operator simulator:

```powershell
cargo run -p ordo-operator-sim -- --origin http://127.0.0.1:4141
```

## Documentation

- [User Guide](docs/user-guide.md)
- [Developer Guide](docs/developer-guide.md)
- [Control API](docs/control-api.md)
- [Architecture](docs/architecture.md)
- [Security](docs/security.md)
- [Plugins](docs/plugins.md)
- [Operator Simulator](docs/operator-simulator.md)

## Status

Ordo is under active beta development. The repo is useful for review,
experimentation, and continued development, but APIs and UXI surfaces may still
change.

Do not commit local runtime databases, user files, secrets, build output, model
files, or personal skills into the public repository.
