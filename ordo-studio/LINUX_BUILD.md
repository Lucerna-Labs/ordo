# Ordo Linux Build

Ordo no longer uses Tauri for the desktop host. Linux builds use the same
runtime and Studio bundle as Windows, rendered through the embedded Servo shell.

## Outputs

From the repository root:

```bash
./Build-Ordo-Linux-Deb.sh
./Build-Ordo-Linux-Portable.sh
./Build-Ordo-Linux-AppImage.sh
```

For user-facing install from source, prefer:

```bash
./Install-Ordo-Linux.sh
```

On Debian-family systems that calls `Install-Ordo-Linux-Deb.sh`, which installs
the generated package with `dpkg -i` instead of relying on `apt install
./file.deb`.

The generated files are written to `dist/`:

- `ordo_0.1.0_amd64.deb` for Debian, Ubuntu, Pop!_OS, and related systems.
- `ordo-linux-0.1.0-x86_64.tar.gz` for portable Linux testing.
- `Ordo-0.1.0-x86_64.AppImage` for the broadest desktop Linux path.

## Build Host Requirements

The build host needs Rust, Node/npm, a C/C++ toolchain, OpenSSL development
headers, X11/XCB/XKB common libraries, Wayland client headers, EGL/GLES, and
`pkg-config`.

On Ubuntu or Pop!_OS:

```bash
sudo apt update
sudo apt install -y git build-essential pkg-config curl libssl-dev \
  libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev \
  libegl1-mesa-dev libgles2-mesa-dev
```

The AppImage builder downloads `linuxdeploy` and `appimagetool` into
`dist/appimage-tools/` when they are not already installed. Set
`ORDO_LINUXDEPLOY` or `ORDO_APPIMAGETOOL` to use local copies instead.

## Runtime Notes

The packaged launchers start the local Ordo runtime, wait for
`127.0.0.1:4141/health`, open the embedded Servo app window, and stop the
runtime when the Servo window closes.

The Servo shell is pointed only at Ordo's local URL. The launcher also sets
proxy variables to dead localhost endpoints so the app window is not used as a
general internet browser.
