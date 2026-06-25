# Ordo

> **Beta software — actively developed.** Ordo works but is still being refined.
> Features may change, things may break. If you hit a problem, please
> [open an issue](https://github.com/Lucerna-Labs/ordo/issues) — it helps me fix
> things faster. See [What to report](#feedback) below.

Ordo is a local-first AI runtime and operator studio by Lucerna Labs, a
division of Lucerna Media.

It combines a Rust/Tokio runtime, a localhost control API, a React UXI, and a
custom embedded Servo shell. The model does not get uncontrolled hands on the
computer: it works through scoped capabilities, policies, logs, review gates,
mode rules, and explicit operator approval.

## Quick Start

For the full install guide with troubleshooting, see [INSTALL.md](INSTALL.md).

### Windows

**Prerequisites:** [Git](https://git-scm.com), [Node.js 20+](https://nodejs.org), [Rust 1.93+](https://rustup.rs).

```powershell
cd $env:USERPROFILE\Desktop
git clone https://github.com/Lucerna-Labs/ordo.git
cd ordo
.\Install-Desktop-Shortcut.cmd
```

Then open **Ordo** from the desktop shortcut.

The first launch builds the Studio UI and Servo shell (5–15 minutes).
Subsequent launches are fast.

To update an existing clone:

```powershell
git pull
.\Install-Desktop-Shortcut.cmd
```

To launch once without installing the shortcut:

```powershell
wscript.exe .\Launch-Ordo-Servo.vbs
```

For troubleshooting with a visible console:

```powershell
.\Launch-Ordo-Servo.ps1
```

Default local runtime URL:

```
http://127.0.0.1:4141
```

### Linux (Pop!_OS / Ubuntu / Debian)

**Option 1 — Prebuilt `.deb`** (recommended, no compiler needed):

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-prebuilt.sh | bash
```

Then open **Ordo** from the app menu.

**Option 2 — Build from source** (developers; compiles Servo + SpiderMonkey, 30–60+ min):

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | bash
```

The installer clones to `~/ordo`, installs all build prerequisites
(Rust, Node, C/C++ toolchain, GL/X11/Wayland headers), builds, and
installs the `.deb` or AppImage.

## Current Desktop Architecture

Ordo no longer uses the old Tauri desktop host. The previous `src-tauri`
workspace and stale Studio/Portable launchers were removed.

Current shape:

- `ordo-cli`: headless runtime binary.
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
- Supports local and cloud model providers through Ordo's provider layer.
- Protects model switching by unloading/ejecting the previous active local
  model before selecting a new one.
- Supports Ollama, LM Studio, Ollama Cloud/OpenAI-compatible endpoints, and
  custom compatible providers.
- Keeps secrets out of prompts and model-visible state.
- Provides mode-scoped behavior, memory, tools, and skills.
- Starts in General mode by default.
- Supports global skills and per-mode skills so small models are not flooded
  with unrelated instructions.
- Supports Agent Teams for bounded multi-agent collaboration, with visible
  team-working indicators in the chat composer.
- Supports Ordo Tech Specialist for diagnostics, installs, MCP/plugin upkeep,
  provider setup, automation setup, avatar setup, and local computer tasks when
  explicitly approved.
- Provides automation for routines, hooks, cron-style schedules, heartbeats,
  webhooks, local events, dreaming reviews, and bounded coding automation.
- Provides Remote Communication setup for email and future channels such as
  Signal and Telegram. SMS is intentionally excluded.
- Provides a side artifact view for generated files, PDFs, docs, spreadsheets,
  email views, and other agent-produced artifacts without replacing chat.
- Provides RAG and persistent memory with a stronger hashing fallback when no
  embedding model is configured. Ollama or llama.cpp embedding models remain
  optional for better semantic retrieval.
- Provides MCP security with trust states, quarantine, drift checks,
  provenance tracking, redaction, and audit logging.

## Operator Surfaces

The left rail is intentionally user-facing:

- Provider
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
- `ordo-cloud`: cloud/provider calls and model lifecycle handling.
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
