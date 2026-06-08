pub mod peer;
pub mod store;
pub use peer::{BuildPlannerPeer, BuildPlannerPeerError};
pub use store::{BuildLedgerStore, BuildLedgerStoreError, BuildLedgerTask};

use chrono::{DateTime, Utc};
use ordo_protocol::{
    BuildErrorClass, BuildGateEvidence, BuildGateResult, BuildPlannerEvent, BuildStep, GateOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildRunStatus {
    Active,
    Halted,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildLedger {
    pub build_id: Uuid,
    pub project_id: String,
    pub status: BuildRunStatus,
    pub current_step: BuildStep,
    pub autonomous_correction: bool,
    #[serde(default)]
    pub requirements: Option<Value>,
    #[serde(default)]
    pub blueprint_versions: Vec<Value>,
    #[serde(default)]
    pub step_outputs: BTreeMap<BuildStep, Value>,
    #[serde(default)]
    pub deferred_debt: Vec<DeferredDebt>,
    #[serde(default)]
    pub couple_markers: Vec<String>,
    #[serde(default)]
    pub retry_ledger: Vec<RetryRecord>,
    #[serde(default)]
    pub launch_proof: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl BuildLedger {
    pub fn new(project_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            build_id: Uuid::new_v4(),
            project_id: project_id.into(),
            status: BuildRunStatus::Active,
            current_step: BuildStep::Intake,
            autonomous_correction: false,
            requirements: None,
            blueprint_versions: Vec::new(),
            step_outputs: BTreeMap::new(),
            deferred_debt: Vec::new(),
            couple_markers: Vec::new(),
            retry_ledger: Vec::new(),
            launch_proof: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn current_skill(&self) -> &'static str {
        self.current_step.skill_id()
    }

    pub fn projected_slice(&self, request: LedgerSliceRequest) -> LedgerSlice {
        match request {
            LedgerSliceRequest::CurrentStep => LedgerSlice {
                project_id: self.project_id.clone(),
                current_step: self.current_step,
                requirements: self.requirements.clone(),
                blueprint_latest: self.blueprint_versions.last().cloned(),
                deferred_debt: self.deferred_debt.clone(),
                couple_markers: self.couple_markers.clone(),
                launch_proof: None,
            },
            LedgerSliceRequest::DeferredDebt => LedgerSlice {
                project_id: self.project_id.clone(),
                current_step: self.current_step,
                requirements: None,
                blueprint_latest: None,
                deferred_debt: self.deferred_debt.clone(),
                couple_markers: Vec::new(),
                launch_proof: None,
            },
            LedgerSliceRequest::LaunchProof => LedgerSlice {
                project_id: self.project_id.clone(),
                current_step: self.current_step,
                requirements: None,
                blueprint_latest: self.blueprint_versions.last().cloned(),
                deferred_debt: self.deferred_debt.clone(),
                couple_markers: self.couple_markers.clone(),
                launch_proof: self.launch_proof.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredDebt {
    pub step: BuildStep,
    pub reason: String,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryRecord {
    pub step: BuildStep,
    pub error_class: BuildErrorClass,
    pub attempt: u8,
    pub summary: String,
    #[serde(default)]
    pub diff: Option<String>,
    pub result: GateOutcome,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerSliceRequest {
    CurrentStep,
    DeferredDebt,
    LaunchProof,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerSlice {
    pub project_id: String,
    pub current_step: BuildStep,
    #[serde(default)]
    pub requirements: Option<Value>,
    #[serde(default)]
    pub blueprint_latest: Option<Value>,
    #[serde(default)]
    pub deferred_debt: Vec<DeferredDebt>,
    #[serde(default)]
    pub couple_markers: Vec<String>,
    #[serde(default)]
    pub launch_proof: Option<Value>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BuildPlannerError {
    #[error("gate result belongs to build {actual}, expected {expected}")]
    WrongBuild { expected: Uuid, actual: Uuid },
    #[error("gate result project {actual} does not match ledger project {expected}")]
    WrongProject { expected: String, actual: String },
    #[error("gate result step {actual:?} does not match current step {expected:?}")]
    WrongStep {
        expected: BuildStep,
        actual: BuildStep,
    },
    #[error("deferred gate result is only valid for crate coupling")]
    DeferredOutsideCouple,
    #[error("cannot advance while deferred debt remains")]
    DeferredDebtRemaining,
    #[error("cannot advance while COUPLE markers remain")]
    CoupleMarkersRemaining,
    #[error("build is not active")]
    BuildNotActive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildPlannerDecision {
    Advance(BuildPlannerEvent),
    HardHalt(BuildPlannerEvent),
    Deferred(BuildPlannerEvent),
    AutonomousRetryEligible {
        step: BuildStep,
        error_class: BuildErrorClass,
        summary: String,
    },
}

pub struct BuildPlanner {
    ledger: BuildLedger,
}

impl BuildPlanner {
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            ledger: BuildLedger::new(project_id),
        }
    }

    pub fn from_ledger(ledger: BuildLedger) -> Self {
        Self { ledger }
    }

    pub fn ledger(&self) -> &BuildLedger {
        &self.ledger
    }

    pub fn ledger_mut(&mut self) -> &mut BuildLedger {
        &mut self.ledger
    }

    pub fn start_event(&self) -> BuildPlannerEvent {
        BuildPlannerEvent::BuildStarted {
            build_id: self.ledger.build_id,
            project_id: self.ledger.project_id.clone(),
            released_skill: self.ledger.current_skill().to_string(),
        }
    }

    pub fn handle_gate_result(
        &mut self,
        result: BuildGateResult,
    ) -> Result<BuildPlannerDecision, BuildPlannerError> {
        self.validate_result_header(&result)?;
        if self.ledger.status != BuildRunStatus::Active {
            return Err(BuildPlannerError::BuildNotActive);
        }

        match result.outcome {
            GateOutcome::Pass { evidence } => self.handle_pass(result.step, evidence),
            GateOutcome::Fail {
                error_class,
                evidence,
            } => Ok(self.handle_fail(result.step, error_class, evidence)),
            GateOutcome::Deferred { reason, .. } => self.handle_deferred(result.step, reason),
        }
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), BuildLedgerStoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&self.ledger)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, BuildLedgerStoreError> {
        let bytes = std::fs::read(path)?;
        let ledger = serde_json::from_slice(&bytes)?;
        Ok(Self::from_ledger(ledger))
    }

    fn validate_result_header(&self, result: &BuildGateResult) -> Result<(), BuildPlannerError> {
        if result.build_id != self.ledger.build_id {
            return Err(BuildPlannerError::WrongBuild {
                expected: self.ledger.build_id,
                actual: result.build_id,
            });
        }
        if result.project_id != self.ledger.project_id {
            return Err(BuildPlannerError::WrongProject {
                expected: self.ledger.project_id.clone(),
                actual: result.project_id.clone(),
            });
        }
        if result.step != self.ledger.current_step {
            return Err(BuildPlannerError::WrongStep {
                expected: self.ledger.current_step,
                actual: result.step,
            });
        }
        Ok(())
    }

    fn handle_pass(
        &mut self,
        step: BuildStep,
        evidence: BuildGateEvidence,
    ) -> Result<BuildPlannerDecision, BuildPlannerError> {
        if matches!(step, BuildStep::BuildTest | BuildStep::LaunchProof)
            && !self.ledger.deferred_debt.is_empty()
        {
            return Err(BuildPlannerError::DeferredDebtRemaining);
        }
        if matches!(step, BuildStep::CrateCouple | BuildStep::LaunchProof)
            && !self.ledger.couple_markers.is_empty()
        {
            return Err(BuildPlannerError::CoupleMarkersRemaining);
        }

        self.record_step_output(step, evidence.summary.clone());
        let next_step = step.next();
        if let Some(next) = next_step {
            self.ledger.current_step = next;
        } else {
            self.ledger.status = BuildRunStatus::Complete;
        }
        self.ledger.updated_at = Utc::now();

        Ok(BuildPlannerDecision::Advance(
            BuildPlannerEvent::StepAdvanced {
                build_id: self.ledger.build_id,
                project_id: self.ledger.project_id.clone(),
                completed_step: step,
                next_step,
                released_skill: next_step.map(|step| step.skill_id().to_string()),
            },
        ))
    }

    fn handle_fail(
        &mut self,
        step: BuildStep,
        error_class: BuildErrorClass,
        evidence: BuildGateEvidence,
    ) -> BuildPlannerDecision {
        if self.ledger.autonomous_correction && error_class.is_bounded_candidate() {
            return BuildPlannerDecision::AutonomousRetryEligible {
                step,
                error_class,
                summary: evidence.summary,
            };
        }

        self.ledger.status = BuildRunStatus::Halted;
        self.ledger.updated_at = Utc::now();
        BuildPlannerDecision::HardHalt(BuildPlannerEvent::HardHalted {
            build_id: self.ledger.build_id,
            project_id: self.ledger.project_id.clone(),
            step,
            error_class,
            summary: evidence.summary,
        })
    }

    fn handle_deferred(
        &mut self,
        step: BuildStep,
        reason: String,
    ) -> Result<BuildPlannerDecision, BuildPlannerError> {
        if step != BuildStep::CrateCouple {
            return Err(BuildPlannerError::DeferredOutsideCouple);
        }
        self.ledger.deferred_debt.push(DeferredDebt {
            step,
            reason: reason.clone(),
            recorded_at: Utc::now(),
        });
        self.ledger.updated_at = Utc::now();
        Ok(BuildPlannerDecision::Deferred(
            BuildPlannerEvent::DeferredDebtRecorded {
                build_id: self.ledger.build_id,
                project_id: self.ledger.project_id.clone(),
                step,
                reason,
            },
        ))
    }

    fn record_step_output(&mut self, step: BuildStep, summary: String) {
        let value = serde_json::json!({
            "summary": summary,
            "recorded_at": Utc::now(),
            "skill": step.skill_id(),
        });
        self.ledger.step_outputs.insert(step, value);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BuildLedger, BuildPlanner, BuildPlannerDecision, BuildPlannerError, BuildRunStatus,
    };
    use ordo_protocol::{
        BuildErrorClass, BuildGateEvidence, BuildGateResult, BuildStep, GateOutcome,
    };

    fn pass_for(planner: &BuildPlanner, step: BuildStep) -> BuildGateResult {
        BuildGateResult {
            build_id: planner.ledger().build_id,
            project_id: planner.ledger().project_id.clone(),
            step,
            outcome: GateOutcome::Pass {
                evidence: BuildGateEvidence::new(format!("{step:?} passed")),
            },
        }
    }

    #[test]
    fn starts_with_intake_skill() {
        let planner = BuildPlanner::new("demo");
        assert_eq!(planner.ledger().current_step, BuildStep::Intake);
        assert_eq!(planner.ledger().current_skill(), "ordo-build-intake");
    }

    #[test]
    fn pass_advances_one_step_and_releases_next_skill() {
        let mut planner = BuildPlanner::new("demo");
        let decision = planner
            .handle_gate_result(pass_for(&planner, BuildStep::Intake))
            .expect("advance");

        assert_eq!(planner.ledger().current_step, BuildStep::Blueprint);
        match decision {
            BuildPlannerDecision::Advance(event) => {
                let text = serde_json::to_string(&event).expect("event json");
                assert!(text.contains("ordo-build-blueprint"));
            }
            other => panic!("unexpected decision {other:?}"),
        }
    }

    #[test]
    fn wrong_step_is_rejected() {
        let mut planner = BuildPlanner::new("demo");
        let err = planner
            .handle_gate_result(pass_for(&planner, BuildStep::Blueprint))
            .expect_err("wrong step");
        assert!(matches!(err, BuildPlannerError::WrongStep { .. }));
    }

    #[test]
    fn fail_hard_halts_when_autonomy_is_off() {
        let mut planner = BuildPlanner::new("demo");
        let result = BuildGateResult {
            build_id: planner.ledger().build_id,
            project_id: planner.ledger().project_id.clone(),
            step: BuildStep::Intake,
            outcome: GateOutcome::Fail {
                error_class: BuildErrorClass::CompileWarnings,
                evidence: BuildGateEvidence::new("warning found"),
            },
        };

        let decision = planner.handle_gate_result(result).expect("halt");
        assert!(matches!(decision, BuildPlannerDecision::HardHalt(_)));
        assert_eq!(planner.ledger().status, BuildRunStatus::Halted);
    }

    #[test]
    fn bounded_fail_can_route_to_autonomous_retry_when_enabled() {
        let mut planner = BuildPlanner::new("demo");
        planner.ledger_mut().autonomous_correction = true;
        let result = BuildGateResult {
            build_id: planner.ledger().build_id,
            project_id: planner.ledger().project_id.clone(),
            step: BuildStep::Intake,
            outcome: GateOutcome::Fail {
                error_class: BuildErrorClass::CompileWarnings,
                evidence: BuildGateEvidence::new("warning found"),
            },
        };

        let decision = planner.handle_gate_result(result).expect("retry eligible");
        assert!(matches!(
            decision,
            BuildPlannerDecision::AutonomousRetryEligible { .. }
        ));
        assert_eq!(planner.ledger().status, BuildRunStatus::Active);
    }

    #[test]
    fn deferred_is_only_valid_for_couple_step() {
        let mut ledger = BuildLedger::new("demo");
        ledger.current_step = BuildStep::CrateCouple;
        let mut planner = BuildPlanner::from_ledger(ledger);
        let result = BuildGateResult {
            build_id: planner.ledger().build_id,
            project_id: planner.ledger().project_id.clone(),
            step: BuildStep::CrateCouple,
            outcome: GateOutcome::Deferred {
                reason: "external system not available".into(),
                evidence: BuildGateEvidence::new("deferred"),
            },
        };

        let decision = planner.handle_gate_result(result).expect("deferred");
        assert!(matches!(decision, BuildPlannerDecision::Deferred(_)));
        assert_eq!(planner.ledger().deferred_debt.len(), 1);
    }
}
