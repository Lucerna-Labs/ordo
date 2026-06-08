//! Orchestrator glue (Stage 5b) — `AssistantService` implements the
//! `ordo-orchestrator` traits, connecting the (pure, tested) driver loop
//! to the live model + the Stage 1 subagent isolation.
//!
//! - **Runner** (`SubagentRunner`): runs each subtask as a real scoped
//!   subagent via `spawn_subagent_in_mode`, so it inherits the per-spawn
//!   private memory scope (`agent:<uuid>`, auto-generated) and the
//!   subtask's tool-lane narrowing. This is the security-sensitive path.
//! - **Planner** (`GoalPlanner`) and **Critic** (`Critic`): a single
//!   tool-less, RAG/memory-off completion turn (they need one LLM call,
//!   not an agentic loop) feeding the pure `parse_plan` /
//!   `parse_critic_verdict`. LLM failure degrades safely (planner → single
//!   task; critic → inconclusive Pass, since the critic is additive).
//!
//! One struct implements all three traits; the runtime shares it as an
//! `Arc` cast to each trait object when constructing an `Orchestrator`.

use async_trait::async_trait;

use ordo_orchestrator::{
    critic_prompt, parse_critic_verdict, parse_plan, planning_prompt, Critic, GoalPlanner,
    PlannedGoal, SubagentRunner, Subtask, SubtaskResult,
};
use ordo_protocol::TaskVerdict;

use crate::service::{AssistantService, SubagentScope};
use crate::types::TurnRequest;

/// Default per-subtask agentic iteration budget for the runner's spawned
/// subagents (in addition to `MAX_SUBAGENT_DEPTH` and the orchestrator's
/// round/attempt budgets).
const DEFAULT_SUBAGENT_ITERATIONS: usize = 6;

/// Live glue between the orchestrator and the assistant. Cheap to clone
/// (wraps the `Arc`-backed `AssistantService`); the runtime wraps one in
/// an `Arc` and casts it to each trait object.
#[derive(Clone)]
pub struct AssistantOrchestration {
    service: AssistantService,
    subagent_max_iterations: usize,
    planner_mode: Option<String>,
    critic_mode: Option<String>,
}

impl AssistantOrchestration {
    pub fn new(service: AssistantService) -> Self {
        Self {
            service,
            subagent_max_iterations: DEFAULT_SUBAGENT_ITERATIONS,
            planner_mode: None,
            critic_mode: None,
        }
    }

    pub fn with_subagent_iterations(mut self, iterations: usize) -> Self {
        self.subagent_max_iterations = iterations.max(1);
        self
    }

    pub fn with_planner_mode(mut self, mode: Option<String>) -> Self {
        self.planner_mode = mode;
        self
    }

    pub fn with_critic_mode(mut self, mode: Option<String>) -> Self {
        self.critic_mode = mode;
        self
    }

    /// One tool-less completion (fresh session, no RAG/memory) — the
    /// planner and critic need a single LLM call, not an agentic turn, and
    /// must not be able to take tool actions.
    async fn complete(&self, prompt: String, mode: Option<String>) -> Result<String, String> {
        let request = TurnRequest {
            user_message: prompt,
            session_id: None,
            use_tools: false,
            use_rag: false,
            use_memory: false,
            review: false,
            stream: false,
            mode,
            ..Default::default()
        };
        self.service
            .turn(request)
            .await
            .map(|result| result.turn.assistant_response)
            .map_err(|err| err.to_string())
    }
}

#[async_trait]
impl GoalPlanner for AssistantOrchestration {
    async fn plan(&self, goal: &str) -> PlannedGoal {
        // v1: empty mode catalogue — the model omits per-subtask modes
        // (subtasks run in the default mode). Advertising the runtime's
        // modes for routing is a follow-up.
        let prompt = planning_prompt(goal, &[]);
        match self.complete(prompt, self.planner_mode.clone()).await {
            Ok(raw) => {
                let mut plan = parse_plan(goal, &raw);
                // v1 advertises no modes to the planner; defensively strip
                // any mode the model emitted so a prompt-injected plan can't
                // route a subtask into a higher-privilege mode (subtasks run
                // in the default mode). Real mode routing will validate
                // against an operator-approved catalogue.
                for task in &mut plan.tasks {
                    task.subtask.mode = None;
                }
                plan
            }
            Err(err) => {
                tracing::warn!(
                    target: "ordo_assistant",
                    error = %err,
                    "planner LLM call failed; running the goal as a single task"
                );
                PlannedGoal::single(goal, None)
            }
        }
    }
}

#[async_trait]
impl SubagentRunner for AssistantOrchestration {
    async fn run_subtask(&self, subtask: Subtask) -> SubtaskResult {
        let scope = SubagentScope {
            // None → spawn_subagent_in_mode auto-generates a private
            // `agent:<uuid>` memory scope, so parallel subtasks can't read
            // or clobber each other's working memory (Stage 1).
            memory_scope: None,
            // Narrow the subagent's tool lanes to whatever the plan
            // assigned this subtask (None = the mode's lanes).
            allowed_lanes: subtask.allowed_lanes.clone(),
            // Orchestration goals are operator-initiated; each subagent
            // starts on a fresh, isolated session. (Propagating taint from
            // a tainted ORIGINATING session, and re-scanning subagent
            // outputs, are tracked follow-ups — see docs §11.)
            inherit_taint: Vec::new(),
        };
        match self
            .service
            .spawn_subagent_in_mode(
                0,
                subtask.goal.clone(),
                Some(self.subagent_max_iterations),
                subtask.mode.clone(),
                scope,
            )
            .await
        {
            Ok(result) => SubtaskResult::ok(subtask.id, result.turn.assistant_response),
            Err(err) => SubtaskResult::err(subtask.id, err.to_string()),
        }
    }
}

#[async_trait]
impl Critic for AssistantOrchestration {
    async fn critique(&self, subtask: &Subtask, output: &str) -> TaskVerdict {
        let prompt = critic_prompt(&subtask.goal, output);
        match self.complete(prompt, self.critic_mode.clone()).await {
            Ok(raw) => parse_critic_verdict(&raw),
            Err(err) => {
                // Additive critic: a failed critic call must not fail
                // output that already cleared the deterministic floor.
                tracing::warn!(
                    target: "ordo_assistant",
                    error = %err,
                    "critic LLM call failed; accepting output by default"
                );
                TaskVerdict::Pass {
                    evidence: "critic unavailable; accepted by default".into(),
                }
            }
        }
    }
}
