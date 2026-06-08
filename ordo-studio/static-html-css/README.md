# Static HTML/CSS Copy — LEGACY old-shell snapshot (NOT a design baseline)

> ⚠️ **Do not design new UXI against this folder.** This is a static snapshot
> of the *previous* "wired shell" (the single-screen "the conversation is the
> control surface" / "15 surfaces" layout). That shell was replaced by the
> live `OrdoShell` (a 41-tab operator shell) and is **explicitly not** the
> current design — `UXI_DEV_NOTES.md` even forbids restoring its
> "conversation is the control surface" headline.

It is kept only as a historical reference for the old visual language.

## The current design baseline is the live UXI

When building or matching Ordo Studio UXI, the source of truth is:

- the live React shell **`ordo-studio/src/OrdoShell.tsx`** (run `npm run dev`
  and open it — 41 tabs across primary / agent / knowledge / connectivity /
  advanced / docs);
- **`ordo-studio/UXI_DEV_NOTES.md`** — the canonical UXI rules/spec.

Match the live shell's operator-first principles (visible controls, unified
tabs, readable status, surfaced logs, no overlapping scroll panes, no bland
default coder UI) — but take the *layout and content* from the live shell, not
from the stale `index.html` / `styles.css` here.
