# Skill Routing — Hybrid (frontmatter declares + mode vetoes)

Status: **in progress** (staged build). This document is the design + build log
for making skill→mode routing scale to operator/community **custom** skills,
without weakening mode isolation.

## Why this exists

A pre-public audit asked: "the planner chooses what skills each mode gets — that
won't work as people add custom skills." Investigation (three parallel readers
over the codebase) showed the concern is valid, and sharper than the framing.

### "Skill" means two different things today

1. **Capability skill-cards** — `ordo-assistant/src/seeder.rs:234`
   (`capability_skill_card`) turns each of the ~120 **bus capabilities**
   (`filesystem.read_file`, `knowledge.summarize`, …) into a self-knowledge RAG
   entry (`KnowledgeKind::Skill`). The assistant finds these via
   `assistant.knowledge_lookup` — dynamic, description/semantic-based. Modes gate
   them by **lane prefix** (`ordo-modes/src/manifest.rs` `allows_capability`,
   `allowed_tool_lanes` / `blocked_tool_capabilities`). These already scale.

2. **Markdown `SKILL.md` playbooks** — `user-files/skills/<id>/` (the
   Claude-Code-style procedural skills: `ordo-build-*`, `rust-vibe-coder`,
   `ordo_rust_architecture`, …). Discovered by a dynamic directory scan, but:
   - surfaced **only to the Studio UI** (`ordo-studio/src-tauri/src/backend.rs`
     `list_local_capabilities` → `local_skill_descriptors`, line ~497) and
     lifecycle-managed by `skills.list/install/delete` (MaintenanceProvider,
     `ordo-mcp-host/src/lib.rs`);
   - **not advertised on the runtime bus**, so the seeder/self-knowledge RAG do
     **not** auto-ingest them — the general assistant does not discover them via
     `knowledge_lookup`;
   - reach work **only through the build pipeline**, where `ordo-build-planner`
     releases them **by hardcoded name** in a fixed 6-step sequence (the
     `ordo-build-pipeline` skill: "the coder never chooses its own skills").

### The two real gaps

- **Gap A — build pipeline.** Step skills are selected by hardcoded name /
  sequence. A custom build-step skill cannot slot in.
- **Gap B — general modes.** `ModeManifest` has `allowed_tool_lanes`,
  `rag_domains`, … but **no skill routing field at all**, and skill frontmatter
  like `available_to_modes` (already present in several skills) is **never
  parsed**. So a custom markdown skill is neither auto-surfaced to the assistant
  nor scopable to a mode, and an operator cannot keep, e.g., an exploit-focused
  skill out of a high-isolation mode.

The metadata to fix this **already exists** in the skills: a de-facto convention
of fenced ` ```yaml ` blocks under `## Installation Metadata` /
`## Mode Assignment Guidance` carrying `id`, `category` (≈ tags),
`available_to_modes`, `risk_level`, `requires_tools`,
`persistent_memory_access`. It is simply never read by the runtime.

## Decision

- **Scope:** Both surfaces. Fix Gap B (general modes) first, then make the build
  pipeline (Gap A) consume the **same** metadata-driven registry.
- **Routing model:** **Hybrid** — a skill **self-declares** where it belongs
  (frontmatter), and a **mode can veto** (and may broaden). Frontmatter is the
  scale mechanism (custom skills opt themselves in with zero central config);
  the mode veto is the safety backstop that preserves isolation.

## Skill metadata model (normalized)

Parsed tolerantly from the messy real-world formats (top `---` YAML frontmatter,
a bare `lane:` first line, and fenced ` ```yaml ` blocks in the body). Unknown
keys are ignored; missing keys take safe defaults.

| Field | Source keys | Meaning | Default |
|---|---|---|---|
| `id` | dir name, `id`, `name` | stable id (dir name wins) | dir name |
| `name` | frontmatter `name` | display name | `id` |
| `description` | frontmatter `description`, else `## Loader Hook` / `## Purpose` | what it is / when to use | "" |
| `tags` | `category`, `tags` | routing tags | `[]` |
| `modes` | `available_to_modes` | modes the skill self-declares for | `[]` (= undeclared) |
| `risk_level` | `risk_level` | `low` \| `medium` \| `high` | `medium` |
| `requires_tools` | `requires_tools` | informational | `false` |
| `lane_label` | `lane:` | UI grouping label | "Installed Skills" |

## Routing rule (the hybrid contract)

A skill `S` is **offered** in mode `M` iff:

```
allows_skill(M, S):
    # --- veto first; safety always wins ---
    if S.id   in M.blocked_skills:        return false
    if any(t in M.blocked_skill_tags for t in S.tags): return false
    if M.max_skill_risk is Some(ceil) and rank(S.risk_level) > rank(ceil): return false

    # --- admission ---
    if S.modes is non-empty:                       # skill self-declares
        if M.id in S.modes:                 return true
        # operator may still broaden a non-declared mode via a tag allow:
        if any(t in M.allowed_skill_tags for t in S.tags): return true
        return false
    if any(t in M.allowed_skill_tags for t in S.tags):     # undeclared, tag opt-in
        return true
    # undeclared, no tag match -> per-mode default
    return M.default_skill_admission != "restrictive"
```

`rank: low=0, medium=1, high=2`. Precedence: **veto > self-declaration >
tag-allow > per-mode default**. Isolation modes set `default_skill_admission:
"restrictive"` and/or `max_skill_risk: "low"`; general modes default permissive.

### Security / isolation invariants

- This affects **discovery/surfacing only**, never execution authority. A skill
  being "offered" does not grant any capability — tool calls remain gated by the
  existing `allowed_tool_lanes` / `blocked_tool_capabilities` lane checks and the
  per-subagent isolation guard. A skill is just instructions; it cannot widen a
  mode's hands.
- **Fail-closed on veto**: a vetoed skill is never offered regardless of its
  self-declaration. New mode fields are `#[serde(default)]` so existing modes
  load unchanged (empty veto lists, permissive default ⇒ today's behavior for
  capability skill-cards is preserved; markdown skills become *visible* where
  previously invisible, which is the intended fix).

## Diagnostics: daily routing audit + bounded self-repair

The diagnostic mode (`id: "diagnostic"`, `ordo-modes/src/defaults.rs`) is the
runtime's bounded self-healing authority — read-wide, write-narrow, its memory
private (`cross_mode_borrow/consult = deny`), and its doctrine already says it
"may maintain peripheral configuration such as … skills … through approved
maintenance tools" and must "classify every finding as symptom, evidence,
likely cause, safe repair, risky repair, or deferred operator decision." Skill
routing is exactly the kind of config that drifts as custom skills are added, so
the diagnostic mode gets a routing audit on its recurring scan.

### What "routed correctly" means (audit anomaly taxonomy)

For every (skill, mode) pair the audit computes `mode.allows_skill(skill)` and
classifies:

- **orphaned** — admitted by *no* mode (dead skill: vetoed/undeclared everywhere
  under restrictive defaults). _safe-repair candidate_ (skill-side).
- **declared-but-vetoed** — the skill self-declares a mode that then vetoes it
  (`blocked_skill[_tags]` / `max_skill_risk`). A real contradiction.
  _deferred_ (mode-side).
- **phantom-mode** — `available_to_modes` names a mode id that does not exist
  (typo). _safe-repair candidate_ (skill-side).
- **undeclared** — no `available_to_modes`, no tag match: relies on each mode's
  default admission. _informational._

### Repair scope (what the diagnostic mode may actually do)

- **Safe, auto-applyable (skill-side):** rewrite a skill's own `skill.md`
  frontmatter via `skills.install` (overwrite) — e.g. correct a `phantom-mode`
  typo to the unambiguous real mode id. The diagnostic mode is granted a
  `skills.` lane for this (and `skills.audit_routing` / `skills.list`), but
  `skills.delete` stays blocked — deletion is destructive, defer it.
- **Risky / deferred (mode-side):** anything that edits a *mode* manifest
  (loosen a veto, add an `allowed_skill_tag`, raise `max_skill_risk`). There is
  **no runtime mode-edit capability** (modes are file-only), so these are
  *structurally* impossible to auto-apply — the audit records them as deferred
  operator decisions, never touching mode policy itself. This is a feature: the
  isolation boundary cannot be self-modified.

### Audit trail + scheduling

- Findings + applied repairs are recorded to the self-heal store
  (`ordo-heal`, `heal_cases`) with a `skill-routing` source, into the
  diagnostic mode's private scope — same machinery the medbay already uses.
- A **daily automation** (`AutomationTrigger::Heartbeat(86400)` /  Cron) with a
  new `AutomationIntent::SkillRoutingAudit`, `scope: Diagnostic`, runs the audit
  capability as `mode:"diagnostic"`. Read-only audit auto-runs
  (`ApprovalPolicy::Never`); safe skill-side repairs are applied within the
  diagnostic sandbox; deferred items surface for the operator.

## Staged plan

- **Stage 1 — `ordo-skills` crate (foundational, no behavior change).**
  Typed `SkillManifest` + tolerant frontmatter parser + `discover_skills(root)`
  directory scanner. Unit-tested against the real in-repo skills. Leaf crate.
  _Status: **DONE.**_
- **Stage 2 — mode-side routing.** Add `allowed_skill_tags`,
  `blocked_skill_tags`, `blocked_skills`, `max_skill_risk`,
  `default_skill_admission` to `ModeManifest` (all `#[serde(default)]`) and an
  `allows_skill(&SkillManifest)` method implementing the rule above.
  `ordo-modes` now depends on the `ordo-skills` leaf crate (keeps risk-rank
  semantics single-sourced). Unit-tested; defaults backward-compatible.
  _Status: **DONE.**_
- **Stage 3 — `SkillRegistry` + surface to the general assistant.** A registry
  that discovers skills and answers `skills_for_mode` (filter via
  `allows_skill`). Ingest discovered markdown skills into the self-knowledge RAG
  (real description + declared modes/tags) and filter what a mode sees. Custom
  skills become discoverable and mode-scoped. _Status: pending._
- **Stage 4 — `skills.audit_routing` (read-only).** A pure function over the
  registry + all mode manifests producing the anomaly report (orphaned /
  declared-but-vetoed / phantom-mode / undeclared) with per-(skill,mode)
  verdicts + veto reasons. New capability in the maintenance surface. The
  diagnostic mode's "verify routing is correct" tool.
  _Status: **DONE** — `ordo-modes::audit` engine + `skills.audit_routing`
  capability (MaintenanceProvider), live-validated._
- **Stage 5 — diagnostic authority + bounded skill-side repair.** Grant the
  diagnostic mode a `skills.` lane (keep `skills.delete` blocked); add
  `skills.repair_routing` applying ONLY safe skill-frontmatter fixes
  (phantom-mode typo correction) via overwrite, recording to the self-heal
  store; mode-side issues recorded as deferred. _Status: pending._
- **Stage 6 — daily-scan automation.** New `AutomationIntent::SkillRoutingAudit`
  → audit capability, `scope: Diagnostic`, daily trigger, run as
  `mode:"diagnostic"`; seed a default automation. _Status: pending._
- **Stage 7 — build pipeline (Gap A).** Make `ordo-build-planner` consume the
  registry: select step skills by metadata (a `pipeline-step` tag + ordering)
  rather than hardcoded names, honoring the veto. _Status: pending._
- **Stage 8 — Studio UI.** Show each skill's modes/tags/risk + the audit report;
  an editor that writes frontmatter; de-dupe the studio's ad-hoc parser onto
  `ordo-skills`. _Status: pending._
- **Each stage:** build with `RUSTFLAGS=-D warnings`, unit tests, and an
  adversarial review for the isolation-sensitive stages (2–6).

## Build log

- **Stage 1 (done).** New leaf crate `ordo-skills`: `SkillManifest` +
  `RiskLevel` + tolerant subset parser (`from_markdown`) + `discover_skills`.
  Handles all three frontmatter styles (top `---`, bare `lane:`, fenced
  ` ```yaml ` metadata blocks); prose colons are not mis-parsed. 7 unit tests;
  `-D warnings` clean. Smoke-tested against all 15 in-repo skills: the `ordo_*`
  skills yield their declared `available_to_modes`/`category`; the build-pipeline
  + `rust-vibe-coder`/`spiderweb-bus` skills are correctly undeclared (empty
  modes/tags → per-mode default at routing time). No wiring yet — pure model.
- **Stage 2 (done).** `ModeManifest` gains five `#[serde(default)]` skill-routing
  fields + `allows_skill(&SkillManifest)` (veto > self-declaration > tag-allow >
  per-mode default), plus `normalize_and_validate` checks for `max_skill_risk`
  and `default_skill_admission`. `ordo-modes` → `ordo-skills` dep added. Fixed
  the five pre-existing `ModeManifest { … }` test literals in built crates
  (`registry.rs` ×2, `ordo-assistant` `service.rs` + `prompt.rs`) to set the new
  fields (orphaned `ordo-mode-planners` is not in the workspace — left alone).
  `ordo-modes` 51 tests pass; `ordo-assistant` tests compile; `-D warnings`
  clean. Existing mode JSON loads unchanged (new fields default empty/None ⇒
  today's behavior preserved).
- **Stage 4 core (done).** `ModeManifest::allows_skill` refactored to delegate to
  a new `skill_verdict(&SkillManifest) -> SkillDecision` (Admitted{Declared,Tag,
  Default} / Vetoed{ById,ByTag,ByRisk} / Rejected{NotDeclared,Restrictive}) with
  a `.reason()` — same behavior, now explainable. New `ordo-modes::audit` module:
  `audit_skill_routing(modes, skills) -> RoutingAudit` flags Orphaned /
  DeclaredButVetoed / PhantomMode / Undeclared per skill, read-only. `ordo-modes`
  56 tests pass (5 new); `-D warnings` clean. This is the engine the diagnostic
  daily scan + the `skills.audit_routing` capability will call.
- **Stage 4 wiring (done, live-validated).** `skills.audit_routing` capability
  on the `MaintenanceProvider` (`ordo-mcp-host`): discovers skills from
  `<user-files>/skills`, lists all modes from the registry (passed via
  `.with_modes()` from `ordo-runtime`), runs `audit_skill_routing`, returns the
  full report + `orphaned`/`anomaly_count`/`unhealthy_count` summary. Errors
  cleanly (`Failed`) when no registry is attached. `ordo-mcp-host` 44 tests (2
  new); `-D warnings` clean. Live on :4142 over the 15 real skills: HTTP 200,
  modes=20, skills=15, **orphaned=[]** (the mode rework healed the old orphans —
  they now route via the new `coding`/`research` modes + dev-mode skill tags),
  22 anomalies — all `phantom_mode` (the 4 `ordo_*` skills still declare
  `orchestration`/`runtime`/`legal_admin`, which don't exist) or informational
  `undeclared`. Those phantom-mode declarations are the S5 safe-repair target.
