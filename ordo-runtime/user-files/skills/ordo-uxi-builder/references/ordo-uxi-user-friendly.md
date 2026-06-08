# Ordo UXI User-Friendly Reference

## Static Snapshot Reference

Use this snapshot as the design source for Ordo-style app surfaces:

```text
ordo-studio/static-html-css/index.html
ordo-studio/static-html-css/styles.css
ordo-studio/static-html-css/README.md
```

The snapshot demonstrates the expected dark operator shell, left navigation,
status row, compact controls, readable cards, bottom composer, and restrained
accent system.

## Control Surfacing

Every feature should answer these questions in the UXI:

```text
What is this?
Is it on or off?
What is selected?
What can I change?
What is risky?
What happened last?
Where are the logs?
How do I undo, pause, delete, test, refresh, or inspect it?
```

Do not leave critical controls only in config files, CLI flags, hidden API
routes, or model-only instructions.

## Exhaustive Logs

For each app or platform feature, create logs/events that include:

```text
timestamp
actor or source
workspace/session/mode when relevant
requested action
capability/provider selected
decision or policy gate
result
error/retry details
security/trust state when relevant
human-readable summary
```

The UXI should provide a readable event/debug surface for operators. Raw logs
may exist on disk, but the operator should not have to hunt through files to
understand normal failures.

## Anti-Bland UXI Check

Reject a screen if it has any of these traits:

```text
unlabeled inputs
unstyled default browser controls as the final design
large empty tables without useful empty states
controls scattered across unrelated panels
actions that are possible but not visible
scrollbars inside scrollbars without a reason
text clipped or overlapping cards
no logs, no status, and no recovery path
```

## Human-Like Verification

Before calling an app-facing change complete:

```text
1. Launch the app the way the user will launch it.
2. Navigate to the changed tab.
3. Perform the primary task.
4. Trigger one error or denial path.
5. Confirm all controls are visible and understandable.
6. Confirm event logs update.
7. Check that text does not overlap, clip, or require hidden scrolling.
8. Capture a report, screenshot, or static snapshot pointer.
```

## Milestone Note Shape

```text
Completed:
UXI controls surfaced:
Logs/events:
Static snapshot/reference:
Human-like usage test:
Launched for confirmation:
Risk:
```
