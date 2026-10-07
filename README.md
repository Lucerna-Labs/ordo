# Ordo

> **Beta software — actively developed.** Ordo works but is still being refined.
> Features may change, things may break. If you hit a problem, please
> [open an issue](https://github.com/Lucerna-Labs/ordo/issues) — it helps me fix
> things faster. See [Feedback](#feedback) below.

Ordo is a local-first AI agent system by Lucerna Labs, a division of Lucerna
Media. This combined build is the new home of the Lightcone project.

It gives you a personal AI assistant that orchestrates tools, capabilities, MCP
servers, agent teams, memory, retrieval, and automation — all on your machine.
The model never gets uncontrolled access to your computer: it works through
scoped capabilities, policies, review gates, mode rules, and explicit operator
approval. Its one permanent language model is the local Bonsai 27B PQ2_0,
routed through the geometric Lightcone and Atom cognitive engine.

Think of it as your own private AI operations center: one resident cognitive
stack, persistent memory, scheduled automations, and a multi-agent workflow
engine — all behind a single operator interface you control. Ordo is no longer
a multi-LLM-provider product.

## Quick Start

Launch this combined build from its parent workspace:

```bash
cd /home/jesse/Desktop/Projects/Atom-lightcone-27b-harness
./launch-ordo.sh
```

The launcher validates the resident model and knowledge pack, builds the
resident cognition server when needed, and starts Ordo's Servo shell. The
control UI uses `http://127.0.0.1:4141`; the private cognition boundary uses
`http://127.0.0.1:4142`.

The generic upstream installers in `INSTALL.md` describe the original
standalone Ordo distribution. They do not contain this workspace's Bonsai
weights, Lightcone, or Atom cognitive engine and are not the launch path for
this combined build.

## Current Desktop Architecture

Ordo no longer uses the old Tauri desktop host. The previous `src-tauri`
workspace and stale Studio/Portable launchers were removed.

Current shape:

- `ordo-cli`: headless runtime binary.
- `../lightcone/target/release/ordo_server`: resident OpenAI-shaped cognition
  boundary used internally by Ordo; it is not a selectable provider.
- `ordo-control`: local HTTP API and static UI asset server.
- `ordo-studio`: React UXI built to `ordo-studio/dist`.
- `ordo-servo-shell`: custom Servo embedder with no address bar or tab strip.
- `Launch-Ordo-Servo.vbs` / `.cmd`: the supported no-console beta launch path.
- `Launch-Ordo-Servo.ps1`: visible diagnostic launcher for troubleshooting.
- `Launch-Ordo-Servo.sh`: Linux Servo launch path for Pop!_OS/Ubuntu-style
  systems and package internals.
- `Build-Ordo-Linux-Deb.sh`: builds the Pop!_OS/Ubuntu `.deb` package with a
  real app-menu launcher.
- `Install-Ordo-Linux.sh`: distro-aware Linux installer. It uses the Debian
  package path on Debian-family systems and falls back to AppImage elsewhere.
- `Install-Ordo-Linux-Deb.sh`: builds and installs the `.deb` with `dpkg -i`;
  it does not rely on `apt install ./file.deb`.
- `Build-Ordo-Linux-AppImage.sh`: builds a broad Linux AppImage from the same
  Servo runtime layout.
- `Build-Ordo-Linux-Portable.sh`: builds a portable Linux `.tar.gz` package for
  distros where native packaging is not ready yet.
- `bootstrap/ordo-windows-portable.zip`: compact runtime/UI payload used to make
  GitHub source ZIP downloads runnable.

Vite is only a development convenience. Servo needs an HTTP origin for module
scripts, and Ordo now provides that origin itself.

## What Ordo Does

- Runs a local AI assistant through a dedicated operator UXI.
- Uses one permanent local model: `ordo-bonsai-27b-lightcone`.
- Routes assistant, orchestration, logic, modes, teams, and avatar cognition
  through the same resident Lightcone + cognitive-engine path.
- Rejects provider, mode, avatar, and request-level attempts to replace the
  model; endpoint and model identity are pinned in memory at each call site.
- Keeps secrets out of prompts and model-visible state.
- Provides mode-scoped behavior, memory, tools, and skills.
- Starts in General mode by default.
- Supports global skills and per-mode skills so small models are not flooded
  with unrelated instructions.
- Supports Agent Teams for bounded multi-agent collaboration, with visible
  team-working indicators in the chat composer.
- Supports Ordo Tech Specialist for diagnostics, installs, MCP/plugin upkeep,
  resident-cognition health, automation setup, avatar setup, and local computer tasks when
  explicitly approved.
- Provides automation for routines, hooks, cron-style schedules, heartbeats,
  webhooks, local events, dreaming reviews, and bounded coding automation.
- Provides Remote Communication setup for email and future channels such as
  Signal and Telegram. SMS is intentionally excluded.
- Provides a side artifact view for generated files, PDFs, docs, spreadsheets,
  email views, and other agent-produced artifacts without replacing chat.
- Stores RAG in workspace-local AtomDB with two isolated slices: neutral
  general retrieval and bounded self-learning feedback. AtomDB's native
  multi-cue retrieval field emits bounded evidence without an embedding model;
  Ollama or llama.cpp embeddings remain an optional additional channel.
- Provides MCP security with trust states, quarantine, drift checks,
  provenance tracking, redaction, and audit logging.

## Operator Surfaces

The left rail is intentionally user-facing:

- Assistant
- Modes
- Review
- Agent Teams
- Skills
- Automation
- Tech Specialist
- Remote Communication
- Docs
- Dev Docs
- Settings

Common setup surfaces that should not clutter the rail live under Settings.
Tech Specialist can guide or perform setup through approved tools, but manual
controls remain available for users who prefer to configure things themselves.

## Important Safety Boundaries

- The general assistant should not install skills, MCPs, plugins, apps,
  webhooks, SSH keys, or API keys. Those tasks belong to Ordo Tech Specialist
  with explicit operator approval.
- Local computer read/write access is denied by default.
- Permissions should be granted by explicit allow/deny UI, not by interpreting
  natural language as consent.
- Secrets must not appear in logs, prompts, screenshots, diagnostic exports, or
  model-visible memory.
- MCPs and plugins are untrusted until inspected and graduated.
- Servo should render Ordo's local UI; it should not become a general internet
  browser.

## Validate

Run the standard function suite:

```powershell
.\Test-Ordo-Functions.ps1 -Suite standard -KeepGoing
```

The suite checks:

- tool availability
- `cargo fmt --check`
- core Rust crate checks
- runtime smoke test
- Studio production build
- Servo shell compile check
- runtime full harness

The Servo beta was pushed after a clean run:

```text
10 passed, 0 failed, 0 skipped
```

## Feedback

Ordo is in active beta. If something breaks or doesn't work the way you'd expect,
please report it — I can't fix what I don't know about.

**[→ Open an issue](https://github.com/Lucerna-Labs/ordo/issues)**

**What to report:**
- Crashes, errors, or unexpected behavior
- Install/launch problems (what OS, what install method)
- Missing or confusing UI
- Feature ideas

When reporting a bug, include:
1. What you did (steps to reproduce)
2. What happened vs. what you expected
3. Your OS and install method (`.deb`, source build, etc.)
4. Any error messages or log output (check `runtime-servo.err.log`)

**The more detail, the faster I can fix it.**

## Repository Map

- `ordo-cli`: runtime binary entrypoint.
- `ordo-runtime`: component boot and supervision.
- `ordo-control`: local API, static UI serving, and operator endpoints.
- `ordo-assistant`: assistant sessions, turn loop, memory, skills, and tool use.
- `ordo-cloud`: internal OpenAI-shaped client used only for the pinned resident
  cognition service in this build.
- `ordo-rag`: local retrieval with hash fallback and optional embeddings.
- `ordo-memory`: persistent working and pinned memory.
- `ordo-mcp-host`: capability providers and MCP-facing tool surface.
- `ordo-security`: scanning, policy, and gated provider execution.
- `ordo-studio`: React operator UXI.
- `ordo-servo-shell`: embedded Servo desktop shell.
- `docs`: architecture, user, security, API, memory, and beta notes.

## Build Notes

Developer Studio build:

```powershell
cd ordo-studio
npm install
npm run build
```

Core Rust check:

```powershell
cargo check -p ordo-cli -p ordo-runtime -p ordo-control -p ordo-assistant
```

Servo shell check:

```powershell
cargo check --manifest-path ordo-servo-shell\Cargo.toml --features servo-engine
```

## Lightweight Copies

For laptop copies, keep source files and the built `ordo-studio/dist` bundle,
but exclude generated or heavyweight folders:

- `.git`
- `target`
- `node_modules`
- `ordo-servo-shell/target`
- `bin/servo-nightly`
- crash dumps
- transient logs

See `README-BETA.md` for current beta notes.
