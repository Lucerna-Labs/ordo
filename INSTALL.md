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

One command. It downloads a prebuilt Ordo `.deb` from GitHub Releases and
installs it — no compiler, no Rust/Node, no long build:

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-prebuilt.sh | bash
```

The script calls `sudo` only for the final package install. To do it by hand,
download `ordo_*_amd64.deb` from the
[latest release](https://github.com/Lucerna-Labs/ordo/releases/latest) and run:

```bash
sudo dpkg -i ordo_*_amd64.deb || sudo apt-get install -f -y
```

`dpkg -i` plus `apt-get install -f` is used on purpose: some apt versions reject
a local `sudo apt install ./file.deb` path with an unsupported-file error, and
`-f` pulls in the runtime libraries.

Then open **Ordo** from your application menu. It launches the embedded Servo
app window, not an external browser.

### Build from source (advanced / developers)

The prebuilt package above is the recommended path. Build from source only if you
are developing Ordo or on a distro without a prebuilt `.deb`.

> This compiles Servo + SpiderMonkey + WebRender (~850 crates): budget **30–60+
> minutes**, ~**8 GB RAM**, and ~**15 GB disk** for the first build.

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | bash
```

This clones Ordo to `~/ordo` (or updates an existing clone), installs every build
prerequisite, then builds and installs the `.deb`. The build automatically sets
`BINDGEN_EXTRA_CLANG_ARGS` so Servo's `mozangle` shader bindings compile on
Pop!_OS 24.04 — where an extra `gcc-14` without its libstdc++ headers otherwise
makes libclang fail with `'array' file not found`. On non-Debian distros the
installer builds an AppImage instead.

## What Gets Installed

- The Ordo runtime binary.
- The embedded Servo shell.
- The built Studio UI bundle.
- A launcher that starts the runtime, opens Servo, and releases port `4141`
  when Ordo closes.
