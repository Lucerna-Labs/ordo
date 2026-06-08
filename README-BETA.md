# Ordo (WebView Beta)

Lucerna Labs · local-first AI runtime with the Ordo Studio React UI hosted in WebView2.

This folder is everything you need. Three ways to use it:

---

## 1. Portable — run directly from this folder

Double-click **`ordo.exe`**. The runtime boots, port `127.0.0.1:4141` comes
up, and the WebView2 window opens with the Studio UI. Your data lives in
`data/` and `user-files/` inside this folder. No install, no admin.

To get a Desktop launcher pointing at this portable copy:
1. Double-click **`Install-Desktop-Shortcut.cmd`**
2. A `Ordo (WebView Beta).lnk` appears on your Desktop pointing at this folder

---

## 2. Installer — `ordo-webview-beta.msi`

Double-click the MSI to install per-user (no UAC):

- Files go to `%LOCALAPPDATA%\Ordo (WebView Beta)\bin\`
- Start Menu tile + Desktop shortcut both created automatically
- Data lives in the install folder (writable user location, no admin needed)
- Uninstall normally via Settings → Apps

---

## 3. Build from source

```
cd ordo-studio && npm install --legacy-peer-deps && npm run build
cd ..
cargo build --release -p ordo-cli
```

Output: `target\release\ordo.exe`. Same as the bundled `ordo.exe`.

---

## What's where

| File | What it is |
|---|---|
| `ordo.exe` | The portable runtime + WebView2 host binary (release build) |
| `ordo-webview-beta.msi` | Per-user Windows installer |
| `Install-Desktop-Shortcut.cmd` | Portable-mode Desktop launcher creator |
| `README-BETA.md` | This file |
| `ordo-studio/dist/` | Bundled React UI (needed alongside `ordo.exe` for portable mode) |
| `ordo-*/` (51 crates) | Full Rust source for the workspace |
| `Cargo.toml`, `Cargo.lock` | Workspace manifest + pinned versions |
| `docs/` | Internal docs + the canonical UXI design spec |

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
