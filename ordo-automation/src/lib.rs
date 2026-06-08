use chrono::{DateTime, Utc};
use ordo_automation_primitives::{
    validate_automation, ApprovalPolicy, AutomationId, AutomationIntent, AutomationScope,
    AutomationSpec, AutomationTrigger, AutomationValidationError, RiskLevel,
};
use ordo_jobs::{Job, JobEvent, JobScheduler, JobTask, JobTrigger, JobType, PolicyLevel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AutomationError {
    #[error(transparent)]
    Validation(#[from] AutomationValidationError),
    #[error("automation already exists")]
    AlreadyExists,
    #[error("automation not found")]
    NotFound,
    #[error("automation requires approval before registration")]
    ApprovalRequired,
}

#[derive(Debug, thiserror::Error)]
pub enum AutomationStoreError {
    #[error("automation store io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("automation store json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Automation(#[from] AutomationError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AutomationEvent {
    Registered {
        automation_id: AutomationId,
        name: String,
    },
    Enabled {
        automation_id: AutomationId,
    },
    Disabled {
        automation_id: AutomationId,
    },
    Deleted {
        automation_id: AutomationId,
    },
    ApprovalRequired {
        automation_id: AutomationId,
        risk: RiskLevel,
    },
    Triggered {
        automation_id: AutomationId,
        job_id: Uuid,
        intent: AutomationIntent,
    },
    Completed {
        automation_id: AutomationId,
        output: Value,
    },
    Failed {
        automation_id: AutomationId,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmConsultPlan {
    pub source_mode: String,
    pub target_mode: String,
    pub question: String,
    pub reason: String,
    pub max_iterations: u8,
}

impl SwarmConsultPlan {
    pub fn new(
        source_mode: impl Into<String>,
        target_mode: impl Into<String>,
        question: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            source_mode: source_mode.into(),
            target_mode: target_mode.into(),
            question: question.into(),
            reason: reason.into(),
            max_iterations: 1,
        }
    }

    pub fn to_intent(&self) -> AutomationIntent {
        AutomationIntent::ConsultMode {
            target_mode: self.target_mode.clone(),
            question: self.question.clone(),
            max_iterations: self.max_iterations,
        }
    }
}

pub struct AutomationOrchestrator {
    specs: HashMap<AutomationId, AutomationSpec>,
    jobs_by_automation: HashMap<AutomationId, Uuid>,
    automations_by_job: HashMap<Uuid, AutomationId>,
    scheduler: JobScheduler,
    event_log: Vec<AutomationEvent>,
}

impl AutomationOrchestrator {
    pub fn new() -> Self {
        Self {
            specs: HashMap::new(),
            jobs_by_automation: HashMap::new(),
            automations_by_job: HashMap::new(),
            scheduler: JobScheduler::new(),
            event_log: Vec::new(),
        }
    }

    pub fn from_specs(specs: Vec<AutomationSpec>) -> Result<Self, AutomationError> {
        let mut orchestrator = Self::new();
        for spec in specs {
            orchestrator.register(spec)?;
        }
        Ok(orchestrator)
    }

    pub fn load_or_seed(
        path: impl AsRef<Path>,
        defaults: Vec<AutomationSpec>,
    ) -> Result<Self, AutomationStoreError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::from_specs(defaults)?);
        }
        let bytes = std::fs::read(path)?;
        if bytes.is_empty() {
            return Ok(Self::from_specs(defaults)?);
        }
        let specs: Vec<AutomationSpec> = serde_json::from_slice(&bytes)?;
        Ok(Self::from_specs(specs)?)
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), AutomationStoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let specs = self.specs_snapshot();
        let bytes = serde_json::to_vec_pretty(&specs)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn specs_snapshot(&self) -> Vec<AutomationSpec> {
        let mut specs: Vec<AutomationSpec> = self.specs.values().cloned().collect();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        specs
    }

    pub fn register(
        &mut self,
        spec: AutomationSpec,
    ) -> Result<Vec<AutomationEvent>, AutomationError> {
        validate_automation(&spec)?;
        if self.specs.contains_key(&spec.id) {
            return Err(AutomationError::AlreadyExists);
        }
        if spec.requires_approval()
            && spec.metadata.get("approved").map(String::as_str) != Some("true")
        {
            let event = AutomationEvent::ApprovalRequired {
                automation_id: spec.id,
                risk: spec.risk(),
            };
            self.event_log.push(event.clone());
            self.specs.insert(spec.id, spec);
            return Ok(vec![event]);
        }

        self.register_without_approval(spec)
    }

    pub fn approve(
        &mut self,
        automation_id: AutomationId,
    ) -> Result<Vec<AutomationEvent>, AutomationError> {
        let spec = self
            .specs
            .get_mut(&automation_id)
            .ok_or(AutomationError::NotFound)?;
        spec.metadata.insert("approved".into(), "true".into());
        spec.updated_at = Utc::now();
        let approved = spec.clone();
        self.register_job_for_spec(&approved)
    }

    pub fn get(&self, automation_id: AutomationId) -> Option<&AutomationSpec> {
        self.specs.get(&automation_id)
    }

    pub fn list(&self) -> Vec<&AutomationSpec> {
        let mut specs: Vec<&AutomationSpec> = self.specs.values().collect();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        specs
    }

    pub fn enable(
        &mut self,
        automation_id: AutomationId,
    ) -> Result<AutomationEvent, AutomationError> {
        let spec = self
            .specs
            .get_mut(&automation_id)
            .ok_or(AutomationError::NotFound)?;
        spec.enabled = true;
        spec.updated_at = Utc::now();
        if let Some(job_id) = self.jobs_by_automation.get(&automation_id) {
            self.scheduler.enable(job_id);
        }
        let event = AutomationEvent::Enabled { automation_id };
        self.event_log.push(event.clone());
        Ok(event)
    }

    pub fn disable(
        &mut self,
        automation_id: AutomationId,
    ) -> Result<AutomationEvent, AutomationError> {
        let spec = self
            .specs
            .get_mut(&automation_id)
            .ok_or(AutomationError::NotFound)?;
        spec.enabled = false;
        spec.updated_at = Utc::now();
        if let Some(job_id) = self.jobs_by_automation.get(&automation_id) {
            self.scheduler.disable(job_id);
        }
        let event = AutomationEvent::Disabled { automation_id };
        self.event_log.push(event.clone());
        Ok(event)
    }

    pub fn delete(
        &mut self,
        automation_id: AutomationId,
    ) -> Result<AutomationEvent, AutomationError> {
        self.specs
            .remove(&automation_id)
            .ok_or(AutomationError::NotFound)?;
        if let Some(job_id) = self.jobs_by_automation.remove(&automation_id) {
            self.automations_by_job.remove(&job_id);
            self.scheduler.remove(&job_id);
        }
        let event = AutomationEvent::Deleted { automation_id };
        self.event_log.push(event.clone());
        Ok(event)
    }

    pub fn tick(&mut self, now: DateTime<Utc>) -> Vec<AutomationEvent> {
        let mut events = Vec::new();
        for job_event in self.scheduler.tick(now) {
            if let Some(event) = self.translate_job_event(job_event) {
                self.event_log.push(event.clone());
                events.push(event);
            }
        }
        events
    }

    pub fn fire_event(&mut self, topic: &str) -> Vec<AutomationEvent> {
        let mut events = Vec::new();
        for job_event in self.scheduler.fire_event(topic) {
            if let Some(event) = self.translate_job_event(job_event) {
                self.event_log.push(event.clone());
                events.push(event);
            }
        }
        events
    }

    pub fn event_log(&self) -> &[AutomationEvent] {
        &self.event_log
    }

    fn register_without_approval(
        &mut self,
        spec: AutomationSpec,
    ) -> Result<Vec<AutomationEvent>, AutomationError> {
        let events = self.register_job_for_spec(&spec)?;
        self.specs.insert(spec.id, spec);
        Ok(events)
    }

    fn register_job_for_spec(
        &mut self,
        spec: &AutomationSpec,
    ) -> Result<Vec<AutomationEvent>, AutomationError> {
        if !spec.enabled {
            let event = AutomationEvent::Registered {
                automation_id: spec.id,
                name: spec.name.clone(),
            };
            self.event_log.push(event.clone());
            return Ok(vec![event]);
        }

        if let Some(existing_job_id) = self.jobs_by_automation.remove(&spec.id) {
            self.automations_by_job.remove(&existing_job_id);
            self.scheduler.remove(&existing_job_id);
        }

        let job = job_from_spec(spec);
        let job_id = job.id;
        let tasks = tasks_from_spec(spec);
        self.scheduler.register(job, tasks);
        self.jobs_by_automation.insert(spec.id, job_id);
        self.automations_by_job.insert(job_id, spec.id);

        let event = AutomationEvent::Registered {
            automation_id: spec.id,
            name: spec.name.clone(),
        };
        self.event_log.push(event.clone());
        Ok(vec![event])
    }

    fn translate_job_event(&self, event: JobEvent) -> Option<AutomationEvent> {
        match event {
            JobEvent::JobTriggered { job } => {
                let automation_id = *self.automations_by_job.get(&job.id)?;
                let spec = self.specs.get(&automation_id)?;
                Some(AutomationEvent::Triggered {
                    automation_id,
                    job_id: job.id,
                    intent: spec.intent.clone(),
                })
            }
            JobEvent::JobCompleted { job_id, output } => {
                let automation_id = *self.automations_by_job.get(&job_id)?;
                Some(AutomationEvent::Completed {
                    automation_id,
                    output,
                })
            }
            JobEvent::JobFailed { job_id, error } => {
                let automation_id = *self.automations_by_job.get(&job_id)?;
                Some(AutomationEvent::Failed {
                    automation_id,
                    error,
                })
            }
            JobEvent::PathChanged { job_id, .. } | JobEvent::ConditionMet { job_id } => {
                let automation_id = *self.automations_by_job.get(&job_id)?;
                let spec = self.specs.get(&automation_id)?;
                Some(AutomationEvent::Triggered {
                    automation_id,
                    job_id,
                    intent: spec.intent.clone(),
                })
            }
        }
    }
}

impl Default for AutomationOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

fn job_from_spec(spec: &AutomationSpec) -> Job {
    let job_type = match spec.trigger {
        AutomationTrigger::At(_) | AutomationTrigger::Manual => JobType::OneShot,
        AutomationTrigger::IntervalSeconds(_)
        | AutomationTrigger::Heartbeat(_)
        | AutomationTrigger::Cron(_) => JobType::Recurring,
        AutomationTrigger::Event { .. }
        | AutomationTrigger::Webhook { .. }
        | AutomationTrigger::LocalSignal { .. } => JobType::EventDriven,
    };

    Job::new(
        &spec.name,
        &spec.description,
        job_type,
        trigger_from_spec(&spec.trigger),
    )
    .with_policy(policy_from_risk(spec.risk()))
}

fn trigger_from_spec(trigger: &AutomationTrigger) -> JobTrigger {
    match trigger {
        AutomationTrigger::Manual => JobTrigger::Event("automation.manual".into()),
        AutomationTrigger::At(at) => JobTrigger::At(*at),
        AutomationTrigger::IntervalSeconds(seconds) => {
            JobTrigger::Interval(Duration::from_secs(*seconds))
        }
        AutomationTrigger::Heartbeat(spec) => {
            JobTrigger::Interval(Duration::from_secs(spec.every_seconds))
        }
        AutomationTrigger::Cron(spec) => JobTrigger::Condition {
            predicate: format!("cron:{}@{}", spec.expression, spec.timezone),
        },
        AutomationTrigger::Event { topic } => JobTrigger::Event(topic.clone()),
        AutomationTrigger::Webhook { path } => JobTrigger::Event(format!("webhook:{path}")),
        AutomationTrigger::LocalSignal { name } => JobTrigger::Event(format!("local:{name}")),
    }
}

fn tasks_from_spec(spec: &AutomationSpec) -> Vec<JobTask> {
    flatten_intent(&spec.intent)
        .into_iter()
        .map(|intent| JobTask {
            capability: capability_for_intent(&intent),
            args: args_for_intent(&intent, &spec.scope),
            requires_approval: spec.approval.requires_approval_for(intent.risk()),
        })
        .collect()
}

fn flatten_intent(intent: &AutomationIntent) -> Vec<AutomationIntent> {
    match intent {
        AutomationIntent::Composite { steps } => steps
            .iter()
            .flat_map(flatten_intent)
            .collect::<Vec<AutomationIntent>>(),
        other => vec![other.clone()],
    }
}

fn capability_for_intent(intent: &AutomationIntent) -> String {
    match intent {
        AutomationIntent::RunCapability { capability, .. } => capability.clone(),
        AutomationIntent::ConsultMode { .. } => "assistant.consult_mode_agent".into(),
        AutomationIntent::SpawnSubagent { .. } => "assistant.spawn_subagent".into(),
        AutomationIntent::DreamingReview { .. } => "memory.dreaming.review".into(),
        AutomationIntent::DiagnosticSweep { .. } => "runtime.diagnostic_sweep".into(),
        AutomationIntent::CodingAutomation { .. } => "assistant.coding_automation".into(),
        AutomationIntent::Maintenance { capability, .. } => capability.clone(),
        AutomationIntent::Composite { .. } => "automation.composite".into(),
    }
}

fn args_for_intent(intent: &AutomationIntent, scope: &AutomationScope) -> Value {
    match intent {
        AutomationIntent::RunCapability { args, .. }
        | AutomationIntent::Maintenance { args, .. } => args.clone(),
        AutomationIntent::ConsultMode {
            target_mode,
            question,
            max_iterations,
        } => json!({
            "target_mode": target_mode,
            "question": question,
            "reason": "automation consult",
            "max_iterations": max_iterations,
            "scope": scope,
        }),
        AutomationIntent::SpawnSubagent {
            mode,
            goal,
            max_iterations,
            ..
        } => json!({
            "mode": mode,
            "goal": goal,
            "max_iterations": max_iterations,
            "scope": scope,
        }),
        AutomationIntent::DreamingReview {
            mode,
            signal_window,
        } => json!({
            "mode": mode,
            "signal_window": signal_window,
            "scope": scope,
        }),
        AutomationIntent::DiagnosticSweep { profile } => json!({
            "profile": profile,
            "scope": scope,
        }),
        AutomationIntent::CodingAutomation {
            workspace_path,
            mode,
            goal,
            max_subagents,
            write_policy,
            commit_policy,
            dependency_policy,
            risk,
        } => json!({
            "workspace_path": workspace_path,
            "mode": mode,
            "goal": goal,
            "max_subagents": max_subagents,
            "write_policy": write_policy,
            "commit_policy": commit_policy,
            "dependency_policy": dependency_policy,
            "risk": risk,
            "scope": scope,
            "guardrails": {
                "core_rust_or_tauri_mutation": "denied",
                "secrets_visible_to_model": false,
                "requires_operator_review_for_writes": *write_policy != ordo_automation_primitives::CodingWritePolicy::InspectOnly,
                "requires_operator_review_for_commits": *commit_policy == ordo_automation_primitives::CodingCommitPolicy::CommitWithApproval,
                "requires_operator_review_for_dependencies": *dependency_policy == ordo_automation_primitives::CodingDependencyPolicy::InstallWithApproval
            }
        }),
        AutomationIntent::Composite { .. } => json!({
            "scope": scope,
        }),
    }
}

fn policy_from_risk(risk: RiskLevel) -> PolicyLevel {
    match risk {
        RiskLevel::SafeReadOnly => PolicyLevel::SafeReadOnly,
        RiskLevel::LocalRead => PolicyLevel::LocalRead,
        RiskLevel::LocalWrite => PolicyLevel::LocalWrite,
        RiskLevel::NetworkRead => PolicyLevel::NetworkRead,
        RiskLevel::NetworkWrite => PolicyLevel::NetworkWrite,
        RiskLevel::PeripheralMaintenance | RiskLevel::CoreMutationDenied => {
            PolicyLevel::RequiresApproval
        }
    }
}

pub fn default_diagnostic_automation() -> AutomationSpec {
    AutomationSpec::new(
        "Diagnostic Sweep",
        "Runs local-only runtime checks and records repair recommendations.",
        AutomationTrigger::Heartbeat(ordo_automation_primitives::HeartbeatSpec::new(3600)),
        AutomationIntent::DiagnosticSweep {
            profile: "local-only".into(),
        },
        AutomationScope::Diagnostic,
        ApprovalPolicy::Never,
    )
}

pub fn default_dreaming_automation() -> AutomationSpec {
    AutomationSpec::new(
        "Dreaming Review",
        "Reviews completed work, failed runs, corrections, and improvement signals.",
        AutomationTrigger::Heartbeat(ordo_automation_primitives::HeartbeatSpec::new(7200)),
        AutomationIntent::DreamingReview {
            mode: "dreaming".into(),
            signal_window: "recent".into(),
        },
        AutomationScope::Mode {
            mode: "dreaming".into(),
        },
        ApprovalPolicy::Never,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_automation_primitives::{CronSpec, HeartbeatSpec};

    #[test]
    fn safe_automation_registers_as_job() {
        let mut orchestrator = AutomationOrchestrator::new();
        let spec = AutomationSpec::new(
            "health",
            "check runtime",
            AutomationTrigger::Heartbeat(HeartbeatSpec::new(60)),
            AutomationIntent::DiagnosticSweep {
                profile: "quick".into(),
            },
            AutomationScope::Diagnostic,
            ApprovalPolicy::Never,
        );
        let id = spec.id;

        let events = orchestrator.register(spec).unwrap();
        assert_eq!(
            events,
            vec![AutomationEvent::Registered {
                automation_id: id,
                name: "health".into()
            }]
        );
    }

    #[test]
    fn risky_automation_waits_for_approval() {
        let mut orchestrator = AutomationOrchestrator::new();
        let spec = AutomationSpec::new(
            "mcp trust",
            "",
            AutomationTrigger::Manual,
            AutomationIntent::Maintenance {
                capability: "mcp.servers.set_trust".into(),
                args: Value::Null,
                risk: RiskLevel::PeripheralMaintenance,
            },
            AutomationScope::Diagnostic,
            ApprovalPolicy::AtOrAbove(RiskLevel::LocalWrite),
        );
        let id = spec.id;

        let events = orchestrator.register(spec).unwrap();
        assert_eq!(
            events,
            vec![AutomationEvent::ApprovalRequired {
                automation_id: id,
                risk: RiskLevel::PeripheralMaintenance
            }]
        );

        let approved = orchestrator.approve(id).unwrap();
        assert!(matches!(
            approved.as_slice(),
            [AutomationEvent::Registered { automation_id, .. }] if *automation_id == id
        ));
    }

    #[test]
    fn due_heartbeat_emits_triggered_intent() {
        let mut orchestrator = AutomationOrchestrator::new();
        let spec = default_diagnostic_automation();
        let id = spec.id;
        orchestrator.register(spec).unwrap();

        let events = orchestrator.tick(Utc::now());
        assert!(events.iter().any(|event| matches!(
            event,
            AutomationEvent::Triggered {
                automation_id,
                intent: AutomationIntent::DiagnosticSweep { .. },
                ..
            } if *automation_id == id
        )));
    }

    #[test]
    fn swarm_consult_plan_stays_as_consult_intent() {
        let plan = SwarmConsultPlan::new(
            "general",
            "security",
            "Review this planned dependency change.",
            "needs security review",
        );

        assert_eq!(
            plan.to_intent(),
            AutomationIntent::ConsultMode {
                target_mode: "security".into(),
                question: "Review this planned dependency change.".into(),
                max_iterations: 1,
            }
        );
    }

    #[test]
    fn cron_is_preserved_as_condition_for_dedicated_engine() {
        let spec = AutomationSpec::new(
            "weekly",
            "",
            AutomationTrigger::Cron(CronSpec::new("0 8 * * MON", "America/New_York")),
            AutomationIntent::DiagnosticSweep {
                profile: "weekly".into(),
            },
            AutomationScope::Diagnostic,
            ApprovalPolicy::Never,
        );
        let job = job_from_spec(&spec);
        assert!(matches!(
            job.trigger,
            JobTrigger::Condition { predicate } if predicate == "cron:0 8 * * MON@America/New_York"
        ));
    }

    #[test]
    fn coding_automation_maps_to_guarded_agent_task() {
        let spec = AutomationSpec::new(
            "coding",
            "check warnings",
            AutomationTrigger::Manual,
            AutomationIntent::CodingAutomation {
                workspace_path: "C:/project".into(),
                mode: "vibe_coding".into(),
                goal: "Run checks and propose warning fixes.".into(),
                max_subagents: 2,
                write_policy: ordo_automation_primitives::CodingWritePolicy::ProposeDiff,
                commit_policy: ordo_automation_primitives::CodingCommitPolicy::NeverCommit,
                dependency_policy:
                    ordo_automation_primitives::CodingDependencyPolicy::NoDependencyChanges,
                risk: RiskLevel::LocalWrite,
            },
            AutomationScope::Workspace {
                path: "C:/project".into(),
            },
            ApprovalPolicy::AtOrAbove(RiskLevel::LocalWrite),
        );

        let tasks = tasks_from_spec(&spec);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].capability, "assistant.coding_automation");
        assert_eq!(tasks[0].args["workspace_path"], "C:/project");
        assert_eq!(
            tasks[0].args["guardrails"]["core_rust_or_tauri_mutation"],
            "denied"
        );
        assert!(tasks[0].requires_approval);
    }

    #[test]
    fn automation_store_round_trips_specs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("ordo-automation-{stamp}.json"));
        let spec = AutomationSpec::new(
            "persisted",
            "durable automation",
            AutomationTrigger::Heartbeat(HeartbeatSpec::new(300)),
            AutomationIntent::DiagnosticSweep {
                profile: "quick".into(),
            },
            AutomationScope::Diagnostic,
            ApprovalPolicy::Never,
        );
        let id = spec.id;

        let orchestrator = AutomationOrchestrator::from_specs(vec![spec]).unwrap();
        orchestrator.save_to_path(&path).unwrap();

        let restored = AutomationOrchestrator::load_or_seed(&path, vec![]).unwrap();
        assert!(restored.get(id).is_some());

        let _ = std::fs::remove_file(path);
    }
}
