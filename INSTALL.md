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

> **Building from source is not supported yet on Linux.** It currently fails on
> some Pop!_OS / clang setups while compiling Servo, so it has been removed from
> these instructions until it works reliably. Use the prebuilt package above.
> Non-Debian distros (Fedora, Arch, openSUSE, …) are not packaged yet either.

## What Gets Installed

- The Ordo runtime binary.
- The embedded Servo shell.
- The built Studio UI bundle.
- A launcher that starts the runtime, opens Servo, and releases port `4141`
  when Ordo closes.
