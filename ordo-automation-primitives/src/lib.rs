use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

pub type AutomationId = Uuid;
pub type AutomationRunId = Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationSpec {
    pub id: AutomationId,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub trigger: AutomationTrigger,
    pub intent: AutomationIntent,
    pub scope: AutomationScope,
    pub approval: ApprovalPolicy,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: BTreeMap<String, String>,
}

impl AutomationSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        trigger: AutomationTrigger,
        intent: AutomationIntent,
        scope: AutomationScope,
        approval: ApprovalPolicy,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: AutomationId::new_v4(),
            name: name.into(),
            description: description.into(),
            enabled: true,
            trigger,
            intent,
            scope,
            approval,
            created_at: now,
            updated_at: now,
            metadata: BTreeMap::new(),
        }
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self.updated_at = Utc::now();
        self
    }

    pub fn risk(&self) -> RiskLevel {
        self.intent.risk()
    }

    pub fn requires_approval(&self) -> bool {
        self.approval.requires_approval_for(self.risk())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutomationTrigger {
    Manual,
    At(DateTime<Utc>),
    IntervalSeconds(u64),
    Heartbeat(HeartbeatSpec),
    Cron(CronSpec),
    Event { topic: String },
    Webhook { path: String },
    LocalSignal { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatSpec {
    pub every_seconds: u64,
    pub jitter_seconds: u64,
    pub resume_thread: Option<String>,
}

impl HeartbeatSpec {
    pub fn new(every_seconds: u64) -> Self {
        Self {
            every_seconds,
            jitter_seconds: 0,
            resume_thread: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronSpec {
    pub expression: String,
    pub timezone: String,
}

impl CronSpec {
    pub fn new(expression: impl Into<String>, timezone: impl Into<String>) -> Self {
        Self {
            expression: expression.into(),
            timezone: timezone.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutomationIntent {
    RunCapability {
        capability: String,
        args: Value,
        risk: RiskLevel,
    },
    ConsultMode {
        target_mode: String,
        question: String,
        max_iterations: u8,
    },
    SpawnSubagent {
        mode: String,
        goal: String,
        max_iterations: u8,
        risk: RiskLevel,
    },
    DreamingReview {
        mode: String,
        signal_window: String,
    },
    DiagnosticSweep {
        profile: String,
    },
    CodingAutomation {
        workspace_path: String,
        mode: String,
        goal: String,
        max_subagents: u8,
        write_policy: CodingWritePolicy,
        commit_policy: CodingCommitPolicy,
        dependency_policy: CodingDependencyPolicy,
        risk: RiskLevel,
    },
    Maintenance {
        capability: String,
        args: Value,
        risk: RiskLevel,
    },
    Composite {
        steps: Vec<AutomationIntent>,
    },
}

impl AutomationIntent {
    pub fn risk(&self) -> RiskLevel {
        match self {
            Self::RunCapability { risk, .. } => *risk,
            Self::ConsultMode { .. } => RiskLevel::SafeReadOnly,
            Self::SpawnSubagent { risk, .. } => *risk,
            Self::DreamingReview { .. } => RiskLevel::LocalRead,
            Self::DiagnosticSweep { .. } => RiskLevel::SafeReadOnly,
            Self::CodingAutomation { risk, .. } => *risk,
            Self::Maintenance { risk, .. } => *risk,
            Self::Composite { steps } => steps
                .iter()
                .map(Self::risk)
                .max()
                .unwrap_or(RiskLevel::SafeReadOnly),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodingWritePolicy {
    InspectOnly,
    ProposeDiff,
    EditWithApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodingCommitPolicy {
    NeverCommit,
    CommitWithApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodingDependencyPolicy {
    NoDependencyChanges,
    ProposeDependencyChanges,
    InstallWithApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutomationScope {
    Global,
    Workspace { path: String },
    Mode { mode: String },
    Diagnostic,
    Device { device_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    SafeReadOnly = 0,
    LocalRead = 1,
    LocalWrite = 2,
    NetworkRead = 3,
    NetworkWrite = 4,
    PeripheralMaintenance = 5,
    CoreMutationDenied = 6,
}

impl RiskLevel {
    pub fn is_autonomous_safe(self) -> bool {
        self <= Self::NetworkRead
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalPolicy {
    Never,
    Always,
    AtOrAbove(RiskLevel),
    ManualOnly,
}

impl ApprovalPolicy {
    pub fn requires_approval_for(self, risk: RiskLevel) -> bool {
        match self {
            Self::Never => false,
            Self::Always | Self::ManualOnly => true,
            Self::AtOrAbove(threshold) => risk >= threshold,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutomationStatus {
    Enabled,
    Disabled,
    WaitingForApproval,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationRunRecord {
    pub id: AutomationRunId,
    pub automation_id: AutomationId,
    pub status: AutomationStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub output: Option<Value>,
    pub error: Option<String>,
}

impl AutomationRunRecord {
    pub fn started(automation_id: AutomationId) -> Self {
        Self {
            id: AutomationRunId::new_v4(),
            automation_id,
            status: AutomationStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            output: None,
            error: None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AutomationValidationError {
    #[error("automation name is required")]
    MissingName,
    #[error("automation trigger interval must be greater than zero")]
    EmptyInterval,
    #[error("cron expression is required")]
    EmptyCron,
    #[error("event topic is required")]
    EmptyEventTopic,
    #[error("webhook path is required")]
    EmptyWebhookPath,
    #[error("local signal name is required")]
    EmptyLocalSignal,
    #[error("capability name is required")]
    EmptyCapability,
    #[error("target mode is required")]
    EmptyTargetMode,
    #[error("subagent goal is required")]
    EmptySubagentGoal,
    #[error("coding automation workspace path is required")]
    EmptyWorkspacePath,
    #[error("coding automation goal is required")]
    EmptyCodingGoal,
    #[error("coding automation cannot request autonomous core mutation")]
    CodingCoreMutationDenied,
    #[error("core mutation is not an automation action")]
    CoreMutationDenied,
}

pub fn validate_automation(spec: &AutomationSpec) -> Result<(), AutomationValidationError> {
    if spec.name.trim().is_empty() {
        return Err(AutomationValidationError::MissingName);
    }

    validate_trigger(&spec.trigger)?;
    validate_intent(&spec.intent)
}

fn validate_trigger(trigger: &AutomationTrigger) -> Result<(), AutomationValidationError> {
    match trigger {
        AutomationTrigger::Manual | AutomationTrigger::At(_) => Ok(()),
        AutomationTrigger::IntervalSeconds(seconds) if *seconds > 0 => Ok(()),
        AutomationTrigger::IntervalSeconds(_) => Err(AutomationValidationError::EmptyInterval),
        AutomationTrigger::Heartbeat(spec) if spec.every_seconds > 0 => Ok(()),
        AutomationTrigger::Heartbeat(_) => Err(AutomationValidationError::EmptyInterval),
        AutomationTrigger::Cron(spec) if !spec.expression.trim().is_empty() => Ok(()),
        AutomationTrigger::Cron(_) => Err(AutomationValidationError::EmptyCron),
        AutomationTrigger::Event { topic } if !topic.trim().is_empty() => Ok(()),
        AutomationTrigger::Event { .. } => Err(AutomationValidationError::EmptyEventTopic),
        AutomationTrigger::Webhook { path } if !path.trim().is_empty() => Ok(()),
        AutomationTrigger::Webhook { .. } => Err(AutomationValidationError::EmptyWebhookPath),
        AutomationTrigger::LocalSignal { name } if !name.trim().is_empty() => Ok(()),
        AutomationTrigger::LocalSignal { .. } => Err(AutomationValidationError::EmptyLocalSignal),
    }
}

fn validate_intent(intent: &AutomationIntent) -> Result<(), AutomationValidationError> {
    match intent {
        AutomationIntent::RunCapability {
            capability, risk, ..
        }
        | AutomationIntent::Maintenance {
            capability, risk, ..
        } => {
            if capability.trim().is_empty() {
                return Err(AutomationValidationError::EmptyCapability);
            }
            if *risk == RiskLevel::CoreMutationDenied {
                return Err(AutomationValidationError::CoreMutationDenied);
            }
            Ok(())
        }
        AutomationIntent::ConsultMode { target_mode, .. } => {
            if target_mode.trim().is_empty() {
                Err(AutomationValidationError::EmptyTargetMode)
            } else {
                Ok(())
            }
        }
        AutomationIntent::SpawnSubagent {
            mode, goal, risk, ..
        } => {
            if mode.trim().is_empty() {
                return Err(AutomationValidationError::EmptyTargetMode);
            }
            if goal.trim().is_empty() {
                return Err(AutomationValidationError::EmptySubagentGoal);
            }
            if *risk == RiskLevel::CoreMutationDenied {
                return Err(AutomationValidationError::CoreMutationDenied);
            }
            Ok(())
        }
        AutomationIntent::DreamingReview { mode, .. } => {
            if mode.trim().is_empty() {
                Err(AutomationValidationError::EmptyTargetMode)
            } else {
                Ok(())
            }
        }
        AutomationIntent::DiagnosticSweep { .. } => Ok(()),
        AutomationIntent::CodingAutomation {
            workspace_path,
            mode,
            goal,
            max_subagents,
            write_policy,
            commit_policy,
            dependency_policy,
            risk,
        } => {
            if workspace_path.trim().is_empty() {
                return Err(AutomationValidationError::EmptyWorkspacePath);
            }
            if mode.trim().is_empty() {
                return Err(AutomationValidationError::EmptyTargetMode);
            }
            if goal.trim().is_empty() {
                return Err(AutomationValidationError::EmptyCodingGoal);
            }
            if *max_subagents == 0 {
                return Err(AutomationValidationError::EmptySubagentGoal);
            }
            if *risk == RiskLevel::CoreMutationDenied
                || *write_policy != CodingWritePolicy::InspectOnly && *risk < RiskLevel::LocalWrite
                || *commit_policy == CodingCommitPolicy::CommitWithApproval
                    && *risk < RiskLevel::LocalWrite
                || *dependency_policy == CodingDependencyPolicy::InstallWithApproval
                    && *risk < RiskLevel::LocalWrite
            {
                return Err(AutomationValidationError::CodingCoreMutationDenied);
            }
            Ok(())
        }
        AutomationIntent::Composite { steps } => {
            for step in steps {
                validate_intent(step)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_policy_gates_risk() {
        let policy = ApprovalPolicy::AtOrAbove(RiskLevel::LocalWrite);
        assert!(!policy.requires_approval_for(RiskLevel::LocalRead));
        assert!(policy.requires_approval_for(RiskLevel::LocalWrite));
    }

    #[test]
    fn composite_uses_highest_risk() {
        let intent = AutomationIntent::Composite {
            steps: vec![
                AutomationIntent::DiagnosticSweep {
                    profile: "quick".into(),
                },
                AutomationIntent::Maintenance {
                    capability: "mcp.servers.set_trust".into(),
                    args: Value::Null,
                    risk: RiskLevel::PeripheralMaintenance,
                },
            ],
        };
        assert_eq!(intent.risk(), RiskLevel::PeripheralMaintenance);
    }

    #[test]
    fn validation_rejects_core_mutation() {
        let spec = AutomationSpec::new(
            "bad",
            "",
            AutomationTrigger::Manual,
            AutomationIntent::RunCapability {
                capability: "runtime.patch_core".into(),
                args: Value::Null,
                risk: RiskLevel::CoreMutationDenied,
            },
            AutomationScope::Global,
            ApprovalPolicy::Always,
        );
        assert_eq!(
            validate_automation(&spec),
            Err(AutomationValidationError::CoreMutationDenied)
        );
    }

    #[test]
    fn coding_automation_requires_workspace_and_goal() {
        let spec = AutomationSpec::new(
            "coding",
            "",
            AutomationTrigger::Manual,
            AutomationIntent::CodingAutomation {
                workspace_path: "".into(),
                mode: "vibe_coding".into(),
                goal: "check warnings".into(),
                max_subagents: 1,
                write_policy: CodingWritePolicy::InspectOnly,
                commit_policy: CodingCommitPolicy::NeverCommit,
                dependency_policy: CodingDependencyPolicy::NoDependencyChanges,
                risk: RiskLevel::LocalRead,
            },
            AutomationScope::Workspace {
                path: "C:/project".into(),
            },
            ApprovalPolicy::Always,
        );
        assert_eq!(
            validate_automation(&spec),
            Err(AutomationValidationError::EmptyWorkspacePath)
        );
    }

    #[test]
    fn coding_automation_propose_diff_is_local_write_risk() {
        let spec = AutomationSpec::new(
            "coding",
            "",
            AutomationTrigger::Manual,
            AutomationIntent::CodingAutomation {
                workspace_path: "C:/project".into(),
                mode: "vibe_coding".into(),
                goal: "fix warnings".into(),
                max_subagents: 2,
                write_policy: CodingWritePolicy::ProposeDiff,
                commit_policy: CodingCommitPolicy::NeverCommit,
                dependency_policy: CodingDependencyPolicy::NoDependencyChanges,
                risk: RiskLevel::LocalWrite,
            },
            AutomationScope::Workspace {
                path: "C:/project".into(),
            },
            ApprovalPolicy::AtOrAbove(RiskLevel::LocalWrite),
        );
        assert!(validate_automation(&spec).is_ok());
        assert_eq!(spec.risk(), RiskLevel::LocalWrite);
    }
}
