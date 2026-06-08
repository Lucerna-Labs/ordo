//! Parallel scoped dispatch (Stage 2).
//!
//! Runs a round of READY subtasks as concurrent, isolated subagents and
//! aggregates their results. The orchestrator depends only on the
//! [`SubagentRunner`] trait, so the concurrency + aggregation logic here
//! is decoupled from `ordo-assistant` and unit-testable with a stub
//! runner. The production runner — over
//! `AssistantService::spawn_subagent_in_mode`, threading each subtask's
//! private memory scope + lane narrowing + inherited taint (Stage 1) — is
//! wired in Stage 5.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use uuid::Uuid;

/// One unit of work the orchestrator dispatches to a single subagent.
#[derive(Debug, Clone)]
pub struct Subtask {
    /// Stable id (matches the task node in the goal DAG).
    pub id: Uuid,
    /// The instruction handed to the subagent.
    pub goal: String,
    /// Mode the subagent runs in (None = the runtime's default mode).
    pub mode: Option<String>,
    /// Tool lanes the subagent may use — NARROWS the mode's lanes
    /// (None = the mode's lanes unchanged). The production runner threads
    /// this into the subagent's `SubagentScope` (Stage 1 isolation).
    pub allowed_lanes: Option<Vec<String>>,
}

impl Subtask {
    /// Convenience constructor: fresh id, no lane narrowing.
    pub fn new(goal: impl Into<String>, mode: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            goal: goal.into(),
            mode,
            allowed_lanes: None,
        }
    }
}

/// Outcome of running one subtask. `output` is `Ok(answer)` on success or
/// `Err(message)` on failure — a failed subtask is a first-class result,
/// not a panic, so one bad subtask never aborts the round.
#[derive(Debug, Clone)]
pub struct SubtaskResult {
    pub id: Uuid,
    pub output: Result<String, String>,
}

impl SubtaskResult {
    pub fn ok(id: Uuid, answer: impl Into<String>) -> Self {
        Self {
            id,
            output: Ok(answer.into()),
        }
    }

    pub fn err(id: Uuid, message: impl Into<String>) -> Self {
        Self {
            id,
            output: Err(message.into()),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.output.is_ok()
    }
}

/// Runs a single subtask as an isolated subagent. Implemented by a glue
/// layer over `AssistantService::spawn_subagent_in_mode` (Stage 5); the
/// orchestrator only knows this trait. Must be cheap to share (`Arc`).
#[async_trait::async_trait]
pub trait SubagentRunner: Send + Sync {
    async fn run_subtask(&self, subtask: Subtask) -> SubtaskResult;
}

/// Dispatch a round of ready subtasks as CONCURRENT scoped subagents,
/// bounded to at most `max_concurrent` in flight at once. A permit is
/// released as each subagent finishes, admitting the next — so a round of
/// 100 subtasks with `max_concurrent = 4` runs four-at-a-time. Returns
/// every subtask's result; a subagent task that *panics* is logged and
/// dropped (its peers still complete). Result order is completion order,
/// not submission order — callers key by [`SubtaskResult::id`].
pub async fn dispatch_subtasks(
    runner: Arc<dyn SubagentRunner>,
    subtasks: Vec<Subtask>,
    max_concurrent: usize,
) -> Vec<SubtaskResult> {
    let limit = max_concurrent.max(1);
    let semaphore = Arc::new(Semaphore::new(limit));
    let mut set: JoinSet<SubtaskResult> = JoinSet::new();
    // Track ids so a panicked/cancelled task (which yields no SubtaskResult)
    // is surfaced as a synthetic error rather than silently lost — the
    // driver then counts it as an attempt and fails it after the cap.
    let mut pending: HashSet<Uuid> = subtasks.iter().map(|s| s.id).collect();

    for subtask in subtasks {
        // Acquire BEFORE spawning so at most `limit` subagents run at
        // once; the spawned task holds the permit and releases it when it
        // finishes, admitting the next. acquire_owned only errors if the
        // semaphore is closed, which never happens here.
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break,
        };
        let runner = Arc::clone(&runner);
        set.spawn(async move {
            let _permit = permit;
            runner.run_subtask(subtask).await
        });
    }

    let mut results = Vec::with_capacity(pending.len());
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(result) => {
                pending.remove(&result.id);
                results.push(result);
            }
            Err(join_err) => {
                tracing::error!(
                    target: "ordo_orchestrator",
                    error = %join_err,
                    "subtask subagent task panicked or was cancelled"
                );
            }
        }
    }
    // Any id that produced no result (its task panicked/cancelled) is
    // surfaced as a synthetic error so the driver can count + cap it.
    for id in pending {
        results.push(SubtaskResult::err(
            id,
            "subagent task panicked or was cancelled",
        ));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Stub runner that records peak concurrency, so the test can assert
    /// the dispatcher both honors `max_concurrent` AND actually runs
    /// subtasks in parallel.
    struct ConcurrencyProbe {
        in_flight: AtomicUsize,
        peak: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl SubagentRunner for ConcurrencyProbe {
        async fn run_subtask(&self, subtask: Subtask) -> SubtaskResult {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            // Hold the slot briefly so siblings overlap.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            SubtaskResult::ok(subtask.id, format!("done: {}", subtask.goal))
        }
    }

    #[tokio::test]
    async fn dispatch_bounds_concurrency_and_aggregates_all() {
        let probe = Arc::new(ConcurrencyProbe {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        });
        let subtasks: Vec<Subtask> = (0..6)
            .map(|i| Subtask::new(format!("task {i}"), None))
            .collect();
        let ids: Vec<Uuid> = subtasks.iter().map(|s| s.id).collect();

        let results = dispatch_subtasks(probe.clone(), subtasks, 2).await;

        // Every subtask produced a result...
        assert_eq!(results.len(), 6);
        assert!(results.iter().all(|r| r.is_ok()));
        for id in ids {
            assert!(
                results.iter().any(|r| r.id == id),
                "missing result for {id}"
            );
        }
        // ...parallelism actually happened...
        let peak = probe.peak.load(Ordering::SeqCst);
        assert!(peak >= 2, "expected concurrency, peak was {peak}");
        // ...but never exceeded the cap.
        assert!(peak <= 2, "max_concurrent=2 violated, peak was {peak}");
    }

    #[tokio::test]
    async fn dispatch_handles_empty_round() {
        let probe = Arc::new(ConcurrencyProbe {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        });
        let results = dispatch_subtasks(probe, Vec::new(), 4).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn max_concurrent_zero_is_clamped_to_one() {
        let probe = Arc::new(ConcurrencyProbe {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        });
        let subtasks: Vec<Subtask> =
            (0..3).map(|i| Subtask::new(format!("t{i}"), None)).collect();
        let results = dispatch_subtasks(probe.clone(), subtasks, 0).await;
        assert_eq!(results.len(), 3);
        // Clamped to 1 → strictly serial.
        assert_eq!(probe.peak.load(Ordering::SeqCst), 1);
    }
}
