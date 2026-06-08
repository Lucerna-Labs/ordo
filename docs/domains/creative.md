# Creative Domain

The `creative.*` domain covers the work of turning requests into usable
creative outputs.

## Scope

- brief intake
- campaign framing
- concept generation
- deliverable planning
- asset packaging
- final creative handoff

## Future capabilities

- `creative.capture_brief`
  - normalize raw requests into structured briefs
- `creative.plan_campaign`
  - break a brief into deliverables, owners, and stages
- `creative.package_assets`
  - group final assets and supporting notes for handoff
- `creative.summarize_deliverables`
  - explain what a creative package contains and why it exists

## Inputs this domain cares about

- target audience
- campaign goal
- channels and formats
- brand constraints
- deadlines
- owners and reviewers

## Outputs this domain should produce

- structured briefs
- deliverable lists
- asset manifests
- handoff notes
- revision context

## Boundaries

- review routing belongs to `workflow.*`
- metadata packaging belongs to `seo.*`
- CMS field mapping and publication payloads belong to `cms.*`
