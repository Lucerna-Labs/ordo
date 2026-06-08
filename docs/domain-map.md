# Domain Map

Ordo should not mix every future skill and tool into one generic
"creative ops" bucket. The system should separate domain knowledge and
capabilities by prefix and ownership.

## Domain prefixes

- `creative.*`
  - intake, briefs, concepts, deliverables, assets, campaign packaging
- `workflow.*`
  - stage routing, handoff, review, approval, revision, scheduling
- `seo.*`
  - metadata packaging, search intent, audit checks, link notes, snippet prep
- `cms.*`
  - field mapping, entry preparation, taxonomy assignment, publish/export

## Separation rules

- Domain-specific reasoning should live in the matching domain docs and pinned
  memory, not in one blended catch-all playbook.
- Capability names should clearly advertise their domain prefix.
- Review and approval logic belongs under `workflow.*` even when the content is
  creative or SEO-related.
- Metadata and search packaging belong under `seo.*`, not under generic content
  creation.
- Template fields, entry payloads, and publishing state belong under `cms.*`.
- Shared runtime primitives such as memory, retrieval, filesystem, transport,
  and self-heal remain cross-domain infrastructure.

## Example capability layout

- `creative.capture_brief`
- `creative.plan_campaign`
- `creative.package_assets`
- `workflow.route_review`
- `workflow.request_revision`
- `workflow.schedule_release`
- `seo.package_metadata`
- `seo.audit_readiness`
- `seo.generate_internal_links`
- `cms.prepare_entry`
- `cms.map_taxonomy`
- `cms.publish_bundle`

## Why this matters

- Each domain can evolve independently without turning the planner into a pile
  of mixed heuristics.
- Retrieval can return the right domain context more reliably.
- Capability inventory will stay understandable as Ordo grows.
- Future provider crates or modules can be split by domain without renaming the
  whole system later.
