//! Policy engine — turns a set of findings into a single `Verdict`.
//!
//! Policies are intentionally simple: a verdict per severity, plus
//! per-rule overrides, plus per-plugin overrides. The most restrictive
//! verdict from any single finding wins.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::classifier::{Finding, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    /// Permit the call; record the audit event.
    Warn,
    /// Block the call and return a failure to the caller.
    Block,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Warn => "warn",
            Verdict::Block => "block",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Verdict::Allow => 0,
            Verdict::Warn => 1,
            Verdict::Block => 2,
        }
    }

    fn escalate(self, other: Verdict) -> Verdict {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
}

/// Per-plugin overrides. An entry like
/// `{ "rule_verdicts": { "pii.email": "allow" } }` says "for this
/// plugin, emails are fine".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginPolicy {
    #[serde(default)]
    pub rule_verdicts: HashMap<String, Verdict>,
    #[serde(default)]
    pub muted_rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default = "default_severity_verdicts")]
    pub severity_verdicts: HashMap<Severity, Verdict>,
    #[serde(default)]
    pub rule_verdicts: HashMap<String, Verdict>,
    #[serde(default)]
    pub plugins: HashMap<String, PluginPolicy>,
}

fn default_severity_verdicts() -> HashMap<Severity, Verdict> {
    let mut map = HashMap::new();
    map.insert(Severity::Info, Verdict::Allow);
    map.insert(Severity::Warn, Verdict::Warn);
    map.insert(Severity::Error, Verdict::Block);
    map
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            severity_verdicts: default_severity_verdicts(),
            rule_verdicts: HashMap::new(),
            plugins: HashMap::new(),
        }
    }
}

impl PolicyConfig {
    /// Resolve a single finding to its effective verdict given this
    /// policy. Resolution order:
    ///   1. Plugin-specific rule_verdict override
    ///   2. Plugin-specific mute (treat as allow)
    ///   3. Global rule_verdict override
    ///   4. Severity default
    pub fn verdict_for(&self, plugin: &str, finding: &Finding) -> Verdict {
        if let Some(plugin_policy) = self.plugins.get(plugin) {
            if plugin_policy
                .muted_rules
                .iter()
                .any(|r| r == &finding.rule_id)
            {
                return Verdict::Allow;
            }
            if let Some(verdict) = plugin_policy.rule_verdicts.get(&finding.rule_id) {
                return *verdict;
            }
        }
        if let Some(verdict) = self.rule_verdicts.get(&finding.rule_id) {
            return *verdict;
        }
        self.severity_verdicts
            .get(&finding.severity)
            .copied()
            .unwrap_or(Verdict::Warn)
    }
}

/// Aggregated decision for a batch of findings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub verdict: Verdict,
    pub findings: Vec<FindingDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingDecision {
    pub finding: Finding,
    pub verdict: Verdict,
}

impl PolicyDecision {
    pub fn from_findings(policy: &PolicyConfig, plugin: &str, findings: Vec<Finding>) -> Self {
        let mut overall = Verdict::Allow;
        let mut resolved = Vec::with_capacity(findings.len());
        for finding in findings {
            let verdict = policy.verdict_for(plugin, &finding);
            overall = overall.escalate(verdict);
            resolved.push(FindingDecision { finding, verdict });
        }
        Self {
            verdict: overall,
            findings: resolved,
        }
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self.verdict, Verdict::Block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::{Finding, FindingLocation, Severity};

    fn make_finding(rule: &str, severity: Severity) -> Finding {
        Finding {
            rule_id: rule.into(),
            severity,
            message: "test".into(),
            match_preview: "***".into(),
            location: FindingLocation {
                pointer: "/".into(),
            },
        }
    }

    #[test]
    fn error_severity_blocks_by_default() {
        let policy = PolicyConfig::default();
        let decision = PolicyDecision::from_findings(
            &policy,
            "any",
            vec![make_finding("secret.openai_key", Severity::Error)],
        );
        assert_eq!(decision.verdict, Verdict::Block);
        assert!(decision.is_blocked());
    }

    #[test]
    fn plugin_mute_downgrades_to_allow() {
        let mut policy = PolicyConfig::default();
        let mut plugin = PluginPolicy::default();
        plugin.muted_rules.push("pii.email".into());
        policy.plugins.insert("trusted".into(), plugin);

        let decision = PolicyDecision::from_findings(
            &policy,
            "trusted",
            vec![make_finding("pii.email", Severity::Info)],
        );
        assert_eq!(decision.verdict, Verdict::Allow);
    }

    #[test]
    fn overall_verdict_is_most_restrictive() {
        let policy = PolicyConfig::default();
        let decision = PolicyDecision::from_findings(
            &policy,
            "any",
            vec![
                make_finding("pii.email", Severity::Info),
                make_finding("secret.aws_access_key", Severity::Error),
                make_finding("prompt.injection", Severity::Warn),
            ],
        );
        assert_eq!(decision.verdict, Verdict::Block);
        assert_eq!(decision.findings.len(), 3);
    }

    #[test]
    fn global_rule_override_beats_severity_default() {
        let mut policy = PolicyConfig::default();
        policy
            .rule_verdicts
            .insert("secret.generic_bearer".into(), Verdict::Block);
        let decision = PolicyDecision::from_findings(
            &policy,
            "anywhere",
            vec![make_finding("secret.generic_bearer", Severity::Warn)],
        );
        assert_eq!(decision.verdict, Verdict::Block);
    }
}
