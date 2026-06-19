# Linux / Pop!_OS Install — Root-Cause Findings

Context: the GitHub one-liner install failed repeatedly on a clean Pop!_OS
machine. This documents *why*, what the desktop build actually is, and the fix
strategy, so we don't re-derive it.

## The core reality: the desktop shell compiles a browser engine

`ordo-servo-shell` depends on `servo = "=0.3.0"` (feature `servo-engine`). That
is the **full Servo engine**, and its dependency tree (852 crates) includes:

```
ordo-servo-shell → servo → servo-script → mozjs → mozjs_sys v140  (SpiderMonkey)
```

plus `aws-lc-sys`, `mozangle` (ANGLE), `webrender`, `stylo`, and the system font
stack (`yeslogic-fontconfig-sys`, `freetype-sys`, `harfbuzz-sys`). So a source
install compiles **Servo + SpiderMonkey + WebRender from source**: ~30–60+ min,
~8 GB RAM, ~15 GB disk. SpiderMonkey is not optional — Servo's DOM/script layer
is built on it. (Vite is unrelated to this; it is only the build-time bundler
for the Studio UI assets. Servo renders the prebuilt `dist/` bundle.)

This is *why* a from-source install is inherently not "user-friendly", and why
the prebuilt package below exists.

## The cascade of clean-machine failures

The install "worked" on CI runners and dev/WSL boxes because they already had
the heavy toolchain. A genuinely clean Pop!_OS/Ubuntu box hit these in turn:

1. **cargo not on PATH** after rustup (already patched before this work).
2. **`npm ci` fails — lockfile drift.** `ordo-studio/package-lock.json` was
   generated on Windows and lacked the Linux `@emnapi/*` wasm variants pulled by
   Vite 8 / Rolldown (`@rolldown/binding-wasm32-wasi`). The build scripts use
   strict `npm ci`, so they died here. CI hid it with `npm ci || npm install`.
   Fix: regenerated a cross-platform lockfile (`npm install --package-lock-only`).
3. **Missing Servo build toolchain.** The apt list had none of `cmake`, `clang`,
   `libclang-dev`, `llvm-dev`, `python3`, `libfontconfig-dev`, `libfreetype-dev`,
   `libharfbuzz-dev` — all required by `mozjs_sys`/`aws-lc-sys`/font `-sys`
   crates. Fix: added the full toolchain to `scripts/install-linux-build-deps.sh`.
4. **Missing `libtss2-dev`.** `ordo-secrets-vault`'s Linux sealer links
   `tss-esapi-sys` (TPM TSS) via pkg-config. `ci.yml` installed `libtss2-dev`,
   but the user installer never did, so `ordo-cli` failed to build. Fix: added
   `libtss2-dev` to the deps script.

## Fix strategy: prebuilt `.deb` + source fallback

Chosen direction: ship a **prebuilt `.deb`** so users download instead of
compiling, with the source build kept as a fallback.

- `scripts/install-linux-prebuilt.sh` — downloads the latest `ordo_*_amd64.deb`
  from GitHub Releases and installs it (`dpkg -i` + `apt-get install -f`). No
  compiler. This is the recommended Pop!_OS path.
- `.github/workflows/release.yml` — the old `studio` job built the **removed**
  Tauri host (dead). Replaced with a `linux-desktop` job on **ubuntu-22.04**
  (forward-compatible `.deb`) that runs `scripts/install-linux-build-deps.sh`
  (same script users run → green release proves the user dep list) then builds
  and publishes the `.deb` (and AppImage).
- `scripts/install-linux-build-deps.sh` — completed toolchain (items 3 + 4).
- Docs updated: `INSTALL.md`, `README.md`, `README-BETA.md`,
  `ordo-studio/LINUX_BUILD.md`.

## Verification (clean-room, real containers)

Reproduced and fixed on clean containers (no desktop, no build tools
pre-installed), so every gap surfaced for real instead of being guessed:

- **Build:** `ubuntu:22.04` built the full `.deb` via `Build-Ordo-Linux-Deb.sh`.
  `mozjs_sys v140` (SpiderMonkey), `aws-lc-sys`, `mozangle`, `mozjs`, and the
  whole Servo shell compiled with **0 errors** using the completed toolchain.
  Pinned to 22.04 so the `.deb` is forward-compatible with 24.04.
- **Lockfile:** regenerated `ordo-studio/package-lock.json` passes `npm ci` on
  **both** Linux (node 24) and Windows (node 24) — 143 packages, exit 0.
- **Runtime Depends:** derived from the shipped binaries' actual `NEEDED` list.
  `ordo` hard-links `libtss2-esys.so.0` + `libtss2-tctildr.so.0` (TPM) and does
  *not* need `libssl3` (aws-lc is static). `ordo-servo-shell` hard-links
  `libfontconfig1`; GL/X11/Wayland are `dlopen`'d and declared explicitly. The
  TPM package names differ across releases (`-0` on 22.04, `-0t64` on 24.04), so
  Depends uses `|` alternatives.
- **Install:** the `.deb` installs cleanly on **clean `ubuntu:22.04` AND
  `ubuntu:24.04`** (`dpkg -i` → `apt-get install -f`, ending `ii ordo`), with
  every `NEEDED` lib resolved and `ordo --help` running. The app-menu entry
  registers (`Exec=/opt/ordo/bin/ordo-launcher`).

Not covered headlessly: actually opening the Servo window (needs a display) and
the runtime serving the Studio bundle from `/opt/ordo/ordo-studio/dist`. Worth a
real Pop!_OS desktop smoke test before tagging a release.

## Follow-ups

- `.deb` runtime `Depends:` is incomplete (missing fontconfig/freetype/harfbuzz/
  tss2 runtime libs); corrected from the built binary's actual `NEEDED` list.
- `ordo-secrets-vault`'s TPM sealer makes `libtss2-dev` a hard build requirement
  on Linux. Consider feature-gating it so non-TPM builds don't need it.
- The release `.deb` build is heavy (full Servo compile). It runs only on
  release tags / manual dispatch, not every push.
