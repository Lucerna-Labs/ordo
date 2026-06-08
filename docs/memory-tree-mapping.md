# Memory Tree Mapping â€” RAG â†’ Tree Node Exercise

**Status: HOMEWORK, pre-filled template. Complete before the next
coding session opens.** See `docs/memory-architecture.md` for why
this exists and what happens after.

The act of filling this table is the work. Every code-edit answer
that seemed obvious evaporates the moment a second column has to be
written for it. Don't skip columns. Don't leave "TBD" if the answer
is knowable â€” replace every TBD with the real value before any
`MemoryRouterService::register_provider` call gets typed.

---

## The table

One row per existing RAG collection. All eight collections currently
live in `ordo-rag`; blueprint shorthand said "10 RAGs" but the
actual inventory is eight (see `docs/rag-map.md`).

| RAG name   | Primary domain                  | serves_paths (tree nodes)              | Retrieval semantics | Provenance native? | Cost hint | Notes / uncertainties |
| ---------- | ------------------------------- | -------------------------------------- | ------------------- | ------------------ | --------- | --------------------- |
| `main`     | Cross-domain base               | `platform/cross`                        | Hybrid              | TBD â€” verify       | Cheap     | The always-included base. Likely needs SPLITTING into `platform/design`, `platform/writing`, `platform/marketing` before provider registration â€” today it's a catch-all, which is exactly what the blueprint warns against. |
| `creative` | Creative operations             | `creative/briefs`, `creative/campaigns`, `creative/assets` | Hybrid | TBD â€” verify       | Moderate  | Three tree nodes, one RAG. Register the RAG against all three paths; the router's `serves_paths: Vec<String>` takes a list. Alternative: three separately-seeded RAG instances, one per node. Decide before registration. |
| `workflow` | Stage/review/approval           | `workflow/review`, `workflow/routing`    | Lexical â†’ Hybrid    | TBD â€” verify       | Cheap     | Workflow content is usually short operational notes â€” lexical BM25 likely beats dense embeddings. Start lexical; promote to hybrid if recall suffers. |
| `seo`      | Search packaging                | `seo/metadata`, `seo/audit`             | Hybrid              | TBD â€” verify       | Moderate  | Some SEO docs are specific enough that exact match ranks first (e.g. field-name lookups). Consider exposing an exact-lookup capability on this provider in addition to hybrid. |
| `cms`      | CMS field + taxonomy            | `cms/entries`, `cms/taxonomies`         | Hybrid              | TBD â€” verify       | Moderate  | Tree taxonomy guidance lives inside a retrieval target about taxonomies â€” recursion is fine, but double-check that CMS-doc retrieval doesn't get confused with memory-tree routing decisions. |
| `ssh`      | Remote shell / host ops         | `interface/ssh`                         | Lexical             | TBD â€” verify       | Cheap     | Command-level lookups are almost always exact-match or lexical. Dense embeddings here likely overkill. |
| `api`      | Generic service integration     | `interface/api`                         | Hybrid              | TBD â€” verify       | Moderate  | Intentionally distinct from `rest` per `docs/interface-map.md` â€” don't merge. |
| `rest`     | REST endpoint contracts         | `interface/rest`                        | Hybrid              | TBD â€” verify       | Moderate  | Same separation rule as `api`. |

### How to fill the `Provenance native?` column

For each RAG, look at the code in `ordo-rag/src/` where retrieval
results are assembled. The column is **yes** iff the hit struct
carries at least:

- `source` (document id / uri)
- `chunk_index` (or equivalent intra-doc locator)
- `score` or equivalent confidence signal

If any of those is missing, the column is **no** â€” and the provider
wrapper in `ordo-memory-router` will synthesize provenance
(`provider_id + timestamp + input query hash`) at the router layer
rather than trusting the provider's output.

Default assumption pre-verification: **yes** (the current RAG code
paths appear to include document_id + chunk_index + score in
`RagHit`). Verify anyway.

### How to pick `Retrieval semantics`

Three-rule flowchart:
1. If the content is short imperative notes (commands, field
   names, small structured data) â†’ **Lexical**.
2. If the content is free-form prose (briefs, guidance, examples)
   â†’ **Hybrid** (lexical + dense, then rerank).
3. If lookups are literal string matches (ids, slugs, URIs) â†’
   **Exact**.

**Dense-only is almost never the right first pick.** Lexical
baselines win in real usage more often than expected; dense shines
when vocabulary diverges between query and content.

### How to pick `Cost hint`

- **Cheap** â€” in-process lookup, no network, <50ms median.
- **Moderate** â€” in-process but embedding computed per-query, or
  large local corpus.
- **Expensive** â€” remote (OpenAI embeddings, Qdrant Cloud, etc.) or
  >500ms median.

Cost feeds the router's under-pressure prioritization: when fast
mode is borderline-confident, the router should prefer cheap
providers.

---

## Tree additions required before registration

The current seed tree (`memory_tree_nodes`) is empty at boot. The
mapping above implies the following node set; they must all be
upserted via `MemoryRouterService::upsert_node` before any provider
registers against them, or registrations will happily attach to
non-existent paths (no validation today) and fast-mode routing will
never match.

```
platform/cross
platform/design            â† new, derived from splitting `main`
platform/writing           â† new, derived from splitting `main`
platform/marketing         â† new, derived from splitting `main`
creative/briefs
creative/campaigns
creative/assets
workflow/review
workflow/routing
seo/metadata
seo/audit
cms/entries
cms/taxonomies
interface/ssh
interface/api
interface/rest
```

Total: 16 nodes. Descriptions and retrieval hints for each should
be concrete enough that the fast router's BM25-over-descriptions
pass produces useful signal â€” vague one-liners kill routing
accuracy.

### Specifically open questions to settle before the session

1. **Is splitting `main` worth it?** The RAG today is a single
   store. Splitting to three tree nodes means either (a) one RAG
   serving three paths (cheap, but the RAG's internal content
   doesn't map cleanly to the three nodes â€” every query hits the
   whole store regardless of which node matched) or (b) three
   separate RAG instances carved out of the current `main` by
   content classification (expensive one-time exercise but
   gives actual per-node specificity). **Recommendation:** (a)
   for v1; revisit after a month of real queries.
2. **Do any RAGs need two providers per path?** The blueprint
   mentions "both a lexical and dense provider for
   voice-examples" as an example. For Lucerna voice work
   specifically, a separate `lucerna/voice` node with BOTH a
   lexical provider (exact-match for known phrases) and a dense
   provider (semantic recall of adjacent phrases) may be worth it.
   This node is NOT in the table above â€” flag for operator
   decision.
3. **What serves `lucerna/voice`?** Not currently a RAG. The
   blueprint puts it front-and-center. Options: seed a new RAG
   from existing voice examples; use `main` against the
   `platform/writing` node (too diffuse); wait until Phase 2 when
   voice content is large enough to warrant its own store. Operator
   call â€” the blueprint flags voice examples as brand-critical,
   which argues for not waiting.

---

## The exercise, as a checklist

- [ ] All 8 RAGs have a concrete `serves_paths` entry (no "TBD")
- [ ] Every `Provenance native?` column is **yes** or **no** â€” not
      "probably"
- [ ] Every `Retrieval semantics` is one of Lexical / Dense /
      Hybrid / Exact â€” picked via the three-rule flowchart
- [ ] Every `Cost hint` is Cheap / Moderate / Expensive
- [ ] Tree-additions-required list is minimized: any path that
      doesn't have both a description and at least one provider
      plan gets pruned or its creation is deferred
- [ ] The three open questions above have a recorded answer (even
      if the answer is "deferred to phase 2")
- [ ] The doc passes a "can a second operator register the first
      provider from this table alone" read-through

Only after every checkbox is ticked does the session open. First
commit that session: register the `workflow` RAG (cheapest row,
simplest content) end-to-end, watch the router produce non-empty
fast-mode results, land that, then batch the rest.

---

## Historical note

This mapping doc is the artifact the blueprint called "a paper
exercise before any provider registration code." See `memory-blueprint-v2` Â§"Crate 2 â€” router". The `ordo-memory-router`
crate is alive on the bus today with an empty registry; filling it
cleanly is what this doc enables. Do not skip it.
