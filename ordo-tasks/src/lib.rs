// ordo-tasks — Task model, queue, statuses, dependency graph, and persistence.
//
// Every unit of work becomes a structured task. Tasks support dependencies,
// approval requirements, and a full state machine from Pending through Completed/Failed.
//
// This crate is a pure-data layer. It defines the types and storage. The
// ordo-dispatcher crate handles routing and execution.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub type GoalId = Uuid;
pub type TaskId = Uuid;
pub type AgentId = Uuid;

// ─── Task Model ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub goal_id: GoalId,
    pub task_type: TaskType,
    pub assigned_agent: Option<AgentId>,
    pub status: TaskStatus,
    pub priority: Priority,
    pub dependencies: Vec<TaskId>,
    pub input: TaskInput,
    pub output: Option<TaskOutput>,
    pub requires_approval: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub retry_count: u32,
    pub max_retries: u32,
    pub error: Option<String>,
}

impl Task {
    pub fn new(
        goal_id: GoalId,
        task_type: TaskType,
        priority: Priority,
        dependencies: Vec<TaskId>,
        input: TaskInput,
        requires_approval: bool,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: TaskId::new_v4(),
            goal_id,
            task_type,
            assigned_agent: None,
            status: TaskStatus::Pending,
            priority,
            dependencies,
            input,
            output: None,
            requires_approval,
            created_at: now,
            updated_at: now,
            retry_count: 0,
            max_retries: 3,
            error: None,
        }
    }

    pub fn assign(&mut self, agent_id: AgentId) {
        self.assigned_agent = Some(agent_id);
        self.updated_at = Utc::now();
    }

    pub fn transition(&mut self, status: TaskStatus) {
        self.status = status;
        self.updated_at = Utc::now();
    }

    pub fn complete(&mut self, output: TaskOutput) {
        self.output = Some(output);
        self.status = TaskStatus::Completed;
        self.updated_at = Utc::now();
    }

    pub fn fail(&mut self, error: String) {
        self.error = Some(error);
        self.retry_count += 1;
        if self.retry_count <= self.max_retries {
            self.status = TaskStatus::Pending;
            self.updated_at = Utc::now();
        } else {
            self.status = TaskStatus::Failed;
            self.updated_at = Utc::now();
        }
    }

    pub fn is_ready(&self, completed: &HashSet<TaskId>) -> bool {
        if self.status != TaskStatus::Pending {
            return false;
        }
        self.dependencies.iter().all(|dep| completed.contains(dep))
    }
}

// ─── Task Type ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    /// Knowledge gathering and source review.
    Research,
    /// Cross-source analysis, comparison, and synthesis.
    Analysis,
    /// Code generation, modification, or review.
    Code,
    /// Runtime, provider, MCP, plugin, or infrastructure operations.
    Operations,
    /// Scheduled, heartbeat, webhook, or event-triggered work.
    Automation,
    /// Runtime self-checks, repair proposals, and local diagnostics.
    Diagnostics,
    /// Security and policy review.
    SecurityReview,
    /// Memory storage, retrieval, or promotion review.
    MemoryUpdate,
    /// Generic capability invocation.
    Capability(String),
    /// Custom task type.
    Custom(String),
}

impl TaskType {
    pub fn label(&self) -> &str {
        match self {
            TaskType::Research => "Research",
            TaskType::Analysis => "Analysis",
            TaskType::Code => "Code",
            TaskType::Operations => "Operations",
            TaskType::Automation => "Automation",
            TaskType::Diagnostics => "Diagnostics",
            TaskType::SecurityReview => "Security Review",
            TaskType::MemoryUpdate => "Memory Update",
            TaskType::Capability(_) => "Capability",
            TaskType::Custom(_) => "Custom",
        }
    }

    pub fn default_capability(&self) -> Option<&str> {
        match self {
            TaskType::Research => Some("research.fetch"),
            TaskType::Code => Some("code.generate"),
            TaskType::Analysis => Some("knowledge.analyze"),
            TaskType::Operations => Some("runtime.describe_profile"),
            TaskType::Automation => Some("jobs.describe"),
            TaskType::Diagnostics => Some("runtime.describe_settings"),
            TaskType::SecurityReview => Some("security.review"),
            TaskType::MemoryUpdate => Some("memory.remember_note"),
            _ => None,
        }
    }
}

// ─── Task Status ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    WaitingForDependency,
    Queued,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
    BlockedByPolicy,
}

impl TaskStatus {
    pub fn label(&self) -> &str {
        match self {
            TaskStatus::Pending => "Pending",
            TaskStatus::WaitingForDependency => "Waiting for Dependency",
            TaskStatus::Queued => "Queued",
            TaskStatus::Running => "Running",
            TaskStatus::WaitingForApproval => "Awaiting Approval",
            TaskStatus::Completed => "Completed",
            TaskStatus::Failed => "Failed",
            TaskStatus::Cancelled => "Cancelled",
            TaskStatus::BlockedByPolicy => "Blocked by Policy",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::Cancelled
                | TaskStatus::BlockedByPolicy
        )
    }
}

// ─── Priority ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low = 0,
    #[default]
    Normal = 1,
    High = 2,
    Critical = 3,
}

// ─── Task Input / Output ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskInput {
    pub goal: String,
    pub context: serde_json::Value,
    pub upstream_outputs: HashMap<TaskId, TaskOutput>,
}

impl TaskInput {
    pub fn new(goal: String, context: serde_json::Value) -> Self {
        Self {
            goal,
            context,
            upstream_outputs: HashMap::new(),
        }
    }

    pub fn with_upstream(mut self, task_id: TaskId, output: TaskOutput) -> Self {
        self.upstream_outputs.insert(task_id, output);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskOutput {
    pub data: serde_json::Value,
    pub summary: String,
    pub confidence: Option<f64>,
    pub metadata: serde_json::Value,
}

impl TaskOutput {
    pub fn new(data: serde_json::Value, summary: String) -> Self {
        Self {
            data,
            summary,
            confidence: None,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = Some(confidence);
        self
    }
}

// ─── Goal (the top-level unit of work) ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub id: GoalId,
    pub description: String,
    pub tasks: Vec<Task>,
    pub status: GoalStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Goal {
    pub fn new(description: String) -> Self {
        let now = Utc::now();
        Self {
            id: GoalId::new_v4(),
            description,
            tasks: vec![],
            status: GoalStatus::Created,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn add_task(&mut self, task: Task) {
        self.tasks.push(task);
        self.updated_at = Utc::now();
    }

    pub fn task_ids(&self) -> Vec<TaskId> {
        self.tasks.iter().map(|t| t.id).collect()
    }

    pub fn completed_task_ids(&self) -> HashSet<TaskId> {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .map(|t| t.id)
            .collect()
    }

    pub fn next_ready_tasks(&self) -> Vec<&Task> {
        let completed = self.completed_task_ids();
        self.tasks
            .iter()
            .filter(|t| t.is_ready(&completed))
            .collect()
    }

    pub fn all_terminal(&self) -> bool {
        self.tasks.iter().all(|t| t.status.is_terminal())
    }

    pub fn succeeded(&self) -> bool {
        self.tasks
            .iter()
            .all(|t| t.status == TaskStatus::Completed || t.status == TaskStatus::Cancelled)
    }

    pub fn transition(&mut self, status: GoalStatus) {
        self.status = status;
        self.updated_at = Utc::now();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalStatus {
    Created,
    Planning,
    Ready,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

impl GoalStatus {
    pub fn label(&self) -> &str {
        match self {
            GoalStatus::Created => "Created",
            GoalStatus::Planning => "Planning",
            GoalStatus::Ready => "Ready",
            GoalStatus::Running => "Running",
            GoalStatus::WaitingForApproval => "Awaiting Approval",
            GoalStatus::Completed => "Completed",
            GoalStatus::Failed => "Failed",
            GoalStatus::Cancelled => "Cancelled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            GoalStatus::Completed | GoalStatus::Failed | GoalStatus::Cancelled
        )
    }
}

// ─── Task Queue ────────────────────────────────────────────────────────────────

/// In-memory task queue with priority ordering and dependency gating.
/// Does NOT persist — persistence is separate via ordo-store if needed.
#[derive(Debug, Default)]
pub struct TaskQueue {
    pub(crate) goals: HashMap<GoalId, Goal>,
    /// Tracks which tasks are waiting on which dependencies.
    /// Key = blocked task, Value = dependencies it's waiting on.
    pub(crate) block_graph: HashMap<TaskId, HashSet<TaskId>>,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self {
            goals: HashMap::new(),
            block_graph: HashMap::new(),
        }
    }

    pub fn enqueue_goal(&mut self, goal: Goal) {
        let goal_id = goal.id;
        for task in &goal.tasks {
            if !task.dependencies.is_empty() {
                let blocked: HashSet<TaskId> = task.dependencies.iter().copied().collect();
                self.block_graph.insert(task.id, blocked);
            }
        }
        self.goals.insert(goal_id, goal);
    }

    pub fn get_goal(&self, goal_id: &GoalId) -> Option<&Goal> {
        self.goals.get(goal_id)
    }

    pub fn get_goal_mut(&mut self, goal_id: &GoalId) -> Option<&mut Goal> {
        self.goals.get_mut(goal_id)
    }

    pub fn list_goals(&self) -> Vec<&Goal> {
        self.goals.values().collect()
    }

    pub fn list_goals_cloned(&self) -> Vec<Goal> {
        self.goals.values().cloned().collect()
    }

    pub fn complete_task(&mut self, task_id: TaskId) {
        // Unblock any tasks that depended on this one
        let mut unblocked: Vec<TaskId> = vec![];
        for (blocked_id, deps) in &mut self.block_graph {
            deps.remove(&task_id);
            if deps.is_empty() {
                unblocked.push(*blocked_id);
            }
        }
        for id in unblocked {
            self.block_graph.remove(&id);
        }
    }

    /// Get the next task that's ready to run (dependencies satisfied, not yet dispatched).
    /// Returns the highest-priority ready task across all goals.
    pub fn next_ready(&self) -> Option<(&Goal, &Task)> {
        let mut candidates: Vec<(&Goal, &Task)> = vec![];
        for goal in self.goals.values() {
            if goal.status.is_terminal() {
                continue;
            }
            let completed = goal.completed_task_ids();
            for task in &goal.tasks {
                if task.is_ready(&completed) && task.status == TaskStatus::Pending {
                    candidates.push((goal, task));
                }
            }
        }
        // Sort by priority (descending), then creation time (ascending)
        candidates.sort_by(|(_, a), (_, b)| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        candidates.first().copied()
    }

    pub fn cancel_goal(&mut self, goal_id: &GoalId) -> Option<Vec<TaskId>> {
        let goal = self.goals.get_mut(goal_id)?;
        let cancelled: Vec<TaskId> = goal
            .tasks
            .iter_mut()
            .filter(|t| !t.status.is_terminal())
            .map(|t| {
                t.status = TaskStatus::Cancelled;
                t.id
            })
            .collect();
        goal.transition(GoalStatus::Cancelled);
        Some(cancelled)
    }

    pub fn remove_goal(&mut self, goal_id: &GoalId) -> Option<Goal> {
        self.goals.remove(goal_id)
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(goal_id: GoalId, task_type: TaskType, deps: Vec<TaskId>) -> Task {
        Task::new(
            goal_id,
            task_type,
            Priority::Normal,
            deps,
            TaskInput::new("test goal".into(), serde_json::json!({})),
            false,
        )
    }

    #[test]
    fn task_status_labels() {
        assert_eq!(TaskStatus::Pending.label(), "Pending");
        assert_eq!(TaskStatus::Completed.label(), "Completed");
        assert_eq!(TaskStatus::Failed.label(), "Failed");
    }

    #[test]
    fn terminal_statuses() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn task_retry_system() {
        let mut task = make_task(Uuid::new_v4(), TaskType::Research, vec![]);
        task.max_retries = 2;

        task.fail("error 1".into());
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.retry_count, 1);

        task.fail("error 2".into());
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.retry_count, 2);

        // Third failure exceeds max_retries
        let mut task2 = make_task(Uuid::new_v4(), TaskType::Research, vec![]);
        task2.max_retries = 2;
        task2.fail("e1".into());
        task2.fail("e2".into());
        task2.fail("e3".into());
        assert_eq!(task2.status, TaskStatus::Failed);
        assert_eq!(task2.retry_count, 3);
    }

    #[test]
    fn dependency_graph_enforces_ordering() {
        let goal_id = Uuid::new_v4();
        let task_a = make_task(goal_id, TaskType::Research, vec![]);
        let task_b = make_task(goal_id, TaskType::Analysis, vec![task_a.id]);
        let task_c = make_task(goal_id, TaskType::MemoryUpdate, vec![task_b.id]);

        let mut goal = Goal::new("test pipeline".into());
        goal.add_task(task_a.clone());
        goal.add_task(task_b.clone());
        goal.add_task(task_c.clone());

        // Initially only A is ready
        let ready = goal.next_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, task_a.id);

        // Complete A -> B becomes ready
        goal.tasks[0].complete(TaskOutput::new(
            serde_json::json!({"result": "research done"}),
            "done".into(),
        ));
        let ready = goal.next_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, task_b.id);
    }

    #[test]
    fn task_queue_dispatches_by_priority() {
        let mut queue = TaskQueue::new();

        let goal_a = Goal::new("low priority".into());
        let mut task_high = make_task(goal_a.id, TaskType::Research, vec![]);
        task_high.priority = Priority::High;

        let goal_b = Goal::new("normal priority".into());
        let task_norm = make_task(goal_b.id, TaskType::Research, vec![]);

        let mut g1 = Goal::new("h".into());
        g1.id = goal_a.id;
        g1.add_task(task_high);
        queue.enqueue_goal(g1);

        let mut g2 = Goal::new("n".into());
        g2.id = goal_b.id;
        g2.add_task(task_norm);
        queue.enqueue_goal(g2);

        let next = queue.next_ready();
        assert!(next.is_some());
        // High priority should come first
        assert_eq!(next.unwrap().1.priority, Priority::High);
    }

    #[test]
    fn goal_lifecycle() {
        let mut goal = Goal::new("test".into());
        assert!(matches!(goal.status, GoalStatus::Created));

        goal.transition(GoalStatus::Planning);
        assert!(matches!(goal.status, GoalStatus::Planning));

        goal.transition(GoalStatus::Running);
        assert!(matches!(goal.status, GoalStatus::Running));

        goal.transition(GoalStatus::Completed);
        assert!(goal.status.is_terminal());
    }

    #[test]
    fn task_output_confidence() {
        let output = TaskOutput::new(serde_json::json!({}), "summary".into()).with_confidence(0.95);
        assert_eq!(output.confidence, Some(0.95));
    }

    #[test]
    fn task_type_default_capabilities() {
        assert_eq!(
            TaskType::Research.default_capability(),
            Some("research.fetch")
        );
        assert_eq!(
            TaskType::Analysis.default_capability(),
            Some("knowledge.analyze")
        );
        assert_eq!(
            TaskType::Diagnostics.default_capability(),
            Some("runtime.describe_settings")
        );
        assert_eq!(
            TaskType::Custom("my.custom".into()).default_capability(),
            None
        );
    }
}
