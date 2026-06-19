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
- Launcher: `Launch-Ordo-Servo.vbs` or `Launch-Ordo-Servo.cmd`.
- Diagnostic launcher: `Launch-Ordo-Servo.ps1`.
- Removed runtime dependency: Tauri. The old `ordo-studio/src-tauri` tree and
  old Studio/Portable launchers have been removed.

## Run On Windows

Install from GitHub source:

```powershell
cd $env:USERPROFILE\Desktop
git clone https://github.com/Lucerna-Labs/ordo.git
cd ordo
.\Install-Desktop-Shortcut.cmd
```

Then open **Ordo** from the desktop shortcut. The shortcut opens the embedded
Servo app window without browser tabs, an address bar, or a leftover console
window.

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

The launcher:

1. Builds the Studio bundle.
2. Starts the Ordo runtime.
3. Waits for the runtime health check.
4. Serves the Studio bundle from Ordo's own localhost server.
5. Opens the embedded Servo shell.

Use `Launch-Ordo-Servo.vbs` when double-clicking from Explorer. The `.cmd`
launcher delegates to it for compatibility. Use `Launch-Ordo-Servo.ps1`
directly only when you want a visible diagnostic console.

When the Servo window closes, the launcher stops the Ordo runtime so port
`4141` is released.

## Run On Pop!_OS / Linux

The Windows `.cmd`, `.vbs`, and `.ps1` launchers do not run Ordo on Linux.
GNOME/Pop!_OS does not reliably launch `.desktop` files from arbitrary source
folders; it often opens them in a text editor. The user-friendly Linux path is
the Debian package, which installs Ordo into the app menu.

Install from GitHub source:

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

After install, open **Ordo** from the Pop!_OS app menu.

If you already have a copy of Ordo, first open a terminal in that project
folder, or `cd` into it. The commands must be run from the folder that contains
`Build-Ordo-Linux-Deb.sh`.

Example if Ordo is on your Desktop:

```bash
cd ~/Desktop/ordo
./Build-Ordo-Linux-Deb.sh
```

Example if Ordo is in Downloads:

```bash
cd ~/Downloads/ordo
./Build-Ordo-Linux-Deb.sh
```

If you do not know where the folder is, search for it:

```bash
find ~ -maxdepth 4 -name "Build-Ordo-Linux-Deb.sh" 2>/dev/null
```

Then `cd` into the folder printed by that command and build the package.

Install the generated package from the same Ordo folder:

```bash
sudo apt install ./dist/ordo_0.1.0_amd64.deb
```

The installed launcher starts the Ordo runtime, waits for
`127.0.0.1:4141/health`, opens the embedded Servo app window, and stops the
runtime when the Servo window closes. It does not launch Chrome, Firefox, or
another external browser.

For troubleshooting only, the launcher can also be run from a terminal:

```bash
cd /path/to/ordo
./Launch-Ordo-Servo.sh
```

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
- RAG and persistent memory with a stronger hashing fallback when no embedding
  model is configured; Ollama or llama.cpp embedding models can still be used
  for better semantic retrieval quality.
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

Runnable lightweight copies may include this small portable binary folder:

- `bin/portable/ordo.exe`
- `bin/portable/ordo-servo-shell.exe`
- `bin/portable/libEGL.dll`
- `bin/portable/libGLESv2.dll`

When those files exist, `Launch-Ordo-Servo.ps1` uses them instead of requiring
Cargo to rebuild the runtime and Servo shell on first launch.

GitHub source ZIP downloads also include `bootstrap/ordo-windows-portable.zip`.
If `bin/portable` or `ordo-studio/dist` is missing, the launcher extracts that
bootstrap payload automatically before starting Ordo.

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
