# RAG Collection Map

Ordo now treats retrieval as a set of focused collections instead of
one flat knowledge pile.

## Collections

- `main`
  - Cross-domain creative intelligence, platform docs, design guidance,
    marketing basics, writing modes, typography, and color theory.
- `creative`
  - Briefs, campaign structure, deliverables, creative direction, asset
    packaging.
- `workflow`
  - Stages, review, approval, revisions, routing, handoffs.
- `seo`
  - Search intent, metadata, on-page packaging, audit readiness.
- `cms`
  - Entries, templates, fields, taxonomies, publish bundles.
- `ssh`
  - Remote shell execution and host-oriented operational guidance.
- `api`
  - Generic service integrations and client configuration guidance.
- `rest`
  - Endpoint/resource-oriented REST request guidance.

## Retrieval rule

- The runtime always includes `main` as the base collection.
- Goal inference then adds any matching domain or interface collections.
- Retrieval should prefer a few focused documents over a large mixed pile.

## Why this exists

- Creative guidance stays separate from workflow logic.
- SEO and CMS context stop getting buried under generic docs.
- Interface guidance does not pollute domain retrieval.
- The main collection can stay small and useful instead of becoming the default
  home for every document.
