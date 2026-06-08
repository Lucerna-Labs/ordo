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
  librsvg2-dev \
  libssl-dev \
  libwebkit2gtk-4.1-dev \
  pkg-config
```

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

For live Linux development:

```bash
npm run tauri:dev
```

## Platform Notes

- Do not hard-code `.exe` paths in the UI or backend. The MCP binary is
  `ordo-mcp.exe` on Windows and `ordo-mcp` on Linux.
- Do not rely on WebView2 behavior for layout or rendering. Linux uses
  WebKitGTK, so the UXI should stay standards-based HTML/CSS.
- Packaged builds must load the bundled `dist` assets, not a `localhost` Vite
  URL.
- Keep platform-specific bundle settings in `tauri.*.conf.json` files instead
  of branching the app code.
