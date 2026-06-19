# Servo Self-Rendering Shell

Date: 2026-06-17

Status: implemented for the current beta path.

## Decision

Ordo uses a custom embedded Servo shell as the desktop renderer. The old
Tauri/WebView host has been removed from the active workspace.

## Why Servo

Ordo needed a desktop shell that belongs to the runtime instead of a general
browser window. Embedding Servo gives Ordo:

- one app window
- no address bar
- no tab strip
- no runtime dependency on Vite
- Rust-side ownership of the shell
- a path toward consistent rendering without Chromium/CEF as the app core

## Runtime Shape

```text
Ordo runtime
  |
  | localhost control API and static asset server
  v
http://127.0.0.1:4141/
  |
  v
ordo-servo-shell
  |
  v
Ordo Studio UXI
```

The Studio bundle is built from `ordo-studio` and served by `ordo-control`:

- `/`
- `/index.html`
- `/assets/*`
- `/api/*`

## Why HTTP Still Matters

Servo should not load the built Studio bundle from `file://`. Module scripts
need a real HTTP origin and correct MIME types. Ordo now provides that origin
itself.

Vite is still useful while developing the frontend, but it is not a runtime
dependency.

## Shell Crate

`ordo-servo-shell` is the custom embedder.

It owns:

- the app window
- Servo WebView construction
- basic shell navigation controls
- Servo graphics backend setup

The shell should keep loading only Ordo's local URL unless an explicit future
feature adds a safe external-opening path.

## Launcher

The supported beta launcher is:

```powershell
.\Launch-Ordo-Servo.ps1
```

Explorer/double-click wrapper:

```text
Launch-Ordo-Servo.cmd
```

The launcher:

1. Builds the Studio bundle.
2. Starts the runtime.
3. Waits for `/health`.
4. Verifies the runtime-served Studio page.
5. Opens the embedded Servo shell.

## Verification

Core checks:

```powershell
npm run build --prefix ordo-studio
cargo check --manifest-path ordo-servo-shell\Cargo.toml --features servo-engine
.\Test-Ordo-Functions.ps1 -Suite standard -KeepGoing
```

Manual proof:

- one Ordo window opens
- no address bar appears
- no tab strip appears
- Assistant renders
- tabs switch
- provider/model controls remain usable
- chat composer remains visible
- Agent Team activity indicator renders when active
- closing the window does not leave orphaned shell UI processes

## Non-Negotiables

- Keep one visible operator surface.
- Do not add hidden sidecar UI windows.
- Keep secrets behind vault/UI paths.
- Keep filesystem access behind explicit permission.
- Keep MCP, plugin, provider, job, hook, and automation actions behind existing
  Ordo boundaries.
- Do not use Servo as a general internet browser.
