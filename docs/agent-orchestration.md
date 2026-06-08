# Agent Orchestration — MiniMax-style multi-agent loop for Ordo

**Status:** DRAFT for review (2026-06-08). No code written yet. This document is the
architecture + staged build plan for review/sign-off before implementation.

**Author intent:** the last major capability before public launch — a local,
MiniMax-style agent system: a main agent splits a goal, dispatches **parallel**
subagents, runs **adversarial quality gates**, with **deterministic code** driving the
loop until done. Built entirely in-process on the Tokio bus (no subprocess, no
microservices), driven by whatever local model the operator has configured
(Qwen / DeepSeek / MiniMax / etc. via the existing credential + mode system).

---

## 1. The target loop

```
        ┌──────────────────────────── Orchestrator (deterministic driver) ────────────────────────────┐
        │                                                                                              │
  Goal ─┤  1. SPLIT (Planner / LLM)  ─►  Task DAG                                                       │
        │  2. DISPATCH ready tasks ─►  N PARALLEL scoped subagents  (fan-out via bus scatter_gather)    │
        │  3. GATE each result      ─►  Verifier/Critic  → Pass / Fail / Revise                         │
        │  4. AGGREGATE + advance   ─►  Pass: mark done · Fail: re-dispatch (bounded) · all done: emit   │
        │  5. ITERATE until: DAG complete · budget hit · hard-halt                                       │
        └──────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Your mapping → Ordo design

| MiniMax role | You mapped it to | Ordo design (this doc) |
|---|---|---|
| Main Agent (splits) | Ordo Planner | LLM decomposition (`AssistantService::plan_goal`) → `ordo-tasks::Goal` DAG. **Not** the existing `RuleBasedPlanner` (keyword matcher). |
| Agent Team (parallel) | spawned Tokio agents | N concurrent **scoped subagents** = `spawn_subagent_in_mode` runs joined via `JoinSet`/`scatter_gather`. |
| Quality Gates | Enforcers + Verifiers | New in-process **Verifier** (deterministic checks + optional LLM Critic) emitting `GateOutcome`. |
| Context Isolation | per-agent memory/tool scope | **Per-subagent** scope tag + tool-lane override + taint propagation (re-key existing mode/session isolation). |
| Long-running loop | "Ordo supervisor runtime" | New **`ordo-orchestrator`** component spawned in `ordo-runtime`. **NOT** the archived `ordo-supervisor` (that is a health rollup — see §3). |

---

## 2. Ground truth — what exists today (evidence-based)

A parallel deep-read of six subsystems established that **the primitives exist but are
disassembled**; almost none of the "orchestration" surface is wired into the running
binary. Summary:

| Piece | Status | Evidence |
|---|---|---|
| `ordo-planner` (`RuleBasedPlanner`) | scaffolding; **not wired** into runtime | single-step keyword matcher; `ordo-runtime` has no dep on it |
| `ordo-agents` registry (12 profiles incl. **Planner + Critic**, tool scopes, `MemoryScope`, `RiskLevel`) | declarative only, **orphaned** | `ordo-agents/src/lib.rs` — data + lookups, no lifecycle; consumed only by non-members |
| `ordo-dispatcher` (`execute_goal`) | **sequential**, stub-only runner, **not a workspace member** | `ordo-dispatcher/src/lib.rs:242` one-task-at-a-time; `max_concurrent=10` never read |
| `spawn_subagent_in_mode` | **live** but single/sequential, reachable only via `consult_mode_agent` | `ordo-assistant/src/service.rs:829`; depth cap `MAX_SUBAGENT_DEPTH=3` |
| Bus `scatter_gather` (N-way fan-in) | **live primitive**, used only by memory-router | `ordo-bus/src/correlator.rs:105` |
| `ordo-tasks` Goal/Task DAG (dependency gating, retry) | complete, **no live producer** | `ordo-tasks/src/lib.rs` |
| `evaluate_gate` + `ordo-build-planner` (deterministic gate state machine) | **live** but operator-driven over HTTP, separate from agent loop | `ordo-build-primitives/src/lib.rs:70`; `ordo-build-planner` |
| Context isolation (mode `memory_scope`, tool lanes, session taint) | **live, fail-closed**; keyed on mode+session, **not subagent** | `ordo-assistant/src/recall.rs:70`, `tools.rs`, `service.rs` taint gate |
| `ordo-security` (pre/post content scan, blocks on Block) | **live**; gates **safety**, not quality | `ordo-security/src/gated.rs:26`, wrapped on every provider |
| `ordo-review` (human approval queue) | **live**; human, not adversarial | `ordo-review`; `run_review_step` in turn loop |
| Protocol orchestration messages (`GoalSubmitted` … `GoalCompleted`) | **dead** (no producer/consumer) | `ordo-protocol/src/lib.rs:266-313` |
| `ordo-runtime` "loop" | process/component supervisor; **no work loop** | boots Tokio peers then `wait_for_shutdown_signal()` |
| run path `RunRequested → execute_plan → RunFinished` | **single sequential pass**, no fan-out/gate/iterate | `ordo-mcp-host/src/lib.rs:861` (for-loop, breaks on first failure) |
| archived `ordo-supervisor` | **health-state rollup**, NOT an orchestrator | `_archive/ordo-supervisor/src/lib.rs` ("what this crate is NOT") |

**Conclusion:** this is an *assembly + 3 net-new pieces* job, not a from-scratch build and
not a simple wiring job. The net-new pieces: (1) the deterministic **Orchestrator**
component, (2) the in-process **Verifier**, (3) **per-subagent** isolation.

---

## 3. Design principles / guardrails

1. **In-process only.** Agents = recursive `AssistantService` turns spawned as Tokio
   tasks. The Orchestrator is a `spawn_component` peer on `ordo-bus`. **No** subprocess,
   port, webview, or second binary. (Matches the runtime architecture; the UI stays the
   Tauri studio talking to the control API.)
2. **Typed messages are the contract.** Reuse the existing `OrdoMessage` orchestration
   variants; add only what's missing, in `ordo-protocol`. No ad-hoc JSON channels.
3. **Reuse, don't reinvent.** `scatter_gather` for fan-out, `ordo-tasks` for the DAG,
   `GateOutcome`/`evaluate_gate` shape for verdicts, `ordo-agents` profiles for
   specialist routing, `ordo-review`/`ordo-security` for the human + safety gates,
   `SystemStateChanged` (orphaned wire) for activity readout.
4. **Fail-closed + bounded.** Every loop has a hard budget (max rounds, max parallel
   agents, max tokens, wall-clock). `ordo-brain::ToolCallGuard` (120 calls/10s) already
   backstops runaway tool use. Empty scope/lane = no access.
5. **Naming:** the long-running driver is the **Orchestrator** (`ordo-orchestrator`).
   Do **not** revive `ordo-supervisor` — it derives health/activity state and is archived.
   Its orphaned `SystemStateChanged`/`ordo.system.state` wire can be reused to surface the
   orchestrator's `ActivityState` (Idle/Processing) to the UI.

---

## 4. Architecture

### 4.1 Components & data flow

```
 Operator / API ──GoalSubmitted──►  ┌─────────────────┐
                                    │  OrchestratorPeer│  (new: ordo-orchestrator, spawned in ordo-runtime boot)
                                    │  state machine   │
                                    └───────┬─────────┘
                  plan_goal (LLM)           │ split
        AssistantService ◄──────────────────┤
                  Task DAG (ordo-tasks)──────┤ ready tasks
                                            ▼
                       ┌──────── parallel dispatch (JoinSet / scatter_gather, ≤ max_concurrent) ────────┐
                       │  subagent A (scope agent:A, lanes={read,web})   spawn_subagent_in_mode(...)      │
                       │  subagent B (scope agent:B, lanes={code,workspace})                              │
                       │  subagent C (scope agent:C, lanes={analysis})                                    │
                       └───────────────────────────────┬───────────────────────────────────────────────┘
                                                        │ TaskCompleted{output}
                                                        ▼
                                            ┌──────────────────────┐
                                            │  Verifier / Critic    │  (new) deterministic checks (+ optional LLM critic)
                                            │  → GateOutcome         │  Pass / Fail{class,evidence} / Deferred
                                            └──────────┬───────────┘
                          Pass → mark done            │            Fail → re-dispatch (bounded) / HardHalt
                                                        ▼
                                            DAG complete? ── no ──► next round (back to dispatch)
                                                        │ yes
                                                        ▼
                                                 GoalCompleted
```

### 4.2 The Orchestrator (deterministic driver)

A pure state machine (mirrors `ordo-build-planner`'s proven shape: Advance / HardHalt /
Deferred / RetryEligible + a ledger) wrapped in a `OrchestratorPeer` that:
- subscribes `topics::GOAL_SUBMIT`, deserializes `GoalSubmitted`;
- calls the **Planner** to split → `ordo-tasks::Goal`;
- each round: take `Goal::next_ready_tasks()`, dispatch them in parallel (≤ `max_concurrent`),
  await results, run each through the **Verifier**, apply verdicts to the DAG
  (`Task::complete` / `Task::fail` retry-to-pending), emit orchestration events;
- terminates on DAG complete / budget exhausted / hard-halt; emits `GoalCompleted`.

The state machine itself is **synchronous + pure** (unit-testable without a bus or model);
the peer wraps it with the async dispatch/await.

### 4.3 The Planner (split)

`AssistantService::plan_goal(goal) -> Goal` — an LLM decomposition turn (structured-output
schema: list of `{id, task_type, description, deps, suggested_agent}`), routed to agent
profiles via `ordo-agents::AgentRegistry::find_for_task_type`. Deterministic fallback:
if decomposition fails or yields one task, run it as a single subagent (degrades to today's
behavior — safe). The `RuleBasedPlanner` is **not** used.

### 4.4 Parallel scoped dispatch (Agent Team)

A `Dispatcher` that runs ready tasks concurrently. Two implementation choices (see §8):
- **Recommended:** a focused new dispatch routine in `ordo-orchestrator` using `JoinSet`
  over `spawn_subagent_in_mode`, honoring `max_concurrent`, aggregating `TaskResult`s.
- Alt: revive `ordo-dispatcher` (add to workspace, rewrite `execute_goal` to fan out, add a
  production `TaskRunner` bridging to the assistant). More baggage; the sequential loop and
  stub runner would be largely rewritten anyway.

Each dispatched subagent is **scoped** (§4.6) and isolated. Results gather via `JoinSet`
(in-process) or `scatter_gather` (if dispatched as bus requests to a subagent-runner peer).

### 4.5 Verifier / Critic (adversarial quality gate)

New in-process gate that judges a subagent artifact against the task's acceptance criteria:
- **Deterministic tier** (cheap, always-on): `evaluate_gate`-style structural checks
  (stub markers, empty/placeholder output, schema conformance, "did it answer the task").
- **LLM-critic tier** (optional, configurable): spawn the orphaned **Critic** agent profile
  as a scoped subagent ("check quality, contradictions, factual accuracy"), prompted to
  *refute* — produces `Pass`/`Fail{class, evidence}`. Adversarial by construction.
- Emits `GateOutcome` (reuse `ordo-protocol::build::GateOutcome`). `Fail` → bounded
  re-dispatch (revise prompt + same scope); repeated fail → `HardHalt` or `Deferred`.
- High-`RiskLevel` capabilities still route through `ordo-review` (human) and every call is
  still wrapped by `ordo-security` (safety). The verifier adds the **quality** axis that's
  missing today.

### 4.6 Per-subagent context isolation (the gap)

Today isolation is keyed on **mode + session**. Add a per-spawn scope so parallel subagents
can't read/clobber each other:
- **Memory scope tag:** inject `agent:<uuid>` (or `task:<id>`) into `recall_in_scopes` and
  auto-tag writes in `meta_remember_fact`, alongside the mode scope. A subagent reads
  `global` + its mode + its own `agent:` scope only.
- **Tool-lane override:** `TurnRequest` gains an optional `allowed_lanes` that *narrows*
  (never widens) the mode's lanes for that spawn — so the planner can hand subagent A
  `{read,web}` and subagent B `{code,workspace}` without authoring two modes.
- **Taint propagation:** `spawn_subagent` currently mints a clean session — fix so a tainted
  parent yields a tainted child (untrusted content can't be laundered through a fresh child).
- These extend `spawn_subagent_in_mode` (new params) + `TurnRequest` fields; the underlying
  primitives (`scope` column, lane allowlist, `session_taint` map) already exist.

---

## 5. Message contract (ordo-protocol)

**Reuse (already defined, currently dead — `lib.rs:266-313`):**
`GoalSubmitted{goal_id, description}`, `PlanCreated{goal_id, task_count, agent_count}`,
`TaskQueued{goal_id, task_id, task_type, assigned_agent}`, `TaskStarted{goal_id, task_id, agent_id}`,
`TaskCompleted{goal_id, task_id, output}`, `TaskFailed{goal_id, task_id, error}`,
`PolicyCheckRequired{…}`, `UserApprovalRequired{…}`, `GoalCompleted{goal_id, succeeded, task_count}`.

**Add (minimal):**
- `TaskVerified { goal_id, task_id, outcome: GateOutcome }` — the verifier verdict on the bus.
- Topics: `GOAL_SUBMIT = "ordo.goal.submit"`, `ORCH_EVENT = "ordo.orchestration.event"`
  (orchestrator publishes the Task*/Goal* lifecycle here; UI/telemetry subscribe).
- Reuse `SystemStateChanged` + `topics::SYSTEM_STATE` to publish orchestrator
  `ActivityState` (Idle/Processing) — revives that orphaned wire for the UI.

No new serialization formats; all `OrdoMessage` variants over `ordo-bus`.

---

## 6. Crate plan

- **NEW `ordo-orchestrator`** — the state machine + `OrchestratorPeer`. Depends on
  `ordo-protocol`, `ordo-bus`, `ordo-tasks`, `ordo-agents` (routing), `ordo-assistant`
  (subagent spawn), `ordo-build-primitives` (gate helpers). Add to `[workspace] members`
  **and** to `ordo-runtime`'s deps (so it's actually built + spawned).
- **MODIFY `ordo-assistant`** — `plan_goal`; per-subagent scope/lane/taint on
  `spawn_subagent*` + `TurnRequest`; expose a `assistant.spawn_subagent` capability
  (currently only `consult_mode_agent` is reachable).
- **MODIFY `ordo-protocol`** — `TaskVerified` + topics (§5).
- **MODIFY `ordo-runtime`** — `spawn_component("orchestrator", …)` in `boot()`; depend on
  `ordo-tasks`/`ordo-agents`/`ordo-orchestrator` (currently depends on none of them).
- **MODIFY `ordo-control`** (Stage 6) — `POST /api/orchestrate` + `/ws/orchestration`.
- **REUSE as-is:** `ordo-bus`, `ordo-tasks`, `ordo-agents`, `ordo-build-primitives`,
  `ordo-security`, `ordo-review`, `ordo-brain` guard.
- **Leave archived:** `ordo-supervisor`. **Decide (§8):** `ordo-dispatcher` (revive vs ignore).

---

## 7. Staged build plan (each stage: builds under `RUSTFLAGS=-D warnings`, unit-tested, committed, sign-off before next)

| Stage | Delivers | Verification |
|---|---|---|
| **0. Contract** | `TaskVerified` + topics in `ordo-protocol`; `ordo-orchestrator` crate skeleton added to workspace + runtime deps (no behavior). | `cargo build` clean; crate compiles. |
| **1. Per-subagent isolation** | scope tag + `allowed_lanes` narrowing + taint propagation on `spawn_subagent*`/`TurnRequest`. | unit tests: A can't read B's scope; tainted parent→tainted child; lane override narrows tools; empty = fail-closed. |
| **2. Parallel scoped dispatch** | dispatch N ready tasks concurrently (`JoinSet`, ≤`max_concurrent`) as scoped subagents; aggregate results. | integration test: 1 goal, 3 independent subtasks run concurrently, results aggregated; concurrency observed. |
| **3. Planner split** | `plan_goal`: goal → `ordo-tasks::Goal` DAG (LLM structured output) + agent routing; deterministic single-task fallback. | test: goal yields a sane DAG; malformed/empty → single-task fallback. |
| **4. Verifier gate** | deterministic checks + optional LLM Critic → `GateOutcome`; Fail → bounded re-dispatch. | test: bad artifact caught + re-dispatched; good accepted; budget caps re-dispatch. |
| **5. Orchestrator loop** | `OrchestratorPeer` spawned in `ordo-runtime`: GoalSubmitted → split → parallel dispatch → gate → iterate-until-done/budget → GoalCompleted; emits Task*/Goal* + ActivityState. | end-to-end: submit goal on bus, observe parallel agents + gates + completion; budget halts a runaway. |
| **6. Surface** | `POST /api/orchestrate` + `/ws/orchestration`; minimal studio panel to submit + watch the team/gates/progress. | invoke from UI, observe live events; cancel works. |

Stages 0–2 are the low-risk substrate; 3–5 are the core loop; 6 is the surface. A credible
**v1 / "ready for public"** cut is **Stages 0–5 + a minimal Stage 6 endpoint** (full studio
panel can follow). Estimate: multi-session; each stage is independently shippable and safe
(degrades to current behavior if disabled).

---

## 8. Decisions needed (please weigh in on review)

1. **Dispatcher:** new dispatch routine in `ordo-orchestrator` (recommended — clean) vs
   revive `ordo-dispatcher` (more legacy baggage; sequential loop + stub runner rewritten anyway)?
2. **Verifier strength:** deterministic-only (cheap, fast) / deterministic + LLM critic
   (recommended, configurable per mode) / human-in-loop via `ordo-review` for high-risk?
3. **Autonomy & approval:** how autonomous before launch? Recommended: autonomous within a
   budget; `ordo-review` human-gate on sensitive capabilities (`is_sensitive_capability`) and
   high `RiskLevel`; everything still safety-scanned by `ordo-security`.
4. **Default budgets:** max parallel agents (e.g. 4), max rounds (e.g. 5), per-goal token
   ceiling, wall-clock cap. (Tunable per mode.)
5. **Planner/Critic model:** which local model drives split + critique (Qwen / DeepSeek /
   MiniMax)? Orchestrator is model-agnostic via the existing credential/mode system; pick
   sensible defaults.
6. **v1 scope for launch:** ship Stages 0–5 + minimal endpoint, or include the full studio
   panel (Stage 6)?

## 9. Risks & mitigations

- **Runaway loops / cost:** hard budgets (§3.4) + `ToolCallGuard` (120/10s) + per-goal
  wall-clock; orchestrator halts and emits `GoalCompleted{succeeded:false}` on exhaustion.
- **Safety:** unchanged — every subagent tool call is still wrapped by `ordo-security`
  (block on secrets/injection/exfil) and subject to mode lanes + taint gating.
- **Determinism:** the driver state machine is pure/synchronous and unit-tested; only the
  planner/critic/subagents are model-driven. Failure of any LLM step degrades gracefully
  (single-task fallback, Fail verdict, or HardHalt) rather than hanging.
- **Quality of the LLM critic:** local models vary; the deterministic tier is the floor, the
  critic is additive; both are configurable.

## 10. Out of scope for v1

Cross-agent voting/consensus; deep multi-level re-planning (depth > `MAX_SUBAGENT_DEPTH`);
agent-to-agent direct messaging; persistent multi-day goals; rich timeline UI. All layer on
the v1 contract later.

---

## 11. Build log

Running notes kept as each stage lands (newest appended). Every stage builds clean under
`RUSTFLAGS=-D warnings` and is committed independently; each degrades safely to current
behaviour if its driver is not invoked.

### Stage 0 — Contract + crate skeleton ✅ (2026-06-08)

**Done:**
- `ordo-protocol`: added `TaskVerdict { Pass{evidence} | Revise{feedback} | Fail{reason} }`
  and `OrdoMessage::TaskVerified { goal_id, task_id, verdict }`; added topics
  `GOAL_SUBMIT = "ordo.goal.submit"` and `ORCH_EVENT = "ordo.orchestration.event"`.
  The `GoalSubmitted … GoalCompleted` orchestration variants already existed (dead) — reused.
- `ordo-router`: added `OrdoMessage::TaskVerified { .. } => "task_verified"` to the exhaustive
  `message_kind` match (required so the workspace still compiles with the new variant).
- New crate **`ordo-orchestrator`**: `OrchestratorBudget` (defaults 4 agents / 5 rounds /
  600s) + `OrchestratorPhase` (Planning/Dispatching/Verifying/Done/Halted) + unit test.
  Added to `[workspace] members`.

**Decisions:**
- **Verdict type:** defined a dedicated `TaskVerdict` instead of reusing `build::GateOutcome`
  — the build outcome carries `BuildErrorClass` (build-ladder-specific) and would couple
  orchestration to the build spine. Orchestration-native verdict is cleaner.
- **Runtime wiring deferred to Stage 5:** `ordo-orchestrator` is a workspace member (so it is
  built + checked) but is NOT yet a dependency of `ordo-runtime`; the dep + `spawn_component`
  land in Stage 5 when the peer exists, avoiding an unused-dependency wart. Stage 0 is verified
  with an explicit `cargo build -p ordo-orchestrator`.
- **Deps minimal:** only `serde` so far; `ordo-tasks` / `ordo-agents` / `ordo-assistant` /
  `ordo-build-primitives` get added in the stage that first uses each.

**Verified:** `cargo build -p ordo-orchestrator -p ordo-router` clean (`-D warnings`);
`cargo test -p ordo-orchestrator` → 1 passed; full `cargo build` (ordo-cli tree) clean —
confirms no other exhaustive `OrdoMessage` match broke.

**Next:** Stage 1 — per-subagent context isolation (per-spawn memory-scope tag + tool-lane
narrowing override + taint propagation on `spawn_subagent*` / `TurnRequest`).
