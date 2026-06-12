// ordo-agents — Agent registry, profiles, tool allowance, memory scopes.
//
// This crate defines agent profiles that describe what each agent is allowed
// to do. The registry is the single source of truth for agent capabilities.
// The dispatcher and policy gate query it at runtime to enforce constraints.

use ordo_tasks::{AgentId, TaskType};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ─── Agent Profile ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: AgentId,
    pub name: String,
    pub description: String,
    pub allowed_tools: HashSet<String>,
    pub memory_scopes: HashSet<MemoryScope>,
    pub output_schema: serde_json::Value,
    pub risk_level: RiskLevel,
}

impl AgentProfile {
    pub fn new(name: &str, description: &str, risk_level: RiskLevel) -> Self {
        Self {
            id: AgentId::new_v4(),
            name: name.into(),
            description: description.into(),
            allowed_tools: HashSet::new(),
            memory_scopes: HashSet::new(),
            output_schema: serde_json::Value::Null,
            risk_level,
        }
    }

    /// Scope this agent to specific tools.
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_tools = tools.into_iter().map(|t| t.into()).collect();
        self
    }

    /// Scope this agent to specific memory scopes.
    pub fn with_memory_scopes(mut self, scopes: impl IntoIterator<Item = MemoryScope>) -> Self {
        self.memory_scopes = scopes.into_iter().collect();
        self
    }

    /// Define the expected output shape for this agent.
    pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
        self.output_schema = schema;
        self
    }

    pub fn may_use(&self, tool: &str) -> bool {
        self.allowed_tools.contains(tool)
    }

    pub fn may_read_scope(&self, scope: &MemoryScope) -> bool {
        self.memory_scopes.contains(scope)
    }
}

// ─── Memory Scopes ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryScope {
    GlobalUserPreferences,
    ProjectContext,
    CodingContext,
    ResearchContext,
    OperationsContext,
    AutomationHistory,
    DiagnosticMemory,
    SecurityRules,
    TemporaryTaskMemory,
    PrivateCredentialsMetadata,
}

impl MemoryScope {
    pub fn label(&self) -> &str {
        match self {
            MemoryScope::GlobalUserPreferences => "Global User Preferences",
            MemoryScope::ProjectContext => "Project Context",
            MemoryScope::CodingContext => "Coding Context",
            MemoryScope::ResearchContext => "Research Context",
            MemoryScope::OperationsContext => "Operations Context",
            MemoryScope::AutomationHistory => "Automation History",
            MemoryScope::DiagnosticMemory => "Diagnostic Memory",
            MemoryScope::SecurityRules => "Security Rules",
            MemoryScope::TemporaryTaskMemory => "Temporary Task Memory",
            MemoryScope::PrivateCredentialsMetadata => "Private Credentials Metadata",
        }
    }
}

// ─── Risk Level ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    /// Level 0: Read-only, safe
    ReadOnly = 0,
    /// Level 1: Local file read
    LocalFileRead = 1,
    /// Level 2: Local file write
    LocalFileWrite = 2,
    /// Level 3: External network read
    ExternalRead = 3,
    /// Level 4: External network write
    ExternalWrite = 4,
    /// Level 5: Account action
    AccountAction = 5,
    /// Level 6: privileged account or system action
    PrivilegedAction = 6,
    /// Level 7: Payment/financial action
    Financial = 7,
    /// Level 8: Destructive action
    Destructive = 8,
}

impl RiskLevel {
    pub fn label(&self) -> &str {
        match self {
            RiskLevel::ReadOnly => "Read-Only",
            RiskLevel::LocalFileRead => "Local File Read",
            RiskLevel::LocalFileWrite => "Local File Write",
            RiskLevel::ExternalRead => "External Network Read",
            RiskLevel::ExternalWrite => "External Network Write",
            RiskLevel::AccountAction => "Account Action",
            RiskLevel::PrivilegedAction => "Privileged Action",
            RiskLevel::Financial => "Financial",
            RiskLevel::Destructive => "Destructive",
        }
    }

    /// Whether this risk level requires human approval.
    pub fn requires_approval(&self) -> bool {
        matches!(
            self,
            RiskLevel::AccountAction
                | RiskLevel::PrivilegedAction
                | RiskLevel::Financial
                | RiskLevel::Destructive
        )
    }

    /// Whether this risk level is safe for autonomous operation.
    pub fn is_autonomous_safe(&self) -> bool {
        *self <= RiskLevel::ExternalWrite
    }
}

// ─── Agent Registry ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct AgentRegistry {
    agents: HashMap<AgentId, AgentProfile>,
    /// Maps TaskType to the preferred agent.
    type_routing: HashMap<TaskType, AgentId>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            type_routing: HashMap::new(),
        }
    }

    pub fn register(&mut self, profile: AgentProfile) -> AgentId {
        let id = profile.id;
        self.agents.insert(id, profile);
        id
    }

    pub fn register_with_routing(
        &mut self,
        profile: AgentProfile,
        routes: impl IntoIterator<Item = TaskType>,
    ) -> AgentId {
        let id = self.register(profile);
        for task_type in routes {
            self.type_routing.insert(task_type, id);
        }
        id
    }

    pub fn get(&self, id: &AgentId) -> Option<&AgentProfile> {
        self.agents.get(id)
    }

    pub fn list(&self) -> Vec<&AgentProfile> {
        self.agents.values().collect()
    }

    pub fn find_for_task_type(&self, task_type: &TaskType) -> Option<&AgentProfile> {
        self.type_routing
            .get(task_type)
            .and_then(|id| self.agents.get(id))
    }

    /// Find all agents that can handle this task type.
    pub fn candidates_for(&self, task_type: &TaskType) -> Vec<&AgentProfile> {
        self.agents
            .values()
            .filter(|a| {
                let label_lower;
                let task_str = match task_type {
                    TaskType::Capability(c) => c.as_str(),
                    TaskType::Custom(c) => c.as_str(),
                    _ => {
                        label_lower = task_type.label().to_lowercase();
                        label_lower.as_str()
                    }
                };
                a.allowed_tools.iter().any(|tool| tool.contains(task_str))
                    || a.name.to_lowercase().contains(task_str)
            })
            .collect()
    }

    /// Create the default agent set for Ordo.
    pub fn default_registry() -> Self {
        let mut registry = Self::new();

        // General Agent — understands intent, talks to user
        let general = AgentProfile::new(
            "General Agent",
            "Talks to users, understands intent, delegates to specialists.",
            RiskLevel::ReadOnly,
        )
        .with_tools(["chat", "classify_intent", "delegate_task"])
        .with_memory_scopes([
            MemoryScope::GlobalUserPreferences,
            MemoryScope::ProjectContext,
        ]);
        let _general_id = registry.register(general);

        // Planner Agent — breaks goals into plans
        let planner = AgentProfile::new(
            "Planner Agent",
            "Breaks user goals into structured task plans.",
            RiskLevel::ReadOnly,
        )
        .with_tools(["plan.goal", "plan.validate", "plan.decompose"])
        .with_memory_scopes([
            MemoryScope::GlobalUserPreferences,
            MemoryScope::ProjectContext,
        ])
        .with_output_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string" },
                "tasks": { "type": "array" }
            }
        }));
        let _planner_id = registry.register(planner);

        // Research Agent — finds and summarizes information
        let research = AgentProfile::new(
            "Research Agent",
            "Finds and summarizes information from web, knowledge bases, and files.",
            RiskLevel::ExternalRead,
        )
        .with_tools([
            "research.fetch",
            "web.search",
            "web.fetch",
            "filesystem.read_file",
            "rag.query",
            "knowledge.summarize",
        ])
        .with_memory_scopes([
            MemoryScope::ProjectContext,
            MemoryScope::TemporaryTaskMemory,
        ]);
        let _research_id = registry.register_with_routing(research, [TaskType::Research]);

        // Analyst Agent — compares evidence and produces structured conclusions
        let analyst = AgentProfile::new(
            "Analyst Agent",
            "Compares sources, checks assumptions, and produces structured analysis.",
            RiskLevel::LocalFileWrite,
        )
        .with_tools([
            "cloud.openai.chat",
            "cloud.anthropic.messages",
            "knowledge.analyze",
        ])
        .with_memory_scopes([
            MemoryScope::GlobalUserPreferences,
            MemoryScope::ProjectContext,
            MemoryScope::ResearchContext,
        ])
        .with_output_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "claims": { "type": "array" },
                "confidence": { "type": "number" }
            }
        }));
        let _analyst_id = registry.register_with_routing(analyst, [TaskType::Analysis]);

        // Operations Agent — inspects platform state and peripheral services
        let operations = AgentProfile::new(
            "Operations Agent",
            "Inspects runtime state, providers, MCP servers, plugins, and local infrastructure.",
            RiskLevel::ReadOnly,
        )
        .with_tools([
            "runtime.describe_profile",
            "runtime.describe_settings",
            "mcp.servers.list",
            "plugins.list",
            "skills.list",
        ])
        .with_memory_scopes([MemoryScope::ProjectContext, MemoryScope::OperationsContext]);
        let _operations_id = registry.register_with_routing(operations, [TaskType::Operations]);

        // Automation Agent — designs visible, bounded autonomous work
        let automation = AgentProfile::new(
            "Automation Agent",
            "Designs and reviews cron jobs, heartbeats, routines, webhooks, and local event jobs.",
            RiskLevel::LocalFileWrite,
        )
        .with_tools([
            "jobs.describe",
            "jobs.validate",
            "jobs.plan",
            "review.request",
        ])
        .with_memory_scopes([
            MemoryScope::AutomationHistory,
            MemoryScope::ProjectContext,
            MemoryScope::SecurityRules,
        ]);
        let _automation_id = registry.register_with_routing(automation, [TaskType::Automation]);

        // Diagnostic Agent — local-only self inspection
        let diagnostic = AgentProfile::new(
            "Diagnostic Agent",
            "Runs local-only self checks and recommends repairs without touching core code.",
            RiskLevel::LocalFileWrite,
        )
        .with_tools([
            "runtime.describe_profile",
            "runtime.describe_settings",
            "runtime.describe_storage",
            "mcp.servers.list",
            "skills.list",
            "plugins.list",
            "assistant.remember_fact",
        ])
        .with_memory_scopes([
            MemoryScope::DiagnosticMemory,
            MemoryScope::OperationsContext,
            MemoryScope::SecurityRules,
        ]);
        let _diagnostic_id = registry.register_with_routing(diagnostic, [TaskType::Diagnostics]);

        // Security Agent — reviews risky actions
        let security = AgentProfile::new(
            "Security Agent",
            "Reviews risky tool calls and policy violations.",
            RiskLevel::ReadOnly,
        )
        .with_tools(["policy.evaluate", "security.audit", "security.review"])
        .with_memory_scopes([
            MemoryScope::SecurityRules,
            MemoryScope::PrivateCredentialsMetadata,
        ]);
        let _security_id = registry.register_with_routing(security, [TaskType::SecurityReview]);

        // Memory Agent — decides what to store
        let memory = AgentProfile::new(
            "Memory Agent",
            "Decides what should be stored in memory and what can be forgotten.",
            RiskLevel::LocalFileWrite,
        )
        .with_tools([
            "memory.pin_note",
            "memory.unpin_note",
            "memory.remember_note",
            "memory.list_working",
        ])
        .with_memory_scopes([
            MemoryScope::TemporaryTaskMemory,
            MemoryScope::GlobalUserPreferences,
        ]);
        let _memory_id = registry.register_with_routing(memory, [TaskType::MemoryUpdate]);

        // Coding Agent — code tasks
        let coding = AgentProfile::new(
            "Coding Agent",
            "Works on code generation and modification tasks.",
            RiskLevel::LocalFileWrite,
        )
        .with_tools([
            "filesystem.read_file",
            "filesystem.write_file",
            "code.generate",
            "code.review",
            "shell.execute",
        ])
        .with_memory_scopes([MemoryScope::CodingContext, MemoryScope::ProjectContext]);
        let _coding_id = registry.register_with_routing(coding, [TaskType::Code]);

        // Critic Agent — quality check
        let critic = AgentProfile::new(
            "Critic Agent",
            "Checks quality, contradictions, and factual accuracy.",
            RiskLevel::ReadOnly,
        )
        .with_tools([
            "cloud.openai.chat",
            "cloud.anthropic.messages",
            "logic.evaluate",
        ])
        .with_memory_scopes([MemoryScope::ProjectContext, MemoryScope::SecurityRules]);
        let _critic_id = registry.register(critic);

        registry
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_tasks::TaskType;

    #[test]
    fn agent_tool_allowance() {
        let agent = AgentProfile::new("test", "test desc", RiskLevel::ReadOnly)
            .with_tools(["filesystem.read_file", "rag.query"]);

        assert!(agent.may_use("filesystem.read_file"));
        assert!(!agent.may_use("filesystem.write_file"));
    }

    #[test]
    fn agent_memory_scopes() {
        let agent = AgentProfile::new("test", "test desc", RiskLevel::ReadOnly)
            .with_memory_scopes([MemoryScope::ResearchContext, MemoryScope::ProjectContext]);

        assert!(agent.may_read_scope(&MemoryScope::ResearchContext));
        assert!(!agent.may_read_scope(&MemoryScope::SecurityRules));
    }

    #[test]
    fn risk_approval_baseline() {
        assert!(!RiskLevel::ReadOnly.requires_approval());
        assert!(!RiskLevel::ExternalWrite.requires_approval());
        assert!(RiskLevel::PrivilegedAction.requires_approval());
        assert!(RiskLevel::Financial.requires_approval());
        assert!(RiskLevel::Destructive.requires_approval());
    }

    #[test]
    fn autonomous_safety() {
        assert!(RiskLevel::ReadOnly.is_autonomous_safe());
        assert!(RiskLevel::LocalFileWrite.is_autonomous_safe());
        assert!(!RiskLevel::Financial.is_autonomous_safe());
    }

    #[test]
    fn default_registry_has_all_agents() {
        let registry = AgentRegistry::default_registry();
        let agents = registry.list();

        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"General Agent"));
        assert!(names.contains(&"Planner Agent"));
        assert!(names.contains(&"Research Agent"));
        assert!(names.contains(&"Analyst Agent"));
        assert!(names.contains(&"Operations Agent"));
        assert!(names.contains(&"Automation Agent"));
        assert!(names.contains(&"Diagnostic Agent"));
        assert!(names.contains(&"Security Agent"));
    }

    #[test]
    fn task_type_routing() {
        let registry = AgentRegistry::default_registry();

        let operations_agent = registry.find_for_task_type(&TaskType::Operations);
        assert!(operations_agent.is_some());
        assert_eq!(operations_agent.unwrap().name, "Operations Agent");

        let research_agent = registry.find_for_task_type(&TaskType::Research);
        assert!(research_agent.is_some());
        assert_eq!(research_agent.unwrap().name, "Research Agent");
    }

    #[test]
    fn privileged_actions_require_approval() {
        let registry = AgentRegistry::default_registry();
        let security = registry
            .find_for_task_type(&TaskType::SecurityReview)
            .unwrap();
        assert_eq!(security.risk_level, RiskLevel::ReadOnly);
        assert!(!security.risk_level.requires_approval());
        assert!(RiskLevel::PrivilegedAction.requires_approval());
    }
}
