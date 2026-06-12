//! The deterministic driver loop (Stage 5a).
//!
//! Ties the building blocks together into the MiniMax-style loop:
//! plan → rounds of (dispatch ready subtasks in parallel → verify each →
//! apply verdicts to the DAG) → iterate until the goal is complete, no
//! progress is possible, or a budget is exhausted.
//!
//! It is pure with respect to the injected [`GoalPlanner`],
//! [`SubagentRunner`], and [`Critic`] — so it is fully unit-testable with
//! stubs and carries no I/O or clock of its own. The peer (Stage 5b)
//! wraps `run` with a wall-clock timeout and the live glue over
//! `AssistantService`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ordo_protocol::TaskVerdict;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::dispatch::{dispatch_subtasks, SubagentRunner, Subtask};
use crate::plan::GoalPlanner;
use crate::verify::{verify, Critic};
use crate::{OrchestratorBudget, OrchestratorPhase};

/// A subtask whose output was accepted by the verifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptedTask {
    pub id: Uuid,
    pub goal: String,
    pub output: String,
}

/// A subtask that terminally failed (Fail verdict, or exhausted attempts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedTask {
    pub id: Uuid,
    pub goal: String,
    pub reason: String,
}

/// Result of an orchestration run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationOutcome {
    pub goal: String,
    /// Terminal phase — [`OrchestratorPhase::Done`] or
    /// [`OrchestratorPhase::Halted`].
    pub phase: OrchestratorPhase,
    /// True iff every subtask in the plan was accepted.
    pub succeeded: bool,
    /// Number of dispatch rounds run.
    pub rounds: usize,
    /// Why the run halted, when it did not complete.
    pub reason: Option<String>,
    pub accepted: Vec<AcceptedTask>,
    pub failed: Vec<FailedTask>,
}

/// The deterministic multi-agent orchestrator.
pub struct Orchestrator {
    planner: Arc<dyn GoalPlanner>,
    runner: Arc<dyn SubagentRunner>,
    critic: Option<Arc<dyn Critic>>,
    budget: OrchestratorBudget,
}

impl Orchestrator {
    pub fn new(
        planner: Arc<dyn GoalPlanner>,
        runner: Arc<dyn SubagentRunner>,
        critic: Option<Arc<dyn Critic>>,
        budget: OrchestratorBudget,
    ) -> Self {
        Self {
            planner,
            runner,
            critic,
            budget,
        }
    }

    /// Run with the budget's wall-clock ceiling, returning a Halted
    /// outcome on timeout. Partial progress isn't reported once the run
    /// future is dropped. This is the entry point for the control API /
    /// peer (the pure `run` carries no clock so it stays unit-testable).
    pub async fn run_bounded(&self, goal: &str) -> OrchestrationOutcome {
        match tokio::time::timeout(self.budget.wall_clock(), self.run(goal)).await {
            Ok(outcome) => outcome,
            Err(_) => OrchestrationOutcome {
                goal: goal.to_string(),
                phase: OrchestratorPhase::Halted,
                succeeded: false,
                rounds: 0,
                reason: Some(format!(
                    "wall-clock budget exhausted ({}s)",
                    self.budget.wall_clock_secs
                )),
                accepted: Vec::new(),
                failed: Vec::new(),
            },
        }
    }

    /// Run the goal to a terminal state. Always terminates: each task
    /// reaches `completed` or `failed` within `max_attempts_per_task`
    /// dispatches, and `max_rounds` is the ultimate backstop.
    pub async fn run(&self, goal: &str) -> OrchestrationOutcome {
        let plan = self.planner.plan(goal).await;

        let mut completed: HashSet<Uuid> = HashSet::new();
        let mut failed: HashMap<Uuid, (String, String)> = HashMap::new(); // id -> (goal, reason)
        let mut accepted: HashMap<Uuid, (String, String)> = HashMap::new(); // id -> (goal, output)
        let mut attempts: HashMap<Uuid, usize> = HashMap::new();
        let mut feedback: HashMap<Uuid, String> = HashMap::new();
        let mut rounds = 0usize;

        let outcome = |phase,
                       succeeded,
                       rounds,
                       reason: Option<String>,
                       accepted: &HashMap<Uuid, (String, String)>,
                       failed: &HashMap<Uuid, (String, String)>| {
            OrchestrationOutcome {
                goal: goal.to_string(),
                phase,
                succeeded,
                rounds,
                reason,
                accepted: accepted
                    .iter()
                    .map(|(id, (g, o))| AcceptedTask {
                        id: *id,
                        goal: g.clone(),
                        output: o.clone(),
                    })
                    .collect(),
                failed: failed
                    .iter()
                    .map(|(id, (g, r))| FailedTask {
                        id: *id,
                        goal: g.clone(),
                        reason: r.clone(),
                    })
                    .collect(),
            }
        };

        loop {
            if plan.is_complete(&completed) {
                return outcome(
                    OrchestratorPhase::Done,
                    true,
                    rounds,
                    None,
                    &accepted,
                    &failed,
                );
            }
            if rounds >= self.budget.max_rounds {
                return outcome(
                    OrchestratorPhase::Halted,
                    false,
                    rounds,
                    Some(format!(
                        "budget exhausted: max_rounds={}",
                        self.budget.max_rounds
                    )),
                    &accepted,
                    &failed,
                );
            }

            // Ready = dep-satisfied, not completed, not terminally failed.
            // Re-injected Revise feedback rides along on the goal text.
            let ready: Vec<Subtask> = plan
                .ready(&completed)
                .into_iter()
                .filter(|s| !failed.contains_key(&s.id))
                .map(|s| {
                    let mut s = s.clone();
                    if let Some(fb) = feedback.get(&s.id) {
                        s.goal = format!(
                            "{}\n\n[Revise] A previous attempt was rejected: {fb}. \
                             Address that and return the corrected result.",
                            s.goal
                        );
                    }
                    s
                })
                .collect();

            if ready.is_empty() {
                // Not complete, yet nothing is runnable → remaining tasks
                // are failed or blocked behind a failed dep (or a cycle).
                return outcome(
                    OrchestratorPhase::Halted,
                    false,
                    rounds,
                    Some("no runnable tasks; remaining are failed or blocked".to_string()),
                    &accepted,
                    &failed,
                );
            }

            rounds += 1;

            // Keep an owned copy keyed by id for verification (dispatch
            // consumes the `ready` vec).
            let round_tasks: HashMap<Uuid, Subtask> =
                ready.iter().map(|s| (s.id, s.clone())).collect();

            let results =
                dispatch_subtasks(Arc::clone(&self.runner), ready, self.budget.max_concurrent)
                    .await;

            for result in results {
                let id = result.id;
                let Some(subtask) = round_tasks.get(&id) else {
                    continue; // result for an unknown id (shouldn't happen)
                };
                let attempted = {
                    let entry = attempts.entry(id).or_insert(0);
                    *entry += 1;
                    *entry
                };
                let exhausted = attempted >= self.budget.max_attempts_per_task;

                match result.output {
                    Err(err) => {
                        if exhausted {
                            failed.insert(
                                id,
                                (subtask.goal.clone(), format!("dispatch failed: {err}")),
                            );
                        } // else: stays pending → retried next round
                    }
                    Ok(output) => match verify(subtask, &output, self.critic.as_deref()).await {
                        TaskVerdict::Pass { .. } => {
                            completed.insert(id);
                            accepted.insert(id, (subtask.goal.clone(), output));
                            feedback.remove(&id);
                        }
                        TaskVerdict::Revise { feedback: fb } => {
                            if exhausted {
                                failed.insert(
                                    id,
                                    (
                                        subtask.goal.clone(),
                                        format!("rejected after {attempted} attempts: {fb}"),
                                    ),
                                );
                                feedback.remove(&id);
                            } else {
                                feedback.insert(id, fb);
                            }
                        }
                        TaskVerdict::Fail { reason } => {
                            failed.insert(id, (subtask.goal.clone(), reason));
                            feedback.remove(&id);
                        }
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::dispatch::SubtaskResult;
    use crate::plan::{PlannedGoal, PlannedTask};

    struct StubPlanner(PlannedGoal);

    #[async_trait::async_trait]
    impl GoalPlanner for StubPlanner {
        async fn plan(&self, _goal: &str) -> PlannedGoal {
            self.0.clone()
        }
    }

    /// Runner that echoes each subtask's (possibly feedback-injected) goal,
    /// so tests can both observe success and inspect re-dispatch text.
    struct EchoRunner;

    #[async_trait::async_trait]
    impl SubagentRunner for EchoRunner {
        async fn run_subtask(&self, subtask: Subtask) -> SubtaskResult {
            SubtaskResult::ok(subtask.id, subtask.goal)
        }
    }

    enum Behavior {
        AlwaysPass,
        AlwaysFail,
        AlwaysRevise,
        ReviseThenPass,
    }

    struct ScriptedCritic {
        behavior: Behavior,
        calls: Mutex<HashMap<Uuid, usize>>,
    }

    impl ScriptedCritic {
        fn new(behavior: Behavior) -> Arc<Self> {
            Arc::new(Self {
                behavior,
                calls: Mutex::new(HashMap::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl Critic for ScriptedCritic {
        async fn critique(&self, subtask: &Subtask, _output: &str) -> TaskVerdict {
            let n = {
                let mut m = self.calls.lock().unwrap();
                let c = m.entry(subtask.id).or_insert(0);
                *c += 1;
                *c
            };
            match self.behavior {
                Behavior::AlwaysPass => TaskVerdict::Pass {
                    evidence: "ok".into(),
                },
                Behavior::AlwaysFail => TaskVerdict::Fail {
                    reason: "wrong".into(),
                },
                Behavior::AlwaysRevise => TaskVerdict::Revise {
                    feedback: "fix it".into(),
                },
                Behavior::ReviseThenPass => {
                    if n == 1 {
                        TaskVerdict::Revise {
                            feedback: "tighten it".into(),
                        }
                    } else {
                        TaskVerdict::Pass {
                            evidence: "ok now".into(),
                        }
                    }
                }
            }
        }
    }

    fn budget(max_rounds: usize, max_attempts: usize) -> OrchestratorBudget {
        OrchestratorBudget {
            max_concurrent: 4,
            max_rounds,
            max_attempts_per_task: max_attempts,
            wall_clock_secs: 600,
        }
    }

    fn chain(n: usize) -> PlannedGoal {
        // a -> b -> c ... linear dependency chain of n tasks.
        let mut tasks = Vec::new();
        let mut prev: Option<Uuid> = None;
        for i in 0..n {
            let subtask = Subtask::new(format!("step {i}"), None);
            let id = subtask.id;
            let deps = prev.into_iter().collect();
            tasks.push(PlannedTask { subtask, deps });
            prev = Some(id);
        }
        PlannedGoal {
            goal: "chain".into(),
            tasks,
        }
    }

    fn orch(
        plan: PlannedGoal,
        critic: Option<Arc<dyn Critic>>,
        budget: OrchestratorBudget,
    ) -> Orchestrator {
        Orchestrator::new(
            Arc::new(StubPlanner(plan)),
            Arc::new(EchoRunner),
            critic,
            budget,
        )
    }

    #[tokio::test]
    async fn completes_a_dag_when_all_pass() {
        let o = orch(chain(3), None, budget(10, 2));
        let out = o.run("g").await;
        assert_eq!(out.phase, OrchestratorPhase::Done);
        assert!(out.succeeded);
        assert_eq!(out.accepted.len(), 3);
        assert!(out.failed.is_empty());
        // A 3-link chain unlocks one task per round.
        assert_eq!(out.rounds, 3);
    }

    #[tokio::test]
    async fn completes_with_a_passing_critic() {
        let critic = ScriptedCritic::new(Behavior::AlwaysPass);
        let o = orch(chain(2), Some(critic), budget(10, 2));
        let out = o.run("g").await;
        assert_eq!(out.phase, OrchestratorPhase::Done);
        assert!(out.succeeded);
        assert_eq!(out.accepted.len(), 2);
    }

    #[tokio::test]
    async fn halts_on_max_rounds() {
        // 5-link chain but only 2 rounds allowed.
        let o = orch(chain(5), None, budget(2, 2));
        let out = o.run("g").await;
        assert_eq!(out.phase, OrchestratorPhase::Halted);
        assert!(!out.succeeded);
        assert_eq!(out.rounds, 2);
        assert_eq!(out.accepted.len(), 2);
        assert!(out.reason.unwrap().contains("max_rounds"));
    }

    #[tokio::test]
    async fn revise_then_pass_redispatches_with_feedback() {
        let critic = ScriptedCritic::new(Behavior::ReviseThenPass);
        let o = orch(chain(1), Some(critic), budget(10, 3));
        let out = o.run("g").await;
        assert_eq!(out.phase, OrchestratorPhase::Done);
        assert!(out.succeeded);
        assert_eq!(out.accepted.len(), 1);
        // Took 2 rounds (Revise then Pass) and the re-dispatch carried feedback.
        assert_eq!(out.rounds, 2);
        assert!(out.accepted[0].output.contains("[Revise]"));
    }

    #[tokio::test]
    async fn revise_exhausts_attempts_then_fails() {
        let critic = ScriptedCritic::new(Behavior::AlwaysRevise);
        let o = orch(chain(1), Some(critic), budget(10, 2));
        let out = o.run("g").await;
        assert_eq!(out.phase, OrchestratorPhase::Halted);
        assert!(!out.succeeded);
        assert_eq!(out.failed.len(), 1);
        assert!(out.failed[0].reason.contains("after 2 attempts"));
    }

    #[tokio::test]
    async fn fail_verdict_blocks_dependents_and_halts() {
        // a -> b; critic Fails 'a', so 'b' never unlocks.
        let critic = ScriptedCritic::new(Behavior::AlwaysFail);
        let o = orch(chain(2), Some(critic), budget(10, 2));
        let out = o.run("g").await;
        assert_eq!(out.phase, OrchestratorPhase::Halted);
        assert!(!out.succeeded);
        assert_eq!(out.failed.len(), 1, "only 'a' failed; 'b' stayed blocked");
        assert!(out.accepted.is_empty());
    }

    #[tokio::test]
    async fn run_bounded_halts_on_wall_clock() {
        struct SlowRunner;
        #[async_trait::async_trait]
        impl SubagentRunner for SlowRunner {
            async fn run_subtask(&self, subtask: Subtask) -> SubtaskResult {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                SubtaskResult::ok(subtask.id, "done")
            }
        }
        let budget = OrchestratorBudget {
            max_concurrent: 1,
            max_rounds: 10,
            max_attempts_per_task: 2,
            wall_clock_secs: 0, // force an immediate wall-clock timeout
        };
        let o = Orchestrator::new(
            Arc::new(StubPlanner(chain(1))),
            Arc::new(SlowRunner),
            None,
            budget,
        );
        let out = o.run_bounded("g").await;
        assert_eq!(out.phase, OrchestratorPhase::Halted);
        assert!(out.reason.unwrap().contains("wall-clock"));
    }
}
