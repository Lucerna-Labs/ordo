# Mode Lifecycle — protected core set + create / delete

Status: **in progress** (staged build). Design + build log for the "Create mode"
feature: a small **protected core set** ships built-in; operators **create**
their own modes at runtime and **delete** the ones they don't want — the core
set is delete-guarded so it can't be removed by accident.

## Why

Ordo previously shipped ~16 domain **templates** (business, personal,
investigations, special_projects_*, three per-OS tech specialists, …). That
doesn't scale and it rots: the skill-routing audit (`docs/skill-routing.md`)
found four skills declaring modes (`coding`, `research`, `runtime`,
`orchestration`, `security`, `legal_admin`) that didn't exist in that template
set. The fix is to stop shipping a pile of templates and instead ship a small,
opinionated, **protected** core and let people expand from there.

## Decisions (operator-confirmed)

- **Protected core set (7):** `general`, `rust_vibe_coder`, `coding`,
  `research`, `security_lab`, `tech_specialist` (one generic — the three per-OS
  specialists are collapsed), and `diagnostic` (system mode that runs the daily
  skill-routing audit). All ship `protected: true`.
- **The old templates are removed from shipped defaults ONLY.** Any that already
  exist on disk under `user-files/modes/` are left untouched and still load —
  removing a template from the compiled set never deletes an operator's existing
  mode or its scoped memory/RAG data.
- **`protected` guards DELETION, not editing.** An operator can tune a core
  mode's config; they just can't casually delete it. User-created modes are
  `protected: false` and freely deletable.

## Hard constraint discovered

There is currently **no runtime mode-edit capability** — `ModeRegistry::upsert`
is not exposed on the bus, and `/api/assistant/modes` is read-only (modes are
loaded from `user-files/modes/*.json` at startup). So "Create mode" is net-new
plumbing: a mode-lifecycle surface (create / rename / delete) that validates,
persists to disk, and hot-reloads the registry — plus the delete-guard and the
Studio UI. (This is also why the diagnostics audit defers all *mode-side*
routing fixes to the operator — the runtime structurally can't self-edit a mode.)

## New-mode defaults

A `modes.create {name}` makes a mode with safe, General-like defaults: id derived
from the name (`[a-z0-9_]`), `memory_scope: ["global", "mode:<id>"]`,
`rag_domains: ["research_<id>"]`, General's read lanes, permissive skill
admission, `protected: false`. The operator tunes lanes/skills/persona afterward.

## Staged plan

- **M1 — `protected` field + core-set defaults.** Add `protected` to
  `ModeManifest`; rewrite `defaults.rs` to the 7-mode protected core; fix
  ripple in tests/literals. _Status: **DONE.**_
- **M2 — registry CRUD + persistence + delete-guard.** `ModeRegistry`
  create/delete/rename that write/remove `user-files/modes/<id>.json` and refuse
  to delete a `protected` mode (unless an explicit force). Hot-reload. Unit
  tested. _Status: pending._
- **M3 — `modes.*` capability + control routes.** `modes.create / rename /
  delete / list` via a provider + `POST/PATCH/DELETE /api/assistant/modes`.
  Wire into the lane allowlist (operator-facing, not assistant-autonomous).
  _Status: pending._
- **M4 — Studio UI.** "Create mode" button + name field; mode list with a
  delete control disabled/guarded for protected modes. _Status: pending._
- **Each stage:** build `-D warnings`, unit tests, adversarial review for the
  delete-guard / protection boundary.

## Follow-ups

- ~~`ordo-classify::mode_classifier` carries its OWN hardcoded mode list out of
  sync with the shipped core set.~~ **RETIRED** — the TF-IDF text→mode
  auto-router had no callers and conflicts with the architecture (mode is chosen
  explicitly at session creation; going forward, operator-created). Deleted the
  module; `ordo-classify` now only does message traffic/route classification.

## Build log

- **M1 (done).** `ModeManifest.protected: bool` (`#[serde(default)]`).
  `defaults.rs` rewritten from 16 templates to the 7-mode protected core; the
  dev/research modes carry `allowed_skill_tags` (`rust`/`architecture`/`coding`/
  `research`) so tag-bearing skills are admitted there even when their
  `available_to_modes` is stale (the audit still flags the stale declaration).
  `diagnostic` gains the `skills.` lane (with `skills.delete` blocked) so it can
  run the routing audit + safe repairs. Fixed ripple: the five `ModeManifest`
  test literals (+`protected: false`) and the registry tests that hard-coded 16
  modes / removed mode ids. `ordo-modes` 53 tests pass; `ordo-classify` 21 pass;
  `ordo-runtime`/`ordo-control` build; `-D warnings` clean.
