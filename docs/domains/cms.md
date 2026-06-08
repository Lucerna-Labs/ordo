# CMS Domain

The `cms.*` domain owns the transformation from approved content into
system-ready entries and publishable payloads.

## Scope

- template mapping
- field preparation
- taxonomy assignment
- collection placement
- entry validation
- publish/export packaging

## Future capabilities

- `cms.prepare_entry`
  - map approved content into CMS fields and structure
- `cms.map_taxonomy`
  - assign categories, collections, and related content groupings
- `cms.validate_payload`
  - confirm that required fields and publish requirements are present
- `cms.publish_bundle`
  - package the finished entry for publish or export

## Inputs this domain cares about

- content model or template
- required fields
- taxonomy rules
- collection placement
- publish state
- channel-specific constraints

## Outputs this domain should produce

- CMS-ready payloads
- taxonomy assignments
- validation findings
- publish bundles
- exportable entry packages

## Boundaries

- creative ideation belongs to `creative.*`
- approvals and stage routing belong to `workflow.*`
- search metadata belongs to `seo.*`
