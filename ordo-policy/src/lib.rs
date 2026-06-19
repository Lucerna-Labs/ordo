// ordo-policy — Policy gate for risk scoring, permissions, and approval requirements.
//
// Sits between agents and tools, agents and memory writes, agents and external actions.
// Every action flowing through the orchestration layer passes through the policy gate
// for risk evaluation. Actions above the autonomous-safe threshold require approval.

use chrono::{DateTime, Utc};
use ordo_agents::RiskLevel;
use ordo_tasks::{AgentId, TaskId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub type ApprovalId = Uuid;

// ─── Proposed Action ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedAction {
    pub agent_id: AgentId,
    pub task_id: Option<TaskId>,
    pub capability: String,
    pub arguments: serde_json::Value,
    pub risk_level: RiskLevel,
    pub requires_approval: bool,
}

impl ProposedAction {
    pub fn new(agent_id: AgentId, capability: &str, risk_level: RiskLevel) -> Self {
        Self {
            agent_id,
            task_id: None,
            capability: capability.into(),
            arguments: serde_json::Value::Null,
            risk_level,
            requires_approval: risk_level.requires_approval(),
        }
    }

    pub fn for_task(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    pub fn with_args(mut self, args: serde_json::Value) -> Self {
        self.arguments = args;
        self
    }
}

// ─── Policy Decision ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyDecision {
    /// Action is safe, proceed.
    Allow,
    /// Action is blocked entirely.
    Block { reason: String },
    /// Action requires human approval before proceeding.
    RequiresApproval,
    /// Action is allowed but will be logged at elevated scrutiny.
    AllowWithCaution { reason: String },
}

impl PolicyDecision {
    pub fn allowed(&self) -> bool {
        matches!(
            self,
            PolicyDecision::Allow | PolicyDecision::AllowWithCaution { .. }
        )
    }
}

// ─── Approval Request ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub requested_by: AgentId,
    pub action: ProposedAction,
    pub risk_level: RiskLevel,
    pub summary: String,
    pub diff: Option<String>,
    pub approve_options: Vec<ApprovalOption>,
    pub status: ApprovalStatus,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl ApprovalRequest {
    pub fn new(agent_id: AgentId, action: ProposedAction, summary: String) -> Self {
        Self {
            id: ApprovalId::new_v4(),
            requested_by: agent_id,
            risk_level: action.risk_level,
            action,
            summary,
            diff: None,
            approve_options: vec![ApprovalOption::ApproveOnce, ApprovalOption::Reject],
            status: ApprovalStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        }
    }

    pub fn with_options(mut self, options: Vec<ApprovalOption>) -> Self {
        self.approve_options = options;
        self
    }

    pub fn with_diff(mut self, diff: String) -> Self {
        self.diff = Some(diff);
        self
    }

    pub fn resolve(&mut self, status: ApprovalStatus) {
        self.status = status;
        self.resolved_at = Some(Utc::now());
    }
}

// ─── Approval Options ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalOption {
    ApproveOnce,
    ApproveForJobType,
    ApproveForProject,
    Reject,
    EditThenApprove,
    EscalateForReview,
}

impl ApprovalOption {
    pub fn label(&self) -> &str {
        match self {
            ApprovalOption::ApproveOnce => "Approve Once",
            ApprovalOption::ApproveForJobType => "Approve for this job type",
            ApprovalOption::ApproveForProject => "Approve for this project",
            ApprovalOption::Reject => "Reject",
            ApprovalOption::EditThenApprove => "Edit then approve",
            ApprovalOption::EscalateForReview => "Escalate for review",
        }
    }
}

// ─── Approval Status ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Edited,
    Escalated,
    Expired,
}

impl ApprovalStatus {
    pub fn resolved(&self) -> bool {
        matches!(
            self,
            ApprovalStatus::Approved
                | ApprovalStatus::Rejected
                | ApprovalStatus::Edited
                | ApprovalStatus::Expired
        )
    }
}

// ─── Action Classification ─────────────────────────────────────────────────────

/// Maps capability names to risk levels for the policy engine.
#[derive(Debug, Clone)]
pub struct ActionClassifier {
    risk_map: HashMap<String, RiskLevel>,
}

impl ActionClassifier {
    pub fn new() -> Self {
        let mut risk_map = HashMap::new();

        // Level 0-1: Read-only
        for cap in &[
            "knowledge.summarize",
            "research.fetch",
            "rag.query",
            "filesystem.read_file",
            "memory.list_pinned",
            "memory.list_working",
            "classify_intent",
            "web.search",
        ] {
            risk_map.insert(cap.to_string(), RiskLevel::ReadOnly);
        }

        // Level 2: Local file write
        for cap in &[
            "filesystem.write_file",
            "memory.pin_note",
            "memory.remember_note",
            "rag.reindex",
        ] {
            risk_map.insert(cap.to_string(), RiskLevel::LocalFileWrite);
        }

        // Level 3: External read
        for cap in &[
            "web.fetch",
            "api.request",
            "cloud.openai.chat",
            "cloud.anthropic.messages",
        ] {
            risk_map.insert(cap.to_string(), RiskLevel::ExternalRead);
        }

        // Level 4: External write
        for cap in &["ssh.run_remote_command", "api.post", "api.put"] {
            risk_map.insert(cap.to_string(), RiskLevel::ExternalWrite);
        }

        // Level 6: Publishing
        for cap in &["apps.publish", "apps.unarchive"] {
            risk_map.insert(cap.to_string(), RiskLevel::Publishing);
        }

        // Level 8: Destructive
        for cap in &[
            "shell.execute",
            "filesystem.delete_file",
            "memory.unpin_note",
        ] {
            risk_map.insert(cap.to_string(), RiskLevel::Destructive);
        }

        Self { risk_map }
    }

    pub fn classify(&self, capability: &str) -> RiskLevel {
        self.risk_map
            .get(capability)
            .copied()
            .unwrap_or(RiskLevel::ReadOnly) // Default: safest
    }

    pub fn classify_action(&self, capability: &str, args: &serde_json::Value) -> RiskLevel {
        // Some capabilities change risk level based on arguments
        let base = self.classify(capability);

        if capability == "api.request" {
            if let Some(method) = args.get("method").and_then(|v| v.as_str()) {
                match method.to_uppercase().as_str() {
                    "GET" | "HEAD" => return RiskLevel::ExternalRead,
                    "POST" | "PUT" | "PATCH" => return RiskLevel::ExternalWrite,
                    "DELETE" => return RiskLevel::Destructive,
                    _ => {}
                }
            }
        }

        if capability == "filesystem.write_file" {
            if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                if path.contains("credentials") || path.contains("secrets") {
                    return RiskLevel::AccountAction;
                }
            }
        }

        base
    }
}

// ─── Policy Gate ───────────────────────────────────────────────────────────────

pub struct PolicyGate {
    classifier: ActionClassifier,
    pending_approvals: HashMap<ApprovalId, ApprovalRequest>,
    approved_actions: HashMap<ApprovalId, ApprovalRequest>,
    rejected_actions: HashMap<ApprovalId, ApprovalRequest>,
}

impl PolicyGate {
    pub fn new() -> Self {
        Self {
            classifier: ActionClassifier::new(),
            pending_approvals: HashMap::new(),
            approved_actions: HashMap::new(),
            rejected_actions: HashMap::new(),
        }
    }

    /// Evaluate an action and return a policy decision.
    pub fn evaluate(
        &self,
        _agent_id: AgentId,
        capability: &str,
        args: &serde_json::Value,
    ) -> PolicyDecision {
        let risk = self.classifier.classify_action(capability, args);

        if risk.requires_approval() {
            return PolicyDecision::RequiresApproval;
        }

        if risk == RiskLevel::ExternalWrite {
            return PolicyDecision::AllowWithCaution {
                reason: format!("external write via {capability}"),
            };
        }

        PolicyDecision::Allow
    }

    /// Request human approval for an action.
    pub fn request_approval(
        &mut self,
        agent_id: AgentId,
        capability: &str,
        args: &serde_json::Value,
        summary: String,
    ) -> ApprovalId {
        let risk = self.classifier.classify_action(capability, args);
        let action = ProposedAction::new(agent_id, capability, risk).with_args(args.clone());
        let request = ApprovalRequest::new(agent_id, action, summary);

        let id = request.id;
        self.pending_approvals.insert(id, request);
        id
    }

    /// Approve a pending request.
    pub fn approve(&mut self, id: &ApprovalId) -> Option<&ApprovalRequest> {
        if let Some(req) = self.pending_approvals.remove(id) {
            let mut req = req;
            req.resolve(ApprovalStatus::Approved);
            self.approved_actions.insert(*id, req);
            return self.approved_actions.get(id);
        }
        None
    }

    /// Reject a pending request.
    pub fn reject(&mut self, id: &ApprovalId) -> Option<&ApprovalRequest> {
        if let Some(req) = self.pending_approvals.remove(id) {
            let mut req = req;
            req.resolve(ApprovalStatus::Rejected);
            self.rejected_actions.insert(*id, req);
            return self.rejected_actions.get(id);
        }
        None
    }

    /// Check if an action has been approved.
    pub fn is_approved(&self, agent_id: AgentId, capability: &str) -> bool {
        self.approved_actions
            .values()
            .any(|r| r.requested_by == agent_id && r.action.capability == capability)
    }

    /// List pending approvals.
    pub fn pending(&self) -> Vec<&ApprovalRequest> {
        self.pending_approvals.values().collect()
    }

    /// List all resolved approvals.
    pub fn resolved(&self) -> Vec<&ApprovalRequest> {
        self.approved_actions
            .values()
            .chain(self.rejected_actions.values())
            .collect()
    }

    /// Count pending approvals.
    pub fn pending_count(&self) -> usize {
        self.pending_approvals.len()
    }

    /// Classify a capability's risk level (for display / audit).
    pub fn classify(&self, capability: &str) -> RiskLevel {
        self.classifier.classify(capability)
    }
}

impl Default for PolicyGate {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_capability_passes() {
        let gate = PolicyGate::new();
        let decision = gate.evaluate(
            Uuid::new_v4(),
            "filesystem.read_file",
            &serde_json::json!({"path": "/tmp/test.txt"}),
        );
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn publish_requires_approval() {
        let gate = PolicyGate::new();
        let decision = gate.evaluate(Uuid::new_v4(), "apps.publish", &serde_json::json!({}));
        assert_eq!(decision, PolicyDecision::RequiresApproval);
    }

    #[test]
    fn destructive_is_blocked() {
        let gate = PolicyGate::new();
        let decision = gate.evaluate(
            Uuid::new_v4(),
            "filesystem.delete_file",
            &serde_json::json!({"path": "/tmp/important.txt"}),
        );
        assert_eq!(decision, PolicyDecision::RequiresApproval);
    }

    #[test]
    fn approval_lifecycle() {
        let mut gate = PolicyGate::new();
        let agent_id = Uuid::new_v4();

        let id = gate.request_approval(
            agent_id,
            "apps.publish",
            &serde_json::json!({}),
            "Publish app update".into(),
        );

        assert_eq!(gate.pending_count(), 1);

        let approved = gate.approve(&id);
        assert!(approved.is_some());
        assert_eq!(gate.pending_count(), 0);
    }

    #[test]
    fn reject_pending_approval() {
        let mut gate = PolicyGate::new();
        let agent_id = Uuid::new_v4();

        let id = gate.request_approval(agent_id, "apps.publish", &serde_json::json!({}), "".into());

        let rejected = gate.reject(&id);
        assert!(rejected.is_some());
        assert_eq!(gate.pending_count(), 0);
    }

    #[test]
    fn action_classifier_sensitive_paths() {
        let classifier = ActionClassifier::new();
        let risk = classifier.classify_action(
            "filesystem.write_file",
            &serde_json::json!({"path": "/home/user/.credentials/secrets.json"}),
        );
        assert_eq!(risk, RiskLevel::AccountAction);
    }

    #[test]
    fn api_method_differentiation() {
        let classifier = ActionClassifier::new();

        let get_risk = classifier.classify_action(
            "api.request",
            &serde_json::json!({"method": "GET", "url": "https://example.com"}),
        );
        assert_eq!(get_risk, RiskLevel::ExternalRead);

        let delete_risk = classifier.classify_action(
            "api.request",
            &serde_json::json!({"method": "DELETE", "url": "https://example.com/data"}),
        );
        assert_eq!(delete_risk, RiskLevel::Destructive);
    }
}
