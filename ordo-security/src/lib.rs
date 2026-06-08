//! Security layer for Ordo.
//!
//! Plugins run as out-of-process MCP subprocesses, which means the
//! host has limited insight into their behavior. This crate closes
//! part of that gap: every tool call into a gated provider has its
//! arguments scanned before execution and its result scanned after,
//! against a set of pluggable `Classifier`s. A `PolicyEngine` decides
//! whether each finding is informational, worth warning about, or
//! worth blocking the call outright. Every decision lands in a
//! bounded `AuditLog` the operator can inspect through the dashboard
//! or the CLI.
//!
//! The first wave of classifiers is regex-based and deliberately
//! boring â€” secret-key shapes, PEM blocks, Authorization headers,
//! classic prompt-injection phrases, filesystem-escape patterns,
//! oversize payloads. ML/LLM-backed classifiers can be added later
//! simply by implementing the `Classifier` trait.

pub mod audit;
pub mod classifier;
pub mod gated;
pub mod pipeline;
pub mod policy;
pub mod rules;

pub use audit::{AuditEvent, AuditLog, SharedAuditLog};
pub use classifier::{Classifier, Finding, FindingLocation, Phase, ScanInput, Severity};
pub use gated::SecurityGatedProvider;
pub use pipeline::{Pipeline, RuleDescriptor};
pub use policy::{FindingDecision, PluginPolicy, PolicyConfig, PolicyDecision, Verdict};
pub use rules::default_classifiers;

use std::sync::Arc;

/// Convenience: build the standard pipeline + default policy + a
/// fresh audit log. Most callers want exactly this combination.
pub fn default_stack(audit_capacity: usize) -> SecurityStack {
    SecurityStack {
        pipeline: Pipeline::new(default_classifiers()),
        policy: Arc::new(PolicyConfig::default()),
        audit: Arc::new(AuditLog::new(audit_capacity)),
    }
}

#[derive(Clone)]
pub struct SecurityStack {
    pub pipeline: Pipeline,
    pub policy: Arc<PolicyConfig>,
    pub audit: SharedAuditLog,
}

impl SecurityStack {
    /// Wrap an inner provider with this stack's configuration.
    pub fn gate(
        &self,
        inner: Arc<dyn ordo_mcp_host::CapabilityProvider>,
        scope: impl Into<String>,
    ) -> SecurityGatedProvider {
        SecurityGatedProvider::new(
            inner,
            self.pipeline.clone(),
            self.policy.clone(),
            self.audit.clone(),
            scope,
        )
    }
}
