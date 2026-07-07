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
  - Diagnostics, setup, maintenance, MCP security, provider setup, local
    computer access rules, and avatar setup guidance.
- `automation`
  - Routines, hooks, schedules, webhooks, local events, and dreaming reviews.
- `remote_communication`
  - Email, account access levels, Ordo communication identity, and future
    approved communication channels.
- `providers`
  - Local/cloud provider setup, model lifecycle, model switching, LM Studio,
    Ollama, and compatible endpoints.
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
  BM25 over an inverted index, PPMI co-occurrence query expansion for
  corpus semantics, and bigram fuzzy matching for typos and identifier
  variants. It is exact, deterministic, and first-class — not a
  fallback.
- An Ollama or llama.cpp embedding model, when configured, adds an
  optional re-ranking channel on top; if the model is unavailable,
  retrieval degrades to the default engine instead of failing.
- Retrieval self-learns from usage, still with no model: feedback on a
  hit (`ordo.rag.feedback.request`) reinforces the chunk, teaches the
  query's terms to route toward the hit's collection, and bridges the
  query's vocabulary to the chunk's in the co-occurrence matrix — so
  wording the corpus never used still finds the right chunks after one
  confirmation. Learning is bounded (clamped bonuses, fading
  affinities, capped bridges) and never conjures a hit with no
  retrieval evidence of its own.

## Why This Exists

- Small models need smaller, cleaner context.
- Mode and skill routing should not flood every turn.
- Retired SEO/CMS/creative guidance should not pollute current Ordo behavior.
- Retrieval should explain what it used and why.
