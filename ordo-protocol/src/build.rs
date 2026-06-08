use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub mod build_topics {
    pub const STEP_COMPLETED: &str = "ordo.build.step.completed";
    pub const GATE_RESULT: &str = "ordo.build.gate.result";
    pub const PLANNER_EVENT: &str = "ordo.build.planner.event";
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum BuildStep {
    Intake,
    Blueprint,
    CrateBuild,
    CrateCouple,
    BuildTest,
    LaunchProof,
}

impl BuildStep {
    pub fn ordinal(self) -> u8 {
        match self {
            Self::Intake => 1,
            Self::Blueprint => 2,
            Self::CrateBuild => 3,
            Self::CrateCouple => 4,
            Self::BuildTest => 5,
            Self::LaunchProof => 6,
        }
    }

    pub fn skill_id(self) -> &'static str {
        match self {
            Self::Intake => "ordo-build-intake",
            Self::Blueprint => "ordo-build-blueprint",
            Self::CrateBuild => "ordo-crate-build",
            Self::CrateCouple => "ordo-crate-couple",
            Self::BuildTest => "ordo-build-test",
            Self::LaunchProof => "ordo-launch-proof",
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::Intake => Some(Self::Blueprint),
            Self::Blueprint => Some(Self::CrateBuild),
            Self::CrateBuild => Some(Self::CrateCouple),
            Self::CrateCouple => Some(Self::BuildTest),
            Self::BuildTest => Some(Self::LaunchProof),
            Self::LaunchProof => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildArtifactRef {
    pub path: String,
    pub sha256_hex: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuildStepCompletedSignal {
    pub build_id: Uuid,
    pub project_id: String,
    pub step: BuildStep,
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<BuildArtifactRef>,
    #[serde(default)]
    pub output: Value,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildGateEvidence {
    pub summary: String,
    #[serde(default)]
    pub details: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<BuildArtifactRef>,
    pub checked_at: DateTime<Utc>,
}

impl BuildGateEvidence {
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            details: Vec::new(),
            artifacts: Vec::new(),
            checked_at: Utc::now(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BuildErrorClass {
    BoundedMechanical,
    BlueprintAmendment,
    CompileErrors,
    CompileWarnings,
    ArchitecturalViolation,
    StubDetected,
    CoupleDebt,
    LaunchProofMissing,
    RuntimePanic,
    UnboundedOwnership,
    RetryExhausted,
    Unknown,
}

impl BuildErrorClass {
    pub fn is_bounded_candidate(self) -> bool {
        matches!(
            self,
            Self::BoundedMechanical | Self::BlueprintAmendment | Self::CompileWarnings
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GateOutcome {
    Pass {
        evidence: BuildGateEvidence,
    },
    Fail {
        error_class: BuildErrorClass,
        evidence: BuildGateEvidence,
    },
    Deferred {
        reason: String,
        evidence: BuildGateEvidence,
    },
}

impl GateOutcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildGateResult {
    pub build_id: Uuid,
    pub project_id: String,
    pub step: BuildStep,
    pub outcome: GateOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BuildPlannerEvent {
    BuildStarted {
        build_id: Uuid,
        project_id: String,
        released_skill: String,
    },
    StepAdvanced {
        build_id: Uuid,
        project_id: String,
        completed_step: BuildStep,
        next_step: Option<BuildStep>,
        released_skill: Option<String>,
    },
    DeferredDebtRecorded {
        build_id: Uuid,
        project_id: String,
        step: BuildStep,
        reason: String,
    },
    HardHalted {
        build_id: Uuid,
        project_id: String,
        step: BuildStep,
        error_class: BuildErrorClass,
        summary: String,
    },
    AutonomousRetryRequested {
        build_id: Uuid,
        project_id: String,
        step: BuildStep,
        error_class: BuildErrorClass,
        summary: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{BuildErrorClass, BuildStep};

    #[test]
    fn build_steps_release_expected_skills() {
        assert_eq!(BuildStep::Intake.skill_id(), "ordo-build-intake");
        assert_eq!(BuildStep::CrateCouple.next(), Some(BuildStep::BuildTest));
        assert_eq!(BuildStep::LaunchProof.next(), None);
    }

    #[test]
    fn bounded_axis_is_not_compiler_vs_architecture() {
        assert!(BuildErrorClass::CompileWarnings.is_bounded_candidate());
        assert!(!BuildErrorClass::CompileErrors.is_bounded_candidate());
        assert!(!BuildErrorClass::ArchitecturalViolation.is_bounded_candidate());
    }
}
