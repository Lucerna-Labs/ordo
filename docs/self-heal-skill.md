# Self-Heal Skill

## Purpose
- Keep Ordo installable, recoverable, and understandable for non-expert
  operators.
- Use a local-only maintenance model for platform repair work instead of mixing
  repair reasoning into the normal user-facing orchestration lane.

## Rules
- Prefer the least disruptive fix that preserves the current local-first
  architecture.
- Reuse a previously successful fix when the incident fingerprint matches a
  known failure.
- Explain both the repair action and the reason behind it in plain language.
- Treat user files, runtime state, and model state as separate lanes.
- If a local model is unavailable, fall back to deterministic repair guidance
  instead of failing open.

## Repair loop
1. Normalize the incident into a stable fingerprint.
2. Check whether the same fingerprint already has a successful repair history.
3. Reapply the known repair when confidence is high.
4. Otherwise, build a repair plan from the platform's architecture and stored
   maintenance history.
5. Record the result so the next occurrence is faster to resolve.

## Scope
- Filesystem root and permissions issues.
- Runtime databank path and local persistence issues.
- Transport and relay fallback mismatches.
- Local model adapter configuration issues.

## Non-goals
- Replacing user-chosen models.
- Acting as the main planner for everyday work.
- Hiding failures instead of recording them.
