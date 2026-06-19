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

- Workflow is currently a supporting automation/review concept, not a product
  lane for creative, SEO, or CMS work.
- New workflow capabilities should support Ordo automation, review, remote
  communication, artifacts, and operator approvals.
