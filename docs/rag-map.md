# RAG Collection Map

Ordo treats retrieval as focused local collections instead of one uncontrolled
knowledge pile.

## Active Collections

- `main`
  - Core Ordo docs, operator guidance, runtime truths, and general knowledge.
- `assistant`
  - Assistant behavior, tool-use guidance, turn-loop notes, and self-knowledge.
- `modes`
  - Mode-specific guidance and behavior.
- `skills`
  - Global and per-mode skill descriptions.
- `agent_teams`
  - Team roles, role instructions, role skills, and bounded collaboration
    patterns.
- `tech_specialist`
  - Diagnostics, setup, maintenance, MCP security, resident cognition health,
    local computer access rules, and avatar setup guidance.
- `automation`
  - Routines, hooks, schedules, webhooks, local events, and dreaming reviews.
- `remote_communication`
  - Email, account access levels, Ordo communication identity, and future
    approved communication channels.
- `providers`
  - Resident cognition health, fixed-model identity, and lifecycle guidance.
    The legacy collection name remains for stored-data compatibility.
- `memory`
  - Pinned memory, working memory, dreaming proposals, and persistence rules.
- `security`
  - MCP/plugin trust, quarantine, drift checks, redaction, approvals, and audit
    rules.

## Retired Collections

The old `creative`, `seo`, and `cms` collections are retired. They should not be
seeded or routed by default.

## Retrieval Rules

- Always keep `main` compact and useful.
- Add focused collections only when the request needs them.
- Prefer a few relevant chunks over a large mixed context dump.
- The default retrieval engine is generative-free and needs no model:
  AtomDB converts query terms into independent activation lanes, ranks
  intersections in its immutable term/passage graph, and emits only bounded
  evidence. Exact phrase and bigram typo handling cover queries the term field
  cannot serve directly. This is the first-class path, not a fallback.
- An Ollama or llama.cpp embedding model, when configured, adds an
  optional re-ranking channel on top; if the model is unavailable,
  retrieval degrades to the default engine instead of failing.
- Retrieval self-learns from usage, still with no model: useful feedback on a
  hit (`ordo.rag.feedback.request`) reinforces the native passage path and adds
  durable query-cue bonds, so wording the corpus never used can find confirmed
  evidence later. Source-specific bonuses are clamped and version-scoped;
  replaced text cannot inherit stale feedback.

## Why This Exists

- Small models need smaller, cleaner context.
- Mode and skill routing should not flood every turn.
- Retired SEO/CMS/creative guidance should not pollute current Ordo behavior.
- Retrieval should explain what it used and why.
