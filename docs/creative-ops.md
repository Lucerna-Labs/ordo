# Creative Ops Roadmap

Ordo is the creative-operations fork of the inherited Codex Claw
runtime. The backend is still the same local-first Tokio-bus system, but the
product direction is now centered on coordinating creative work from intake to
publish.

This roadmap is the umbrella view. Domain-specific skill and tool boundaries
now live in:
- `docs/domain-map.md`
- `docs/domains/creative.md`
- `docs/domains/workflow.md`
- `docs/domains/seo.md`
- `docs/domains/cms.md`

## Target operating model

- Intake and briefing:
  - capture campaign requests, creative briefs, channel requirements, and
    deadlines
- Production planning:
  - break briefs into deliverables, dependencies, and approval checkpoints
- Asset workflow:
  - track drafts, revisions, owner handoff, and final-ready packaging
- SEO packaging:
  - prepare titles, descriptions, slugs, internal-link notes, structured
    metadata, and search intent framing
- CMS preparation:
  - map content to templates, fields, taxonomies, collections, and publish
    states
- Publish and feedback:
  - prepare publish bundles, record launch status, and feed performance notes
    back into future planning

## Product truths

- Ordo should keep creative assets and user-owned content on the
  user-files side of the system rather than mixing them into runtime state.
- Workflow state, guidance, and historical decisions should remain queryable
  through the same bus-first model as the rest of the runtime.
- Brand rules, editorial standards, SEO checklists, and CMS field contracts are
  good candidates for pinned memory and retrieval seeding.
- Retrieval should stay split between focused domain collections and a compact
  `main` creative-intelligence collection for shared guidance such as design,
  marketing, writing, typography, and color theory.
- The current fork should not pretend those creative providers already exist;
  it should use the inherited orchestration, retrieval, and control surfaces as
  the baseline for building them honestly.

## Near-term provider goals

- `creative.capture_brief`
- `creative.plan_pipeline`
- `workflow.route_review`
- `seo.package_metadata`
- `seo.audit_readiness`
- `cms.prepare_entry`
- `cms.publish_bundle`

## Success criteria for the fork

- A creative operator can understand what stage a piece of work is in.
- SEO and CMS readiness are visible before publish time instead of being late
  manual afterthoughts.
- The runtime can answer workflow questions from seeded project docs and pinned
  operational memory.
- Future domain-specific providers can slot into the existing planner, memory,
  and capability inventory surfaces without reworking the architecture.
