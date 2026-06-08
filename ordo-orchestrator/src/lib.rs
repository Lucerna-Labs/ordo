//! Deterministic multi-agent orchestrator (Stage 0 skeleton).
//!
//! This crate owns the MiniMax-style loop: split a goal -> dispatch
//! PARALLEL scoped subagents -> run adversarial quality gates ->
//! aggregate -> iterate until done or budget-exhausted. It runs
//! IN-PROCESS as a spawned Tokio peer on `ordo-bus`; there is no
//! subprocess or separate service.
//!
//! See `docs/agent-orchestration.md` for the full architecture and the
//! staged build plan. Stage 0 establishes the crate, the run budget,
//! and the phase enum (stable contract). Behaviour — planner split
//! (Stage 3), parallel scoped dispatch (Stage 2), verifier gate
//! (Stage 4), and the driver loop + peer (Stage 5) — lands later.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Hard budgets that bound a single orchestration run. The driver is
/// fail-closed: when any budget is exhausted it stops and reports the
/// goal as incomplete rather than looping unbounded. Defaults match the
/// agreed v1 caps (4 parallel agents, 5 rounds, 10-minute wall-clock).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorBudget {
    /// Maximum subagents dispatched concurrently within one round.
    pub max_concurrent: usize,
    /// Maximum split -> dispatch -> gate rounds before the driver
    /// force-completes the goal.
    pub max_rounds: usize,
    /// Wall-clock ceiling for the whole goal, in seconds.
    pub wall_clock_secs: u64,
}

impl Default for OrchestratorBudget {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_rounds: 5,
            wall_clock_secs: 600,
        }
    }
}

impl OrchestratorBudget {
    /// The wall-clock ceiling as a [`Duration`].
    pub fn wall_clock(&self) -> Duration {
        Duration::from_secs(self.wall_clock_secs)
    }
}

/// Phases of one orchestration run. The driver state machine (Stage 5)
/// advances through these; defined here so the lifecycle contract is
/// stable from Stage 0 onward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestratorPhase {
    /// Splitting the goal into a task DAG (Planner).
    Planning,
    /// Running ready tasks as parallel scoped subagents.
    Dispatching,
    /// Gating subagent outputs through the verifier.
    Verifying,
    /// All tasks accepted; goal complete.
    Done,
    /// Stopped early — budget exhausted or hard-halt.
    Halted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_defaults_match_v1_caps() {
        let b = OrchestratorBudget::default();
        assert_eq!(b.max_concurrent, 4);
        assert_eq!(b.max_rounds, 5);
        assert_eq!(b.wall_clock(), Duration::from_secs(600));
    }
}
