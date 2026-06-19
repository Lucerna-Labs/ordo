# Servo Self-Rendering Shell Pivot

Date: 2026-06-17

## Decision

Ordo's next desktop shell direction is a Servo-backed self-rendering shell
instead of continuing to deepen the Tauri/WebView2/WebKitGTK host as the
canonical renderer.

The current Tauri Studio remains the known-good compatibility shell until the
Servo shell can boot, render the existing UXI, talk to the runtime, and pass the
same operator proof gates.

## Why Servo

Servo now fits the shape Ordo wanted from the start:

- Rust-native browser engine ownership instead of OS-specific WebView behavior.
- Embeddable rendering API for application shells.
- Cross-platform target surface across Windows, Linux, macOS, Android, and
  OpenHarmony.
- A path toward consistent rendering and self-owned desktop behavior without
  bringing in Chromium/CEF as the application core.

Primary references:

- https://servo.org/
- https://github.com/servo/servo
- https://servo.org/blog/2026/04/13/servo-0.1.0-release/

## Non-Negotiables

- Keep one visible operator surface.
- Do not reintroduce hidden sidecar UI windows.
- Keep the Ordo runtime headless and observable through the existing control API
  and bus contracts.
- Preserve the existing React/HTML/CSS UXI as the first content target unless a
  specific Servo limitation forces a scoped adaptation.
- Keep provider secrets, filesystem access, hooks, jobs, MCPs, plugins, modes,
  and runtime actions behind the existing Ordo boundaries.
- Do not remove Tauri until Servo has proven parity.

## Target Shape

```text
Ordo runtime
  |
  | control API / bus-backed commands
  v
Servo shell process
  |
  | Servo WebView / embedded engine
  v
Ordo UXI bundle
```

The UXI should not know whether it is hosted by Tauri or Servo except through a
small shell capability adapter. Runtime calls should continue to route through
`src/api.ts` style boundaries rather than direct shared state.

## Migration Plan

1. Freeze Tauri as the compatibility host.
   - Keep `ordo-studio` buildable.
   - Keep `npm run build` and `npm run check:tauri` green for the compatibility
     shell while Servo work proceeds.

2. Add a separate Servo shell target.
   - Prefer a new clearly named shell crate or package over mutating the Tauri
     shell in place.
   - Load the built `ordo-studio/dist` assets first.
   - Connect shell capabilities through a narrow adapter instead of importing
     Tauri assumptions.

3. Define the shell capability adapter.
   - Open external URL when explicitly requested.
   - Pick files/folders through a visible operator action.
   - Read local catalogs only through approved runtime or shell commands.
   - Emit debug/event records for shell-level actions.

4. Prove rendering.
   - Launch one visible window titled `Ordo`.
   - Render the Assistant tab from the existing UXI.
   - Switch tabs and confirm the UXI re-renders from real state.
   - Call `/health` on the runtime and display the result through the UXI.

5. Prove parity before replacing launchers.
   - Modes, plugins, MCPs, providers, files, hooks, routines, docs, and event
     logs must remain visible.
   - No second browser window opens during normal launch.
   - No direct secret values are logged or surfaced.

## Open Engineering Questions

- Whether to embed the published `servo` crate directly or vendor/pin a Servo
  LTS release branch for deterministic builds.
- Whether Servo shell code should live under `ordo-studio` or as a sibling crate
  such as `ordo-servo-shell`.
- How much of the current Tauri local fallback command set should move to the
  runtime API versus a renderer-neutral shell adapter.
- Which platform should be the first proof target: Windows for this portable
  bundle, or Linux for the cleanest Servo development loop.

## Verification Gates

Minimum proof for the first Servo milestone:

```powershell
cargo check -p <servo-shell-crate>
cd ordo-studio
npm run build
```

Manual/operator proof:

- One window opens.
- The existing UXI paints.
- A real click changes rendered state.
- A runtime-backed call returns visible data.
- Closing the window shuts down the shell cleanly without orphaning UI
  processes.

## Current Status

The first integration point now exists as `ordo-servo-shell`.

- Default `cargo check --manifest-path ordo-servo-shell/Cargo.toml` stays light
  and does not compile Servo.
- `Launch-Ordo-Servo.ps1` builds the Studio bundle, starts the Ordo runtime,
  verifies the runtime-served Studio bundle at `http://127.0.0.1:4141/`, and
  opens the official Servo nightly `servoshell.exe` window. The interactive
  Servo path no longer uses the Vite development server or a helper static
  server.
- `cargo run --manifest-path ordo-servo-shell/Cargo.toml --features
  servo-engine -- --target
  ordo-studio/dist/index.html --out data/servo-render-proof.png` launches the
  Servo render proof against the Studio bundle.
- `Launch-Ordo-Servo-Proof.ps1` wraps the Studio build plus Servo render proof.

On this Windows workspace, the feature build passes but the render proof stops
before pixel readback because Servo/surfman cannot initialize EGL
(`egl function was not loaded`). The launcher now reports that as an explicit
diagnostic. This is not yet the interactive replacement shell; it is the first
renderer integration gate while the Tauri shell remains the compatibility host.
