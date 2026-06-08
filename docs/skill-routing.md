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

## Staged plan

- **Stage 1 — `ordo-skills` crate (foundational, no behavior change).**
  Typed `SkillManifest` + tolerant frontmatter parser + `discover_skills(root)`
  directory scanner. Unit-tested against the real in-repo skills. Leaf crate.
  _Status: **DONE.**_
- **Stage 2 — mode-side routing.** Add `allowed_skill_tags`,
  `blocked_skill_tags`, `blocked_skills`, `max_skill_risk`,
  `default_skill_admission` to `ModeManifest` (all `#[serde(default)]`) and an
  `allows_skill(...)` method implementing the rule above (takes primitives, so
  `ordo-modes` need not depend on `ordo-skills`). Unit-tested; defaults are
  backward-compatible. _Status: pending._
- **Stage 3 — surface to the general assistant.** Ingest discovered markdown
  skills into the self-knowledge RAG (real description + declared modes/tags),
  and filter what a mode sees by `allows_skill`. Custom skills become
  discoverable and mode-scoped. _Status: pending._
- **Stage 4 — build pipeline.** Make `ordo-build-planner` consume the registry:
  select step skills by metadata (a `pipeline-step` tag + ordering) rather than
  hardcoded names, honoring the veto, so a custom build-step skill can slot in.
  _Status: pending._
- **Stage 5 — Studio UI.** Show each skill's modes/tags/risk and an editor that
  writes the frontmatter; de-dupe the studio's ad-hoc parser onto `ordo-skills`.
  _Status: pending._
- **Each stage:** build with `RUSTFLAGS=-D warnings`, unit tests, and an
  adversarial review for the isolation-sensitive stages (2–4).

## Build log

- **Stage 1 (done).** New leaf crate `ordo-skills`: `SkillManifest` +
  `RiskLevel` + tolerant subset parser (`from_markdown`) + `discover_skills`.
  Handles all three frontmatter styles (top `---`, bare `lane:`, fenced
  ` ```yaml ` metadata blocks); prose colons are not mis-parsed. 7 unit tests;
  `-D warnings` clean. Smoke-tested against all 15 in-repo skills: the `ordo_*`
  skills yield their declared `available_to_modes`/`category`; the build-pipeline
  + `rust-vibe-coder`/`spiderweb-bus` skills are correctly undeclared (empty
  modes/tags → per-mode default at routing time). No wiring yet — pure model.
