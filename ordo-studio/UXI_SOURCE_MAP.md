# Ordo Studio UXI Source Map

This folder is the confirmed Ordo Studio UXI extracted from:

`I:\OpenClawBackups\ordo-V1\ordo-studio`

## Entry Points

- `src/main.tsx` mounts the React app.
- `src/App.tsx` loads the Studio shell.
- `src/OrdoShell.tsx` is the main 41-tab UXI (one monolith rendering every tab).
- `src/index.css` contains the global page styling.
- `src/ui.tsx` contains reusable UI primitives, colors, cards, fields, buttons, modals, and controls.

## Support Code

- `src/api.ts` contains the API client (Tauri `invoke` + control-API HTTP) used by the shell.
- `src/extensions/*` is the UI-extensions surface: `useUiExtensions` (lists
  `/api/ui-extensions`), `ExtensionHost` (sandboxed iframe + postMessage bridge),
  and `ExtensionsSurface` (the "Extensions" tab).

> The old island — `src/components/*`, `src/hooks/*`, `src/types.ts`,
> `src/fallbacks.ts` — was the previous wired shell. It was imported by nothing
> in the live tree and was **removed in the UXI reconcile** (recoverable from
> git history).

## Static HTML/CSS Copy (LEGACY — not a baseline)

- `static-html-css/` is a stale snapshot of the PREVIOUS shell ("the
  conversation is the control surface" / 15 surfaces), **not** the current
  41-tab UXI. Do not design against it (see `static-html-css/README.md`). The
  design baseline is the live `src/OrdoShell.tsx` + `UXI_DEV_NOTES.md`.

## Recovery Notes

- `UXI_DEV_NOTES.md` documents how this WebView/Tauri UXI was recovered, what
  must stay true, and the checklist to use if the UXI breaks again.

## Run

```powershell
npm ci --legacy-peer-deps
npm run dev -- --host 127.0.0.1 --port 5179
```

The confirmed preview is the Studio UI shown at `http://127.0.0.1:5179/`.
