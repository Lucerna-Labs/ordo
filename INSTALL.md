# Ordo Install Guide

Ordo launches as a local runtime plus the embedded Servo desktop shell. It does
not need Chrome, Firefox, Vite, or Tauri for normal use.

## Windows

Install Git for Windows if needed, then open PowerShell:

```powershell
cd $env:USERPROFILE\Desktop
git clone https://github.com/Lucerna-Labs/ordo.git
cd ordo
.\Install-Desktop-Shortcut.cmd
```

Then open **Ordo** from the desktop shortcut.

To launch once from the project folder without installing the shortcut:

```powershell
wscript.exe .\Launch-Ordo-Servo.vbs
```

For troubleshooting with a visible console:

```powershell
.\Launch-Ordo-Servo.ps1
```

To update an existing Windows clone:

```powershell
git pull
.\Install-Desktop-Shortcut.cmd
```

## Linux (Pop!_OS / Ubuntu / Debian)

There are two ways to install on Linux. **Pick the prebuilt path** unless you
have a reason to build from source.

### Recommended: prebuilt package (no compiler)

This downloads a prebuilt Ordo `.deb` from GitHub Releases and installs it. It
needs no Rust, no Node, and no multi-hour build:

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-prebuilt.sh | bash
```

The script calls `sudo` only for the final package install. If you prefer to do
it by hand, download the `ordo_*_amd64.deb` from the
[latest release](https://github.com/Lucerna-Labs/ordo/releases/latest) and run:

```bash
sudo dpkg -i ordo_0.1.0_amd64.deb || sudo apt-get install -f -y
```

`dpkg -i` plus `apt-get install -f` is used on purpose: some apt versions reject
a local `sudo apt install ./file.deb` path with an unsupported-file error, and
`-f` pulls in the runtime libraries.

Then open **Ordo** from your application menu. It launches the embedded Servo
app window, not an external browser.

> If the command reports that no prebuilt `.deb` was published yet, use the
> source build below.

### Fallback: build from source

> **Heads up — this compiles a browser engine.** Ordo's desktop shell embeds
> Servo, which builds Servo + SpiderMonkey + WebRender from source (~850 crates).
> Budget **30–60+ minutes**, ~**8 GB RAM**, and ~**15 GB disk** for the first
> build. This is why the prebuilt package above exists.

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | bash
```

If `curl` is not available, use `wget`:

```bash
wget -qO- https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | bash
```

The bootstrap installer clones Ordo to `~/ordo` (or updates an existing clone
with `git pull --ff-only`; it never overwrites a non-Git folder named `~/ordo`),
installs every build prerequisite, then builds and installs Ordo. On
Debian-family systems (Pop!_OS, Ubuntu, Linux Mint, Debian) it builds the `.deb`
and installs it with `dpkg -i`; on other distros it builds the AppImage path.

Install somewhere other than `~/ordo`:

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | ORDO_DIR="$HOME/Desktop/ordo" bash
```

If you are already inside an updated Ordo project folder, run:

```bash
./Install-Ordo-Linux.sh
```

On non-Debian distros, `Install-Ordo-Linux.sh` builds the AppImage and prints
the path to launch, usually:

```bash
./dist/Ordo-0.1.0-x86_64.AppImage
```

If AppImage tooling is not available on your distro, build the portable tarball:

```bash
./Build-Ordo-Linux-Portable.sh
tar -xzf ./dist/ordo-linux-0.1.0-x86_64.tar.gz -C ./dist
./dist/ordo-linux-0.1.0-x86_64/launch-ordo.sh
```

The extracted portable folder includes an optional shortcut installer:

```bash
./install-desktop-shortcut.sh
```

## Linux Build Dependencies (source builds only)

The source installer installs these for you on Debian-family systems. You only
need this list if you are setting up a build host by hand. It is larger than a
typical app's because the Servo + SpiderMonkey build needs a full C/C++ + LLVM
toolchain and the system font stack:

```bash
sudo apt update
sudo apt install -y \
  git curl wget build-essential pkg-config cmake clang libclang-dev llvm-dev \
  python3 python-is-python3 m4 \
  libssl-dev libtss2-dev zlib1g-dev \
  libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev \
  libegl1-mesa-dev libgles2-mesa-dev libgl1-mesa-dev \
  libfontconfig-dev libfreetype-dev libharfbuzz-dev
```

You also need Rust (via rustup) and Node.js 20+. The bootstrap installer sets
both up automatically; to do it manually, install rustup and Node 20+ first.

Other distros need the equivalent packages: a C/C++ toolchain, CMake, Clang +
libclang, LLVM, Python 3, OpenSSL, fontconfig/FreeType/HarfBuzz, X11/XCB/XKB,
Wayland, EGL/GLES, and `pkg-config`.

## What Gets Installed

- The Ordo runtime binary.
- The embedded Servo shell.
- The built Studio UI bundle.
- A launcher that starts the runtime, opens Servo, and releases port `4141`
  when Ordo closes.
