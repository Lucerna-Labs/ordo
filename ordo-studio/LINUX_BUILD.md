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

The embedded Servo shell pulls the full `servo` crate, which builds Servo +
SpiderMonkey (`mozjs_sys`) + WebRender + Stylo from source. That needs much more
than the runtime alone: Rust, Node/npm, a C/C++ toolchain, **CMake**, **Clang +
libclang**, **LLVM**, **Python 3**, the TPM TSS headers (`ordo-secrets-vault`'s
Linux sealer), the system font stack (fontconfig/FreeType/HarfBuzz), and the
X11/XCB/XKB, Wayland, and EGL/GLES dev headers.

On Ubuntu or Pop!_OS:

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

`scripts/install-linux-build-deps.sh` installs exactly this set (plus Rust and
Node) for you, and the release CI runs the same script — so a green release
build is a real check that the source-install dependency list is complete.

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
