# Workflow Domain

The `workflow.*` domain owns movement through stages, approvals, revisions, and
handoffs.

## Scope

- stage progression
- review routing
- approval checkpoints
- revision loops
- scheduling and release readiness
- owner handoff tracking

## Future capabilities

- `workflow.route_review`
  - send work into the next review checkpoint
- `workflow.request_revision`
  - capture revision feedback and move work backward safely
- `workflow.advance_stage`
  - promote a deliverable into the next workflow state
- `workflow.schedule_release`
  - prepare release timing and publish readiness

## Inputs this domain cares about

- current stage
- required approvers
- blocked dependencies
- revision notes
- release timing
- final sign-off state

## Outputs this domain should produce

- stage transitions
- review tasks
- revision queues
- approval status
- release schedules

## Boundaries

- brief creation belongs to `creative.*`
- SEO checks belong to `seo.*`
- CMS publishing and entry payloads belong to `cms.*`
