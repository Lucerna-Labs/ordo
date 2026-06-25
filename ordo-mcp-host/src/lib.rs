pub mod bridged_providers;
pub mod external;
pub mod external_mcp;

mod provider_maintenance;
mod provider_filesystem;
mod provider_knowledge;
mod provider_ordo_ops;
mod provider_interface_ops;
mod provider_cloud_ops;
mod provider_runtime;
mod provider_self_heal;
mod provider_memory;
mod provider_llm;
mod provider_review;
mod provider_assistant;
mod helpers;

// Re-export all providers so downstream crates (`ordo-runtime`, etc.)
// can import them the same way as before the split.
pub use provider_assistant::{AssistantProvider};
pub use provider_cloud_ops::{CloudOpsProvider};
pub use provider_runtime::{RuntimeInfoProvider, RuntimePolicySnapshot};
pub use provider_filesystem::{FilesystemProvider};
pub use provider_interface_ops::{InterfaceOpsProvider};
pub use provider_knowledge::{KnowledgeProvider};
pub use provider_llm::{OrdoLlmProvider};
pub use provider_maintenance::{MaintenanceProvider};
pub use provider_memory::{MemoryToolsProvider};
pub use provider_ordo_ops::{OrdoOpsProvider};
pub use provider_review::{ReviewProvider};
pub use provider_self_heal::{SelfHealToolsProvider};

pub use bridged_providers::{
    AppsCapabilityAdapter, CodeCapabilityAdapter, FilesCapabilityAdapter, LogicCapabilityAdapter,
    StrainerCapabilityAdapter,
};
pub use external::{ExternalMcpError, ExternalMcpProvider, ExternalMcpServerConfig};
pub use external_mcp::{ExternalMcpToolsProvider, McpManagementProvider};

use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use async_trait::async_trait;
use futures::StreamExt;
use ordo_automation::{
    default_diagnostic_automation, default_dreaming_automation, AutomationOrchestrator,
};
use ordo_automation_primitives::AutomationId;
use ordo_bus::Bus;
use ordo_heal::{SelfHealCaseSummary, SelfHealStorageTask};
use ordo_protocol::{
    infer_knowledge_task, topics, CapabilityActivation, CapabilityDescriptor, CapabilityTier,
    CorrelationId, Envelope, ExecutionPlan, KnowledgeTask, MemoryTier, NodeId, NodeStatus,
    OrdoMessage, RagHit, RunStatus, SelfHealIncident, SelfHealUrgency,
};
use ordo_store::{RuntimeSettings, RuntimeSettingsTask, RuntimeSettingsUpdate};
use serde_json::{json, Value};
use tokio::task;
use tokio::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CapabilityMatch {
    pub capability: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ProviderRun {
    pub steps: Vec<ProviderStep>,
}

#[derive(Debug, Clone)]
pub struct ProviderStep {
    pub capability: String,
    pub name: String,
    pub status: ProviderRunStatus,
}

#[derive(Debug, Clone)]
pub enum ProviderRunStatus {
    Completed { output: String },
    Failed { error: String },
}

#[derive(Debug, Clone)]
pub enum ToolCallResult {
    Completed { result: Value },
    Failed { error: String },
}

#[async_trait]
pub trait CapabilityProvider: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Vec<String>;
    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor>;
    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch>;
    async fn handle_run(&self, goal: &str, context: &[RagHit]) -> Option<ProviderRun>;
    async fn handle_tool_call(&self, capability: &str, arguments: &Value)
        -> Option<ToolCallResult>;
}

pub const SKILLS_LIST: &str = "skills.list";
pub const SKILLS_GET: &str = "skills.get";
pub const SKILLS_INSTALL: &str = "skills.install";
pub const SKILLS_DELETE: &str = "skills.delete";
pub const SKILLS_AUDIT_ROUTING: &str = "skills.audit_routing";
pub const SKILLS_REPAIR_ROUTING: &str = "skills.repair_routing";
pub const PLUGINS_LIST: &str = "plugins.list";
pub const PLUGINS_INSTALL: &str = "plugins.install";
pub const PLUGINS_DELETE: &str = "plugins.delete";
pub const PLUGINS_SET_ENABLED: &str = "plugins.set_enabled";
pub const AUTOMATION_LIST: &str = "automation.list";
pub const AUTOMATION_INSPECT: &str = "automation.inspect";
pub const AGENT_TEAMS_LIST: &str = "agent_teams.list";
pub const AGENT_TEAMS_GET: &str = "agent_teams.get";
pub const AGENT_TEAMS_UPSERT: &str = "agent_teams.upsert";
pub const AGENT_TEAMS_DELETE: &str = "agent_teams.delete";
pub const AGENT_TEAMS_SET_ACTIVE: &str = "agent_teams.set_active";
pub const LOGS_SYSTEM_TAIL: &str = "logs.system_tail";


pub struct McpHost {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
    providers: Vec<Arc<dyn CapabilityProvider>>,
}

impl McpHost {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
            providers: Vec::new(),
        }
    }

    pub fn add_provider(&mut self, provider: Arc<dyn CapabilityProvider>) {
        self.providers.push(provider);
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut requirement_sub = self.bus.subscribe(topics::REQUIREMENT).await?;
        let mut capability_inventory_sub = self
            .bus
            .subscribe(topics::CAPABILITY_INVENTORY_REQUEST)
            .await?;
        let mut run_sub = self.bus.subscribe(topics::RUN_REQUEST).await?;
        let mut tool_sub = self.bus.subscribe(topics::TOOL_REQUEST).await?;
        let node_id = self.node_id.clone();
        let bus = self.bus.clone();
        let providers = self.providers.clone();
        let started_at = Instant::now();

        tracing::info!(
            "MCP Host {} online with {} providers",
            node_id.0,
            providers.len()
        );

        let h_bus = bus.clone();
        let h_node_id = node_id.clone();
        let h_providers = providers.clone();
        let version = env!("CARGO_PKG_VERSION").to_string();
        task::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                let status = NodeStatus {
                    id: h_node_id.clone(),
                    name: "mcp-host".to_string(),
                    uptime_secs: started_at.elapsed().as_secs(),
                    version: version.clone(),
                    capabilities: h_providers.iter().flat_map(|p| p.capabilities()).collect(),
                };
                let envelope = Envelope::new(h_node_id.clone(), OrdoMessage::Heartbeat(status));
                let _ = h_bus.publish(topics::HEARTBEAT, envelope).await;
            }
        });

        loop {
            tokio::select! {
                requirement = requirement_sub.next() => {
                    let Some(envelope) = requirement else {
                        break;
                    };
                    let correlation_id = envelope.correlation_id.clone();
                    if let OrdoMessage::RequirementMessage { requirement } = envelope.payload {
                        for provider in &providers {
                            if let Some(response) = provider.handle_requirement(&requirement).await {
                                let response = Envelope::new(
                                    node_id.clone(),
                                    OrdoMessage::CapabilityMessage {
                                        capability: response.capability,
                                        description: response.description,
                                    },
                                );
                                let response = with_correlation(response, correlation_id.clone());
                                let _ = bus.publish(topics::CAPABILITY_RESPONSE, response).await;
                            }
                        }
                    }
                }
                capability_inventory = capability_inventory_sub.next() => {
                    let Some(envelope) = capability_inventory else {
                        break;
                    };
                    let correlation_id = envelope.correlation_id.clone();
                    if let OrdoMessage::CapabilityInventoryRequested = envelope.payload {
                        let mut descriptors = providers
                            .iter()
                            .flat_map(|provider| provider.capability_descriptors())
                            .collect::<Vec<_>>();
                        descriptors.sort_by(|left, right| left.capability.cmp(&right.capability));
                        descriptors.dedup_by(|left, right| left.capability == right.capability);
                        let capabilities = descriptors
                            .iter()
                            .map(|descriptor| descriptor.capability.clone())
                            .collect::<Vec<_>>();
                        let response = Envelope::new(
                            node_id.clone(),
                            OrdoMessage::CapabilityInventorySnapshot {
                                capabilities,
                                descriptors,
                            },
                        );
                        let response = with_correlation(response, correlation_id);
                        let _ = bus
                            .publish(topics::CAPABILITY_INVENTORY_RESPONSE, response)
                            .await;
                    }
                }
                run_request = run_sub.next() => {
                    let Some(envelope) = run_request else {
                        break;
                    };
                    let correlation_id = envelope.correlation_id.clone();
                    if let OrdoMessage::RunRequested {
                        run_id,
                        goal,
                        context,
                        plan,
                    } = envelope.payload {
                        if let Some(plan) = plan {
                            execute_plan(
                                &bus,
                                &node_id,
                                &providers,
                                correlation_id.clone(),
                                run_id,
                                plan,
                            ).await;
                            continue;
                        }
                        let mut matched = false;

                        for provider in &providers {
                            if let Some(provider_run) = provider.handle_run(&goal, &context).await {
                                matched = true;
                                let accepted = Envelope::new(
                                    node_id.clone(),
                                    OrdoMessage::RunAccepted { run_id },
                                );
                                let accepted = with_correlation(accepted, correlation_id.clone());
                                let _ = bus.publish(topics::RUN_EVENT, accepted).await;

                                let mut completed_steps = 0usize;
                                let mut run_status = RunStatus::Succeeded;

                                for step in provider_run.steps {
                                    let step_id = Uuid::new_v4();
                                    let started = Envelope::new(
                                        node_id.clone(),
                                        OrdoMessage::StepStarted {
                                            run_id,
                                            step_id,
                                            name: step.name,
                                        },
                                    );
                                    let started =
                                        with_correlation(started, correlation_id.clone());
                                    let _ = bus.publish(topics::RUN_EVENT, started).await;

                                    completed_steps += 1;
                                    let event = match step.status {
                                        ProviderRunStatus::Completed { output } => Envelope::new(
                                            node_id.clone(),
                                            OrdoMessage::StepCompleted {
                                                run_id,
                                                step_id,
                                                output: format!(
                                                    "{} via {}",
                                                    output, step.capability
                                                ),
                                            },
                                        ),
                                        ProviderRunStatus::Failed { error } => {
                                            run_status = RunStatus::Failed;
                                            Envelope::new(
                                                node_id.clone(),
                                                OrdoMessage::StepFailed {
                                                    run_id,
                                                    step_id,
                                                    error: format!(
                                                        "{} via {}",
                                                        error, step.capability
                                                    ),
                                                },
                                            )
                                        }
                                    };
                                    let event =
                                        with_correlation(event, correlation_id.clone());
                                    let _ = bus.publish(topics::RUN_EVENT, event).await;

                                    if matches!(run_status, RunStatus::Failed) {
                                        break;
                                    }
                                }

                                let finished = Envelope::new(
                                    node_id.clone(),
                                    OrdoMessage::RunFinished {
                                        run_id,
                                        status: run_status,
                                        completed_steps,
                                    },
                                );
                                let finished =
                                    with_correlation(finished, correlation_id.clone());
                                let _ = bus.publish(topics::RUN_EVENT, finished).await;
                                break;
                            }
                        }

                        if !matched {
                            publish_unmatched_run_failure(
                                &bus,
                                &node_id,
                                correlation_id,
                                run_id,
                                &goal,
                            )
                            .await;
                        }
                    }
                }
                tool_request = tool_sub.next() => {
                    let Some(envelope) = tool_request else {
                        break;
                    };
                    let correlation_id = envelope.correlation_id.clone();
                    if let OrdoMessage::ToolCallRequested {
                        invocation_id,
                        capability,
                        arguments,
                    } = envelope.payload {
                        let mut handled = false;

                        for provider in &providers {
                            if let Some(result) =
                                provider.handle_tool_call(&capability, &arguments).await
                            {
                                handled = true;
                                let response = match result {
                                    ToolCallResult::Completed { result } => Envelope::new(
                                        node_id.clone(),
                                        OrdoMessage::ToolCallCompleted {
                                            invocation_id,
                                            capability: capability.clone(),
                                            result,
                                        },
                                    ),
                                    ToolCallResult::Failed { error } => Envelope::new(
                                        node_id.clone(),
                                        OrdoMessage::ToolCallFailed {
                                            invocation_id,
                                            capability: capability.clone(),
                                            error,
                                        },
                                    ),
                                };
                                let response =
                                    with_correlation(response, correlation_id.clone());
                                let _ = bus.publish(topics::TOOL_RESPONSE, response).await;
                                break;
                            }
                        }

                        if !handled {
                            let response = Envelope::new(
                                node_id.clone(),
                                OrdoMessage::ToolCallFailed {
                                    invocation_id,
                                    capability,
                                    error: "no provider handled the requested capability"
                                        .to_string(),
                                },
                            );
                            let response = with_correlation(response, correlation_id);
                            let _ = bus.publish(topics::TOOL_RESPONSE, response).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

async fn publish_unmatched_run_failure(
    bus: &Arc<dyn Bus>,
    node_id: &NodeId,
    correlation_id: Option<CorrelationId>,
    run_id: Uuid,
    goal: &str,
) {
    let accepted = Envelope::new(node_id.clone(), OrdoMessage::RunAccepted { run_id });
    let accepted = with_correlation(accepted, correlation_id.clone());
    let _ = bus.publish(topics::RUN_EVENT, accepted).await;

    let failed_step = Envelope::new(
        node_id.clone(),
        OrdoMessage::StepFailed {
            run_id,
            step_id: Uuid::new_v4(),
            error: format!("no provider accepted run goal '{}'", goal),
        },
    );
    let failed_step = with_correlation(failed_step, correlation_id.clone());
    let _ = bus.publish(topics::RUN_EVENT, failed_step).await;

    let finished = Envelope::new(
        node_id.clone(),
        OrdoMessage::RunFinished {
            run_id,
            status: RunStatus::Failed,
            completed_steps: 0,
        },
    );
    let finished = with_correlation(finished, correlation_id);
    let _ = bus.publish(topics::RUN_EVENT, finished).await;
}

async fn execute_plan(
    bus: &Arc<dyn Bus>,
    node_id: &NodeId,
    providers: &[Arc<dyn CapabilityProvider>],
    correlation_id: Option<CorrelationId>,
    run_id: Uuid,
    plan: ExecutionPlan,
) {
    let accepted = Envelope::new(node_id.clone(), OrdoMessage::RunAccepted { run_id });
    let accepted = with_correlation(accepted, correlation_id.clone());
    let _ = bus.publish(topics::RUN_EVENT, accepted).await;

    let mut completed_steps = 0usize;
    let mut run_status = RunStatus::Succeeded;

    for planned_step in plan.steps {
        let step_id = Uuid::new_v4();
        let started = Envelope::new(
            node_id.clone(),
            OrdoMessage::StepStarted {
                run_id,
                step_id,
                name: planned_step.name.clone(),
            },
        );
        let started = with_correlation(started, correlation_id.clone());
        let _ = bus.publish(topics::RUN_EVENT, started).await;

        let mut handled = false;
        for provider in providers {
            if let Some(result) = provider
                .handle_tool_call(&planned_step.capability, &planned_step.arguments)
                .await
            {
                handled = true;
                completed_steps += 1;
                let event = match result {
                    ToolCallResult::Completed { result } => Envelope::new(
                        node_id.clone(),
                        OrdoMessage::StepCompleted {
                            run_id,
                            step_id,
                            output: format!("{} via {}", result, planned_step.capability),
                        },
                    ),
                    ToolCallResult::Failed { error } => {
                        run_status = RunStatus::Failed;
                        Envelope::new(
                            node_id.clone(),
                            OrdoMessage::StepFailed {
                                run_id,
                                step_id,
                                error: format!("{} via {}", error, planned_step.capability),
                            },
                        )
                    }
                };
                let event = with_correlation(event, correlation_id.clone());
                let _ = bus.publish(topics::RUN_EVENT, event).await;
                break;
            }
        }

        if !handled {
            run_status = RunStatus::Failed;
            let failed = Envelope::new(
                node_id.clone(),
                OrdoMessage::StepFailed {
                    run_id,
                    step_id,
                    error: format!(
                        "no provider handled planned capability '{}'",
                        planned_step.capability
                    ),
                },
            );
            let failed = with_correlation(failed, correlation_id.clone());
            let _ = bus.publish(topics::RUN_EVENT, failed).await;
        }

        if matches!(run_status, RunStatus::Failed) {
            break;
        }
    }

    let finished = Envelope::new(
        node_id.clone(),
        OrdoMessage::RunFinished {
            run_id,
            status: run_status,
            completed_steps,
        },
    );
    let finished = with_correlation(finished, correlation_id);
    let _ = bus.publish(topics::RUN_EVENT, finished).await;
}

fn with_correlation(
    envelope: Envelope<OrdoMessage>,
    correlation_id: Option<CorrelationId>,
) -> Envelope<OrdoMessage> {
    match correlation_id {
        Some(cid) => envelope.with_correlation(cid),
        None => envelope,
    }
}


mod tests {
    use super::{
        publish_unmatched_run_failure, CapabilityProvider, FilesystemProvider, KnowledgeProvider,
        MaintenanceProvider, OrdoOpsProvider, ToolCallResult,
    };
    use futures::StreamExt;
    use ordo_bus::{Bus, InProcessBus};
    use ordo_protocol::{topics, CorrelationId, NodeId, OrdoMessage, RunStatus};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::time::{timeout, Duration};
    use uuid::Uuid;

    #[tokio::test]
    async fn unmatched_run_failure_publishes_accepted_before_failure() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut events = bus
            .subscribe(topics::RUN_EVENT)
            .await
            .expect("subscribe run events");
        let node_id = NodeId::new();
        let correlation_id = Some(CorrelationId::new());
        let run_id = Uuid::new_v4();

        publish_unmatched_run_failure(
            &bus,
            &node_id,
            correlation_id.clone(),
            run_id,
            "missing lane",
        )
        .await;

        let first = timeout(Duration::from_secs(1), events.next())
            .await
            .expect("run accepted timed out")
            .expect("run accepted event");
        let second = timeout(Duration::from_secs(1), events.next())
            .await
            .expect("step failed timed out")
            .expect("step failed event");
        let third = timeout(Duration::from_secs(1), events.next())
            .await
            .expect("run finished timed out")
            .expect("run finished event");

        assert_eq!(first.correlation_id, correlation_id);
        match first.payload {
            OrdoMessage::RunAccepted { run_id: observed } => assert_eq!(observed, run_id),
            other => panic!("expected RunAccepted first, got {other:?}"),
        }
        match second.payload {
            OrdoMessage::StepFailed {
                run_id: observed,
                error,
                ..
            } => {
                assert_eq!(observed, run_id);
                assert!(error.contains("no provider accepted run goal"));
            }
            other => panic!("expected StepFailed second, got {other:?}"),
        }
        match third.payload {
            OrdoMessage::RunFinished {
                run_id: observed,
                status,
                completed_steps,
            } => {
                assert_eq!(observed, run_id);
                assert_eq!(status, RunStatus::Failed);
                assert_eq!(completed_steps, 0);
            }
            other => panic!("expected RunFinished third, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn skills_audit_routing_reports_orphaned_phantom_mode_skill() {
        let base = std::env::temp_dir().join("ordo-mcp-host-audit-ok");
        let _ = std::fs::remove_dir_all(&base);
        let skill_dir = base.join("skills").join("phantom_skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: Phantom\navailable_to_modes: [no_such_mode]\n---\n# x",
        )
        .unwrap();

        let provider = MaintenanceProvider::new(&base, base.join("plugins"))
            .with_modes(ordo_modes::ModeRegistry::from_defaults().unwrap());
        let out = provider
            .handle_tool_call("skills.audit_routing", &json!({}))
            .await;
        match out {
            Some(ToolCallResult::Completed { result }) => {
                let orphaned = result["orphaned"].as_array().unwrap();
                assert!(
                    orphaned.iter().any(|v| v == "phantom_skill"),
                    "phantom_skill should be orphaned: {result}"
                );
                assert!(result["anomaly_count"].as_u64().unwrap() >= 1);
            }
            _ => panic!("expected a completed audit"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn skills_repair_routing_drops_phantom_modes_but_defers_all_phantom_skill() {
        let base = std::env::temp_dir().join("ordo-mcp-host-repair");
        let _ = std::fs::remove_dir_all(&base);
        let skills_dir = base.join("skills");
        // "coding" is a real core mode; the rest are phantom.
        let fixable = skills_dir.join("fixable");
        std::fs::create_dir_all(&fixable).unwrap();
        std::fs::write(
            fixable.join("skill.md"),
            "## Installation Metadata\n\n```yaml\navailable_to_modes:\n  - coding\n  - orchestration\n  - runtime\n```\n",
        )
        .unwrap();
        let only_phantom = skills_dir.join("only_phantom");
        std::fs::create_dir_all(&only_phantom).unwrap();
        std::fs::write(
            only_phantom.join("skill.md"),
            "```yaml\navailable_to_modes:\n  - orchestration\n```\n",
        )
        .unwrap();

        let provider = MaintenanceProvider::new(&base, base.join("plugins"))
            .with_modes(ordo_modes::ModeRegistry::from_defaults().unwrap());

        // dry-run: plans the fix, defers the all-phantom skill, writes nothing.
        let dry = match provider
            .handle_tool_call("skills.repair_routing", &json!({}))
            .await
        {
            Some(ToolCallResult::Completed { result }) => result,
            _ => panic!("expected a completed dry-run"),
        };
        assert_eq!(dry["applied"], json!(false));
        assert_eq!(dry["safe_repairs"].as_array().unwrap().len(), 1);
        assert_eq!(dry["safe_repairs"][0]["skill_id"], "fixable");
        assert_eq!(dry["deferred"].as_array().unwrap().len(), 1);
        assert_eq!(dry["deferred"][0]["skill_id"], "only_phantom");
        // nothing written yet
        assert_eq!(
            ordo_skills::discover_skills(&skills_dir)
                .unwrap()
                .iter()
                .find(|s| s.id == "fixable")
                .unwrap()
                .modes
                .len(),
            3
        );

        // apply: rewrites the fixable skill to keep only the real mode.
        let applied = match provider
            .handle_tool_call("skills.repair_routing", &json!({ "apply": true }))
            .await
        {
            Some(ToolCallResult::Completed { result }) => result,
            _ => panic!("expected a completed apply"),
        };
        assert_eq!(applied["applied"], json!(true));
        let after = ordo_skills::discover_skills(&skills_dir).unwrap();
        assert_eq!(
            after.iter().find(|s| s.id == "fixable").unwrap().modes,
            vec!["coding"]
        );
        // the all-phantom skill is untouched
        assert_eq!(
            after.iter().find(|s| s.id == "only_phantom").unwrap().modes,
            vec!["orchestration"]
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn skills_audit_routing_without_registry_fails_cleanly() {
        let base = std::env::temp_dir().join("ordo-mcp-host-audit-noreg");
        let provider = MaintenanceProvider::new(&base, base.join("plugins")); // no modes
        match provider
            .handle_tool_call("skills.audit_routing", &json!({}))
            .await
        {
            Some(ToolCallResult::Failed { error }) => assert!(error.contains("registry")),
            _ => panic!("expected a clean failure without a registry"),
        }
    }

    async fn call(capability: &str, args: serde_json::Value) -> serde_json::Value {
        let provider = OrdoOpsProvider::new();
        let result = provider
            .handle_tool_call(capability, &args)
            .await
            .expect("ordo-ops capability should handle this call");
        match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("call failed: {error}"),
        }
    }

    // --- Regression: once a provider matches its OWN capability, a missing
    // required argument is a validation failure and must be reported as
    // `Some(ToolCallResult::Failed)`. Returning `None` would make the host's
    // dispatch loop emit the misleading "no provider handled" routing error
    // (the pre-fix bug). A capability the provider does NOT own must still
    // decline with `None`. ---

    #[tokio::test]
    async fn knowledge_missing_goal_fails_validation_instead_of_declining() {
        let provider = KnowledgeProvider;

        let missing = provider
            .handle_tool_call("knowledge.summarize", &json!({}))
            .await;
        assert!(
            missing.is_some(),
            "a missing 'goal' must not decline to None (which reads as 'no provider handled')"
        );
        match missing.expect("some result") {
            ToolCallResult::Failed { error } => {
                assert!(
                    error.contains("goal"),
                    "error should name the field: {error}"
                )
            }
            ToolCallResult::Completed { .. } => {
                panic!("missing 'goal' should fail validation, not complete")
            }
        }

        // With the required argument it completes.
        assert!(matches!(
            provider
                .handle_tool_call("knowledge.summarize", &json!({ "goal": "what is ordo" }))
                .await,
            Some(ToolCallResult::Completed { .. })
        ));

        // A capability it does not own is still declined with `None`.
        assert!(provider
            .handle_tool_call("filesystem.read_file", &json!({ "goal": "x" }))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn filesystem_missing_path_fails_validation_instead_of_declining() {
        let provider = FilesystemProvider::default();

        let missing = provider
            .handle_tool_call("filesystem.read_file", &json!({}))
            .await;
        assert!(
            missing.is_some(),
            "a missing 'path' must not decline to None (which reads as 'no provider handled')"
        );
        match missing.expect("some result") {
            ToolCallResult::Failed { error } => {
                assert!(
                    error.contains("path"),
                    "error should name the field: {error}"
                )
            }
            ToolCallResult::Completed { .. } => {
                panic!("missing 'path' should fail validation, not complete")
            }
        }

        // A capability it does not own is still declined with `None`.
        assert!(provider
            .handle_tool_call("knowledge.summarize", &json!({}))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn plan_initiative_splits_deliverables_into_three_phases() {
        let result = call(
            "planning.plan_initiative",
            json!({ "deliverables": ["a", "b", "c", "d", "e", "f"] }),
        )
        .await;
        let phases = result
            .get("phases")
            .and_then(|value| value.as_array())
            .expect("phases");
        assert_eq!(phases.len(), 3);
    }

    #[tokio::test]
    async fn package_resources_groups_by_kind() {
        let result = call(
            "planning.package_resources",
            json!({
                "resources": [
                    { "path": "a.png" },
                    { "path": "b.mp4" },
                    { "path": "c.jpg" },
                ],
            }),
        )
        .await;
        assert_eq!(
            result.get("count").and_then(|value| value.as_u64()),
            Some(3)
        );
        let by_kind = result
            .get("by_kind")
            .and_then(|value| value.as_object())
            .expect("by_kind");
        assert_eq!(
            by_kind.get("image").and_then(|value| value.as_u64()),
            Some(2)
        );
        assert_eq!(
            by_kind.get("video").and_then(|value| value.as_u64()),
            Some(1)
        );
    }

    #[tokio::test]
    async fn orchestration_advance_stage_moves_forward() {
        let result = call("orchestration.advance_stage", json!({ "stage": "draft" })).await;
        assert_eq!(
            result.get("next_stage").and_then(|value| value.as_str()),
            Some("planning-review")
        );
    }

    #[tokio::test]
    async fn orchestration_route_review_picks_reviewer() {
        let result = call(
            "orchestration.route_review",
            json!({ "stage": "planning-review" }),
        )
        .await;
        assert_eq!(
            result.get("next_reviewer").and_then(|value| value.as_str()),
            Some("editor")
        );
    }
}

#[cfg(test)]
mod real_capability_tests {
    //! Tests for the capabilities that now produce real artifacts (files,
    //! directory walks, structured findings) rather than plain JSON
    //! templates.

    use super::{CapabilityProvider, OrdoOpsProvider, ToolCallResult};
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_user_files() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("ordo-ordo-ops-{stamp}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    async fn call_with_root(
        root: &std::path::Path,
        capability: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let provider = OrdoOpsProvider::new().with_user_files_path(root);
        let result = provider
            .handle_tool_call(capability, &args)
            .await
            .expect("capability handled");
        match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("capability failed: {error}"),
        }
    }

    #[tokio::test]
    async fn package_resources_walks_real_directory() {
        let root = temp_user_files();
        let src = root.join("media");
        std::fs::create_dir_all(src.join("hero")).expect("mkdir hero");
        std::fs::write(src.join("hero").join("banner.png"), b"fake png").expect("write png");
        std::fs::write(src.join("hero").join("response.md"), b"hero response").expect("write md");
        std::fs::write(src.join("voiceover.mp4"), b"fake mp4").expect("write mp4");

        let result = call_with_root(
            &root,
            "planning.package_resources",
            json!({ "input_directory": "media" }),
        )
        .await;

        assert_eq!(result["count"].as_u64(), Some(3));
        let by_kind = result["by_kind"].as_object().expect("by_kind");
        assert_eq!(by_kind.get("image").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(by_kind.get("video").and_then(|v| v.as_u64()), Some(1));
        let manifest = result["manifest"].as_array().expect("manifest");
        assert!(manifest.iter().any(|entry| entry["path"]
            .as_str()
            .map(|p| p.ends_with("banner.png"))
            .unwrap_or(false)));
        // Every entry should report a size from the real metadata walk.
        assert!(manifest
            .iter()
            .all(|entry| entry["size_bytes"].as_u64().is_some()));
        assert!(result["total_bytes"].as_u64().unwrap_or(0) > 0);
        assert!(result["artifact_path"]
            .as_str()
            .map(|p| p.starts_with("resources/") && p.ends_with(".json"))
            .unwrap_or(false));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn package_resources_rejects_path_traversal() {
        let root = temp_user_files();
        let provider = OrdoOpsProvider::new().with_user_files_path(&root);
        let result = provider
            .handle_tool_call(
                "planning.package_resources",
                &json!({ "input_directory": "../outside" }),
            )
            .await
            .expect("handled");
        match result {
            ToolCallResult::Failed { error } => {
                assert!(
                    error.contains("escapes"),
                    "expected sandbox error, got: {error}"
                );
            }
            ToolCallResult::Completed { .. } => panic!("expected sandbox rejection"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod interface_ops_tests {
    use super::{CapabilityProvider, InterfaceOpsProvider, ToolCallResult};
    use serde_json::json;

    async fn call(capability: &str, args: serde_json::Value) -> serde_json::Value {
        let provider = InterfaceOpsProvider::new();
        let result = provider
            .handle_tool_call(capability, &args)
            .await
            .expect("interface-ops capability should handle this call");
        match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("call failed: {error}"),
        }
    }

    #[tokio::test]
    async fn ssh_describe_host_composes_target() {
        let result = call(
            "ssh.describe_host",
            json!({ "host": "build-01.example", "user": "deploy", "port": 2222 }),
        )
        .await;
        assert_eq!(
            result
                .pointer("/ssh_host/target")
                .and_then(|value| value.as_str()),
            Some("deploy@build-01.example:2222")
        );
    }

    #[tokio::test]
    async fn ssh_prepare_command_prefixes_working_dir() {
        let result = call(
            "ssh.prepare_command",
            json!({
                "host": "build-01.example",
                "command": "cargo test",
                "working_dir": "/srv/app",
            }),
        )
        .await;
        assert_eq!(
            result
                .pointer("/ssh_command/composed")
                .and_then(|value| value.as_str()),
            Some("cd /srv/app && cargo test")
        );
    }

    #[tokio::test]
    async fn ssh_sync_workspace_rejects_unknown_direction() {
        let provider = InterfaceOpsProvider::new();
        let result = provider
            .handle_tool_call(
                "ssh.sync_workspace",
                &json!({
                    "host": "h",
                    "local_path": "/a",
                    "remote_path": "/b",
                    "direction": "sideways",
                }),
            )
            .await
            .expect("call");
        assert!(matches!(result, ToolCallResult::Failed { .. }));
    }

    #[tokio::test]
    async fn api_describe_client_reports_scope_count() {
        let result = call(
            "api.describe_client",
            json!({
                "name": "stripe",
                "base_url": "https://api.stripe.com",
                "scopes": ["read", "write"],
            }),
        )
        .await;
        assert_eq!(
            result
                .pointer("/api_client/scope_count")
                .and_then(|value| value.as_u64()),
            Some(2)
        );
    }

    #[tokio::test]
    async fn api_prepare_auth_returns_steps_for_oauth2() {
        let result = call(
            "api.prepare_auth",
            json!({ "client": "stripe", "auth_style": "oauth2" }),
        )
        .await;
        let steps = result
            .pointer("/api_auth/steps")
            .and_then(|value| value.as_array())
            .expect("steps");
        assert_eq!(steps.len(), 4);
    }

    #[tokio::test]
    async fn rest_describe_endpoint_infers_resource() {
        let result = call(
            "rest.describe_endpoint",
            json!({ "method": "get", "path": "/v1/articles" }),
        )
        .await;
        assert_eq!(
            result
                .pointer("/rest_endpoint/method")
                .and_then(|value| value.as_str()),
            Some("GET")
        );
        assert_eq!(
            result
                .pointer("/rest_endpoint/resource")
                .and_then(|value| value.as_str()),
            Some("articles")
        );
    }

    #[tokio::test]
    async fn rest_prepare_request_strips_body_for_get() {
        let result = call(
            "rest.prepare_request",
            json!({ "method": "GET", "path": "/v1/articles", "body": { "a": 1 } }),
        )
        .await;
        assert_eq!(
            result
                .pointer("/rest_request/has_body")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        assert!(result
            .pointer("/rest_request/body")
            .map(|value| value.is_null())
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn rest_validate_response_flags_missing_fields() {
        let result = call(
            "rest.validate_response",
            json!({
                "status": 200,
                "expected_status": 200,
                "required_fields": ["id", "title"],
                "body": { "id": 1 },
            }),
        )
        .await;
        assert_eq!(
            result.get("valid").and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn rest_sync_resource_returns_step_plan() {
        let result = call(
            "rest.sync_resource",
            json!({
                "source": "https://a.example/v1/posts",
                "target": "https://b.example/v1/posts",
                "direction": "mirror",
            }),
        )
        .await;
        let steps = result
            .pointer("/rest_sync/steps")
            .and_then(|value| value.as_array())
            .expect("steps");
        assert_eq!(steps.len(), 4);
    }
}

#[cfg(test)]
mod planning_llm_tests {
    use super::{
        topics, CapabilityProvider, Envelope, NodeId, OrdoLlmProvider, OrdoMessage, ToolCallResult,
    };
    use futures::StreamExt;
    use ordo_bus::{Bus, InProcessBus};
    use ordo_cloud::{CloudCredentialStore, CloudCredentialTask, CloudCredentialUpdate};
    use ordo_protocol::RagHit;
    use serde_json::json;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_task() -> CloudCredentialTask {
        let store = CloudCredentialStore::in_memory().expect("store");
        CloudCredentialTask::start(store)
    }

    #[tokio::test]
    async fn draft_notes_returns_not_configured_without_credential() {
        let provider = OrdoLlmProvider::new(make_task());
        let result = provider
            .handle_tool_call("orchestration.draft_notes", &json!({ "prompt": "hello" }))
            .await
            .expect("capability handled");
        match result {
            ToolCallResult::Failed { error } => {
                assert!(error.contains("not configured"), "got: {error}");
            }
            ToolCallResult::Completed { .. } => {
                panic!("expected Failed when no credential configured");
            }
        }
    }

    #[tokio::test]
    async fn draft_notes_dispatches_to_openai_when_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-gpt",
                "choices": [
                    {
                        "index": 0,
                        "message": { "role": "assistant", "content": "Draft orchestration note." },
                        "finish_reason": "stop"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let task = make_task();
        task.upsert(CloudCredentialUpdate {
            service: "openai".into(),
            label: Some("OpenAI test".into()),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            extras: None,
        })
        .await
        .expect("upsert");

        let provider = OrdoLlmProvider::new(task);
        let result = provider
            .handle_tool_call(
                "orchestration.draft_notes",
                &json!({
                    "prompt": "prepare restart validation notes",
                    "temperature": 0.2,
                }),
            )
            .await
            .expect("capability handled");
        let value = match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("expected success, got: {error}"),
        };
        assert_eq!(
            value.get("assistant_message").and_then(|v| v.as_str()),
            Some("Draft orchestration note.")
        );
        assert_eq!(
            value.get("capability").and_then(|v| v.as_str()),
            Some("orchestration.draft_notes")
        );
        assert_eq!(
            value.get("credential_service").and_then(|v| v.as_str()),
            Some("openai")
        );
    }

    #[tokio::test]
    async fn orchestration_draft_notes_routes_to_anthropic_when_credential_style_matches() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-claude",
                "stop_reason": "end_turn",
                "content": [
                    { "type": "text", "text": "Anthropic reviewer note." }
                ]
            })))
            .mount(&server)
            .await;

        let task = make_task();
        task.upsert(CloudCredentialUpdate {
            service: "anthropic".into(),
            label: Some("Anthropic test".into()),
            auth_style: Some("anthropic".into()),
            secret: Some("sk-ant-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            extras: None,
        })
        .await
        .expect("upsert");

        let provider = OrdoLlmProvider::new(task).with_default_service("anthropic");
        let result = provider
            .handle_tool_call(
                "orchestration.draft_notes",
                &json!({ "prompt": "runtime restart validation" }),
            )
            .await
            .expect("capability handled");
        let value = match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("expected success, got: {error}"),
        };
        assert!(
            value
                .get("assistant_text")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("reviewer note"))
                .unwrap_or(false),
            "expected assistant_text with orchestration note, got: {value:?}"
        );
    }

    /// Stand in for the real RagPeer: listen for `RagQueryRequested` and
    /// reply with a fixed hit on `RAG_QUERY_RESPONSE`.
    async fn fake_rag_peer(
        bus: Arc<dyn Bus>,
        fixture_snippet: &'static str,
        ready: tokio::sync::oneshot::Sender<()>,
    ) {
        let mut sub = bus
            .subscribe(topics::RAG_QUERY_REQUEST)
            .await
            .expect("subscribe RAG_QUERY_REQUEST");
        // Signal that we are subscribed BEFORE the caller publishes any query.
        // The bus is a broadcast channel with no replay, so a request published
        // before this subscribe completes is dropped — which made the retrieval
        // intermittently return 0 hits under scheduling load (a flaky test).
        let _ = ready.send(());
        while let Some(event) = sub.next().await {
            if let OrdoMessage::RagQueryRequested { query, .. } = &event.payload {
                let mut reply = Envelope::new(
                    NodeId::new(),
                    OrdoMessage::RagQueryCompleted {
                        query: query.clone(),
                        hits: vec![RagHit {
                            document_id: "operator profile-voice".into(),
                            uri: "docs/operator profile-voice.md".into(),
                            title: "Operator Notes".into(),
                            collection: "main".into(),
                            chunk_index: 0,
                            score: 0.91,
                            snippet: fixture_snippet.to_string(),
                            tags: vec!["operator profile".into()],
                        }],
                    },
                );
                if let Some(correlation) = event.correlation_id.clone() {
                    reply = reply.with_correlation(correlation);
                }
                let _ = bus.publish(topics::RAG_QUERY_RESPONSE, reply).await;
            }
        }
    }

    #[tokio::test]
    async fn draft_notes_injects_rag_snippets_into_system_prompt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-gpt",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "Grounded reply" },
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;

        let task = make_task();
        task.upsert(CloudCredentialUpdate {
            service: "openai".into(),
            label: Some("OpenAI test".into()),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            extras: None,
        })
        .await
        .expect("upsert");

        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let rag_bus = bus.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            fake_rag_peer(
                rag_bus,
                "OPERATOR NOTES: Confident, grounded, technical. Avoid speculation.",
                ready_tx,
            )
            .await;
        });
        // Wait until the fake RAG peer is actually subscribed before issuing the
        // query. The bus has no replay, so without this the request can be
        // published into the void → 0 hits → the assertions below flake.
        ready_rx.await.expect("rag peer subscribed");

        let provider = OrdoLlmProvider::new(task).with_bus(bus);
        let result = provider
            .handle_tool_call(
                "orchestration.draft_notes",
                &json!({
                    "prompt": "runtime validation",
                    "rag_query": "operator style",
                }),
            )
            .await
            .expect("handled");
        let value = match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("failed: {error}"),
        };

        assert_eq!(value["rag_context_hits"].as_u64(), Some(1));
        let sources = value["rag_context_sources"]
            .as_array()
            .expect("rag_context_sources");
        assert_eq!(
            sources[0]["document_id"].as_str(),
            Some("operator profile-voice")
        );

        // Inspect the body the mock actually received to prove the
        // snippet was injected into the system message.
        let received = server.received_requests().await.expect("recv list");
        assert_eq!(received.len(), 1);
        let body_json: serde_json::Value =
            serde_json::from_slice(&received[0].body).expect("body json");
        let messages = body_json["messages"].as_array().expect("messages array");
        let serialized = serde_json::to_string(messages).expect("serialize messages");
        assert!(
            serialized.contains("Confident, grounded, technical"),
            "expected RAG snippet in messages, got: {serialized}"
        );
        // We expect base system + rag system + user = 3 messages.
        assert_eq!(messages.len(), 3, "got messages: {serialized}");
    }

    #[tokio::test]
    async fn draft_notes_skips_rag_when_opted_out() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-gpt",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "ok" },
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;

        let task = make_task();
        task.upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            ..Default::default()
        })
        .await
        .expect("upsert");

        // Bus present, but `rag: false` should skip the prefetch entirely.
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let provider = OrdoLlmProvider::new(task).with_bus(bus);
        let result = provider
            .handle_tool_call(
                "orchestration.draft_notes",
                &json!({ "prompt": "hi", "rag": false }),
            )
            .await
            .expect("handled");
        let value = match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("failed: {error}"),
        };
        assert_eq!(value["rag_context_hits"].as_u64(), Some(0));
        assert!(value.get("rag_context_sources").is_none());
    }

    #[tokio::test]
    async fn draft_notes_routes_through_review_and_substitutes_edit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-gpt",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "Draft with sk-AbCdEfGhIjKlMnOpQrStUvWxYz0000" },
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;

        let task = make_task();
        task.upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            ..Default::default()
        })
        .await
        .expect("upsert");

        let review_service = ordo_review::ReviewService::new(
            ordo_review::ReviewStore::in_memory().expect("review store"),
        );
        let provider = OrdoLlmProvider::new(task).with_review(review_service.clone());

        // Drive the LLM call concurrently with the operator "pressing
        // Edit" in a separate task. The LLM result should have its
        // assistant_message substituted with the edited text.
        let call_future = async {
            provider
                .handle_tool_call(
                    "orchestration.draft_notes",
                    &json!({
                        "prompt": "restart validation",
                        "review": true,
                        "review_title": "Restart validation notes",
                    }),
                )
                .await
                .expect("handled")
        };

        let operator_service = review_service.clone();
        let operator_future = async move {
            // Wait until the agent has queued its request.
            for _ in 0..20 {
                let pending = operator_service.pending().expect("pending");
                if let Some(request) = pending.first() {
                    operator_service
                        .decide(
                            request.id,
                            ordo_review::ReviewDecisionKind::Edit {
                                content: "Polished draft without any secret material".into(),
                                note: Some("stripped the leaked key".into()),
                            },
                        )
                        .expect("decide");
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("agent never queued a review request");
        };

        let (agent_result, _) = tokio::join!(call_future, operator_future);
        let value = match agent_result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("agent call failed: {error}"),
        };
        assert_eq!(
            value["assistant_message"].as_str(),
            Some("Polished draft without any secret material")
        );
        assert_eq!(
            value["review"]["state"].as_str(),
            Some("edited_and_approved")
        );
        assert_eq!(value["review"]["edited"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn draft_notes_fails_when_review_denied() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-gpt",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "Off-operator profile draft" },
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;
        let task = make_task();
        task.upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            ..Default::default()
        })
        .await
        .expect("upsert");
        let review_service = ordo_review::ReviewService::new(
            ordo_review::ReviewStore::in_memory().expect("review store"),
        );
        let provider = OrdoLlmProvider::new(task).with_review(review_service.clone());

        let operator = {
            let review_service = review_service.clone();
            tokio::spawn(async move {
                for _ in 0..20 {
                    let pending = review_service.pending().expect("pending");
                    if let Some(request) = pending.first() {
                        review_service
                            .decide(
                                request.id,
                                ordo_review::ReviewDecisionKind::Deny {
                                    note: Some("off-operator profile".into()),
                                },
                            )
                            .expect("decide");
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                panic!("no review queued");
            })
        };

        let result = provider
            .handle_tool_call(
                "orchestration.draft_notes",
                &json!({ "prompt": "off operator profile", "review": true }),
            )
            .await
            .expect("handled");
        operator.await.expect("operator join");

        match result {
            ToolCallResult::Failed { error } => {
                assert!(error.contains("denied"), "got: {error}");
            }
            ToolCallResult::Completed { .. } => panic!("expected Failed after deny"),
        }
    }
}

#[cfg(test)]
mod cloud_credentials_secret_tests {
    //! Regression coverage for the HTTP upsert empty-secret semantics.
    //!
    //! The Studio's Edit-credential modal never carries the (redacted)
    //! secret, so saving an edit to any other field sends `secret: ""`.
    //! That must PRESERVE the stored key (matching the bus path's
    //! `full_into_update`), not wipe it. A non-empty secret still rotates.
    use super::CloudOpsProvider;
    use crate::provider_cloud_ops::cloud_credentials_upsert;
    use ordo_cloud::{CloudCredentialStore, CloudCredentialTask};
    use serde_json::json;

    fn provider_with_task() -> (CloudOpsProvider, CloudCredentialTask) {
        let store = CloudCredentialStore::in_memory().expect("store");
        let task = CloudCredentialTask::start(store);
        (CloudOpsProvider::new(task.clone()), task)
    }

    #[tokio::test]
    async fn empty_secret_preserves_existing_on_upsert() {
        let (provider, task) = provider_with_task();

        // Create with a real secret.
        cloud_credentials_upsert(
            &provider,
            &json!({
                "service": "minimax",
                "label": "MiniMax",
                "auth_style": "bearer",
                "secret": "sk-original-key",
                "base_url": "https://api.minimax.io/v1",
                "extras": { "group_id": "grp-1" }
            }),
        )
        .await
        .expect("create");

        // Edit other fields (label + group_id) with an EMPTY secret —
        // exactly what the Studio Edit modal sends.
        cloud_credentials_upsert(
            &provider,
            &json!({
                "service": "minimax",
                "label": "MiniMax (edited)",
                "auth_style": "bearer",
                "secret": "",
                "base_url": "https://api.minimax.io/v1",
                "extras": { "group_id": "grp-2" }
            }),
        )
        .await
        .expect("edit");

        let cred = task
            .get("minimax".into())
            .await
            .expect("get")
            .expect("credential exists");
        assert_eq!(
            cred.secret, "sk-original-key",
            "empty secret must PRESERVE the stored key, not zero it"
        );
        assert_eq!(cred.label, "MiniMax (edited)", "other fields still update");
        assert_eq!(
            cred.extras.get("group_id").map(String::as_str),
            Some("grp-2"),
            "extras still update when secret is empty"
        );
    }

    #[tokio::test]
    async fn missing_secret_field_preserves_existing_on_upsert() {
        let (provider, task) = provider_with_task();
        cloud_credentials_upsert(
            &provider,
            &json!({
                "service": "openai",
                "label": "OpenAI",
                "auth_style": "bearer",
                "secret": "sk-keep-me",
                "base_url": "https://api.openai.com/v1",
                "extras": {}
            }),
        )
        .await
        .expect("create");

        // No `secret` key at all → also preserve.
        cloud_credentials_upsert(
            &provider,
            &json!({
                "service": "openai",
                "label": "OpenAI (renamed)",
                "auth_style": "bearer",
                "base_url": "https://api.openai.com/v1",
                "extras": {}
            }),
        )
        .await
        .expect("edit");

        let cred = task
            .get("openai".into())
            .await
            .expect("get")
            .expect("credential exists");
        assert_eq!(
            cred.secret, "sk-keep-me",
            "omitted secret preserves the key"
        );
        assert_eq!(cred.label, "OpenAI (renamed)");
    }

    #[tokio::test]
    async fn non_empty_secret_rotates_on_upsert() {
        let (provider, task) = provider_with_task();
        cloud_credentials_upsert(
            &provider,
            &json!({
                "service": "openai",
                "label": "OpenAI",
                "auth_style": "bearer",
                "secret": "sk-old",
                "base_url": "https://api.openai.com/v1",
                "extras": {}
            }),
        )
        .await
        .expect("create");

        cloud_credentials_upsert(
            &provider,
            &json!({
                "service": "openai",
                "label": "OpenAI",
                "auth_style": "bearer",
                "secret": "sk-new-rotated",
                "base_url": "https://api.openai.com/v1",
                "extras": {}
            }),
        )
        .await
        .expect("rotate");

        let cred = task
            .get("openai".into())
            .await
            .expect("get")
            .expect("credential exists");
        assert_eq!(
            cred.secret, "sk-new-rotated",
            "a real (non-empty) secret must still rotate the stored key"
        );
    }
}
