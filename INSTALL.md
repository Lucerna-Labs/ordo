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

Install with the bootstrap command:

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | bash
```

The bootstrap installer works whether `~/ordo` is missing or already exists as
a Git clone. It also installs missing build prerequisites on Debian-family
systems, including Pop!_OS, Ubuntu, Linux Mint, and Debian:

- Git
- Rust/Cargo through rustup
- Node.js 24 and npm through NodeSource when Node is missing or too old
- C/C++ build tools
- OpenSSL, X11/XCB/XKB, Wayland, EGL/GLES, and `pkg-config`

If `curl` is not available:

```bash
wget -qO- https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | bash
```

That is the recommended Linux entry point. It updates an existing `~/ordo`
clone with `git pull --ff-only`, or clones Ordo fresh if the folder is missing.
It will not delete or overwrite a non-Git folder named `~/ordo`.

To install somewhere else:

```bash
curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-from-github.sh | ORDO_DIR="$HOME/Desktop/ordo" bash
```

If you are already inside an updated Ordo project folder, you can still run:

```bash
./Install-Ordo-Linux.sh
```

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
