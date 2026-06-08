//! DRIFT plan + decision types.
//!
//! The three-stage flow lives in [`crate::BrokerService`]; this
//! module just carries the associated data.

use ordo_protocol::{CapabilityCanary, CapabilityHandle};

/// What a planner hands back after `BrokerService::plan`. The
/// planner injects `canary.canary_token` into the tool's prompt
/// context and flips `canary.injected_into_context = true` before
/// the tool runs. The `handle.id` is what the tool sees in place
/// of the real secret.
#[derive(Debug, Clone)]
pub struct DriftPlan {
    pub handle: CapabilityHandle,
    pub canary: CapabilityCanary,
}

/// Result of DRIFT validation. Today only `Proceed` is possible
/// because failures surface as `BrokerError::DriftDetected`; the
/// enum exists so future lattice states (e.g. "proceed with
/// reduced budget") can slot in without a breaking signature
/// change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftDecision {
    Proceed,
}

/// Context object passed to DRIFT callers. Today it's a simple
/// newtype over the capability id; the shape is here so the
/// caller signature stays stable if richer context is needed
/// later (per-request deadlines, etc).
#[derive(Debug, Clone)]
pub struct DriftContext {
    pub capability_id: String,
}

impl DriftContext {
    pub fn new(capability_id: impl Into<String>) -> Self {
        Self {
            capability_id: capability_id.into(),
        }
    }
}
