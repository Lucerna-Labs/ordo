# Ordo Studio UXI Source Map

This folder is the confirmed Ordo Studio UXI extracted from:

`I:\OpenClawBackups\ordo-V1\ordo-studio`

## Entry Points

- `src/main.tsx` mounts the React app.
- `src/App.tsx` loads the Studio shell.
- `src/OrdoShell.tsx` is the main 15-tab UXI.
- `src/index.css` contains the global page styling.
- `src/ui.tsx` contains reusable UI primitives, colors, cards, fields, buttons, modals, and controls.

## Support Code

- `src/api.ts` contains API client functions used by the shell.
- `src/types.ts` contains shared TypeScript types.
- `src/fallbacks.ts` contains fallback/demo data.
- `src/components/*` contains older/supporting component surfaces still referenced by the app or useful for lifting patterns.
- `src/hooks/*` contains React hooks for system state and UI extensions.

## Static HTML/CSS Copy

- `static-html-css/index.html` contains a standalone HTML copy of the confirmed UXI surface.
- `static-html-css/styles.css` contains the matching standalone CSS.
- `static-html-css/README.md` explains the static copy.

## Recovery Notes

- `UXI_DEV_NOTES.md` documents how this WebView/Tauri UXI was recovered, what
  must stay true, and the checklist to use if the UXI breaks again.

## Run

```powershell
npm ci --legacy-peer-deps
npm run dev -- --host 127.0.0.1 --port 5179
```

The confirmed preview is the Studio UI shown at `http://127.0.0.1:5179/`.
