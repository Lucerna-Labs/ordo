// ordo-jobs — Job scheduler for autonomous work.
//
// Jobs are different from tasks. A task is user-initiated, part of a goal plan,
// and executed by the Dispatcher. A job runs autonomously — scheduled, recurring,
// event-triggered, or file-watching.
//
// The Job Scheduler maintains a set of job definitions and a tick loop that
// checks which jobs are due. It does NOT execute jobs directly — it emits
// events that the Dispatcher or Brain picks up.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::broadcast;
use uuid::Uuid;

pub type JobId = Uuid;
pub type WorkerId = Uuid;

// ─── Job Model ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub name: String,
    pub description: String,
    pub job_type: JobType,
    pub trigger: JobTrigger,
    pub assigned_worker: Option<WorkerId>,
    pub status: JobStatus,
    pub policy_level: PolicyLevel,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub run_count: u64,
    pub error_count: u64,
}

impl Job {
    pub fn new(name: &str, description: &str, job_type: JobType, trigger: JobTrigger) -> Self {
        let now = Utc::now();
        Self {
            id: JobId::new_v4(),
            name: name.into(),
            description: description.into(),
            job_type,
            trigger,
            assigned_worker: None,
            status: JobStatus::Enabled,
            policy_level: PolicyLevel::SafeReadOnly,
            last_run: None,
            next_run: None,
            created_at: now,
            updated_at: now,
            run_count: 0,
            error_count: 0,
        }
    }

    pub fn with_policy(mut self, level: PolicyLevel) -> Self {
        self.policy_level = level;
        self
    }

    pub fn completed_run(&mut self) {
        self.last_run = Some(Utc::now());
        self.run_count += 1;
        self.updated_at = Utc::now();

        if let JobTrigger::Interval(dur) = &self.trigger {
            self.next_run = Some(
                Utc::now() + chrono::Duration::from_std(*dur).unwrap_or(chrono::Duration::hours(1)),
            );
        }
    }

    pub fn recorded_error(&mut self) {
        self.error_count += 1;
        self.last_run = Some(Utc::now());
        self.updated_at = Utc::now();
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        if self.status != JobStatus::Enabled {
            return false;
        }
        match &self.trigger {
            JobTrigger::At(at) => *at <= now && self.run_count == 0,
            JobTrigger::Interval(_) => {
                self.next_run.map(|nr| nr <= now).unwrap_or(true) // Never ran — due immediately
            }
            JobTrigger::Event(_) => false, // Event jobs aren't time-based
            JobTrigger::WatchPath(_) => false, // Watch jobs poll separately
            JobTrigger::Condition { .. } => false, // Condition jobs check separately
        }
    }
}

// ─── Job Type ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobType {
    /// One-shot: runs once at a specific time
    OneShot,
    /// Runs repeatedly on a schedule
    Recurring,
    /// Triggered by a system event
    EventDriven,
    /// Polls a condition or watches a path
    Watcher,
}

// ─── Job Trigger ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobTrigger {
    /// Run at a specific timestamp.
    At(DateTime<Utc>),
    /// Run every N duration.
    Interval(Duration),
    /// Triggered by a named event kind.
    Event(String),
    /// Watch a file system path for changes.
    WatchPath(PathBuf),
    /// Evaluate a condition string when polled.
    Condition { predicate: String },
}

impl JobTrigger {
    pub fn label(&self) -> String {
        match self {
            JobTrigger::At(dt) => format!("At {}", dt.format("%Y-%m-%d %H:%M")),
            JobTrigger::Interval(d) => format!("Every {:.0}s", d.as_secs()),
            JobTrigger::Event(e) => format!("On event: {e}"),
            JobTrigger::WatchPath(p) => format!("Watch: {}", p.display()),
            JobTrigger::Condition { predicate } => format!("When: {predicate}"),
        }
    }
}

// ─── Job Status ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Enabled,
    Disabled,
    Running,
    Paused,
    Error,
    Completed,
}

impl JobStatus {
    pub fn label(&self) -> &str {
        match self {
            JobStatus::Enabled => "Enabled",
            JobStatus::Disabled => "Disabled",
            JobStatus::Running => "Running",
            JobStatus::Paused => "Paused",
            JobStatus::Error => "Error",
            JobStatus::Completed => "Completed",
        }
    }
}

// ─── Policy Level ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PolicyLevel {
    /// Read-only, safe for full autonomy
    #[default]
    SafeReadOnly = 0,
    /// Local file read
    LocalRead = 1,
    /// Local file write
    LocalWrite = 2,
    /// External network read
    NetworkRead = 3,
    /// External network write
    NetworkWrite = 4,
    /// Requires user approval for high-risk actions
    RequiresApproval = 5,
}

impl PolicyLevel {
    pub fn label(&self) -> &str {
        match self {
            PolicyLevel::SafeReadOnly => "Safe Read-Only",
            PolicyLevel::LocalRead => "Local Read",
            PolicyLevel::LocalWrite => "Local Write",
            PolicyLevel::NetworkRead => "Network Read",
            PolicyLevel::NetworkWrite => "Network Write",
            PolicyLevel::RequiresApproval => "Requires Approval",
        }
    }

    pub fn is_autonomous_safe(&self) -> bool {
        *self <= PolicyLevel::NetworkRead
    }
}

// ─── Job Event (emitted by the scheduler) ──────────────────────────────────────

#[derive(Debug, Clone)]
pub enum JobEvent {
    /// A job is due and should be dispatched.
    JobTriggered { job: Job },
    /// A job completed successfully.
    JobCompleted {
        job_id: JobId,
        output: serde_json::Value,
    },
    /// A job failed.
    JobFailed { job_id: JobId, error: String },
    /// A watcher job detected a change.
    PathChanged { job_id: JobId, path: PathBuf },
    /// A condition was met.
    ConditionMet { job_id: JobId },
}

// ─── Job Definition (what the job actually does) ───────────────────────────────

/// A task to execute when a job fires. Jobs emit these as structured work
/// items that the Dispatcher or Brain can pick up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobTask {
    pub capability: String,
    pub args: serde_json::Value,
    pub requires_approval: bool,
}

// ─── Job Scheduler ─────────────────────────────────────────────────────────────

pub struct JobScheduler {
    jobs: HashMap<JobId, (Job, Vec<JobTask>)>,
    event_tx: broadcast::Sender<JobEvent>,
}

impl JobScheduler {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            jobs: HashMap::new(),
            event_tx: tx,
        }
    }

    pub fn event_receiver(&self) -> broadcast::Receiver<JobEvent> {
        self.event_tx.subscribe()
    }

    // ─── Job CRUD ───────────────────────────────────────────────────────────

    pub fn register(&mut self, job: Job, tasks: Vec<JobTask>) -> JobId {
        let id = job.id;
        self.jobs.insert(id, (job, tasks));
        id
    }

    pub fn get(&self, id: &JobId) -> Option<&(Job, Vec<JobTask>)> {
        self.jobs.get(id)
    }

    pub fn get_mut(&mut self, id: &JobId) -> Option<&mut (Job, Vec<JobTask>)> {
        self.jobs.get_mut(id)
    }

    pub fn remove(&mut self, id: &JobId) -> Option<(Job, Vec<JobTask>)> {
        self.jobs.remove(id)
    }

    pub fn list(&self) -> Vec<&Job> {
        self.jobs.values().map(|(j, _)| j).collect()
    }

    pub fn list_with_tasks(&self) -> Vec<(&Job, &Vec<JobTask>)> {
        self.jobs.values().map(|(j, t)| (j, t)).collect()
    }

    pub fn enable(&mut self, id: &JobId) -> bool {
        if let Some((job, _)) = self.jobs.get_mut(id) {
            job.status = JobStatus::Enabled;
            job.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    pub fn disable(&mut self, id: &JobId) -> bool {
        if let Some((job, _)) = self.jobs.get_mut(id) {
            job.status = JobStatus::Disabled;
            job.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    // ─── Tick Loop ──────────────────────────────────────────────────────────

    /// Tick the scheduler. Checks all enabled time-based jobs and emits
    /// `JobTriggered` events for any that are due.
    pub fn tick(&mut self, now: DateTime<Utc>) -> Vec<JobEvent> {
        let mut events = vec![];
        for (job, _tasks) in self.jobs.values_mut() {
            if job.is_due(now) {
                job.status = JobStatus::Running;
                let event = JobEvent::JobTriggered { job: job.clone() };
                let _ = self.event_tx.send(event.clone());
                events.push(event);
            }
        }
        events
    }

    /// Fire a specific job by event kind (for event-driven jobs).
    pub fn fire_event(&mut self, event_kind: &str) -> Vec<JobEvent> {
        let mut events = vec![];
        for (job, _tasks) in self.jobs.values_mut() {
            if let JobTrigger::Event(ref evt) = job.trigger {
                if evt == event_kind && job.status == JobStatus::Enabled {
                    job.status = JobStatus::Running;
                    let event = JobEvent::JobTriggered { job: job.clone() };
                    let _ = self.event_tx.send(event.clone());
                    events.push(event);
                }
            }
        }
        events
    }

    /// Notify watcher jobs that a path changed.
    pub fn notify_path(&mut self, path: &std::path::Path) -> Vec<JobEvent> {
        let mut events = vec![];
        for (id, (job, _)) in &mut self.jobs {
            if let JobTrigger::WatchPath(ref watch_path) = job.trigger {
                if path.starts_with(watch_path) && job.status == JobStatus::Enabled {
                    let event = JobEvent::PathChanged {
                        job_id: *id,
                        path: path.to_path_buf(),
                    };
                    let _ = self.event_tx.send(event.clone());
                    events.push(event);
                }
            }
        }
        events
    }

    /// Mark a triggered job as completed.
    pub fn mark_completed(&mut self, id: &JobId, output: serde_json::Value) {
        if let Some((job, _)) = self.jobs.get_mut(id) {
            job.completed_run();
            job.status = match job.job_type {
                JobType::OneShot => JobStatus::Completed,
                JobType::Recurring => JobStatus::Enabled,
                JobType::EventDriven | JobType::Watcher => JobStatus::Enabled,
            };
        }
        let _ = self.event_tx.send(JobEvent::JobCompleted {
            job_id: *id,
            output,
        });
    }

    /// Mark a triggered job as failed.
    pub fn mark_failed(&mut self, id: &JobId, error: String) {
        if let Some((job, _)) = self.jobs.get_mut(id) {
            job.recorded_error();
            job.status = match job.job_type {
                JobType::OneShot => JobStatus::Error,
                _ => JobStatus::Enabled, // Recurring/events keep trying
            };
        }
        let _ = self
            .event_tx
            .send(JobEvent::JobFailed { job_id: *id, error });
    }

    // ─── Default Jobs ───────────────────────────────────────────────────────

    pub fn with_default_jobs() -> Self {
        let mut s = Self::new();

        // System health check every hour.
        let health = Job::new(
            "System Health Check",
            "Reviews runtime, storage, and provider health without changing state.",
            JobType::Recurring,
            JobTrigger::Interval(Duration::from_secs(3600)),
        )
        .with_policy(PolicyLevel::SafeReadOnly);
        s.register(
            health,
            vec![JobTask {
                capability: "runtime.describe_profile".into(),
                args: serde_json::json!({}),
                requires_approval: false,
            }],
        );

        // Provider availability check.
        let provider_check = Job::new(
            "Provider Availability Check",
            "Checks configured model providers and reports degraded lanes.",
            JobType::Recurring,
            JobTrigger::Interval(Duration::from_secs(1800)),
        )
        .with_policy(PolicyLevel::LocalWrite);
        s.register(
            provider_check,
            vec![JobTask {
                capability: "cloud.credentials.list".into(),
                args: serde_json::json!({}),
                requires_approval: false,
            }],
        );

        // MCP and plugin audit.
        let integration_audit = Job::new(
            "MCP And Plugin Audit",
            "Lists MCP servers, skills, and plugins so the operator can spot missing integrations.",
            JobType::Recurring,
            JobTrigger::Interval(Duration::from_secs(86400)),
        )
        .with_policy(PolicyLevel::SafeReadOnly);
        s.register(
            integration_audit,
            vec![JobTask {
                capability: "mcp.servers.list".into(),
                args: serde_json::json!({"mode": "scan_all"}),
                requires_approval: false,
            }],
        );

        s
    }
}

impl Default for JobScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_due_on_schedule() {
        let now = Utc::now();
        let past = now - chrono::Duration::seconds(10);
        let job = Job::new("test", "", JobType::OneShot, JobTrigger::At(past));
        assert!(job.is_due(now));
    }

    #[test]
    fn job_not_due_yet() {
        let now = Utc::now();
        let future = now + chrono::Duration::hours(1);
        let job = Job::new("test", "", JobType::OneShot, JobTrigger::At(future));
        assert!(!job.is_due(now));
    }

    #[test]
    fn interval_job_due_first_time() {
        let now = Utc::now();
        let job = Job::new(
            "test",
            "",
            JobType::Recurring,
            JobTrigger::Interval(Duration::from_secs(60)),
        );
        // Never ran, next_run is None — should be due
        assert!(job.is_due(now));
    }

    #[test]
    fn interval_job_not_due_right_after_run() {
        let now = Utc::now();
        let mut job = Job::new(
            "test",
            "",
            JobType::Recurring,
            JobTrigger::Interval(Duration::from_secs(60)),
        );
        job.completed_run();
        assert!(!job.is_due(now));
    }

    #[test]
    fn disabled_job_not_due() {
        let now = Utc::now();
        let past = now - chrono::Duration::seconds(10);
        let mut job = Job::new("test", "", JobType::OneShot, JobTrigger::At(past));
        job.status = JobStatus::Disabled;
        assert!(!job.is_due(now));
    }

    #[test]
    fn scheduler_ticks_due_jobs() {
        let mut s = JobScheduler::new();
        let past = Utc::now() - chrono::Duration::seconds(10);
        let job = Job::new("due", "", JobType::OneShot, JobTrigger::At(past));
        s.register(job, vec![]);

        let events = s.tick(Utc::now());
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn default_jobs_exist() {
        let s = JobScheduler::with_default_jobs();
        let jobs = s.list();
        assert_eq!(jobs.len(), 3);
        let names: Vec<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
        assert!(names.contains(&"System Health Check"));
        assert!(names.contains(&"Provider Availability Check"));
        assert!(names.contains(&"MCP And Plugin Audit"));
    }

    #[test]
    fn one_shot_completes_on_first_run() {
        let mut s = JobScheduler::new();
        let past = Utc::now() - chrono::Duration::seconds(10);
        let job = Job::new("one", "", JobType::OneShot, JobTrigger::At(past));
        let id = job.id;
        s.register(job, vec![]);

        s.tick(Utc::now());
        s.mark_completed(&id, serde_json::json!({"ok": true}));

        let jobs = s.list();
        let j = jobs.iter().find(|j| j.id == id).unwrap();
        assert_eq!(j.status, JobStatus::Completed);
    }

    #[test]
    fn recurring_job_reenables_after_completion() {
        let mut s = JobScheduler::new();
        let past = Utc::now() - chrono::Duration::seconds(10);
        let job = Job::new("recur", "", JobType::Recurring, JobTrigger::At(past));
        let id = job.id;
        s.register(job, vec![]);

        s.tick(Utc::now());
        s.mark_completed(&id, serde_json::json!({}));

        let jobs = s.list();
        let j = jobs.iter().find(|j| j.id == id).unwrap();
        assert_eq!(j.status, JobStatus::Enabled);
    }

    #[test]
    fn policy_levels() {
        assert!(PolicyLevel::SafeReadOnly.is_autonomous_safe());
        assert!(PolicyLevel::NetworkRead.is_autonomous_safe());
        assert!(!PolicyLevel::NetworkWrite.is_autonomous_safe());
        assert!(!PolicyLevel::RequiresApproval.is_autonomous_safe());
    }
}
