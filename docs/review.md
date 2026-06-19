# Review

Review is Ordo's human approval lane.

It exists so sensitive outputs and actions can pause for an operator decision
instead of shipping blindly.

## What Review Does

Review can support:

- approval
- denial
- edit-and-approve
- notes
- pending/recent queues
- event notifications to the Studio

Review is useful for:

- high-risk tool calls
- external communication drafts
- local computer writes
- plugin/MCP trust changes
- automation changes
- model/provider changes
- Agent Team plans that need confirmation

## REST Shape

Representative routes:

- `GET /api/review/pending`
- `GET /api/review/recent?limit=N`
- `GET /api/review/:id`
- `POST /api/review/:id/approve`
- `POST /api/review/:id/deny`
- `POST /api/review/:id/edit`

Decision routes should be idempotent-safe and clearly report already-resolved
requests.

## Events

The Studio can subscribe to review events so the operator sees when a request
opens or resolves.

Review events should not include secrets.

## Tech Specialist

Tech Specialist may explain why a review gate appeared, inspect non-secret
context, and help the user decide what is safe. It should not approve actions
on the user's behalf without explicit operator consent.
