# Ordo Install Guide

Ordo runs as a local Rust runtime plus an embedded Servo desktop shell. You do
**not** need Chrome, Firefox, or Tauri.

---

## Prerequisites

### Windows

| Requirement | Why | How to check |
|---|---|---|
| **Git** | Clone the repo | `git --version` |
| **Node.js 20+** and **npm** | Build the Studio UI | `node --version` |
| **Rust 1.93+** | Build the runtime + Servo shell | `rustc --version` |

Install Rust from <https://rustup.rs> if you don't have it.

Install Node.js from <https://nodejs.org> (the LTS version is fine).

### Linux (Pop!_OS / Ubuntu / Debian)

No prerequisites for the prebuilt install (it downloads a `.deb`). For
source builds, the installer script installs everything automatically.

---

## Install

### Option A: Linux prebuilt `.deb` (fastest, no compiler)

No Rust, no Node, no 30-minute Servo build — just downloads and installs:

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-prebuilt.sh | bash
```

Then open **Ordo** from your application menu.

To install a specific release by hand:

```bash
# Download from https://github.com/Lucerna-Labs/ordo/releases/latest
sudo dpkg -i ordo_*_amd64.deb || sudo apt-get install -f -y
```

The `apt-get install -f` resolves runtime dependencies that `dpkg` alone
won't pull in.

---

**Option B: Windows from source**

```powershell
cd $env:USERPROFILE\Desktop
git clone https://github.com/Lucerna-Labs/ordo.git
cd ordo
cargo run -- serve
```

Or create a desktop shortcut first, then launch from it:

```powershell
.\Install-Desktop-Shortcut.cmd
```

The shortcut runs `Launch-Ordo-Servo.vbs`, which calls `cargo run -- serve`
behind the scenes — Ordo manages the entire lifecycle: builds the Studio UI,
starts the runtime, opens the embedded Servo window, and shuts everything
down cleanly when you close the window.

**What happens on first launch:**

1. Ordo shows a progress page (at `http://127.0.0.1:4141/boot`) with
   real-time build status for each step
2. Builds the Studio UI (`npm install && npm run build`)
3. Builds the Servo shell (`cargo build`)
4. Starts the Ordo runtime
5. Opens the Servo window — closing it cleanly stops the runtime

This takes **5–15 minutes** the first time (compiling Rust). Subsequent
launches are fast — binaries are cached.

**To update an existing install:**

```powershell
git pull
cargo run -- serve
```

**For troubleshooting with a visible console:**

```powershell
.\Launch-Ordo-Servo.ps1
```

---

### Option C: Linux from source (developers)

> **Budget:** 30–60+ minutes, ~8 GB RAM, ~15 GB disk (compiles Servo +
> SpiderMonkey + WebRender — ~850 crates).

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | bash
```

This clones to `~/ordo`, installs all build prerequisites (Rust, Node,
C/C++ toolchain, GL/X11/Wayland headers), builds, and installs the `.deb`.

On non-Debian distros it builds an AppImage instead.

---

## Launch

After install, Ordo runs at:

```
http://127.0.0.1:4141
```

- **Windows:** Open the **Ordo** desktop shortcut.
- **Linux:** Open **Ordo** from your application menu.

The embedded Servo window is the intended way to use Ordo — not an
external browser.

---

## Troubleshooting

### "npm was not found on PATH"
Install Node.js 20+ from <https://nodejs.org>.

### "cargo was not found on PATH"
Install Rust from <https://rustup.rs>, then reopen your terminal.

### Runtime health check failed
The runtime didn't start within 120 seconds. Check the logs:

```powershell
# Windows
Get-Content runtime-servo.err.log -Tail 50
```

Common causes: port 4141 already in use, missing dependencies, or the
Studio build failed silently.

### Port 4141 already in use
The launcher kills stale listeners automatically, but if something else
is using the port:

```bash
# Linux
sudo lsof -i :4141
sudo kill <PID>

# Windows (PowerShell)
Get-NetTCPConnection -LocalPort 4141 | Select OwningProcess
Stop-Process -Id <PID>
```

### Servo shell build failed (Linux)
Ensure you have the full build prerequisites. The source installer handles
this automatically, but if you're building manually:

```bash
sudo apt-get install -y build-essential cmake clang libclang-dev llvm-dev \
  python3 m4 libssl-dev libtss2-dev zlib1g-dev \
  libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev \
  libegl1-mesa-dev libgles2-mesa-dev libgl1-mesa-dev \
  libfontconfig-dev libfreetype-dev libharfbuzz-dev
```

### Servo ANGLE DLLs missing (Windows)
The launcher downloads Servo nightly automatically for the ANGLE runtime
DLLs (`libEGL.dll`, `libGLESv2.dll`). If that fails, check your internet
connection or run the diagnostic launcher for details:

```powershell
.\Launch-Ordo-Servo.ps1
```
