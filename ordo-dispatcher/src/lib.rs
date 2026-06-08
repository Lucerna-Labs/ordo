// ordo-dispatcher — Routes tasks to agents, handles retries, timeouts, and failure escalation.
//
// The Dispatcher is the bridge between the Planner's plan and the Agent Runtime's
// execution. It owns the task queue and makes routing decisions based on the agent
// registry. It does NOT execute tasks — it delegates to the Brain's tool invocation
// system and tracks results.

use ordo_agents::AgentRegistry;
use ordo_tasks::{
    AgentId, Goal, GoalId, GoalStatus, Task, TaskId,
    TaskOutput, TaskQueue, TaskStatus, TaskType,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

// ─── Dispatcher Errors ─────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("no agent registered for task type {task_type:?}")]
    NoMatchingAgent { task_type: TaskType },
    #[error("agent {agent_id} is not allowed to use tool '{tool}'")]
    ToolNotAllowed { agent_id: AgentId, tool: String },
    #[error("task {task_id} has unmet dependencies")]
    DependenciesNotSatisfied { task_id: TaskId },
    #[error("task {task_id} timed out after {duration:?}")]
    TaskTimedOut { task_id: TaskId, duration: Duration },
    #[error("task {task_id} failed after all retries: {error}")]
    TaskExhaustedRetries { task_id: TaskId, error: String },
    #[error("goal {goal_id} not found")]
    GoalNotFound { goal_id: GoalId },
}

// ─── Dispatch Config ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DispatchConfig {
    /// Default timeout per task.
    pub task_timeout: Duration,
    /// Maximum retries before failing a task permanently.
    pub max_retries: u32,
    /// How long to wait before retrying a failed task.
    pub retry_delay: Duration,
    /// Maximum concurrent tasks across all goals.
    pub max_concurrent: usize,
}

impl Default for DispatchConfig {
    fn default() -> Self {
        Self {
            task_timeout: Duration::from_secs(300),
            max_retries: 3,
            retry_delay: Duration::from_secs(5),
            max_concurrent: 10,
        }
    }
}

// ─── Task Execution Result ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: TaskId,
    pub goal_id: GoalId,
    pub output: Option<TaskOutput>,
    pub status: TaskStatus,
    pub agent_id: Option<AgentId>,
    pub error: Option<String>,
    pub duration: Duration,
}

// ─── Task Runner Trait ─────────────────────────────────────────────────────────
///
use async_trait::async_trait;

/// Implemented by the Brain or MCP host to actually execute a task.
/// The Dispatcher calls this and waits for the result.

#[async_trait]
pub trait TaskRunner: Send + Sync {
    /// Execute a single task and return its result.
    async fn run_task(
        &self,
        task: &Task,
        agent_id: Option<AgentId>,
    ) -> Result<TaskOutput, String>;
}

// ─── Dispatcher ────────────────────────────────────────────────────────────────

pub struct Dispatcher {
    queue: RwLock<TaskQueue>,
    registry: RwLock<AgentRegistry>,
    config: DispatchConfig,
    running: RwLock<HashMap<TaskId, GoalId>>,
    completed: RwLock<Vec<TaskResult>>,
}

impl Dispatcher {
    pub fn new(registry: AgentRegistry, config: DispatchConfig) -> Self {
        Self {
            queue: RwLock::new(TaskQueue::new()),
            registry: RwLock::new(registry),
            config,
            running: RwLock::new(HashMap::new()),
            completed: RwLock::new(vec![]),
        }
    }

    pub fn with_default_registry() -> Self {
        Self::new(AgentRegistry::default_registry(), DispatchConfig::default())
    }

    /// Register a prepared goal with its task plan. The goal transitions from
    /// Created → Ready once all tasks are enqueued.
    pub fn submit_goal(&self, mut goal: Goal) {
        goal.transition(GoalStatus::Ready);
        self.queue.write().enqueue_goal(goal);
    }

    /// Get the next task that's ready to execute.
    pub fn next_ready(&self) -> Option<(Goal, Task)> {
        let queue = self.queue.read();
        queue.next_ready().map(|(g, t)| (g.clone(), t.clone()))
    }

    /// Claim a task for execution (marks it Queued → Running).
    pub fn claim(&self, task_id: TaskId, agent_id: AgentId) -> Result<Task, DispatchError> {
        let mut queue = self.queue.write();
        let goal_id = queue
            .list_goals()
            .iter()
            .find(|g| g.tasks.iter().any(|t| t.id == task_id))
            .map(|g| g.id);

        let goal_id = goal_id.ok_or(DispatchError::GoalNotFound {
            goal_id: GoalId::nil(),
        })?;

        let goal = queue
            .get_goal_mut(&goal_id)
            .ok_or_else(|| DispatchError::GoalNotFound { goal_id })?;

        let task = goal
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| DispatchError::GoalNotFound { goal_id })?;

        task.assign(agent_id);
        task.transition(TaskStatus::Running);

        self.running.write().insert(task_id, goal_id);
        Ok(task.clone())
    }

    /// Record a completed task and unblock dependents.
    pub fn complete(&self, result: TaskResult) {
        let task_id = result.task_id;
        let goal_id = result.goal_id;

        {
            let mut queue = self.queue.write();
            if let Some(goal) = queue.get_goal_mut(&goal_id) {
                if let Some(task) = goal.tasks.iter_mut().find(|t| t.id == task_id) {
                    match &result.output {
                        Some(output) => task.complete(output.clone()),
                        None => task.transition(result.status),
                    }
                }
            }
            queue.complete_task(task_id);
        }

        self.running.write().remove(&task_id);
        self.completed.write().push(result.clone());

        // Transition goal if all tasks are terminal
        {
            let queue = self.queue.read();
            if let Some(goal) = queue.get_goal(&goal_id) {
                if goal.all_terminal() {
                    drop(queue);
                    let mut queue = self.queue.write();
                    if let Some(goal) = queue.get_goal_mut(&goal_id) {
                        let new_status = if goal.succeeded() {
                            GoalStatus::Completed
                        } else {
                            GoalStatus::Failed
                        };
                        goal.transition(new_status);
                    }
                }
            }
        }
    }

    /// Route a task to the appropriate agent based on task type.
    pub fn route(&self, task: &Task) -> Result<AgentId, DispatchError> {
        let registry = self.registry.read();

        // First try: exact task type match
        if let Some(agent) = registry.find_for_task_type(&task.task_type) {
            return Ok(agent.id);
        }

        // Second try: capability-based match
        if let TaskType::Capability(cap) = &task.task_type {
            let candidates = registry.candidates_for(&TaskType::Capability(cap.clone()));
            if let Some(agent) = candidates.first() {
                return Ok(agent.id);
            }
        }

        // Third try: find any agent with a matching tool
        let cap_name = task
            .task_type
            .default_capability()
            .unwrap_or("");
        let binding = registry.list();
        let candidates = binding
            .iter()
            .find(|a| a.allowed_tools.contains(cap_name));

        if let Some(agent) = candidates {
            return Ok(agent.id);
        }

        Err(DispatchError::NoMatchingAgent {
            task_type: task.task_type.clone(),
        })
    }

    // ─── Execution Engine ────────────────────────────────────────────────────

    /// Execute a full goal to completion using the provided runner.
    /// This is the main orchestration loop — it iterates through ready tasks,
    /// routes them to agents, runs them, retries on failure, and returns
    /// when all tasks are terminal.
    pub async fn execute_goal(
        &self,
        goal_id: GoalId,
        runner: &dyn TaskRunner,
    ) -> Result<Vec<TaskResult>, DispatchError> {
        let mut results: Vec<TaskResult> = vec![];

        loop {
            let goal = {
                let queue = self.queue.read();
                queue.get_goal(&goal_id).cloned()
            };

            let goal = match goal {
                Some(g) => g,
                None => return Err(DispatchError::GoalNotFound { goal_id }),
            };

            if goal.all_terminal() {
                break;
            }

            // Get next ready task
            let next = {
                let queue = self.queue.read();
                queue.next_ready().map(|(g, t)| (g.clone(), t.clone()))
            };

            let (_goal, task) = match next {
                Some((g, t)) if g.id == goal_id => (g, t),
                _ => break, // No more ready tasks for this goal
            };

            // Route to agent
            let agent_id = self.route(&task)?;
            let task_id = task.id;

            // Claim and execute
            self.claim(task_id, agent_id)?;

            let start = std::time::Instant::now();

            let exec_result = tokio::time::timeout(
                self.config.task_timeout,
                runner.run_task(&task, Some(agent_id)),
            )
            .await;

            let duration = start.elapsed();

            let result = match exec_result {
                Ok(Ok(output)) => TaskResult {
                    task_id,
                    goal_id,
                    output: Some(output),
                    status: TaskStatus::Completed,
                    agent_id: Some(agent_id),
                    error: None,
                    duration,
                },
                Ok(Err(err)) => {
                    // Check if we can retry
                    let should_retry = {
                        let queue = self.queue.read();
                        if let Some(goal) = queue.get_goal(&goal_id) {
                            goal.tasks
                                .iter()
                                .find(|t| t.id == task_id)
                                .map(|t| t.retry_count < t.max_retries)
                                .unwrap_or(false)
                        } else {
                            false
                        }
                    };

                    if should_retry {
                        // Fail with retry — task returns to Pending
                        {
                            let mut queue = self.queue.write();
                            if let Some(goal) = queue.get_goal_mut(&goal_id) {
                                if let Some(task) = goal.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.fail(err.clone());
                                }
                            }
                        }
                        tokio::time::sleep(self.config.retry_delay).await;
                        continue; // Skip recording, task will be picked up again
                    } else {
                        TaskResult {
                            task_id,
                            goal_id,
                            output: None,
                            status: TaskStatus::Failed,
                            agent_id: Some(agent_id),
                            error: Some(err),
                            duration,
                        }
                    }
                }
                Err(_timeout) => TaskResult {
                    task_id,
                    goal_id,
                    output: None,
                    status: TaskStatus::Failed,
                    agent_id: Some(agent_id),
                    error: Some(format!(
                        "Timed out after {:?}",
                        self.config.task_timeout
                    )),
                    duration,
                },
            };

            self.complete(result.clone());
            results.push(result);
        }

        Ok(results)
    }

    // ─── Inspection ──────────────────────────────────────────────────────────

    pub fn goals(&self) -> Vec<Goal> {
        self.queue.read().list_goals_cloned()
    }

    pub fn goal_status(&self, goal_id: &GoalId) -> Option<GoalStatus> {
        self.queue
            .read()
            .get_goal(goal_id)
            .map(|g| g.status)
    }

    pub fn running_count(&self) -> usize {
        self.running.read().len()
    }

    pub fn completed_results(&self) -> Vec<TaskResult> {
        self.completed.read().clone()
    }

    pub fn get_registry(&self) -> &RwLock<AgentRegistry> {
        &self.registry
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_tasks::{Priority, TaskInput};
    use uuid::Uuid;

    fn make_goal_with_tasks(description: &str, task_types: Vec<TaskType>) -> Goal {
        let mut goal = Goal::new(description.into());
        let mut prev_id: Option<TaskId> = None;
        for ttype in task_types {
            let deps: Vec<TaskId> = prev_id.iter().copied().collect();
            let task = Task::new(
                goal.id,
                ttype,
                Priority::Normal,
                deps,
                TaskInput::new(goal.description.clone(), serde_json::json!({})),
                false,
            );
            prev_id = Some(task.id);
            goal.add_task(task);
        }
        goal
    }

    struct StubRunner {
        should_fail: bool,
    }

    #[async_trait::async_trait]
    impl TaskRunner for StubRunner {
        async fn run_task(
            &self,
            _task: &Task,
            _agent_id: Option<AgentId>,
        ) -> Result<TaskOutput, String> {
            if self.should_fail {
                Err("stub failure".into())
            } else {
                Ok(TaskOutput::new(
                    serde_json::json!({"result": "ok"}),
                    "stub completed".into(),
                ))
            }
        }
    }

    #[test]
    fn dispatcher_routes_by_task_type() {
        let dispatcher = Dispatcher::with_default_registry();
        let task = Task::new(
            Uuid::new_v4(),
            TaskType::Seo,
            Priority::Normal,
            vec![],
            TaskInput::new("test".into(), serde_json::json!({})),
            false,
        );
        let agent_id = dispatcher.route(&task);
        assert!(agent_id.is_ok());
    }

    #[test]
    fn dispatcher_rejects_unknown_type() {
        let dispatcher = Dispatcher::with_default_registry();
        let task = Task::new(
            Uuid::new_v4(),
            TaskType::Custom("unknown_strange_thing".into()),
            Priority::Normal,
            vec![],
            TaskInput::new("test".into(), serde_json::json!({})),
            false,
        );
        let result = dispatcher.route(&task);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_goal_runs_all_tasks() {
        let dispatcher = Dispatcher::with_default_registry();
        let runner = StubRunner { should_fail: false };
        let goal = make_goal_with_tasks("test pipeline", vec![
            TaskType::Research,
            TaskType::Draft,
        ]);
        let goal_id = goal.id;

        dispatcher.submit_goal(goal);
        let results = dispatcher.execute_goal(goal_id, &runner).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.status == TaskStatus::Completed));
        assert_eq!(
            dispatcher.goal_status(&goal_id).unwrap(),
            GoalStatus::Completed
        );
    }

    #[tokio::test]
    async fn execute_goal_retries_on_failure() {
        let dispatcher = Dispatcher::with_default_registry();
        let runner = StubRunner { should_fail: true };
        let mut goal = make_goal_with_tasks("failing", vec![TaskType::Research]);

        // Set max_retries to 0 so it fails immediately without retry
        for t in &mut goal.tasks {
            t.max_retries = 0;
        }
        let goal_id = goal.id;
        dispatcher.submit_goal(goal);

        let results = dispatcher.execute_goal(goal_id, &runner).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, TaskStatus::Failed);
        assert_eq!(dispatcher.goal_status(&goal_id).unwrap(), GoalStatus::Failed);
    }

    #[test]
    fn dispatcher_config_defaults() {
        let config = DispatchConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.max_concurrent, 10);
    }
}
