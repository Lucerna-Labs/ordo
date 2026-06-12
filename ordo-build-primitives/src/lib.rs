use chrono::Utc;
use ordo_protocol::{BuildErrorClass, BuildGateEvidence, BuildStep, GateOutcome};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildArtifactSnapshot {
    pub path: String,
    pub content: String,
}

impl BuildArtifactSnapshot {
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateInput {
    pub step: BuildStep,
    #[serde(default)]
    pub artifacts: Vec<BuildArtifactSnapshot>,
    #[serde(default)]
    pub command_output: String,
    pub command_success: bool,
    #[serde(default)]
    pub screenshot_captured: bool,
    #[serde(default)]
    pub ui_round_trip_observed: bool,
}

impl GateInput {
    pub fn new(step: BuildStep) -> Self {
        Self {
            step,
            artifacts: Vec::new(),
            command_output: String::new(),
            command_success: true,
            screenshot_captured: false,
            ui_round_trip_observed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatePolicy {
    pub allow_couple_markers: bool,
    pub require_compile_clean: bool,
    pub require_launch_round_trip: bool,
}

impl GatePolicy {
    pub fn for_step(step: BuildStep) -> Self {
        Self {
            allow_couple_markers: matches!(step, BuildStep::CrateBuild),
            require_compile_clean: matches!(
                step,
                BuildStep::CrateBuild
                    | BuildStep::CrateCouple
                    | BuildStep::BuildTest
                    | BuildStep::LaunchProof
            ),
            require_launch_round_trip: matches!(step, BuildStep::LaunchProof),
        }
    }
}

pub fn evaluate_gate(input: &GateInput) -> GateOutcome {
    evaluate_gate_with_policy(input, &GatePolicy::for_step(input.step))
}

pub fn evaluate_gate_with_policy(input: &GateInput, policy: &GatePolicy) -> GateOutcome {
    if let Some((path, marker)) = first_stub_marker(&input.artifacts) {
        return fail(
            BuildErrorClass::StubDetected,
            format!("stub marker `{marker}` found in {path}"),
        );
    }

    if let Some((path, marker)) = first_architecture_violation(&input.artifacts) {
        return fail(
            BuildErrorClass::ArchitecturalViolation,
            format!("forbidden architecture marker `{marker}` found in {path}"),
        );
    }

    if !policy.allow_couple_markers {
        if let Some(path) = first_couple_marker(&input.artifacts) {
            return fail(
                BuildErrorClass::CoupleDebt,
                format!("COUPLE marker remains in {path}"),
            );
        }
    }

    if policy.require_compile_clean {
        if !input.command_success || command_output_has_error(&input.command_output) {
            return fail(
                BuildErrorClass::CompileErrors,
                "compile/test command reported errors",
            );
        }
        if command_output_has_warning(&input.command_output) {
            return fail(
                BuildErrorClass::CompileWarnings,
                "compile/test command emitted warnings",
            );
        }
    }

    if policy.require_launch_round_trip
        && (!input.screenshot_captured || !input.ui_round_trip_observed)
    {
        return fail(
            BuildErrorClass::LaunchProofMissing,
            "launch proof requires both a screenshot and a UiInput round trip",
        );
    }

    GateOutcome::Pass {
        evidence: evidence(format!("{} gate passed", input.step.skill_id())),
    }
}

fn fail(error_class: BuildErrorClass, summary: impl Into<String>) -> GateOutcome {
    GateOutcome::Fail {
        error_class,
        evidence: evidence(summary),
    }
}

fn evidence(summary: impl Into<String>) -> BuildGateEvidence {
    BuildGateEvidence {
        summary: summary.into(),
        details: Vec::new(),
        artifacts: Vec::new(),
        checked_at: Utc::now(),
    }
}

fn first_stub_marker(artifacts: &[BuildArtifactSnapshot]) -> Option<(String, &'static str)> {
    const MARKERS: [&str; 5] = [
        "todo!",
        "unimplemented!",
        "panic!(\"TODO",
        "placeholder",
        "stub",
    ];
    first_marker(artifacts, &MARKERS)
}

fn first_architecture_violation(
    artifacts: &[BuildArtifactSnapshot],
) -> Option<(String, &'static str)> {
    const MARKERS: [&str; 8] = [
        "std::process::Command",
        "process::Command",
        "tauri",
        "webview",
        "wry",
        "TcpListener",
        "axum::Server",
        "[[bin]]",
    ];
    first_marker(artifacts, &MARKERS)
}

fn first_couple_marker(artifacts: &[BuildArtifactSnapshot]) -> Option<String> {
    artifacts
        .iter()
        .find(|artifact| artifact.content.contains("COUPLE:"))
        .map(|artifact| artifact.path.clone())
}

fn first_marker(
    artifacts: &[BuildArtifactSnapshot],
    markers: &[&'static str],
) -> Option<(String, &'static str)> {
    artifacts.iter().find_map(|artifact| {
        markers
            .iter()
            .copied()
            .find(|marker| artifact.content.contains(marker))
            .map(|marker| (artifact.path.clone(), marker))
    })
}

fn command_output_has_error(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    lowered.contains("error:")
        || lowered.contains("could not compile")
        || lowered.contains("failed")
}

fn command_output_has_warning(output: &str) -> bool {
    output.to_ascii_lowercase().contains("warning:")
}

#[cfg(test)]
mod tests {
    use super::{evaluate_gate, BuildArtifactSnapshot, GateInput};
    use ordo_protocol::{BuildErrorClass, BuildStep, GateOutcome};

    fn failure_class(outcome: GateOutcome) -> BuildErrorClass {
        match outcome {
            GateOutcome::Fail { error_class, .. } => error_class,
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[test]
    fn rejects_stubs_before_compile_success_matters() {
        let mut input = GateInput::new(BuildStep::CrateBuild);
        input
            .artifacts
            .push(BuildArtifactSnapshot::new("src/lib.rs", "todo!()"));

        assert_eq!(
            failure_class(evaluate_gate(&input)),
            BuildErrorClass::StubDetected
        );
    }

    #[test]
    fn allows_couple_marker_during_crate_build_only() {
        let mut input = GateInput::new(BuildStep::CrateBuild);
        input.artifacts.push(BuildArtifactSnapshot::new(
            "src/lib.rs",
            "// COUPLE: wire later",
        ));
        assert!(evaluate_gate(&input).is_pass());

        input.step = BuildStep::CrateCouple;
        assert_eq!(
            failure_class(evaluate_gate(&input)),
            BuildErrorClass::CoupleDebt
        );
    }

    #[test]
    fn warnings_are_gate_failures() {
        let mut input = GateInput::new(BuildStep::BuildTest);
        input.command_output = "warning: unused import".into();

        assert_eq!(
            failure_class(evaluate_gate(&input)),
            BuildErrorClass::CompileWarnings
        );
    }

    #[test]
    fn launch_proof_requires_round_trip() {
        let mut input = GateInput::new(BuildStep::LaunchProof);
        input.screenshot_captured = true;

        assert_eq!(
            failure_class(evaluate_gate(&input)),
            BuildErrorClass::LaunchProofMissing
        );

        input.ui_round_trip_observed = true;
        assert!(evaluate_gate(&input).is_pass());
    }
}
