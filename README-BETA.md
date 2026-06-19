# Ordo (WebView Beta)

Lucerna Labs, a division of Lucerna Media · local-first AI runtime with the Ordo
Studio React UI hosted in a Tauri WebView2 window. The desktop app is **Ordo Studio**; the headless
runtime (control API on `127.0.0.1:4141`) is the `ordo` binary.

This folder is the full workspace. Three ways to use it:

> **Servo self-rendering path:** the current packaged app is still the Tauri
> compatibility shell. The Servo integration now lives in
> `ordo-servo-shell`; run `Launch-Ordo-Servo.ps1` to build the Studio bundle,
> start the runtime, serve the built assets from Ordo's own control API, and
> open the Servo shell. `Launch-Ordo-Servo-Proof.ps1` remains available for
> offscreen screenshot diagnostics.

> **Linux / macOS:** the steps below are Windows-centric (`.exe`, PowerShell).
> Use the `Launch-Ordo-Studio.sh` / `Launch-Ordo-Portable.sh` scripts, and see
> [`ordo-studio/LINUX_BUILD.md`](ordo-studio/LINUX_BUILD.md) for the build
> prerequisites.

---

## 1. Run from this workspace (source)

Double-click **`Launch-Ordo-Studio.cmd`**. It starts the Ordo runtime from this
workspace (`cargo`, control API at `127.0.0.1:4141`) and opens the Ordo Studio
UI. `Launch-Ordo-Portable.cmd` is the equivalent portable/detached launcher —
it builds the runtime (`cargo build --bin ordo --features sandbox-wasm,native-exec`)
and runs it hidden/detached. Your data lives in `data/` and `user-files/` inside
this folder. Requires `npm` and `cargo` on PATH (the launcher checks).

To get a Desktop launcher pointing at this workspace:
1. Double-click **`Install-Desktop-Shortcut.cmd`**
2. An **`Ordo Studio.lnk`** appears on your Desktop, targeting
   `Launch-Ordo-Studio.cmd`

---

## 2. Install the packaged app (no toolchain needed)

Prebuilt Windows installers live under **`installers/`**:

- **`installers\Ordo_0.1.0_x64_en-US.msi`** — per-user MSI (no UAC)
- **`installers\Ordo_0.1.0_x64-setup.exe`** — NSIS setup (alternative)

Either one installs the packaged Tauri app per-user, adds Start Menu + Desktop
entries, and is removable via Settings → Apps. The packaged desktop host binary
is `bin\windows\Ordo.exe`.

---

## 3. Build artifacts from source

```
# React UI bundle (ordo-studio/dist)
cd ordo-studio && npm install && npm run build && cd ..

# Headless runtime  ->  target\release\ordo.exe
cargo build --release -p ordo-cli

# Packaged desktop app + Windows installers (NSIS + MSI)
cd ordo-studio && npm run tauri:build:windows
```

`cargo build --release -p ordo-cli` produces `target\release\ordo.exe` (the
headless runtime). `npm run tauri:build:windows` produces the `.msi` / `-setup.exe`
bundles that are mirrored into `installers/`.

---

## What's where

| File | What it is |
|---|---|
| `Launch-Ordo-Studio.cmd` / `.ps1` | Run the runtime + Studio UI from this workspace |
| `Launch-Ordo-Portable.cmd` / `.ps1` | Portable/detached launcher (builds + runs hidden) |
| `Install-Desktop-Shortcut.cmd` | Creates the `Ordo Studio.lnk` Desktop launcher |
| `installers\Ordo_0.1.0_x64_en-US.msi` | Per-user Windows MSI installer |
| `installers\Ordo_0.1.0_x64-setup.exe` | NSIS Windows setup installer |
| `bin\windows\Ordo.exe` | Packaged Tauri desktop host (WebView2) |
| `target\release\ordo.exe` | Headless runtime binary (built from `ordo-cli`) |
| `README-BETA.md` | This file |
| `ordo-studio/dist/` | Bundled React UI (loaded by the Tauri host) |
| `ordo-*/` (~60 crates) | Full Rust source for the workspace |
| `Cargo.toml`, `Cargo.lock` | Workspace manifest + pinned versions |
| `docs/` | Internal docs (developer guide, UI extensions, UXI dev notes) |

---

## Once it's running

The Studio UI opens on the **Assistant** tab. Hit the **Provider** lane
in the sidebar to manage cloud credentials — each row has **Test**
(connectivity probe) and **Discover** (lists installed models on the
provider so you can click one to set it as active).

Provider templates available: Ollama Local, Ollama Cloud, LM Studio,
LocalAI, OpenAI Compatible (generic), Google, Anthropic, OpenAI, OpenRouter.

Default endpoint: `http://127.0.0.1:4141`. Hit `/health` to check the
runtime, `/api/capabilities` to list available tools.
