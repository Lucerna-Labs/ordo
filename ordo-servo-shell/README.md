# Ordo Servo Shell

`ordo-servo-shell` is the Servo-backed self-rendering integration target for
Ordo Studio.

Default builds do not compile Servo. This keeps the normal Ordo runtime checks
fast and keeps the existing Tauri compatibility shell stable while the Servo
path matures.

## Default Check

```powershell
cargo check --manifest-path ordo-servo-shell/Cargo.toml
cargo run --manifest-path ordo-servo-shell/Cargo.toml
```

## Servo Render Proof

Launch the interactive shell:

```powershell
.\Launch-Ordo-Servo.ps1
```

The launcher builds `ordo-studio/dist`, starts the Ordo runtime, verifies that
Ordo's control API is serving the Studio bundle from `http://127.0.0.1:4141/`,
builds the embedded `ordo-servo-shell.exe`, and opens Ordo inside that custom
Servo embedder window. It does not use the Vite dev server or the official
Servo browser chrome for the interactive Servo path.

Build the Studio bundle, then render it through Servo's software rendering
context:

```powershell
cd ordo-studio
npm run build
cd ..
cargo run --manifest-path ordo-servo-shell/Cargo.toml --features servo-engine -- --target ordo-studio/dist/index.html --out data/servo-render-proof.png
```

Windows convenience launcher:

```powershell
.\Launch-Ordo-Servo-Proof.ps1
```

The proof writes a PNG under `data/servo-render-proof.png` by default when the
platform graphics context initializes successfully.

On Windows, the embedded shell uses surfman's ANGLE backend. The launcher copies
`libEGL.dll` and `libGLESv2.dll` from the Servo nightly into the embedded shell
output directory when those DLLs are missing.

## Renderer Boundary

- Tauri remains the compatibility shell.
- Servo owns the new self-rendering shell path.
- Runtime behavior stays behind Ordo's control API and bus contracts.
- Secrets and local filesystem actions must remain behind explicit Ordo
  capability boundaries.
