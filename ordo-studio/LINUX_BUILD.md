# Ordo Studio Linux Build

This app is a Tauri v2 desktop shell. On Windows it uses WebView2; on Linux the
same Tauri app uses the native WebKitGTK webview. The UI remains the same
HTML/CSS/React UXI and the Rust side remains the local command bridge.

## Targets

Linux builds produce:

- `deb` for Debian/Ubuntu-family systems
- `AppImage` for portable Linux distribution

The Linux-specific Tauri config lives in:

```text
src-tauri/tauri.linux.conf.json
```

Windows packaging remains separate in:

```text
src-tauri/tauri.windows.conf.json
```

## Ubuntu/Debian Build Host

Install the native build dependencies on the Linux machine or WSL distro that
will create the Linux package:

```bash
sudo apt update
sudo apt install -y \
  build-essential \
  curl \
  file \
  libayatana-appindicator3-dev \
  libjavascriptcoregtk-4.1-dev \
  librsvg2-dev \
  libsoup-3.0-dev \
  libssl-dev \
  libtss2-dev \
  libwebkit2gtk-4.1-dev \
  patchelf \
  pkg-config
```

`libtss2-dev` provides the TPM 2.0 TSS system library that the secrets vault uses
on Linux (the `tss-esapi` crate, gated to `cfg(target_os = "linux")` in
`ordo-secrets-vault`). Without it the workspace fails to build with
`Package 'tss2-sys' ... not found`.

(The webview deps match the set the release CI installs in
`.github/workflows/release.yml`.)

Install the project dependencies:

```bash
npm ci
```

Build the Linux desktop package:

```bash
npm run tauri:build:linux
```

The packaged files will be written under:

```text
src-tauri/target/release/bundle/
```

## Development Run

For live Linux development, from the repo root:

```bash
./Launch-Ordo-Studio.sh     # runtime + Studio dev shell (waits for /health first)
./Launch-Ordo-Portable.sh   # build + run just the headless runtime
```

Or directly, from `ordo-studio/`:

```bash
npm run tauri:dev
```

## Platform Notes

- **Avatar microphone.** On Linux the shell renders with webkit2gtk, which (unlike
  WebView2) denies `getUserMedia` until the host grants it. `main.rs` installs a
  `cfg(target_os = "linux")` permission handler that auto-allows microphone/camera
  requests, so the avatar's in-tab "tap to talk" works. The **avatar pop-out**
  (the Bot button → `xdg-open` → your browser) also works. The WebView2-only
  `additionalBrowserArgs` in `tauri.conf.json` are ignored on webkit2gtk
  (harmless).

- Do not hard-code `.exe` paths in the UI or backend. The MCP binary is
  `ordo-mcp.exe` on Windows and `ordo-mcp` on Linux.
- Do not rely on WebView2 behavior for layout or rendering. Linux uses
  WebKitGTK, so the UXI should stay standards-based HTML/CSS.
- Packaged builds must load the bundled `dist` assets, not a `localhost` Vite
  URL.
- Keep platform-specific bundle settings in `tauri.*.conf.json` files instead
  of branching the app code.
