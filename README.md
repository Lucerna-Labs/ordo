# Ordo

Ordo is a local-first AI runtime and operator studio by Lucerna Labs, a
division of Lucerna Media.

It combines a Rust/Tokio runtime, a localhost control API, a React UXI, and a
custom embedded Servo shell. The model does not get uncontrolled hands on the
computer: it works through scoped capabilities, policies, logs, review gates,
mode rules, and explicit operator approval.

Ordo is beta software. The current desktop path is Windows-first through the
Servo shell; the Rust runtime remains portable source code.

## Quick Start

### Windows Source Install

Install Git for Windows if needed, then run this from PowerShell:

```powershell
cd $env:USERPROFILE\Desktop
git clone https://github.com/Lucerna-Labs/ordo.git
cd ordo
.\Install-Desktop-Shortcut.cmd
```

Then open **Ordo** from the desktop shortcut. The shortcut uses
`Launch-Ordo-Servo.vbs`, so Ordo opens in the embedded Servo app window without
leaving a console window behind.

To launch once from the project folder without installing the shortcut:

```powershell
wscript.exe .\Launch-Ordo-Servo.vbs
```

For troubleshooting, use the visible diagnostic launcher:

```powershell
.\Launch-Ordo-Servo.ps1
```

If you already have a copy of Ordo, open PowerShell in that project folder and
run:

```powershell
git pull
.\Install-Desktop-Shortcut.cmd
```

For Explorer/double-click use inside the project folder:

```text
Launch-Ordo-Servo.vbs
```

`Launch-Ordo-Servo.cmd` delegates to the same hidden launcher for compatibility.
Use `Launch-Ordo-Servo.ps1` directly only when you want a visible diagnostic
console.

The launcher builds `ordo-studio`, starts the Ordo runtime, waits for
`/health`, serves the built UI from Ordo's own localhost server, and opens the
embedded Servo app window. When the Servo window closes, the launcher stops the
Ordo runtime so port `4141` is released.

GitHub source ZIP downloads include a compact Windows bootstrap payload at
`bootstrap/ordo-windows-portable.zip`. On first launch, `Launch-Ordo-Servo.ps1`
extracts it if `bin/portable` or `ordo-studio/dist` is missing, so the source
ZIP can launch without requiring a full Rust/Servo rebuild.

Default local runtime URL:

```text
http://127.0.0.1:4141
```

On Pop!_OS / Linux, use the Debian package path. GNOME/Pop!_OS often opens
`.desktop` files from source folders in a text editor, so Ordo installs a real
app-menu launcher instead:

```bash
sudo apt update
sudo apt install -y git build-essential pkg-config curl libssl-dev \
  libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev \
  libegl1-mesa-dev libgles2-mesa-dev

git clone https://github.com/Lucerna-Labs/ordo.git
cd ordo
./Build-Ordo-Linux-Deb.sh
sudo apt install ./dist/ordo_0.1.0_amd64.deb
```

If you already have a copy of Ordo, `cd` into that folder instead:

```bash
cd ~/Desktop/ordo
./Build-Ordo-Linux-Deb.sh
sudo apt install ./dist/ordo_0.1.0_amd64.deb
```

Then open **Ordo** from the app menu. That opens the embedded Servo app window,
not an external browser.

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
