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

## Linux

Install Git first if needed, then clone Ordo:

```bash
cd ~/Desktop
git clone https://github.com/Lucerna-Labs/ordo.git
cd ordo
./Install-Ordo-Linux.sh
```

That is the recommended Linux entry point.

On Debian-family systems, including Pop!_OS, Ubuntu, Linux Mint, and Debian,
the installer builds the `.deb` package and installs it with `dpkg -i`. It uses
`apt-get install -f` only if dependencies need repair.

Do not use this command:

```bash
sudo apt install ./dist/ordo_0.1.0_amd64.deb
```

Some distro versions reject local `.deb` paths with an unsupported-file error.

On non-Debian distros, `Install-Ordo-Linux.sh` builds the AppImage path instead.
After it finishes, launch the AppImage printed by the script, usually:

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

## Linux Build Dependencies

On Pop!_OS, Ubuntu, or Debian:

```bash
sudo apt update
sudo apt install -y git build-essential pkg-config curl libssl-dev \
  libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev \
  libegl1-mesa-dev libgles2-mesa-dev
```

Other distros need equivalent Rust, Node/npm, OpenSSL, X11/XCB/XKB, Wayland,
EGL/GLES, and `pkg-config` packages.

## What Gets Installed

- The Ordo runtime binary.
- The embedded Servo shell.
- The built Studio UI bundle.
- A launcher that starts the runtime, opens Servo, and releases port `4141`
  when Ordo closes.

