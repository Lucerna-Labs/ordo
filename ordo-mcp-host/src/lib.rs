pub mod bridged_providers;
pub mod external;
pub mod external_mcp;
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
pub const SKILLS_INSTALL: &str = "skills.install";
pub const SKILLS_DELETE: &str = "skills.delete";
pub const PLUGINS_LIST: &str = "plugins.list";
pub const PLUGINS_INSTALL: &str = "plugins.install";
pub const PLUGINS_DELETE: &str = "plugins.delete";
pub const PLUGINS_SET_ENABLED: &str = "plugins.set_enabled";
pub const AUTOMATION_LIST: &str = "automation.list";
pub const AUTOMATION_INSPECT: &str = "automation.inspect";
pub const LOGS_SYSTEM_TAIL: &str = "logs.system_tail";

const MAINTENANCE_CAPABILITIES: &[&str] = &[
    SKILLS_LIST,
    SKILLS_INSTALL,
    SKILLS_DELETE,
    PLUGINS_LIST,
    PLUGINS_INSTALL,
    PLUGINS_DELETE,
    PLUGINS_SET_ENABLED,
    AUTOMATION_LIST,
    AUTOMATION_INSPECT,
    LOGS_SYSTEM_TAIL,
];

pub struct MaintenanceProvider {
    user_files_root: PathBuf,
    plugins_root: PathBuf,
}

impl MaintenanceProvider {
    pub fn new(user_files_root: impl Into<PathBuf>, plugins_root: impl Into<PathBuf>) -> Self {
        Self {
            user_files_root: user_files_root.into(),
            plugins_root: plugins_root.into(),
        }
    }

    fn skills_root(&self) -> PathBuf {
        self.user_files_root.join("skills")
    }

    fn automations_path(&self) -> PathBuf {
        self.user_files_root.join("automations.json")
    }
}

#[async_trait]
impl CapabilityProvider for MaintenanceProvider {
    fn name(&self) -> &str {
        "ordo-maintenance"
    }

    fn capabilities(&self) -> Vec<String> {
        MAINTENANCE_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        MAINTENANCE_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    maintenance_description(capability),
                    CapabilityTier::Core,
                    CapabilityActivation::Eager,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let result = match capability {
            SKILLS_LIST => maintenance_list_skills(&self.skills_root()),
            SKILLS_INSTALL => maintenance_install_skill(&self.skills_root(), arguments),
            SKILLS_DELETE => maintenance_delete_named_dir(&self.skills_root(), arguments, "skill"),
            PLUGINS_LIST => maintenance_list_plugins(&self.plugins_root),
            PLUGINS_INSTALL => maintenance_install_plugin(&self.plugins_root, arguments),
            PLUGINS_DELETE => maintenance_delete_named_dir(&self.plugins_root, arguments, "plugin"),
            PLUGINS_SET_ENABLED => maintenance_set_plugin_enabled(&self.plugins_root, arguments),
            AUTOMATION_LIST => maintenance_list_automations(&self.automations_path()),
            AUTOMATION_INSPECT => {
                maintenance_inspect_automation(&self.automations_path(), arguments)
            }
            LOGS_SYSTEM_TAIL => maintenance_tail_system_logs(&self.user_files_root, arguments),
            _ => return None,
        };
        Some(match result {
            Ok(value) => ToolCallResult::Completed { result: value },
            Err(error) => ToolCallResult::Failed { error },
        })
    }
}

fn maintenance_description(capability: &str) -> &'static str {
    match capability {
        SKILLS_LIST => "List locally installed Ordo skills under user-files/skills.",
        SKILLS_INSTALL => "Install or update a local Ordo skill by writing user-files/skills/<id>/skill.md.",
        SKILLS_DELETE => "Delete a local Ordo skill directory by id.",
        PLUGINS_LIST => "List local plugin manifests under user-files/plugins.",
        PLUGINS_INSTALL => "Install or update a plugin manifest under user-files/plugins/<name>/plugin.json. Restart required to load.",
        PLUGINS_DELETE => "Delete a local plugin directory by name. Restart required to unload if active.",
        PLUGINS_SET_ENABLED => "Enable or disable a local plugin manifest. Restart required to apply.",
        AUTOMATION_LIST => "List registered Ordo automations and their recent automation events.",
        AUTOMATION_INSPECT => "Inspect one registered Ordo automation by id without mutating it.",
        LOGS_SYSTEM_TAIL => "Read a bounded tail of local Ordo runtime system logs for diagnostics.",
        _ => "Ordo maintenance capability.",
    }
}

fn maintenance_load_automations(path: &Path) -> Result<AutomationOrchestrator, String> {
    AutomationOrchestrator::load_or_seed(
        path,
        vec![
            default_diagnostic_automation(),
            default_dreaming_automation(),
        ],
    )
    .map_err(|err| err.to_string())
}

fn maintenance_list_automations(path: &Path) -> Result<Value, String> {
    let automation = maintenance_load_automations(path)?;
    let automations = automation.list().into_iter().cloned().collect::<Vec<_>>();
    Ok(json!({
        "path": path.display().to_string(),
        "count": automations.len(),
        "automations": automations,
        "events": automation.event_log(),
    }))
}

fn maintenance_inspect_automation(path: &Path, arguments: &Value) -> Result<Value, String> {
    let id = first_string(arguments, &["id", "automation_id"])
        .ok_or_else(|| "automation.inspect requires id or automation_id".to_string())?;
    let automation_id = AutomationId::parse_str(id.trim()).map_err(|err| err.to_string())?;
    let automation = maintenance_load_automations(path)?;
    let spec = automation
        .get(automation_id)
        .cloned()
        .ok_or_else(|| "automation not found".to_string())?;
    Ok(json!({ "automation": spec }))
}

fn maintenance_tail_system_logs(root: &Path, arguments: &Value) -> Result<Value, String> {
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(1, 1_000) as usize;
    let source = optional_string(arguments, "source")
        .unwrap_or_else(|| "both".to_string())
        .to_ascii_lowercase();
    let mut logs = Vec::new();
    for (name, path) in system_log_candidates(root) {
        let include = match source.as_str() {
            "stdout" | "out" => name.contains("out"),
            "stderr" | "err" => name.contains("err"),
            "both" | "all" => true,
            other => {
                return Err(format!(
                    "unsupported log source '{other}'; use stdout, stderr, or both"
                ))
            }
        };
        if !include || !path.is_file() {
            continue;
        }
        let lines = tail_file_lines(&path, limit)?;
        logs.push(json!({
            "name": name,
            "path": path.display().to_string(),
            "line_count": lines.len(),
            "lines": lines,
        }));
    }
    Ok(json!({
        "source": source,
        "limit": limit,
        "count": logs.len(),
        "logs": logs,
    }))
}

fn system_log_candidates(root: &Path) -> Vec<(String, PathBuf)> {
    let mut dirs = Vec::new();
    dirs.push(root.to_path_buf());
    if let Some(parent) = root.parent() {
        dirs.push(parent.to_path_buf());
        if let Some(grandparent) = parent.parent() {
            dirs.push(grandparent.to_path_buf());
        }
    }
    if let Ok(current_dir) = std::env::current_dir() {
        dirs.push(current_dir);
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut candidates = Vec::new();
    for dir in dirs {
        for name in ["runtime-dev.out.log", "runtime-dev.err.log"] {
            let path = dir.join(name);
            if seen.insert(path.clone()) {
                candidates.push((name.to_string(), path));
            }
        }
        for name in ["ordo.log", "runtime.log", "system.log"] {
            let path = dir.join("logs").join(name);
            if seen.insert(path.clone()) {
                candidates.push((name.to_string(), path));
            }
        }
    }
    candidates
}

fn tail_file_lines(path: &Path, limit: usize) -> Result<Vec<String>, String> {
    let content = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let mut lines = content
        .lines()
        .rev()
        .take(limit)
        .map(str::to_string)
        .collect::<Vec<_>>();
    lines.reverse();
    Ok(lines)
}

fn maintenance_list_skills(root: &Path) -> Result<Value, String> {
    std::fs::create_dir_all(root).map_err(|err| err.to_string())?;
    let mut skills = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        if !entry.file_type().map_err(|err| err.to_string())?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let skill_path = entry.path().join("skill.md");
        skills.push(json!({
            "id": id,
            "path": skill_path.display().to_string(),
            "installed": skill_path.exists(),
        }));
    }
    Ok(json!({ "root": root.display().to_string(), "count": skills.len(), "skills": skills }))
}

fn maintenance_install_skill(root: &Path, arguments: &Value) -> Result<Value, String> {
    let id = required_name(arguments, &["id", "name"])?;
    let body = first_string(arguments, &["body", "content", "skill_md"])
        .ok_or_else(|| "skills.install requires body, content, or skill_md".to_string())?;
    if body.trim().is_empty() {
        return Err("skills.install body cannot be empty".into());
    }
    let overwrite = arguments
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let dir = safe_named_dir(root, &id)?;
    let skill_path = dir.join("skill.md");
    if skill_path.exists() && !overwrite {
        return Err(format!(
            "skill '{id}' already exists; pass overwrite=true to replace it"
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    std::fs::write(&skill_path, body).map_err(|err| err.to_string())?;
    if let Some(metadata) = arguments.get("metadata") {
        let metadata_path = dir.join("metadata.json");
        let encoded = serde_json::to_string_pretty(metadata).map_err(|err| err.to_string())?;
        std::fs::write(metadata_path, encoded).map_err(|err| err.to_string())?;
    }
    Ok(json!({
        "id": id,
        "path": skill_path.display().to_string(),
        "note": "skill installed locally; refresh the UXI or restart runtime if the skill index is cached",
    }))
}

fn maintenance_list_plugins(root: &Path) -> Result<Value, String> {
    std::fs::create_dir_all(root).map_err(|err| err.to_string())?;
    let mut plugins = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        if !entry.file_type().map_err(|err| err.to_string())?.is_dir() {
            continue;
        }
        let manifest_path = entry.path().join("plugin.json");
        if !manifest_path.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&manifest_path).map_err(|err| err.to_string())?;
        let manifest: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
        plugins.push(json!({
            "name": manifest.get("name").and_then(Value::as_str).unwrap_or(""),
            "enabled": manifest.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            "manifest_path": manifest_path.display().to_string(),
        }));
    }
    Ok(json!({ "root": root.display().to_string(), "count": plugins.len(), "plugins": plugins }))
}

fn maintenance_install_plugin(root: &Path, arguments: &Value) -> Result<Value, String> {
    let manifest_value = arguments.get("manifest").unwrap_or(arguments).clone();
    let mut manifest: Value = manifest_value;
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "plugins.install requires manifest.name".to_string())?;
    validate_name(&name)?;
    let command = manifest
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if command.is_empty() {
        return Err("plugins.install requires manifest.command".into());
    }
    let core_override = manifest
        .get("core_override")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !core_override {
        if let Some(lanes) = manifest.get("expected_lanes").and_then(Value::as_array) {
            for lane in lanes.iter().filter_map(Value::as_str) {
                if [
                    "cloud.",
                    "runtime.",
                    "filesystem.",
                    "self_heal.",
                    "memory.",
                    "knowledge.",
                ]
                .iter()
                .any(|reserved| lane.starts_with(reserved))
                {
                    return Err(format!(
                        "plugin lane '{lane}' is reserved for core runtime providers"
                    ));
                }
            }
        }
    }
    if manifest.get("version").is_none() {
        manifest["version"] = Value::String("0.0.0".into());
    }
    if manifest.get("enabled").is_none() {
        manifest["enabled"] = Value::Bool(true);
    }
    let overwrite = arguments
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let dir = safe_named_dir(root, &name)?;
    let manifest_path = dir.join("plugin.json");
    if manifest_path.exists() && !overwrite {
        return Err(format!(
            "plugin '{name}' already exists; pass overwrite=true to replace it"
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let encoded = serde_json::to_string_pretty(&manifest).map_err(|err| err.to_string())?;
    std::fs::write(&manifest_path, encoded).map_err(|err| err.to_string())?;
    Ok(json!({
        "name": name,
        "manifest_path": manifest_path.display().to_string(),
        "note": "plugin manifest installed locally; restart runtime to spawn it",
    }))
}

fn maintenance_set_plugin_enabled(root: &Path, arguments: &Value) -> Result<Value, String> {
    let name = required_name(arguments, &["name", "id"])?;
    let enabled = arguments
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| "plugins.set_enabled requires enabled=true or false".to_string())?;
    let dir = safe_named_dir(root, &name)?;
    let manifest_path = dir.join("plugin.json");
    let raw = std::fs::read_to_string(&manifest_path).map_err(|err| err.to_string())?;
    let mut manifest: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
    manifest["enabled"] = Value::Bool(enabled);
    let encoded = serde_json::to_string_pretty(&manifest).map_err(|err| err.to_string())?;
    std::fs::write(&manifest_path, encoded).map_err(|err| err.to_string())?;
    Ok(json!({
        "name": name,
        "enabled": enabled,
        "manifest_path": manifest_path.display().to_string(),
        "note": "restart runtime to apply plugin enabled state",
    }))
}

fn maintenance_delete_named_dir(
    root: &Path,
    arguments: &Value,
    label: &str,
) -> Result<Value, String> {
    let name = required_name(arguments, &["id", "name"])?;
    let dir = safe_named_dir(root, &name)?;
    if !dir.exists() {
        return Ok(
            json!({ "name": name, "deleted": false, "note": format!("{label} did not exist") }),
        );
    }
    if dir == root {
        return Err(format!("refusing to delete {label} root"));
    }
    std::fs::remove_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(json!({ "name": name, "deleted": true, "path": dir.display().to_string() }))
}

fn required_name(arguments: &Value, keys: &[&str]) -> Result<String, String> {
    for key in keys {
        if let Some(value) = arguments.get(*key).and_then(Value::as_str) {
            let value = value.trim().to_string();
            validate_name(&value)?;
            return Ok(value);
        }
    }
    Err(format!(
        "missing required name field: {}",
        keys.join(" or ")
    ))
}

fn first_string(arguments: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| arguments.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn validate_name(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("name cannot be empty".into());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "invalid name '{value}'; use ASCII letters, numbers, hyphens, or underscores"
        ));
    }
    Ok(())
}

fn safe_named_dir(root: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    std::fs::create_dir_all(root).map_err(|err| err.to_string())?;
    Ok(root.join(name))
}
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
                            let failed_step = Envelope::new(
                                node_id.clone(),
                                OrdoMessage::StepFailed {
                                    run_id,
                                    step_id: Uuid::new_v4(),
                                    error: format!("no provider accepted run goal '{}'", goal),
                                },
                            );
                            let failed_step =
                                with_correlation(failed_step, correlation_id.clone());
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

#[derive(Debug, Clone, Default)]
pub struct FilesystemProvider {
    root: Option<PathBuf>,
}

impl FilesystemProvider {
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    fn capability_description(&self) -> String {
        match &self.root {
            Some(root) => format!("Reads and writes files under {}.", root.display()),
            None => "Reads and writes files from the local disk.".to_string(),
        }
    }

    fn resolve_path(&self, requested: &str) -> Result<PathBuf, String> {
        let requested_path = PathBuf::from(requested);
        let Some(root) = &self.root else {
            return Ok(requested_path);
        };

        let normalized_root = normalize_path(root);
        let combined = if requested_path.is_absolute() {
            requested_path
        } else {
            normalized_root.join(requested_path)
        };
        let normalized = normalize_path(&combined);
        if normalized.starts_with(&normalized_root) {
            Ok(normalized)
        } else {
            Err(format!(
                "path '{}' escapes configured root {}",
                requested,
                normalized_root.display()
            ))
        }
    }
}

#[async_trait]
impl CapabilityProvider for FilesystemProvider {
    fn name(&self) -> &str {
        "filesystem"
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "filesystem.read_file".to_string(),
            "filesystem.write_file".to_string(),
        ]
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor::new(
                "filesystem.read_file",
                self.name(),
                self.capability_description(),
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "filesystem.write_file",
                self.name(),
                self.capability_description(),
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
        ]
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        if requirement.contains("read file") {
            Some(CapabilityMatch {
                capability: "filesystem.read_file".to_string(),
                description: self.capability_description(),
            })
        } else {
            None
        }
    }

    async fn handle_run(&self, goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        let normalized_goal = goal.to_ascii_lowercase();
        if normalized_goal.contains("read file") || normalized_goal.contains("read") {
            let Some(path) = extract_read_path(goal) else {
                return Some(ProviderRun {
                    steps: vec![ProviderStep {
                        capability: "filesystem.read_file".to_string(),
                        name: "filesystem.read_file".to_string(),
                        status: ProviderRunStatus::Failed {
                            error: "run goal did not include a readable file path".to_string(),
                        },
                    }],
                });
            };

            let resolved_path = match self.resolve_path(&path) {
                Ok(path) => path,
                Err(error) => {
                    return Some(ProviderRun {
                        steps: vec![ProviderStep {
                            capability: "filesystem.read_file".to_string(),
                            name: "filesystem.read_file".to_string(),
                            status: ProviderRunStatus::Failed { error },
                        }],
                    });
                }
            };

            let status = match std::fs::read_to_string(&resolved_path) {
                Ok(contents) => ProviderRunStatus::Completed {
                    output: format!(
                        "read {} bytes from {} preview='{}'",
                        contents.len(),
                        resolved_path.display(),
                        preview_text(&contents)
                    ),
                },
                Err(err) => ProviderRunStatus::Failed {
                    error: format!("failed to read {}: {}", resolved_path.display(), err),
                },
            };
            Some(ProviderRun {
                steps: vec![ProviderStep {
                    capability: "filesystem.read_file".to_string(),
                    name: "filesystem.read_file".to_string(),
                    status,
                }],
            })
        } else {
            None
        }
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        match capability {
            "filesystem.read_file" => {
                let path = arguments.get("path")?.as_str()?;
                let resolved_path = match self.resolve_path(path) {
                    Ok(path) => path,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(match std::fs::read_to_string(&resolved_path) {
                    Ok(contents) => {
                        let mut result = json!({
                            "path": resolved_path.display().to_string(),
                            "bytes": contents.len(),
                            "preview": preview_text(&contents),
                        });
                        attach_context_to_output(&mut result, arguments);
                        ToolCallResult::Completed { result }
                    }
                    Err(err) => ToolCallResult::Failed {
                        error: format!("failed to read {}: {}", resolved_path.display(), err),
                    },
                })
            }
            "filesystem.write_file" => {
                let path = arguments.get("path")?.as_str()?;
                let content = arguments.get("content")?.as_str()?;
                let resolved_path = match self.resolve_path(path) {
                    Ok(path) => path,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                if let Some(parent) = resolved_path.parent() {
                    if let Err(err) = std::fs::create_dir_all(parent) {
                        return Some(ToolCallResult::Failed {
                            error: format!(
                                "failed to prepare parent directory for {}: {}",
                                resolved_path.display(),
                                err
                            ),
                        });
                    }
                }
                Some(match std::fs::write(&resolved_path, content) {
                    Ok(()) => {
                        let mut result = json!({
                            "path": resolved_path.display().to_string(),
                            "bytes": content.len(),
                            "status": "written",
                        });
                        attach_context_to_output(&mut result, arguments);
                        ToolCallResult::Completed { result }
                    }
                    Err(err) => ToolCallResult::Failed {
                        error: format!("failed to write {}: {}", resolved_path.display(), err),
                    },
                })
            }
            _ => None,
        }
    }
}

pub struct KnowledgeProvider;

#[async_trait]
impl CapabilityProvider for KnowledgeProvider {
    fn name(&self) -> &str {
        "knowledge"
    }

    fn capabilities(&self) -> Vec<String> {
        KnowledgeTask::ALL
            .iter()
            .map(|task| task.capability().to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        KnowledgeTask::ALL
            .iter()
            .map(|task| {
                CapabilityDescriptor::new(
                    task.capability(),
                    self.name(),
                    task.description(),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        infer_knowledge_task(requirement).map(|task| CapabilityMatch {
            capability: task.capability().to_string(),
            description: task.description().to_string(),
        })
    }

    async fn handle_run(&self, goal: &str, context: &[RagHit]) -> Option<ProviderRun> {
        let task = infer_knowledge_task(goal)?;
        let snippets = knowledge_snippets_from_hits(context);
        let sources = knowledge_sources_from_hits(context);
        let result = synthesize_knowledge_result(task, goal, &snippets, &sources);

        Some(ProviderRun {
            steps: vec![
                ProviderStep {
                    capability: task.capability().to_string(),
                    name: "knowledge.prepare_context".to_string(),
                    status: ProviderRunStatus::Completed {
                        output: format!("prepared {} retrieved context hit(s)", context.len()),
                    },
                },
                ProviderStep {
                    capability: task.capability().to_string(),
                    name: task.capability().to_string(),
                    status: ProviderRunStatus::Completed {
                        output: knowledge_result_preview(task, &result),
                    },
                },
            ],
        })
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let task = knowledge_task_from_capability(capability)?;

        let goal = arguments.get("goal")?.as_str()?;
        let snippets = knowledge_snippets_from_arguments(arguments);
        let sources = knowledge_sources_from_arguments(arguments);

        Some(ToolCallResult::Completed {
            result: synthesize_knowledge_result(task, goal, &snippets, &sources),
        })
    }
}

/// Pure-data capability provider for the Ordo domain families:
/// planning.*, orchestration.*, research.*, content_store.*. Each capability transforms a
/// structured input into a structured output without hitting the filesystem
/// or the network, giving the planner and routers a stable surface to wire
/// real ordo-ops engines into later.
#[derive(Debug, Default, Clone)]
pub struct OrdoOpsProvider {
    /// When set, artifact-producing capabilities (capture_brief,
    /// plan_initiative, package_resources, summarize_deliverables,
    /// schedule_release, request_revision) persist a markdown/JSON response
    /// of their output under `<user_files_path>/<lane>/<slug>.{md,json}`
    /// and include the path in the returned value as `artifact_path`.
    user_files_path: Option<PathBuf>,
}

impl OrdoOpsProvider {
    pub fn new() -> Self {
        Self {
            user_files_path: None,
        }
    }

    /// Enable artifact persistence. All produced briefs/plans/manifests
    /// will land inside subdirectories of `path`.
    pub fn with_user_files_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_files_path = Some(path.into());
        self
    }
}

const PLANNING_CAPTURE_BRIEF: &str = "planning.capture_brief";
const PLANNING_PLAN_INITIATIVE: &str = "planning.plan_initiative";
const PLANNING_PACKAGE_RESOURCES: &str = "planning.package_resources";
const PLANNING_SUMMARIZE_DELIVERABLES: &str = "planning.summarize_deliverables";
const ORCHESTRATION_ROUTE_REVIEW: &str = "orchestration.route_review";
const ORCHESTRATION_REQUEST_REVISION: &str = "orchestration.request_revision";
const ORCHESTRATION_ADVANCE_STAGE: &str = "orchestration.advance_stage";
const ORCHESTRATION_SCHEDULE_RELEASE: &str = "orchestration.schedule_release";
const RESEARCH_PACKAGE_METADATA: &str = "research.package_metadata";
const RESEARCH_AUDIT_READINESS: &str = "research.audit_readiness";
const CONTENT_STORE_FIELD_MAPPING: &str = "content_store.field_mapping";
const CONTENT_STORE_PUBLISH_READINESS: &str = "content_store.publish_readiness";

const ORDO_OPS_CAPABILITIES: &[&str] = &[
    PLANNING_CAPTURE_BRIEF,
    PLANNING_PLAN_INITIATIVE,
    PLANNING_PACKAGE_RESOURCES,
    PLANNING_SUMMARIZE_DELIVERABLES,
    ORCHESTRATION_ROUTE_REVIEW,
    ORCHESTRATION_REQUEST_REVISION,
    ORCHESTRATION_ADVANCE_STAGE,
    ORCHESTRATION_SCHEDULE_RELEASE,
    RESEARCH_PACKAGE_METADATA,
    RESEARCH_AUDIT_READINESS,
    CONTENT_STORE_FIELD_MAPPING,
    CONTENT_STORE_PUBLISH_READINESS,
];

fn planning_ops_description(capability: &str) -> &'static str {
    match capability {
        PLANNING_CAPTURE_BRIEF => {
            "Captures a structured initiative brief from title/goal/audience/deliverables inputs."
        }
        PLANNING_PLAN_INITIATIVE => {
            "Produces an ordered set of initiative phases from a deliverables list."
        }
        PLANNING_PACKAGE_RESOURCES => {
            "Packages a set of planning resources into a manifest with counts by kind."
        }
        PLANNING_SUMMARIZE_DELIVERABLES => "Summarizes a deliverables list with per-type counts.",
        ORCHESTRATION_ROUTE_REVIEW => {
            "Routes a review for the given stage to the next reviewer and stage."
        }
        ORCHESTRATION_REQUEST_REVISION => {
            "Creates a revision request record with stage, reason, and optional due date."
        }
        ORCHESTRATION_ADVANCE_STAGE => {
            "Advances the orchestration from the current stage to the next valid stage."
        }
        ORCHESTRATION_SCHEDULE_RELEASE => {
            "Produces a release schedule backing out from the target date."
        }
        RESEARCH_PACKAGE_METADATA => {
            "Packages Research metadata (title, description, keywords) into a tag bundle."
        }
        RESEARCH_AUDIT_READINESS => {
            "Audits Research title/description/keywords for common readiness issues."
        }
        CONTENT_STORE_FIELD_MAPPING => {
            "Maps a set of source fields to a canonical Content Store schema."
        }
        CONTENT_STORE_PUBLISH_READINESS => {
            "Checks a Content Store record for required fields before publish."
        }
        _ => "Ordo operations capability.",
    }
}

#[async_trait]
impl CapabilityProvider for OrdoOpsProvider {
    fn name(&self) -> &str {
        "ordo-ops"
    }

    fn capabilities(&self) -> Vec<String> {
        ORDO_OPS_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ORDO_OPS_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    planning_ops_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let result = match capability {
            PLANNING_CAPTURE_BRIEF => capture_brief(arguments),
            PLANNING_PLAN_INITIATIVE => plan_initiative(arguments),
            PLANNING_PACKAGE_RESOURCES => {
                package_resources(arguments, self.user_files_path.as_deref())
            }
            PLANNING_SUMMARIZE_DELIVERABLES => summarize_deliverables(arguments),
            ORCHESTRATION_ROUTE_REVIEW => route_review(arguments),
            ORCHESTRATION_REQUEST_REVISION => request_revision(arguments),
            ORCHESTRATION_ADVANCE_STAGE => advance_stage(arguments),
            ORCHESTRATION_SCHEDULE_RELEASE => schedule_release(arguments),
            RESEARCH_PACKAGE_METADATA => package_research_metadata(arguments),
            RESEARCH_AUDIT_READINESS => audit_research_readiness(arguments),
            CONTENT_STORE_FIELD_MAPPING => map_content_store_fields(arguments),
            CONTENT_STORE_PUBLISH_READINESS => check_content_store_publish_readiness(arguments),
            _ => return None,
        };
        Some(match result {
            Ok(mut value) => {
                if let Some(root) = &self.user_files_path {
                    if let Err(err) = persist_artifact(root, capability, arguments, &mut value) {
                        tracing::warn!(
                            target: "ordo_mcp_host::planning_ops",
                            capability,
                            error = %err,
                            "failed to persist artifact"
                        );
                    }
                }
                attach_context_to_output(&mut value, arguments);
                ToolCallResult::Completed { result: value }
            }
            Err(error) => ToolCallResult::Failed { error },
        })
    }
}

fn capture_brief(arguments: &Value) -> Result<Value, String> {
    let title = require_string(arguments, "title")?;
    let goal = require_string(arguments, "goal")?;
    let audience = optional_string(arguments, "audience").unwrap_or_default();
    let deliverables = optional_string_array(arguments, "deliverables");
    Ok(json!({
        "brief": {
            "title": title,
            "goal": goal,
            "audience": audience,
            "deliverables": deliverables,
            "deliverable_count": deliverables.len(),
        },
    }))
}

fn plan_initiative(arguments: &Value) -> Result<Value, String> {
    let deliverables = optional_string_array(arguments, "deliverables");
    let phases = if deliverables.is_empty() {
        vec![
            json!({ "phase": "discovery", "deliverables": Vec::<String>::new() }),
            json!({ "phase": "production", "deliverables": Vec::<String>::new() }),
            json!({ "phase": "launch", "deliverables": Vec::<String>::new() }),
        ]
    } else {
        let third = deliverables.len().div_ceil(3);
        let (discovery, rest) = deliverables.split_at(third.min(deliverables.len()));
        let split = third.min(rest.len());
        let (production, launch) = rest.split_at(split);
        vec![
            json!({ "phase": "discovery", "deliverables": discovery }),
            json!({ "phase": "production", "deliverables": production }),
            json!({ "phase": "launch", "deliverables": launch }),
        ]
    };
    Ok(json!({ "phases": phases }))
}

fn package_resources(arguments: &Value, user_files_root: Option<&Path>) -> Result<Value, String> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut manifest: Vec<Value> = Vec::new();
    let mut total_bytes: u64 = 0;

    // Preferred mode: walk a real directory. We sandbox every walk to the
    // configured `user_files_path` so a malformed argument can't enumerate
    // the whole filesystem.
    if let Some(rel) = arguments.get("input_directory").and_then(|v| v.as_str()) {
        let root = user_files_root.ok_or_else(|| {
            "package_resources with input_directory requires the provider to be configured with \
             a user-files path"
                .to_string()
        })?;
        let target = sandbox_path(root, rel).map_err(|err| err.to_string())?;
        if !target.exists() {
            return Err(format!(
                "input_directory '{rel}' does not exist under user-files"
            ));
        }
        walk_resources(
            &target,
            root,
            &target,
            &mut manifest,
            &mut counts,
            &mut total_bytes,
        )?;
    }

    // Back-compat: an explicit `resources` array keeps working and can
    // coexist with `input_directory`.
    let inline = arguments
        .get("resources")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    for resource in &inline {
        let path = resource
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let kind = resource
            .get("kind")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| infer_resource_kind(&path));
        *counts.entry(kind.clone()).or_insert(0) += 1;
        manifest.push(json!({ "path": path, "kind": kind }));
    }

    Ok(json!({
        "manifest": manifest,
        "count": manifest.len(),
        "by_kind": counts,
        "total_bytes": total_bytes,
    }))
}

fn walk_resources(
    dir: &Path,
    root: &Path,
    base: &Path,
    manifest: &mut Vec<Value>,
    counts: &mut std::collections::BTreeMap<String, usize>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|err| err.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let file_type = entry.file_type().map_err(|err| err.to_string())?;
        let path = entry.path();
        if file_type.is_dir() {
            walk_resources(&path, root, base, manifest, counts, total_bytes)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let meta = entry.metadata().map_err(|err| err.to_string())?;
        let size = meta.len();
        *total_bytes = total_bytes.saturating_add(size);

        let relative_to_root = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let relative_to_base = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let kind = infer_resource_kind(&relative_to_root);
        *counts.entry(kind.clone()).or_insert(0) += 1;

        let modified_rfc3339 = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::<chrono::Utc>::from(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(d.as_secs()),
                )
                .to_rfc3339()
            });

        manifest.push(json!({
            "path": relative_to_root,
            "relative_path": relative_to_base,
            "kind": kind,
            "size_bytes": size,
            "modified": modified_rfc3339,
        }));
    }
    Ok(())
}

/// Resolve `rel` against `root` while preventing escapes via `..`.
fn sandbox_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut resolved = root.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(format!(
                    "path '{rel}' must be relative to the user-files root"
                ));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() || resolved.as_path() < root {
                    return Err(format!("path '{rel}' escapes the user-files root"));
                }
            }
            Component::Normal(segment) => resolved.push(segment),
        }
    }
    if !resolved.starts_with(root) {
        return Err(format!("path '{rel}' escapes the user-files root"));
    }
    Ok(resolved)
}

fn summarize_deliverables(arguments: &Value) -> Result<Value, String> {
    let deliverables = optional_string_array(arguments, "deliverables");
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for deliverable in &deliverables {
        let kind = classify_deliverable(deliverable);
        *counts.entry(kind).or_insert(0) += 1;
    }
    Ok(json!({
        "total": deliverables.len(),
        "by_kind": counts,
    }))
}

fn route_review(arguments: &Value) -> Result<Value, String> {
    let stage = require_string(arguments, "stage")?;
    let (next_reviewer, next_stage) = match stage.as_str() {
        "draft" => ("planning-lead", "planning-review"),
        "planning-review" => ("editor", "editorial-review"),
        "editorial-review" => ("research-lead", "research-review"),
        "research-review" => ("content_store-admin", "publish-ready"),
        "publish-ready" => ("release-manager", "scheduled"),
        other => {
            return Err(format!("unknown review stage '{other}'"));
        }
    };
    Ok(json!({
        "stage": stage,
        "next_reviewer": next_reviewer,
        "next_stage": next_stage,
    }))
}

fn request_revision(arguments: &Value) -> Result<Value, String> {
    let stage = require_string(arguments, "stage")?;
    let reason = require_string(arguments, "reason")?;
    let due = optional_string(arguments, "due");
    Ok(json!({
        "revision_request": {
            "stage": stage,
            "reason": reason,
            "due": due,
        },
    }))
}

fn advance_stage(arguments: &Value) -> Result<Value, String> {
    let stage = require_string(arguments, "stage")?;
    let next_stage = match stage.as_str() {
        "draft" => "planning-review",
        "planning-review" => "editorial-review",
        "editorial-review" => "research-review",
        "research-review" => "publish-ready",
        "publish-ready" => "scheduled",
        "scheduled" => "released",
        other => {
            return Err(format!("cannot advance from unknown stage '{other}'"));
        }
    };
    Ok(json!({ "stage": stage, "next_stage": next_stage }))
}

fn schedule_release(arguments: &Value) -> Result<Value, String> {
    let release_date = require_string(arguments, "release_date")?;
    let default_stages = [
        "draft",
        "planning-review",
        "editorial-review",
        "research-review",
        "publish-ready",
        "scheduled",
    ];
    let stages = arguments
        .get("stages")
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| default_stages.iter().map(|s| (*s).to_string()).collect());
    Ok(json!({
        "release_date": release_date,
        "stages": stages,
        "stage_count": stages.len(),
    }))
}

fn package_research_metadata(arguments: &Value) -> Result<Value, String> {
    let title = require_string(arguments, "title")?;
    let description = require_string(arguments, "description")?;
    let keywords = optional_string_array(arguments, "keywords");
    Ok(json!({
        "research_metadata": {
            "title": title,
            "description": description,
            "keywords": keywords,
            "keyword_count": keywords.len(),
        },
    }))
}

fn audit_research_readiness(arguments: &Value) -> Result<Value, String> {
    let title = require_string(arguments, "title")?;
    let description = require_string(arguments, "description")?;
    let keywords = optional_string_array(arguments, "keywords");
    let slug = optional_string(arguments, "slug");
    let body = optional_string(arguments, "body");

    let mut findings: Vec<Value> = Vec::new();
    let mut push = |severity: &str, code: &str, message: String| {
        findings.push(json!({
            "severity": severity,
            "code": code,
            "message": message,
        }));
    };

    // --- title ---------------------------------------------------------
    let title_len = title.chars().count();
    if title_len == 0 {
        push("error", "title_empty", "title is empty".into());
    } else if title_len < 10 {
        push(
            "warn",
            "title_too_short",
            format!("title is {title_len} chars (recommend Ã¢â€°Â¥ 10)"),
        );
    } else if title_len > 70 {
        push(
            "warn",
            "title_too_long",
            format!("title is {title_len} chars; most search engines truncate beyond 60Ã¢â‚¬â€œ70"),
        );
    }
    if title.trim() != title {
        push(
            "info",
            "title_whitespace",
            "title has leading or trailing whitespace".into(),
        );
    }
    if title.chars().filter(|c| c.is_uppercase()).count() >= title_len.saturating_sub(2)
        && title_len > 10
    {
        push(
            "info",
            "title_all_caps",
            "title is all-caps; consider title case for readability".into(),
        );
    }

    // --- description ---------------------------------------------------
    let description_len = description.chars().count();
    if description_len == 0 {
        push("error", "description_empty", "description is empty".into());
    } else if description_len < 50 {
        push(
            "warn",
            "description_too_short",
            format!("description is {description_len} chars (recommend 50Ã¢â‚¬â€œ160)"),
        );
    } else if description_len > 160 {
        push(
            "warn",
            "description_too_long",
            format!("description is {description_len} chars; SERP snippets cap at ~160"),
        );
    }

    // --- slug ----------------------------------------------------------
    if let Some(slug) = &slug {
        if slug.is_empty() {
            push("error", "slug_empty", "slug is empty".into());
        } else {
            let valid = slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !valid {
                push(
                    "error",
                    "slug_format",
                    "slug must be lowercase ASCII letters, digits, and hyphens only".into(),
                );
            }
            if slug.starts_with('-') || slug.ends_with('-') {
                push(
                    "warn",
                    "slug_edge_hyphen",
                    "slug should not start or end with a hyphen".into(),
                );
            }
            if slug.contains("--") {
                push(
                    "info",
                    "slug_double_hyphen",
                    "slug contains consecutive hyphens".into(),
                );
            }
            if slug.chars().count() > 75 {
                push(
                    "warn",
                    "slug_too_long",
                    format!(
                        "slug is {} chars; recommend Ã¢â€°Â¤ 75",
                        slug.chars().count()
                    ),
                );
            }
        }
    }

    // --- keywords ------------------------------------------------------
    if keywords.is_empty() {
        push("warn", "keywords_missing", "no keywords provided".into());
    } else if keywords.len() > 10 {
        push(
            "info",
            "keywords_too_many",
            format!(
                "{} keywords provided; prefer 3Ã¢â‚¬â€œ10 focused terms",
                keywords.len()
            ),
        );
    }

    // --- keyword coverage ---------------------------------------------
    let haystack = format!(
        "{} {} {}",
        title.to_lowercase(),
        description.to_lowercase(),
        body.as_deref().unwrap_or("").to_lowercase()
    );
    let mut uncovered = Vec::new();
    for keyword in &keywords {
        let needle = keyword.trim().to_lowercase();
        if !needle.is_empty() && !haystack.contains(&needle) {
            uncovered.push(keyword.clone());
        }
    }
    if !uncovered.is_empty() {
        push(
            "warn",
            "keyword_not_covered",
            format!(
                "keywords not found in title/description/body: {}",
                uncovered.join(", ")
            ),
        );
    }

    let error_count = findings
        .iter()
        .filter(|f| f["severity"].as_str() == Some("error"))
        .count();
    let warn_count = findings
        .iter()
        .filter(|f| f["severity"].as_str() == Some("warn"))
        .count();

    // Legacy flat `issues` shape kept for back-compat with existing
    // callers that assert on it.
    let issues: Vec<String> = findings
        .iter()
        .filter(|f| matches!(f["severity"].as_str(), Some("error") | Some("warn")))
        .filter_map(|f| f["message"].as_str().map(str::to_string))
        .collect();

    Ok(json!({
        "title_length": title_len,
        "description_length": description_len,
        "keyword_count": keywords.len(),
        "uncovered_keywords": uncovered,
        "findings": findings,
        "error_count": error_count,
        "warn_count": warn_count,
        "issues": issues,
        "ready": error_count == 0 && warn_count == 0,
    }))
}

fn map_content_store_fields(arguments: &Value) -> Result<Value, String> {
    let source = arguments
        .get("source_fields")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let mut mapping = serde_json::Map::new();
    for (key, value) in source {
        let canonical = match key.to_ascii_lowercase().as_str() {
            "headline" | "name" => "title",
            "body" | "content" | "article" => "body",
            "slug" | "uri" | "url" => "slug",
            "author" | "byline" => "author",
            "tags" | "keywords" | "labels" => "tags",
            "publish_at" | "scheduled_at" | "release_date" => "publish_at",
            _ => &key,
        };
        mapping.insert(canonical.to_string(), value);
    }
    Ok(json!({ "content_store_fields": mapping }))
}

fn check_content_store_publish_readiness(arguments: &Value) -> Result<Value, String> {
    let fields = arguments
        .get("fields")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let required = ["title", "body", "slug", "publish_at"];
    let missing: Vec<String> = required
        .iter()
        .filter(|key| !fields.contains_key(**key))
        .map(|key| (*key).to_string())
        .collect();
    Ok(json!({
        "ready": missing.is_empty(),
        "missing": missing,
        "required": required,
    }))
}

fn require_string(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("missing required string field '{key}'"))
}

fn optional_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn optional_string_array(arguments: &Value, key: &str) -> Vec<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn infer_resource_kind(path: &str) -> String {
    let lowered = path.to_ascii_lowercase();
    if lowered.ends_with(".jpg")
        || lowered.ends_with(".jpeg")
        || lowered.ends_with(".png")
        || lowered.ends_with(".gif")
        || lowered.ends_with(".webp")
    {
        "image".to_string()
    } else if lowered.ends_with(".mp4") || lowered.ends_with(".mov") || lowered.ends_with(".webm") {
        "video".to_string()
    } else if lowered.ends_with(".mp3") || lowered.ends_with(".wav") || lowered.ends_with(".flac") {
        "audio".to_string()
    } else if lowered.ends_with(".md") || lowered.ends_with(".txt") || lowered.ends_with(".html") {
        "response".to_string()
    } else {
        "other".to_string()
    }
}

fn classify_deliverable(deliverable: &str) -> String {
    let lowered = deliverable.to_ascii_lowercase();
    if lowered.contains("video") {
        "video".to_string()
    } else if lowered.contains("image") || lowered.contains("photo") || lowered.contains("banner") {
        "image".to_string()
    } else if lowered.contains("response")
        || lowered.contains("article")
        || lowered.contains("post")
    {
        "response".to_string()
    } else if lowered.contains("email") || lowered.contains("newsletter") {
        "email".to_string()
    } else {
        "other".to_string()
    }
}

/// Kebab-case slug for a free-form title. Strips punctuation, collapses
/// whitespace, and trims hyphens so the slug is safe to use as a
/// filename.
fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_hyphen = false;
    for ch in input.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_hyphen = false;
        } else if !prev_hyphen && !out.is_empty() {
            out.push('-');
            prev_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("untitled");
    }
    out
}

/// Render one of the capability outputs into a markdown string.
fn render_artifact_markdown(capability: &str, arguments: &Value, result: &Value) -> String {
    match capability {
        PLANNING_CAPTURE_BRIEF => {
            let brief = &result["brief"];
            let deliverables = brief["deliverables"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| format!("- {s}\n"))
                        .collect::<String>()
                })
                .unwrap_or_default();
            format!(
                "# {title}\n\n\
                 **Goal:** {goal}\n\n\
                 **Audience:** {audience}\n\n\
                 ## Deliverables ({count})\n\n{deliverables}\n\
                 ---\n\
                 _Captured by Ordo `planning.capture_brief`._\n",
                title = brief["title"].as_str().unwrap_or("Untitled Brief"),
                goal = brief["goal"].as_str().unwrap_or(""),
                audience = brief["audience"].as_str().unwrap_or("Ã¢â‚¬â€"),
                count = brief["deliverable_count"].as_u64().unwrap_or(0),
            )
        }
        PLANNING_PLAN_INITIATIVE => {
            let mut md = String::from("# Campaign Plan\n\n");
            if let Some(phases) = result["phases"].as_array() {
                for phase in phases {
                    let name = phase["phase"].as_str().unwrap_or("phase");
                    md.push_str(&format!("## {name}\n\n"));
                    if let Some(items) = phase["deliverables"].as_array() {
                        if items.is_empty() {
                            md.push_str("_(no deliverables assigned)_\n\n");
                        } else {
                            for item in items {
                                if let Some(s) = item.as_str() {
                                    md.push_str(&format!("- {s}\n"));
                                }
                            }
                            md.push('\n');
                        }
                    }
                }
            }
            md.push_str("---\n_Generated by `planning.plan_initiative`._\n");
            md
        }
        PLANNING_SUMMARIZE_DELIVERABLES => {
            let mut md = String::from("# Deliverables Summary\n\n");
            md.push_str(&format!(
                "Total: **{}**\n\n",
                result["total"].as_u64().unwrap_or(0)
            ));
            md.push_str("## By kind\n\n");
            if let Some(counts) = result["by_kind"].as_object() {
                for (kind, n) in counts {
                    md.push_str(&format!("- `{kind}` Ãƒâ€” {}\n", n));
                }
            }
            md.push_str("\n---\n_Generated by `planning.summarize_deliverables`._\n");
            md
        }
        ORCHESTRATION_SCHEDULE_RELEASE => {
            let mut md = String::from("# Release Schedule\n\n");
            md.push_str(&format!(
                "Target release: **{}**\n\n",
                result["release_date"].as_str().unwrap_or("(unset)")
            ));
            if let Some(schedule) = result["schedule"].as_array() {
                md.push_str("| Stage | Date |\n|---|---|\n");
                for entry in schedule {
                    md.push_str(&format!(
                        "| {} | {} |\n",
                        entry["stage"].as_str().unwrap_or(""),
                        entry["date"].as_str().unwrap_or("")
                    ));
                }
            }
            md.push_str("\n---\n_Generated by `orchestration.schedule_release`._\n");
            md
        }
        ORCHESTRATION_REQUEST_REVISION => {
            let rev = &result["revision_request"];
            format!(
                "# Revision Request\n\n\
                 **Stage:** {stage}\n\n\
                 **Reason:** {reason}\n\n\
                 **Due:** {due}\n\n\
                 ---\n_Generated by `orchestration.request_revision`._\n",
                stage = rev["stage"].as_str().unwrap_or(""),
                reason = rev["reason"].as_str().unwrap_or(""),
                due = rev["due"].as_str().unwrap_or("Ã¢â‚¬â€"),
            )
        }
        _ => {
            // Fallback for anything we don't have a dedicated template
            // for Ã¢â‚¬â€ dump the input + output as JSON.
            format!(
                "# {capability}\n\n## Arguments\n\n```json\n{}\n```\n\n## Result\n\n```json\n{}\n```\n",
                serde_json::to_string_pretty(arguments).unwrap_or_default(),
                serde_json::to_string_pretty(result).unwrap_or_default(),
            )
        }
    }
}

/// For artifact-producing capabilities: render a markdown file to
/// `<root>/<subdir>/<slug>.md` (or a sibling `.json` for pure-data JSON
/// manifests), and inject `artifact_path` into the result so callers can
/// link to it.
fn persist_artifact(
    root: &Path,
    capability: &str,
    arguments: &Value,
    result: &mut Value,
) -> std::io::Result<()> {
    let (subdir, slug_basis, extension): (&str, String, &str) = match capability {
        PLANNING_CAPTURE_BRIEF => (
            "briefs",
            arguments
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("brief")
                .to_string(),
            "md",
        ),
        PLANNING_PLAN_INITIATIVE => {
            let seed = arguments
                .get("title")
                .and_then(|v| v.as_str())
                .or_else(|| arguments.get("initiative").and_then(|v| v.as_str()))
                .unwrap_or("initiative-plan")
                .to_string();
            ("initiatives", seed, "md")
        }
        PLANNING_PACKAGE_RESOURCES => {
            let seed = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| arguments.get("input_directory").and_then(|v| v.as_str()))
                .unwrap_or("resources")
                .to_string();
            ("resources", seed, "json")
        }
        PLANNING_SUMMARIZE_DELIVERABLES => (
            "deliverables",
            arguments
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("deliverables-summary")
                .to_string(),
            "md",
        ),
        ORCHESTRATION_SCHEDULE_RELEASE => (
            "releases",
            arguments
                .get("title")
                .and_then(|v| v.as_str())
                .or_else(|| arguments.get("release_date").and_then(|v| v.as_str()))
                .unwrap_or("release")
                .to_string(),
            "md",
        ),
        ORCHESTRATION_REQUEST_REVISION => {
            let seed = arguments
                .get("stage")
                .and_then(|v| v.as_str())
                .unwrap_or("revision")
                .to_string();
            (
                "revisions",
                format!("{seed}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")),
                "md",
            )
        }
        _ => return Ok(()),
    };

    let slug = slugify(&slug_basis);
    let dir = root.join(subdir);
    std::fs::create_dir_all(&dir)?;
    let file_path = dir.join(format!("{slug}.{extension}"));

    let body = if extension == "json" {
        serde_json::to_string_pretty(result).unwrap_or_else(|_| "{}".into())
    } else {
        render_artifact_markdown(capability, arguments, result)
    };
    std::fs::write(&file_path, body)?;

    if let Some(object) = result.as_object_mut() {
        let relative = file_path
            .strip_prefix(root)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .replace('\\', "/");
        object.insert("artifact_path".into(), Value::String(relative));
        object.insert(
            "artifact_absolute_path".into(),
            Value::String(file_path.to_string_lossy().to_string()),
        );
    }
    Ok(())
}

pub struct InterfaceOpsProvider;

impl InterfaceOpsProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for InterfaceOpsProvider {
    fn default() -> Self {
        Self::new()
    }
}

const SSH_DESCRIBE_HOST: &str = "ssh.describe_host";
const SSH_PREPARE_COMMAND: &str = "ssh.prepare_command";
const SSH_SYNC_WORKSPACE: &str = "ssh.sync_workspace";
const API_DESCRIBE_CLIENT: &str = "api.describe_client";
const API_PREPARE_AUTH: &str = "api.prepare_auth";
const API_DISPATCH_WEBHOOK: &str = "api.dispatch_webhook";
const REST_DESCRIBE_ENDPOINT: &str = "rest.describe_endpoint";
const REST_PREPARE_REQUEST: &str = "rest.prepare_request";
const REST_VALIDATE_RESPONSE: &str = "rest.validate_response";
const REST_SYNC_RESOURCE: &str = "rest.sync_resource";

const INTERFACE_OPS_CAPABILITIES: &[&str] = &[
    SSH_DESCRIBE_HOST,
    SSH_PREPARE_COMMAND,
    SSH_SYNC_WORKSPACE,
    API_DESCRIBE_CLIENT,
    API_PREPARE_AUTH,
    API_DISPATCH_WEBHOOK,
    REST_DESCRIBE_ENDPOINT,
    REST_PREPARE_REQUEST,
    REST_VALIDATE_RESPONSE,
    REST_SYNC_RESOURCE,
];

fn interface_ops_description(capability: &str) -> &'static str {
    match capability {
        SSH_DESCRIBE_HOST => {
            "Describes a remote host target: user, host, port, and identity hints."
        }
        SSH_PREPARE_COMMAND => {
            "Prepares a remote command plan for an SSH host without executing it."
        }
        SSH_SYNC_WORKSPACE => "Plans a workspace sync between local and remote paths over SSH.",
        API_DESCRIBE_CLIENT => {
            "Describes an external API client: base URL, auth style, and scopes."
        }
        API_PREPARE_AUTH => "Prepares an auth refresh descriptor for an API client.",
        API_DISPATCH_WEBHOOK => "Prepares a webhook dispatch payload for an API client.",
        REST_DESCRIBE_ENDPOINT => "Describes a REST endpoint: method, path, and resource kind.",
        REST_PREPARE_REQUEST => {
            "Prepares a REST request body/headers against an endpoint description."
        }
        REST_VALIDATE_RESPONSE => {
            "Validates a REST response against a declared status and required fields."
        }
        REST_SYNC_RESOURCE => {
            "Plans a REST resource sync (fetch then write) between two endpoints."
        }
        _ => "Interface Ops capability.",
    }
}

#[async_trait]
impl CapabilityProvider for InterfaceOpsProvider {
    fn name(&self) -> &str {
        "interface-ops"
    }

    fn capabilities(&self) -> Vec<String> {
        INTERFACE_OPS_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        INTERFACE_OPS_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    interface_ops_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let result = match capability {
            SSH_DESCRIBE_HOST => describe_ssh_host(arguments),
            SSH_PREPARE_COMMAND => prepare_ssh_command(arguments),
            SSH_SYNC_WORKSPACE => sync_ssh_workspace(arguments),
            API_DESCRIBE_CLIENT => describe_api_client(arguments),
            API_PREPARE_AUTH => prepare_api_auth(arguments),
            API_DISPATCH_WEBHOOK => dispatch_api_webhook(arguments),
            REST_DESCRIBE_ENDPOINT => describe_rest_endpoint(arguments),
            REST_PREPARE_REQUEST => prepare_rest_request(arguments),
            REST_VALIDATE_RESPONSE => validate_rest_response(arguments),
            REST_SYNC_RESOURCE => sync_rest_resource(arguments),
            _ => return None,
        };
        Some(match result {
            Ok(mut value) => {
                attach_context_to_output(&mut value, arguments);
                ToolCallResult::Completed { result: value }
            }
            Err(error) => ToolCallResult::Failed { error },
        })
    }
}

fn describe_ssh_host(arguments: &Value) -> Result<Value, String> {
    let host = require_string(arguments, "host")?;
    let user = optional_string(arguments, "user").unwrap_or_else(|| "root".to_string());
    let port = arguments
        .get("port")
        .and_then(|value| value.as_u64())
        .unwrap_or(22);
    let identity = optional_string(arguments, "identity");
    Ok(json!({
        "ssh_host": {
            "user": user,
            "host": host,
            "port": port,
            "identity": identity,
            "target": format!("{user}@{host}:{port}"),
        },
    }))
}

fn prepare_ssh_command(arguments: &Value) -> Result<Value, String> {
    let host = require_string(arguments, "host")?;
    let command = require_string(arguments, "command")?;
    let user = optional_string(arguments, "user").unwrap_or_else(|| "root".to_string());
    let port = arguments
        .get("port")
        .and_then(|value| value.as_u64())
        .unwrap_or(22);
    let working_dir = optional_string(arguments, "working_dir");
    let composed = match &working_dir {
        Some(dir) => format!("cd {dir} && {command}"),
        None => command.clone(),
    };
    Ok(json!({
        "ssh_command": {
            "target": format!("{user}@{host}:{port}"),
            "working_dir": working_dir,
            "command": command,
            "composed": composed,
        },
    }))
}

fn sync_ssh_workspace(arguments: &Value) -> Result<Value, String> {
    let host = require_string(arguments, "host")?;
    let local_path = require_string(arguments, "local_path")?;
    let remote_path = require_string(arguments, "remote_path")?;
    let direction = optional_string(arguments, "direction").unwrap_or_else(|| "push".to_string());
    if direction != "push" && direction != "pull" {
        return Err(format!(
            "unknown sync direction '{direction}' (expected 'push' or 'pull')"
        ));
    }
    let user = optional_string(arguments, "user").unwrap_or_else(|| "root".to_string());
    Ok(json!({
        "ssh_sync": {
            "direction": direction,
            "local_path": local_path,
            "remote_path": remote_path,
            "target": format!("{user}@{host}"),
        },
    }))
}

fn describe_api_client(arguments: &Value) -> Result<Value, String> {
    let name = require_string(arguments, "name")?;
    let base_url = require_string(arguments, "base_url")?;
    let auth_style =
        optional_string(arguments, "auth_style").unwrap_or_else(|| "bearer".to_string());
    let scopes = optional_string_array(arguments, "scopes");
    Ok(json!({
        "api_client": {
            "name": name,
            "base_url": base_url,
            "auth_style": auth_style,
            "scopes": scopes,
            "scope_count": scopes.len(),
        },
    }))
}

fn prepare_api_auth(arguments: &Value) -> Result<Value, String> {
    let client = require_string(arguments, "client")?;
    let auth_style =
        optional_string(arguments, "auth_style").unwrap_or_else(|| "bearer".to_string());
    let refresh_url = optional_string(arguments, "refresh_url");
    let steps = match auth_style.as_str() {
        "bearer" => vec![
            "load_refresh_token",
            "exchange_for_access_token",
            "cache_token",
        ],
        "basic" => vec!["load_credentials", "compose_basic_header"],
        "api_key" => vec!["load_api_key", "set_header"],
        "oauth2" => vec![
            "load_refresh_token",
            "post_refresh_request",
            "parse_token_response",
            "cache_token",
        ],
        other => {
            return Err(format!("unknown auth style '{other}'"));
        }
    };
    Ok(json!({
        "api_auth": {
            "client": client,
            "auth_style": auth_style,
            "refresh_url": refresh_url,
            "steps": steps,
        },
    }))
}

fn dispatch_api_webhook(arguments: &Value) -> Result<Value, String> {
    let client = require_string(arguments, "client")?;
    let event = require_string(arguments, "event")?;
    let payload = arguments.get("payload").cloned().unwrap_or(json!({}));
    let target = optional_string(arguments, "target");
    Ok(json!({
        "api_webhook": {
            "client": client,
            "event": event,
            "target": target,
            "payload": payload,
        },
    }))
}

fn describe_rest_endpoint(arguments: &Value) -> Result<Value, String> {
    let method = require_string(arguments, "method")?;
    let path = require_string(arguments, "path")?;
    let resource =
        optional_string(arguments, "resource").unwrap_or_else(|| infer_rest_resource(&path));
    let method_upper = method.to_ascii_uppercase();
    const ALLOWED: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    if !ALLOWED.iter().any(|m| *m == method_upper) {
        return Err(format!("unsupported HTTP method '{method}'"));
    }
    Ok(json!({
        "rest_endpoint": {
            "method": method_upper,
            "path": path,
            "resource": resource,
        },
    }))
}

fn prepare_rest_request(arguments: &Value) -> Result<Value, String> {
    let method = require_string(arguments, "method")?;
    let path = require_string(arguments, "path")?;
    let body = arguments.get("body").cloned().unwrap_or(json!({}));
    let headers = arguments
        .get("headers")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let method_upper = method.to_ascii_uppercase();
    let has_body = matches!(method_upper.as_str(), "POST" | "PUT" | "PATCH");
    Ok(json!({
        "rest_request": {
            "method": method_upper,
            "path": path,
            "headers": headers,
            "body": if has_body { body } else { Value::Null },
            "has_body": has_body,
        },
    }))
}

fn validate_rest_response(arguments: &Value) -> Result<Value, String> {
    let status = arguments
        .get("status")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "missing required numeric field 'status'".to_string())?;
    let expected_status = arguments
        .get("expected_status")
        .and_then(|value| value.as_u64())
        .unwrap_or(200);
    let required_fields = optional_string_array(arguments, "required_fields");
    let body = arguments.get("body").cloned().unwrap_or(json!({}));
    let mut issues = Vec::new();
    if status != expected_status {
        issues.push(format!(
            "status {status} did not match expected {expected_status}"
        ));
    }
    if let Some(object) = body.as_object() {
        for field in &required_fields {
            if !object.contains_key(field) {
                issues.push(format!("missing required field '{field}'"));
            }
        }
    } else if !required_fields.is_empty() {
        issues.push("body is not an object; required fields cannot be checked".to_string());
    }
    Ok(json!({
        "valid": issues.is_empty(),
        "issues": issues,
        "status": status,
        "expected_status": expected_status,
    }))
}

fn sync_rest_resource(arguments: &Value) -> Result<Value, String> {
    let source = require_string(arguments, "source")?;
    let target = require_string(arguments, "target")?;
    let resource = optional_string(arguments, "resource")
        .unwrap_or_else(|| infer_rest_resource(&source.clone()));
    let direction = optional_string(arguments, "direction").unwrap_or_else(|| "pull".to_string());
    if direction != "pull" && direction != "push" && direction != "mirror" {
        return Err(format!(
            "unknown rest sync direction '{direction}' (expected 'pull', 'push', or 'mirror')"
        ));
    }
    let steps = match direction.as_str() {
        "pull" => vec!["GET source", "transform", "PUT target"],
        "push" => vec!["GET target", "compare", "POST source"],
        "mirror" => vec!["GET source", "GET target", "diff", "apply both"],
        _ => unreachable!(),
    };
    Ok(json!({
        "rest_sync": {
            "source": source,
            "target": target,
            "resource": resource,
            "direction": direction,
            "steps": steps,
        },
    }))
}

fn infer_rest_resource(path: &str) -> String {
    path.rsplit('/')
        .find(|segment| {
            !segment.is_empty() && !segment.starts_with(':') && !segment.starts_with('{')
        })
        .unwrap_or("resource")
        .to_string()
}

/// Capability provider that wires real outbound cloud calls onto the bus.
/// Credentials live in the local SQLite store via `CloudCredentialTask`.
/// When a service has no stored credential, capabilities return a
/// structured `not_configured` error rather than panicking, matching the
/// "local-first, not local-only" contract.
pub struct CloudOpsProvider {
    credentials: ordo_cloud::CloudCredentialTask,
    http: ordo_cloud::CloudHttp,
}

impl CloudOpsProvider {
    pub fn new(credentials: ordo_cloud::CloudCredentialTask) -> Self {
        Self {
            credentials,
            http: ordo_cloud::CloudHttp::new(),
        }
    }

    pub fn with_http(
        credentials: ordo_cloud::CloudCredentialTask,
        http: ordo_cloud::CloudHttp,
    ) -> Self {
        Self { credentials, http }
    }
}

const CLOUD_OPENAI_CHAT: &str = "cloud.openai.chat";
const CLOUD_OPENAI_EMBED: &str = "cloud.openai.embed";
const CLOUD_ANTHROPIC_MESSAGES: &str = "cloud.anthropic.messages";
const CLOUD_REST_REQUEST: &str = "cloud.rest.request";
const CLOUD_CREDENTIALS_LIST: &str = "cloud.credentials.list";
const CLOUD_CREDENTIALS_TEST: &str = "cloud.credentials.test";
const CLOUD_CREDENTIALS_MODELS: &str = "cloud.credentials.models";
const CLOUD_CREDENTIALS_UPSERT: &str = "cloud.credentials.upsert";
const CLOUD_CREDENTIALS_DELETE: &str = "cloud.credentials.delete";

const CLOUD_OPS_CAPABILITIES: &[&str] = &[
    CLOUD_OPENAI_CHAT,
    CLOUD_OPENAI_EMBED,
    CLOUD_ANTHROPIC_MESSAGES,
    CLOUD_REST_REQUEST,
    CLOUD_CREDENTIALS_LIST,
    CLOUD_CREDENTIALS_TEST,
    CLOUD_CREDENTIALS_MODELS,
    CLOUD_CREDENTIALS_UPSERT,
    CLOUD_CREDENTIALS_DELETE,
];

fn cloud_ops_description(capability: &str) -> &'static str {
    match capability {
        CLOUD_OPENAI_CHAT => {
            "Calls OpenAI chat/completions using a configured `openai` credential."
        }
        CLOUD_OPENAI_EMBED => "Calls OpenAI embeddings using a configured `openai` credential.",
        CLOUD_ANTHROPIC_MESSAGES => {
            "Calls Anthropic /messages using a configured `anthropic` credential."
        }
        CLOUD_REST_REQUEST => {
            "Sends an authenticated REST request against any configured cloud service."
        }
        CLOUD_CREDENTIALS_LIST => "Lists stored cloud credentials with secrets redacted.",
        CLOUD_CREDENTIALS_TEST => {
            "Tests one stored cloud credential and returns a redacted pass/fail status."
        }
        CLOUD_CREDENTIALS_MODELS => {
            "Discovers model identifiers exposed by one stored cloud credential."
        }
        CLOUD_CREDENTIALS_UPSERT => "Creates or updates a stored cloud credential.",
        CLOUD_CREDENTIALS_DELETE => "Deletes a stored cloud credential by service name.",
        _ => "Cloud Ops capability.",
    }
}

async fn run_cloud_tool_call(
    provider: &CloudOpsProvider,
    capability: &str,
    arguments: &Value,
) -> Option<ToolCallResult> {
    let result = match capability {
        CLOUD_OPENAI_CHAT => {
            cloud_service_call(provider, "openai", arguments, |http, cred, args| {
                Box::pin(async move { ordo_cloud::openai::chat(http, cred, &args).await })
            })
            .await
        }
        CLOUD_OPENAI_EMBED => {
            cloud_service_call(provider, "openai", arguments, |http, cred, args| {
                Box::pin(async move { ordo_cloud::openai::embed(http, cred, &args).await })
            })
            .await
        }
        CLOUD_ANTHROPIC_MESSAGES => {
            cloud_service_call(provider, "anthropic", arguments, |http, cred, args| {
                Box::pin(async move { ordo_cloud::anthropic::messages(http, cred, &args).await })
            })
            .await
        }
        CLOUD_REST_REQUEST => cloud_rest_request(provider, arguments).await,
        CLOUD_CREDENTIALS_LIST => cloud_credentials_list(provider).await,
        CLOUD_CREDENTIALS_TEST => cloud_credentials_test(provider, arguments).await,
        CLOUD_CREDENTIALS_MODELS => cloud_credentials_models(provider, arguments).await,
        CLOUD_CREDENTIALS_UPSERT => cloud_credentials_upsert(provider, arguments).await,
        CLOUD_CREDENTIALS_DELETE => cloud_credentials_delete(provider, arguments).await,
        _ => return None,
    };
    Some(match result {
        Ok(mut value) => {
            attach_context_to_output(&mut value, arguments);
            ToolCallResult::Completed { result: value }
        }
        Err(error) => ToolCallResult::Failed { error },
    })
}

type CloudFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = ordo_cloud::CloudResult<Value>> + Send + 'a>,
>;

async fn cloud_service_call<F>(
    provider: &CloudOpsProvider,
    kind: &str,
    arguments: &Value,
    call: F,
) -> Result<Value, String>
where
    F: for<'a> FnOnce(
        &'a ordo_cloud::CloudHttp,
        &'a ordo_cloud::CloudCredential,
        Value,
    ) -> CloudFuture<'a>,
{
    // Provider-neutral credential resolution. The `kind` arg ("openai",
    // "anthropic", â€¦) is a HINT, not a service-name lookup: it says
    // which wire shape the caller expects. We walk in this order:
    //   1. an explicit `credential` arg (per-call override)
    //   2. a credential keyed under the kind name (legacy callers + the
    //      common case where someone configured "openai" or "anthropic"
    //      under that exact key)
    //   3. any configured credential whose `auth_style` is compatible
    //      with the kind (OpenAI-shape: anything except "anthropic";
    //      Anthropic-shape: only "anthropic")
    // This means a single Ollama / LM Studio / OpenRouter credential
    // configured under any service name still satisfies cloud.openai.chat.
    let explicit = arguments
        .get("credential")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let mut tried: Vec<String> = Vec::new();
    let mut credential: Option<ordo_cloud::CloudCredential> = None;

    async fn try_named(
        provider: &CloudOpsProvider,
        name: String,
        tried: &mut Vec<String>,
    ) -> Option<ordo_cloud::CloudCredential> {
        if tried.iter().any(|n| n == &name) {
            return None;
        }
        tried.push(name.clone());
        provider.credentials.get(name).await.ok().flatten()
    }

    if let Some(s) = explicit {
        if let Some(named) = try_named(provider, s.clone(), &mut tried).await {
            if !named.enabled() {
                return Err(format!(
                    "credential for service '{s}' is paused; enable it in the Provider tab before use"
                ));
            }
            credential = Some(named);
        }
    }
    if credential.is_none() {
        credential = try_named(provider, kind.to_string(), &mut tried)
            .await
            .filter(ordo_cloud::CloudCredential::enabled);
    }
    if credential.is_none() {
        let kind_is_anthropic = kind == "anthropic";
        if let Ok(all) = provider.credentials.list().await {
            for cred in all {
                if !cred.enabled() {
                    continue;
                }
                let cred_is_anthropic = cred.auth_style == "anthropic";
                if cred_is_anthropic == kind_is_anthropic {
                    credential = Some(cred);
                    break;
                }
            }
        }
    }
    let credential = credential.ok_or_else(|| {
        format!(
            "no compatible credential configured for kind '{kind}'; \
             configure one in the Cloud tab or via cloud.credentials.upsert"
        )
    })?;

    // If the caller didn't specify `model`, surface the credential's
    // extras.model â€” set in the Cloud tab's Configure modal â€” so local
    // OpenAI-compatible servers (Ollama, LM Studio) route to whichever
    // model the operator has loaded instead of the cloud-provider
    // default like `gpt-4o-mini`.
    let mut args = arguments.clone();
    if args.get("model").is_none() {
        if let Some(model) = credential.extras.get("model") {
            if let Some(obj) = args.as_object_mut() {
                obj.insert("model".to_string(), json!(model));
            }
        }
    }

    call(&provider.http, &credential, args)
        .await
        .map_err(|err| err.to_string())
}

async fn cloud_rest_request(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    let service = arguments
        .get("service")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required field 'service'".to_string())?
        .to_string();
    let method = arguments
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let url = arguments
        .get("url")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required field 'url' (absolute or relative path)".to_string())?
        .to_string();
    let credential = match provider.credentials.get(service.clone()).await {
        Ok(Some(credential)) => credential,
        Ok(None) => {
            return Err(format!(
                "credential for service '{service}' is not configured"
            ));
        }
        Err(err) => return Err(err.to_string()),
    };
    if !credential.enabled() {
        return Err(format!(
            "credential for service '{service}' is paused; enable it in the Provider tab before use"
        ));
    }
    let body = arguments.get("body").cloned();
    let headers = ordo_cloud::headers_from_value(arguments.get("headers"));
    let method = match method.as_str() {
        "GET" => ordo_cloud::Method::GET,
        "POST" => ordo_cloud::Method::POST,
        "PUT" => ordo_cloud::Method::PUT,
        "PATCH" => ordo_cloud::Method::PATCH,
        "DELETE" => ordo_cloud::Method::DELETE,
        other => return Err(format!("unsupported HTTP method '{other}'")),
    };
    let response = provider
        .http
        .send_json(&credential, method, &url, body.as_ref(), &headers)
        .await
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "service": service,
        "url": url,
        "response": response,
    }))
}

async fn cloud_credentials_list(provider: &CloudOpsProvider) -> Result<Value, String> {
    let credentials = provider
        .credentials
        .list()
        .await
        .map_err(|err| err.to_string())?;
    let redacted: Vec<Value> = credentials
        .iter()
        .map(ordo_cloud::CloudCredential::redacted)
        .collect();
    Ok(json!({
        "count": redacted.len(),
        "credentials": redacted,
    }))
}

async fn cloud_credential_for_read(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<ordo_cloud::CloudCredential, String> {
    if let Some(service) = arguments
        .get("service")
        .or_else(|| arguments.get("credential"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return provider
            .credentials
            .get(service.to_string())
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("credential for service '{service}' is not configured"));
    }

    let credentials = provider
        .credentials
        .list()
        .await
        .map_err(|err| err.to_string())?;
    match credentials.as_slice() {
        [credential] => Ok(credential.clone()),
        [] => Err("no cloud credentials are configured".into()),
        _ => Err(
            "service or credential must be provided when multiple credentials are configured"
                .into(),
        ),
    }
}

/// The service name the caller asked about, echoed back into the
/// `{ok:false, ...}` envelopes for the test/models tools so the studio
/// can label which provider failed even when no credential row exists.
fn requested_service(arguments: &Value) -> Value {
    arguments
        .get("service")
        .or_else(|| arguments.get("credential"))
        .cloned()
        .unwrap_or(Value::Null)
}

async fn cloud_credentials_test(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    // A missing / ambiguous credential is an expected operator state,
    // not a server fault — return a clean {ok:false,error} (HTTP 200)
    // so the studio surfaces the reason instead of a bare 500.
    let credential = match cloud_credential_for_read(provider, arguments).await {
        Ok(credential) => credential,
        Err(error) => {
            return Ok(json!({
                "service": requested_service(arguments),
                "ok": false,
                "error": error,
            }));
        }
    };
    let service = credential.service.clone();
    if !credential.enabled() {
        return Ok(json!({
            "service": service,
            "ok": false,
            "error": "credential is paused",
            "credential": credential.redacted(),
        }));
    }

    match ordo_cloud::test_credential(&provider.http, &credential).await {
        Ok(()) => Ok(json!({
            "service": service,
            "ok": true,
            "error": null,
            "credential": credential.redacted(),
        })),
        Err(error) => Ok(json!({
            "service": service,
            "ok": false,
            "error": error,
            "credential": credential.redacted(),
        })),
    }
}

async fn cloud_credentials_models(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    // Same graceful-degradation contract as cloud_credentials_test: an
    // unconfigured / ambiguous service yields {ok:false,error} rather
    // than a 500, so "Discover Models" shows a readable message.
    let credential = match cloud_credential_for_read(provider, arguments).await {
        Ok(credential) => credential,
        Err(error) => {
            return Ok(json!({
                "service": requested_service(arguments),
                "ok": false,
                "error": error,
                "count": 0,
                "models": [],
            }));
        }
    };
    let service = credential.service.clone();
    if !credential.enabled() {
        return Ok(json!({
            "service": service,
            "ok": false,
            "error": "credential is paused",
            "count": 0,
            "models": [],
            "credential": credential.redacted(),
        }));
    }

    match ordo_cloud::list_models(&provider.http, &credential).await {
        Ok(models) => Ok(json!({
            "service": service,
            "ok": true,
            "error": null,
            "count": models.len(),
            "models": models,
            "credential": credential.redacted(),
        })),
        Err(error) => Ok(json!({
            "service": service,
            "ok": false,
            "error": error,
            "count": 0,
            "models": [],
            "credential": credential.redacted(),
        })),
    }
}

async fn cloud_credentials_upsert(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    let service = arguments
        .get("service")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required field 'service'".to_string())?
        .to_string();
    let update = ordo_cloud::CloudCredentialUpdate {
        service,
        label: arguments
            .get("label")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        auth_style: arguments
            .get("auth_style")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        secret: arguments
            .get("secret")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        base_url: arguments
            .get("base_url")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        extras: arguments
            .get("extras")
            .and_then(|value| value.as_object())
            .map(|object| {
                object
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            }),
    };
    let credential = provider
        .credentials
        .upsert(update)
        .await
        .map_err(|err| err.to_string())?;
    Ok(json!({ "credential": credential.redacted() }))
}

async fn cloud_credentials_delete(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    let service = arguments
        .get("service")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required field 'service'".to_string())?
        .to_string();
    let removed = provider
        .credentials
        .delete(service.clone())
        .await
        .map_err(|err| err.to_string())?;
    Ok(json!({ "service": service, "removed": removed }))
}

#[async_trait]
impl CapabilityProvider for CloudOpsProvider {
    fn name(&self) -> &str {
        "cloud-ops"
    }

    fn capabilities(&self) -> Vec<String> {
        CLOUD_OPS_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        CLOUD_OPS_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    cloud_ops_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        run_cloud_tool_call(self, capability, arguments).await
    }
}

#[derive(Debug, Clone)]
pub struct RuntimePolicySnapshot {
    pub profile: String,
    pub control_api_bind: Option<String>,
    pub rag_enabled: bool,
    pub knowledge_enabled: bool,
    pub rag_activation_mode: String,
    pub knowledge_activation_mode: String,
    pub rag_budget_bytes: usize,
    pub memory_working_budget_bytes: usize,
    pub memory_pinned_budget_bytes: usize,
    pub self_heal_history_budget_bytes: usize,
    pub self_heal_llama_cpp_binary: Option<String>,
    pub self_heal_model_path: Option<String>,
    pub self_heal_model_context_size: usize,
    pub self_heal_model_max_tokens: usize,
    pub self_heal_model_temperature: f32,
    pub llama_cpp_configured: bool,
    pub embedding_backend: String,
    pub embedding_dimensions: usize,
    pub embedding_llama_cpp_binary: Option<String>,
    pub embedding_model_path: Option<String>,
    pub embedding_context_size: usize,
}

#[derive(Clone)]
pub struct RuntimeInfoProvider {
    snapshot: RuntimePolicySnapshot,
    settings_task: Option<RuntimeSettingsTask>,
}

impl RuntimeInfoProvider {
    pub fn new(snapshot: RuntimePolicySnapshot) -> Self {
        Self {
            snapshot,
            settings_task: None,
        }
    }

    pub fn with_settings_task(
        snapshot: RuntimePolicySnapshot,
        settings_task: RuntimeSettingsTask,
    ) -> Self {
        Self {
            snapshot,
            settings_task: Some(settings_task),
        }
    }

    pub fn with_settings_path(snapshot: RuntimePolicySnapshot, settings_path: PathBuf) -> Self {
        let settings_task = RuntimeSettingsTask::open(settings_path)
            .expect("open runtime settings task for runtime info provider");
        Self::with_settings_task(snapshot, settings_task)
    }

    fn supports_settings_management(&self) -> bool {
        self.settings_task.is_some()
    }

    async fn load_persisted_settings(&self) -> Result<Value, String> {
        let Some(settings_task) = &self.settings_task else {
            return Ok(json!({
                "profile": Value::Null,
                "rag_budget_bytes": Value::Null,
                "memory_working_budget_bytes": Value::Null,
                "memory_pinned_budget_bytes": Value::Null,
                "self_heal_history_budget_bytes": Value::Null,
                "self_heal_llama_cpp_binary": Value::Null,
                "self_heal_model_path": Value::Null,
                "self_heal_model_context_size": Value::Null,
                "self_heal_model_max_tokens": Value::Null,
                "self_heal_model_temperature": Value::Null,
                "embedding_llama_cpp_binary": Value::Null,
                "embedding_model_path": Value::Null,
                "embedding_dimensions": Value::Null,
                "embedding_context_size": Value::Null,
            }));
        };

        let settings = settings_task
            .load()
            .await
            .map_err(|err| format!("failed to load runtime settings: {err}"))?;
        Ok(runtime_settings_json(&settings))
    }

    async fn persist_settings_update(&self, arguments: &Value) -> Result<Value, String> {
        let Some(settings_task) = &self.settings_task else {
            return Err("runtime settings persistence is not configured".to_string());
        };

        let profile = arguments
            .get("profile")
            .map(parse_runtime_profile_argument)
            .transpose()?;
        let rag_budget_bytes = parse_runtime_budget_argument(arguments, "rag_budget_bytes")?;
        let memory_working_budget_bytes =
            parse_runtime_budget_argument(arguments, "memory_working_budget_bytes")?;
        let memory_pinned_budget_bytes =
            parse_runtime_budget_argument(arguments, "memory_pinned_budget_bytes")?;
        let self_heal_history_budget_bytes =
            parse_runtime_budget_argument(arguments, "self_heal_history_budget_bytes")?;
        let self_heal_llama_cpp_binary =
            parse_runtime_optional_string_argument(arguments, "self_heal_llama_cpp_binary")?;
        let self_heal_model_path =
            parse_runtime_optional_string_argument(arguments, "self_heal_model_path")?;
        let self_heal_model_context_size =
            parse_runtime_budget_argument(arguments, "self_heal_model_context_size")?;
        let self_heal_model_max_tokens =
            parse_runtime_budget_argument(arguments, "self_heal_model_max_tokens")?;
        let self_heal_model_temperature =
            parse_runtime_f32_string_argument(arguments, "self_heal_model_temperature")?;

        let embedding_llama_cpp_binary =
            parse_runtime_optional_string_argument(arguments, "embedding_llama_cpp_binary")?;
        let embedding_model_path =
            parse_runtime_optional_string_argument(arguments, "embedding_model_path")?;
        let embedding_dimensions =
            parse_runtime_budget_argument(arguments, "embedding_dimensions")?;
        let embedding_context_size =
            parse_runtime_budget_argument(arguments, "embedding_context_size")?;

        let update = RuntimeSettingsUpdate {
            profile,
            rag_budget_bytes,
            memory_working_budget_bytes,
            memory_pinned_budget_bytes,
            self_heal_history_budget_bytes,
            self_heal_llama_cpp_binary,
            self_heal_model_path,
            self_heal_model_context_size,
            self_heal_model_max_tokens,
            self_heal_model_temperature,
            embedding_llama_cpp_binary,
            embedding_model_path,
            embedding_dimensions,
            embedding_context_size,
        };

        if update == RuntimeSettingsUpdate::default() {
            return Err("no runtime settings fields were provided".to_string());
        }

        let persisted = settings_task
            .update(update)
            .await
            .map_err(|err| format!("failed to update runtime settings: {err}"))?;

        Ok(json!({
            "persisted": runtime_settings_json(&persisted),
            "restart_required": true,
        }))
    }
}

#[async_trait]
impl CapabilityProvider for RuntimeInfoProvider {
    fn name(&self) -> &str {
        "runtime"
    }

    fn capabilities(&self) -> Vec<String> {
        let mut capabilities = vec![
            "runtime.describe_profile".to_string(),
            "runtime.describe_storage".to_string(),
        ];
        if self.supports_settings_management() {
            capabilities.push("runtime.describe_settings".to_string());
            capabilities.push("runtime.update_settings".to_string());
        }
        capabilities
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        let mut descriptors = vec![
            CapabilityDescriptor::new(
                "runtime.describe_profile",
                self.name(),
                "Reports the active runtime profile and which optional lanes are enabled.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "runtime.describe_storage",
                self.name(),
                "Reports the current storage and self-heal retention budgets.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
        ];
        if self.supports_settings_management() {
            descriptors.push(CapabilityDescriptor::new(
                "runtime.describe_settings",
                self.name(),
                "Reports persisted runtime settings that a future UI can manage.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ));
            descriptors.push(CapabilityDescriptor::new(
                "runtime.update_settings",
                self.name(),
                "Persists runtime profile and storage settings for the next restart.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ));
        }
        descriptors
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        let lowered = requirement.to_ascii_lowercase();
        if self.supports_settings_management()
            && (lowered.contains("update runtime settings")
                || lowered.contains("change runtime profile")
                || lowered.contains("save storage budget"))
        {
            Some(CapabilityMatch {
                capability: "runtime.update_settings".to_string(),
                description: "Persists runtime profile and storage settings for the next restart."
                    .to_string(),
            })
        } else if self.supports_settings_management()
            && (lowered.contains("runtime settings")
                || lowered.contains("storage settings")
                || lowered.contains("settings ui"))
        {
            Some(CapabilityMatch {
                capability: "runtime.describe_settings".to_string(),
                description: "Reports persisted runtime settings for UI and restart planning."
                    .to_string(),
            })
        } else if lowered.contains("runtime profile") || lowered.contains("runtime mode") {
            Some(CapabilityMatch {
                capability: "runtime.describe_profile".to_string(),
                description: "Reports the active runtime profile and enabled capability lanes."
                    .to_string(),
            })
        } else if lowered.contains("storage budget")
            || lowered.contains("memory budget")
            || lowered.contains("rag budget")
        {
            Some(CapabilityMatch {
                capability: "runtime.describe_storage".to_string(),
                description: "Reports current storage and retention budgets.".to_string(),
            })
        } else {
            None
        }
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        match capability {
            "runtime.describe_profile" => Some(ToolCallResult::Completed {
                result: json!({
                    "profile": self.snapshot.profile,
                    "control_api_bind": self.snapshot.control_api_bind,
                    "control_api_enabled": self.snapshot.control_api_bind.is_some(),
                    "rag_enabled": self.snapshot.rag_enabled,
                    "knowledge_enabled": self.snapshot.knowledge_enabled,
                    "rag_activation_mode": self.snapshot.rag_activation_mode,
                    "knowledge_activation_mode": self.snapshot.knowledge_activation_mode,
                    "llama_cpp_configured": self.snapshot.llama_cpp_configured,
                    "embedding_backend": self.snapshot.embedding_backend,
                    "embedding_dimensions": self.snapshot.embedding_dimensions,
                }),
            }),
            "runtime.describe_storage" => Some(ToolCallResult::Completed {
                result: json!({
                    "rag_budget_bytes": self.snapshot.rag_budget_bytes,
                    "memory_working_budget_bytes": self.snapshot.memory_working_budget_bytes,
                    "memory_pinned_budget_bytes": self.snapshot.memory_pinned_budget_bytes,
                    "self_heal_history_budget_bytes": self.snapshot.self_heal_history_budget_bytes,
                    "self_heal_model_context_size": self.snapshot.self_heal_model_context_size,
                    "self_heal_model_max_tokens": self.snapshot.self_heal_model_max_tokens,
                    "self_heal_model_temperature": rounded_runtime_float(
                        self.snapshot.self_heal_model_temperature,
                    ),
                }),
            }),
            "runtime.describe_settings" => match self.load_persisted_settings().await {
                Ok(persisted) => Some(ToolCallResult::Completed {
                    result: json!({
                        "effective": {
                            "profile": self.snapshot.profile,
                            "control_api_bind": self.snapshot.control_api_bind,
                            "control_api_enabled": self.snapshot.control_api_bind.is_some(),
                            "rag_enabled": self.snapshot.rag_enabled,
                            "knowledge_enabled": self.snapshot.knowledge_enabled,
                            "rag_activation_mode": self.snapshot.rag_activation_mode,
                            "knowledge_activation_mode": self.snapshot.knowledge_activation_mode,
                            "rag_budget_bytes": self.snapshot.rag_budget_bytes,
                            "memory_working_budget_bytes": self.snapshot.memory_working_budget_bytes,
                            "memory_pinned_budget_bytes": self.snapshot.memory_pinned_budget_bytes,
                            "self_heal_history_budget_bytes": self.snapshot.self_heal_history_budget_bytes,
                            "self_heal_llama_cpp_binary": self.snapshot.self_heal_llama_cpp_binary,
                            "self_heal_model_path": self.snapshot.self_heal_model_path,
                            "self_heal_model_context_size": self.snapshot.self_heal_model_context_size,
                            "self_heal_model_max_tokens": self.snapshot.self_heal_model_max_tokens,
                            "self_heal_model_temperature": rounded_runtime_float(
                                self.snapshot.self_heal_model_temperature,
                            ),
                            "llama_cpp_configured": self.snapshot.llama_cpp_configured,
                            "embedding_backend": self.snapshot.embedding_backend,
                            "embedding_dimensions": self.snapshot.embedding_dimensions,
                            "embedding_llama_cpp_binary": self.snapshot.embedding_llama_cpp_binary,
                            "embedding_model_path": self.snapshot.embedding_model_path,
                            "embedding_context_size": self.snapshot.embedding_context_size,
                        },
                        "persisted": persisted,
                        "restart_required_for_changes": true,
                    }),
                }),
                Err(error) => Some(ToolCallResult::Failed { error }),
            },
            "runtime.update_settings" => match self.persist_settings_update(arguments).await {
                Ok(result) => Some(ToolCallResult::Completed { result }),
                Err(error) => Some(ToolCallResult::Failed { error }),
            },
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct SelfHealToolsProvider {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
    store: SelfHealStorageTask,
}

impl SelfHealToolsProvider {
    pub fn new(store: SelfHealStorageTask, bus: Arc<dyn Bus>) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
            store,
        }
    }

    async fn list_cases(&self, limit: usize) -> Result<Value, String> {
        let cases = self
            .store
            .list_cases(limit)
            .await
            .map_err(|err| format!("failed to list self-heal cases: {err}"))?;
        Ok(json!({
            "count": cases.len(),
            "results": cases
                .into_iter()
                .map(|case| self.case_json(&case))
                .collect::<Vec<_>>(),
        }))
    }

    async fn forget_case(&self, fingerprint: &str) -> Result<Value, String> {
        let removed = self
            .store
            .forget_case(fingerprint.to_string())
            .await
            .map_err(|err| format!("failed to forget self-heal case: {err}"))?;
        Ok(json!({
            "fingerprint": fingerprint,
            "removed": removed,
        }))
    }

    fn case_json(&self, case: &SelfHealCaseSummary) -> Value {
        json!({
            "fingerprint": case.fingerprint,
            "component": case.component,
            "symptom": case.symptom,
            "summary": case.summary,
            "why": case.why,
            "actions": case.actions,
            "source": case.source,
            "occurrence_count": case.occurrence_count,
            "updated_at": case.updated_at,
        })
    }

    fn pinned_case_note(case: &SelfHealCaseSummary) -> String {
        let action_lines = case
            .actions
            .iter()
            .map(|action| format!("- {action}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Self-heal fix: {fingerprint}\nComponent: {component}\nSymptom: {symptom}\nSummary: {summary}\nWhy: {why}\nSource: {source}\nOccurrences: {occurrence_count}\nActions:\n{actions}",
            fingerprint = case.fingerprint,
            component = case.component,
            symptom = case.symptom,
            summary = case.summary,
            why = case.why,
            source = case.source,
            occurrence_count = case.occurrence_count,
            actions = action_lines,
        )
    }

    fn export_case_markdown(case: &SelfHealCaseSummary) -> String {
        let actions = case
            .actions
            .iter()
            .map(|action| format!("- {action}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "# Self-heal case: {fingerprint}\n\n## Summary\n{summary}\n\n## Component\n{component}\n\n## Symptom\n{symptom}\n\n## Why it worked\n{why}\n\n## Source\n{source}\n\n## Occurrences\n{occurrence_count}\n\n## Actions\n{actions}\n",
            fingerprint = case.fingerprint,
            summary = case.summary,
            component = case.component,
            symptom = case.symptom,
            why = case.why,
            source = case.source,
            occurrence_count = case.occurrence_count,
            actions = actions,
        )
    }

    fn export_case_filename(fingerprint: &str) -> String {
        let safe = fingerprint
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
                _ => '-',
            })
            .collect::<String>();
        format!("self-heal-{safe}.md")
    }

    async fn export_case(&self, fingerprint: &str) -> Result<Value, String> {
        let Some(case) = self
            .store
            .get_case(fingerprint.to_string())
            .await
            .map_err(|err| format!("failed to query self-heal case: {err}"))?
        else {
            return Err(format!(
                "no remembered self-heal case for fingerprint '{fingerprint}'"
            ));
        };

        Ok(json!({
            "fingerprint": fingerprint,
            "filename": Self::export_case_filename(fingerprint),
            "case": self.case_json(&case),
            "markdown": Self::export_case_markdown(&case),
        }))
    }

    async fn replay_case(&self, fingerprint: &str) -> Result<Value, String> {
        let Some(case) = self
            .store
            .get_case(fingerprint.to_string())
            .await
            .map_err(|err| format!("failed to query self-heal case: {err}"))?
        else {
            return Err(format!(
                "no remembered self-heal case for fingerprint '{fingerprint}'"
            ));
        };

        let incident = SelfHealIncident {
            incident_id: Uuid::new_v4(),
            component: case.component.clone(),
            symptom: case.symptom.clone(),
            fingerprint: case.fingerprint.clone(),
            urgency: SelfHealUrgency::Medium,
            logs: vec![
                "operator replay requested from remembered case".to_string(),
                format!("replaying remembered fix for {}", case.summary),
            ],
        };
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::SelfHealRequested {
                incident: incident.clone(),
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::SELF_HEAL_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for self-heal response: {err}"))?;
        self.bus
            .publish(topics::SELF_HEAL_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish self-heal replay request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::SelfHealPlanned {
                        incident_id,
                        fingerprint: seen_fingerprint,
                        plan,
                    } = event.payload
                    {
                        if incident_id == incident.incident_id
                            && seen_fingerprint == case.fingerprint
                        {
                            return Ok(json!({
                                "fingerprint": case.fingerprint,
                                "incident_id": incident_id,
                                "replayed": true,
                                "plan": {
                                    "summary": plan.summary,
                                    "why": plan.why,
                                    "actions": plan.actions,
                                    "source": format!("{:?}", plan.source),
                                    "reused_previous_fix": plan.reused_previous_fix,
                                    "memory_hits": plan.memory_hits,
                                },
                            }));
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for self-heal replay result".to_string());
                }
            }
        }
    }

    async fn pin_case(&self, fingerprint: &str) -> Result<Value, String> {
        let Some(case) = self
            .store
            .get_case(fingerprint.to_string())
            .await
            .map_err(|err| format!("failed to query self-heal case: {err}"))?
        else {
            return Err(format!(
                "no remembered self-heal case for fingerprint '{fingerprint}'"
            ));
        };

        let content = Self::pinned_case_note(&case);
        let prefix = format!("Self-heal fix: {fingerprint}\n");
        let existing = self.list_pinned_memory(256).await?;
        let mut replaced_existing = 0usize;
        for previous in existing {
            if previous.starts_with(&prefix)
                && previous != content
                && self.remove_pinned_memory(previous).await?
            {
                replaced_existing += 1;
            }
        }
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryStoreRequested {
                content: content.clone(),
                tier: MemoryTier::Pinned,
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_STORE_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory store response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_STORE_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory store request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryStoreCompleted {
                        content: seen_content,
                        tier: seen_tier,
                        stored,
                    } = event.payload
                    {
                        if seen_content == content && seen_tier == MemoryTier::Pinned {
                            return Ok(json!({
                                "fingerprint": fingerprint,
                                "replaced_existing": replaced_existing,
                                "stored": stored,
                                "tier": "pinned",
                                "content": content,
                            }));
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory store confirmation".to_string());
                }
            }
        }
    }

    async fn list_pinned_memory(&self, limit: usize) -> Result<Vec<String>, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryListRequested {
                tier: MemoryTier::Pinned,
                limit,
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_LIST_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory list response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_LIST_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory list request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryListed {
                        tier: seen_tier,
                        results,
                    } = event.payload
                    {
                        if seen_tier == MemoryTier::Pinned {
                            return Ok(results);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory list response".to_string());
                }
            }
        }
    }

    async fn remove_pinned_memory(&self, content: String) -> Result<bool, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryRemoveRequested {
                content: content.clone(),
                tier: MemoryTier::Pinned,
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_REMOVE_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory remove response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_REMOVE_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory remove request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryRemoveCompleted {
                        content: seen_content,
                        tier: seen_tier,
                        removed,
                    } = event.payload
                    {
                        if seen_content == content && seen_tier == MemoryTier::Pinned {
                            return Ok(removed);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory remove confirmation".to_string());
                }
            }
        }
    }
}

#[async_trait]
impl CapabilityProvider for SelfHealToolsProvider {
    fn name(&self) -> &str {
        "self-heal"
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "self_heal.list_cases".to_string(),
            "self_heal.forget_case".to_string(),
            "self_heal.pin_case".to_string(),
            "self_heal.replay_case".to_string(),
            "self_heal.export_case".to_string(),
        ]
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor::new(
                "self_heal.list_cases",
                self.name(),
                "Lists remembered self-heal fixes and incident fingerprints.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "self_heal.forget_case",
                self.name(),
                "Removes a remembered self-heal case and its retained attempts.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "self_heal.pin_case",
                self.name(),
                "Pins a remembered self-heal case into always-available memory.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "self_heal.replay_case",
                self.name(),
                "Replays a remembered self-heal case through the live self-heal lane.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "self_heal.export_case",
                self.name(),
                "Exports a remembered self-heal case as an operator-friendly memory pack.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
        ]
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        let lowered = requirement.to_ascii_lowercase();
        if lowered.contains("self-heal history")
            || lowered.contains("remembered fixes")
            || lowered.contains("repair history")
        {
            Some(CapabilityMatch {
                capability: "self_heal.list_cases".to_string(),
                description: "Lists remembered self-heal fixes and incident fingerprints."
                    .to_string(),
            })
        } else if lowered.contains("forget self-heal")
            || lowered.contains("delete remembered fix")
            || lowered.contains("remove repair memory")
        {
            Some(CapabilityMatch {
                capability: "self_heal.forget_case".to_string(),
                description: "Removes a remembered self-heal case and its retained attempts."
                    .to_string(),
            })
        } else if lowered.contains("pin self-heal")
            || lowered.contains("promote repair memory")
            || lowered.contains("save remembered fix")
        {
            Some(CapabilityMatch {
                capability: "self_heal.pin_case".to_string(),
                description: "Pins a remembered self-heal case into always-available memory."
                    .to_string(),
            })
        } else if lowered.contains("replay self-heal")
            || lowered.contains("re-run remembered fix")
            || lowered.contains("retry remembered repair")
        {
            Some(CapabilityMatch {
                capability: "self_heal.replay_case".to_string(),
                description: "Replays a remembered self-heal case through the live repair lane."
                    .to_string(),
            })
        } else if lowered.contains("export self-heal")
            || lowered.contains("export repair memory")
            || lowered.contains("share remembered fix")
        {
            Some(CapabilityMatch {
                capability: "self_heal.export_case".to_string(),
                description: "Exports a remembered self-heal case as a reusable memory pack."
                    .to_string(),
            })
        } else {
            None
        }
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        match capability {
            "self_heal.list_cases" => {
                let limit = match parse_limit_argument(arguments, "limit", 10) {
                    Ok(limit) => limit,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(match self.list_cases(limit).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "self_heal.forget_case" => {
                let fingerprint = arguments.get("fingerprint")?.as_str()?.trim().to_string();
                if fingerprint.is_empty() {
                    return Some(ToolCallResult::Failed {
                        error: "fingerprint must not be empty".to_string(),
                    });
                }
                Some(match self.forget_case(&fingerprint).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "self_heal.pin_case" => {
                let fingerprint = arguments.get("fingerprint")?.as_str()?.trim().to_string();
                if fingerprint.is_empty() {
                    return Some(ToolCallResult::Failed {
                        error: "fingerprint must not be empty".to_string(),
                    });
                }
                Some(match self.pin_case(&fingerprint).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "self_heal.replay_case" => {
                let fingerprint = arguments.get("fingerprint")?.as_str()?.trim().to_string();
                if fingerprint.is_empty() {
                    return Some(ToolCallResult::Failed {
                        error: "fingerprint must not be empty".to_string(),
                    });
                }
                Some(match self.replay_case(&fingerprint).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "self_heal.export_case" => {
                let fingerprint = arguments.get("fingerprint")?.as_str()?.trim().to_string();
                if fingerprint.is_empty() {
                    return Some(ToolCallResult::Failed {
                        error: "fingerprint must not be empty".to_string(),
                    });
                }
                Some(match self.export_case(&fingerprint).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct MemoryToolsProvider {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
}

impl MemoryToolsProvider {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
        }
    }

    fn note_content(arguments: &Value) -> Result<String, String> {
        let content = if let Some(content) = arguments.as_str() {
            content.trim()
        } else {
            arguments
                .get("content")
                .or_else(|| arguments.get("note"))
                .or_else(|| arguments.get("text"))
                .or_else(|| arguments.get("message"))
                .or_else(|| arguments.get("body"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .ok_or_else(|| {
                    "content, note, text, message, or body must be provided as a string".to_string()
                })?
        };
        if content.is_empty() {
            Err("content must not be empty".to_string())
        } else {
            Ok(content.to_string())
        }
    }

    async fn store_memory(&self, content: String, tier: MemoryTier) -> Result<bool, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryStoreRequested {
                content: content.clone(),
                tier,
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_STORE_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory store response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_STORE_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory store request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryStoreCompleted {
                        content: seen_content,
                        tier: seen_tier,
                        stored,
                    } = event.payload
                    {
                        if seen_content == content && seen_tier == tier {
                            return Ok(stored);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory store confirmation".to_string());
                }
            }
        }
    }

    async fn remove_memory(&self, content: String, tier: MemoryTier) -> Result<bool, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryRemoveRequested {
                content: content.clone(),
                tier,
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_REMOVE_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory remove response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_REMOVE_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory remove request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryRemoveCompleted {
                        content: seen_content,
                        tier: seen_tier,
                        removed,
                    } = event.payload
                    {
                        if seen_content == content && seen_tier == tier {
                            return Ok(removed);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory remove confirmation".to_string());
                }
            }
        }
    }

    async fn list_memory(&self, tier: MemoryTier, limit: usize) -> Result<Vec<String>, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryListRequested { tier, limit },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_LIST_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory list response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_LIST_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory list request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryListed {
                        tier: seen_tier,
                        results,
                    } = event.payload
                    {
                        if seen_tier == tier {
                            return Ok(results);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory list response".to_string());
                }
            }
        }
    }
}

#[async_trait]
impl CapabilityProvider for MemoryToolsProvider {
    fn name(&self) -> &str {
        "memory"
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "memory.pin_note".to_string(),
            "memory.unpin_note".to_string(),
            "memory.remember_note".to_string(),
            "memory.list_pinned".to_string(),
            "memory.list_working".to_string(),
        ]
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor::new(
                "memory.pin_note",
                self.name(),
                "Pins an important memory so it stays in the always-available lane.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "memory.unpin_note",
                self.name(),
                "Removes a pinned memory from the always-available lane.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "memory.remember_note",
                self.name(),
                "Stores a normal working-memory note.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "memory.list_pinned",
                self.name(),
                "Lists recently pinned memories for review or UI display.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "memory.list_working",
                self.name(),
                "Lists recent working-memory notes for review or UI display.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
        ]
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        let lowered = requirement.to_ascii_lowercase();
        if lowered.contains("pin memory")
            || lowered.contains("important memory")
            || lowered.contains("always available memory")
        {
            Some(CapabilityMatch {
                capability: "memory.pin_note".to_string(),
                description: "Pins an important memory into the reserved memory lane.".to_string(),
            })
        } else if lowered.contains("unpin memory")
            || lowered.contains("remove pinned memory")
            || lowered.contains("delete important memory")
        {
            Some(CapabilityMatch {
                capability: "memory.unpin_note".to_string(),
                description: "Removes a pinned memory from the reserved memory lane.".to_string(),
            })
        } else if lowered.contains("remember this") || lowered.contains("save memory note") {
            Some(CapabilityMatch {
                capability: "memory.remember_note".to_string(),
                description: "Stores a note in working memory.".to_string(),
            })
        } else if lowered.contains("list pinned memory")
            || lowered.contains("show pinned memory")
            || lowered.contains("important memories")
        {
            Some(CapabilityMatch {
                capability: "memory.list_pinned".to_string(),
                description: "Lists recently pinned memories.".to_string(),
            })
        } else if lowered.contains("list working memory")
            || lowered.contains("show working memory")
            || lowered.contains("recent memory notes")
        {
            Some(CapabilityMatch {
                capability: "memory.list_working".to_string(),
                description: "Lists recent working-memory notes.".to_string(),
            })
        } else {
            None
        }
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        match capability {
            "memory.pin_note" => {
                let content = match Self::note_content(arguments) {
                    Ok(content) => content,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(
                    match self.store_memory(content.clone(), MemoryTier::Pinned).await {
                        Ok(stored) => ToolCallResult::Completed {
                            result: json!({
                                "content": content,
                                "tier": "pinned",
                                "stored": stored,
                            }),
                        },
                        Err(error) => ToolCallResult::Failed { error },
                    },
                )
            }
            "memory.unpin_note" => {
                let content = match Self::note_content(arguments) {
                    Ok(content) => content,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(
                    match self
                        .remove_memory(content.clone(), MemoryTier::Pinned)
                        .await
                    {
                        Ok(removed) => ToolCallResult::Completed {
                            result: json!({
                                "content": content,
                                "tier": "pinned",
                                "removed": removed,
                            }),
                        },
                        Err(error) => ToolCallResult::Failed { error },
                    },
                )
            }
            "memory.remember_note" => {
                let content = match Self::note_content(arguments) {
                    Ok(content) => content,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(
                    match self
                        .store_memory(content.clone(), MemoryTier::Working)
                        .await
                    {
                        Ok(stored) => ToolCallResult::Completed {
                            result: json!({
                                "content": content,
                                "tier": "working",
                                "stored": stored,
                            }),
                        },
                        Err(error) => ToolCallResult::Failed { error },
                    },
                )
            }
            "memory.list_pinned" => {
                let limit = match parse_limit_argument(arguments, "limit", 10) {
                    Ok(limit) => limit,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(match self.list_memory(MemoryTier::Pinned, limit).await {
                    Ok(results) => ToolCallResult::Completed {
                        result: json!({
                            "tier": "pinned",
                            "count": results.len(),
                            "results": results,
                        }),
                    },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "memory.list_working" => {
                let limit = match parse_limit_argument(arguments, "limit", 10) {
                    Ok(limit) => limit,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(match self.list_memory(MemoryTier::Working, limit).await {
                    Ok(results) => ToolCallResult::Completed {
                        result: json!({
                            "tier": "working",
                            "count": results.len(),
                            "results": results,
                        }),
                    },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            _ => None,
        }
    }
}

fn parse_runtime_profile_argument(value: &Value) -> Result<String, String> {
    let profile = value
        .as_str()
        .ok_or_else(|| "profile must be a string".to_string())?
        .to_ascii_lowercase();
    if matches!(profile.as_str(), "minimal" | "standard" | "full") {
        Ok(profile)
    } else {
        Err(format!(
            "unsupported profile '{}'; expected minimal, standard, or full",
            profile
        ))
    }
}

fn parse_runtime_budget_argument(arguments: &Value, key: &str) -> Result<Option<usize>, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    let numeric = value
        .as_u64()
        .ok_or_else(|| format!("{key} must be a positive integer"))?;
    if numeric == 0 {
        return Err(format!("{key} must be greater than zero"));
    }
    usize::try_from(numeric)
        .map(Some)
        .map_err(|_| format!("{key} is too large for this platform"))
}

fn parse_runtime_optional_string_argument(
    arguments: &Value,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(Some(String::new())),
        Value::String(value) => Ok(Some(value.trim().to_string())),
        _ => Err(format!("{key} must be a string or null")),
    }
}

fn parse_runtime_f32_string_argument(
    arguments: &Value,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    let parsed = match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| format!("{key} must be a finite number"))?,
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("{key} must be a finite number"))?,
        _ => return Err(format!("{key} must be a number")),
    };
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!("{key} must be a non-negative number"));
    }
    Ok(Some(parsed.to_string()))
}

fn runtime_settings_json(settings: &RuntimeSettings) -> Value {
    json!({
        "profile": settings.profile,
        "rag_budget_bytes": settings.rag_budget_bytes,
        "memory_working_budget_bytes": settings.memory_working_budget_bytes,
        "memory_pinned_budget_bytes": settings.memory_pinned_budget_bytes,
        "self_heal_history_budget_bytes": settings.self_heal_history_budget_bytes,
        "self_heal_llama_cpp_binary": settings.self_heal_llama_cpp_binary,
        "self_heal_model_path": settings.self_heal_model_path,
        "self_heal_model_context_size": settings.self_heal_model_context_size,
        "self_heal_model_max_tokens": settings.self_heal_model_max_tokens,
        "self_heal_model_temperature": settings
            .self_heal_model_temperature
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok()),
        "embedding_llama_cpp_binary": settings.embedding_llama_cpp_binary,
        "embedding_model_path": settings.embedding_model_path,
        "embedding_dimensions": settings.embedding_dimensions,
        "embedding_context_size": settings.embedding_context_size,
    })
}

fn rounded_runtime_float(value: f32) -> f64 {
    ((value as f64) * 1000.0).round() / 1000.0
}

fn parse_limit_argument(arguments: &Value, key: &str, default: usize) -> Result<usize, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(default);
    };
    let numeric = value
        .as_u64()
        .ok_or_else(|| format!("{key} must be a positive integer"))?;
    if numeric == 0 {
        return Err(format!("{key} must be greater than zero"));
    }
    usize::try_from(numeric).map_err(|_| format!("{key} is too large for this platform"))
}

fn extract_read_path(goal: &str) -> Option<String> {
    let normalized_goal = goal.to_ascii_lowercase();
    let marker = "read file";
    let start = normalized_goal.find(marker)?;
    let remainder = goal[start + marker.len()..].trim_start();
    if remainder.is_empty() {
        return None;
    }

    if let Some(stripped) = remainder.strip_prefix('"') {
        let end = stripped.find('"')?;
        return Some(stripped[..end].to_string());
    }

    remainder
        .split_whitespace()
        .next()
        .map(std::string::ToString::to_string)
}

fn preview_text(contents: &str) -> String {
    let preview: String = contents.chars().take(120).collect();
    preview.replace('\r', " ").replace('\n', "\\n")
}

/// Copy planner-attached context fields (context_hits, context_sources) into
/// the provider output so callers can see which retrieved snippets informed
/// the step. No-op when the arguments don't carry context.
fn attach_context_to_output(result: &mut Value, arguments: &Value) {
    if let Some(object) = result.as_object_mut() {
        if let Some(hits) = arguments.get("context_hits") {
            object.insert("context_hits".to_string(), hits.clone());
        }
        if let Some(sources) = arguments.get("context_sources") {
            object.insert("context_sources".to_string(), sources.clone());
        }
    }
}

fn knowledge_task_from_capability(capability: &str) -> Option<KnowledgeTask> {
    KnowledgeTask::ALL
        .iter()
        .copied()
        .find(|task| task.capability() == capability)
}

fn knowledge_snippets_from_hits(context: &[RagHit]) -> Vec<String> {
    context
        .iter()
        .take(4)
        .map(|hit| compact_snippet(&hit.snippet, 180))
        .collect()
}

fn knowledge_sources_from_hits(context: &[RagHit]) -> Vec<String> {
    context
        .iter()
        .take(4)
        .map(|hit| format!("{}#{}", hit.title, hit.chunk_index))
        .collect()
}

fn knowledge_snippets_from_arguments(arguments: &Value) -> Vec<String> {
    arguments
        .get("snippets")
        .and_then(|value| value.as_array())
        .map(|snippets| {
            snippets
                .iter()
                .filter_map(|value| value.as_str())
                .take(4)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn knowledge_sources_from_arguments(arguments: &Value) -> Vec<String> {
    arguments
        .get("sources")
        .and_then(|value| value.as_array())
        .map(|sources| {
            sources
                .iter()
                .filter_map(|value| value.as_str())
                .take(4)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn synthesize_knowledge_result(
    task: KnowledgeTask,
    goal: &str,
    snippets: &[String],
    sources: &[String],
) -> Value {
    let snippets = knowledge_fallback_snippets(snippets);

    match task {
        KnowledgeTask::Summarize => json!({
            "goal": goal,
            "summary": knowledge_summary_text(goal, &snippets, sources),
            "snippet_count": snippets.len(),
            "source_count": sources.len(),
        }),
        KnowledgeTask::AnswerQuestion => json!({
            "goal": goal,
            "answer": knowledge_answer_text(goal, &snippets, sources),
            "snippet_count": snippets.len(),
            "source_count": sources.len(),
        }),
        KnowledgeTask::CompareSources => {
            let observations = knowledge_observations(&snippets, sources);
            let comparison = if observations.len() >= 2 {
                format!(
                    "comparison for '{}': {} versus {}",
                    goal, observations[0], observations[1]
                )
            } else {
                format!(
                    "comparison for '{}': only one context slice was available: {}",
                    goal,
                    observations
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "no retrieved context was available".to_string())
                )
            };
            json!({
                "goal": goal,
                "comparison": comparison,
                "observations": observations,
                "source_count": sources.len(),
            })
        }
        KnowledgeTask::IdentifyFollowUps => {
            let followups = knowledge_followups(&snippets, sources);
            json!({
                "goal": goal,
                "followups": followups,
                "followup_count": followups.len(),
                "source_count": sources.len(),
            })
        }
    }
}

fn knowledge_result_preview(task: KnowledgeTask, result: &Value) -> String {
    match task {
        KnowledgeTask::Summarize => result
            .get("summary")
            .and_then(|value| value.as_str())
            .unwrap_or("summary unavailable")
            .to_string(),
        KnowledgeTask::AnswerQuestion => result
            .get("answer")
            .and_then(|value| value.as_str())
            .unwrap_or("answer unavailable")
            .to_string(),
        KnowledgeTask::CompareSources => result
            .get("comparison")
            .and_then(|value| value.as_str())
            .unwrap_or("comparison unavailable")
            .to_string(),
        KnowledgeTask::IdentifyFollowUps => result
            .get("followups")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "follow-ups unavailable".to_string()),
    }
}

fn knowledge_fallback_snippets(snippets: &[String]) -> Vec<String> {
    if snippets.is_empty() {
        vec!["no retrieved context was available".to_string()]
    } else {
        snippets.to_vec()
    }
}

fn knowledge_summary_text(goal: &str, snippets: &[String], sources: &[String]) -> String {
    if sources.is_empty() {
        format!("summary for '{}': {}", goal, snippets.join(" | "))
    } else {
        format!(
            "summary for '{}': {} sources=[{}]",
            goal,
            snippets.join(" | "),
            sources.join(", ")
        )
    }
}

fn knowledge_answer_text(goal: &str, snippets: &[String], sources: &[String]) -> String {
    let answer = snippets
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if sources.is_empty() {
        format!("answer for '{}': {}", goal, answer)
    } else {
        format!(
            "answer for '{}': {} evidence=[{}]",
            goal,
            answer,
            sources.join(", ")
        )
    }
}

fn knowledge_observations(snippets: &[String], sources: &[String]) -> Vec<String> {
    snippets
        .iter()
        .enumerate()
        .map(|(index, snippet)| {
            let source = sources
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("context#{}", index + 1));
            format!("{source}: {snippet}")
        })
        .collect()
}

fn knowledge_followups(snippets: &[String], sources: &[String]) -> Vec<String> {
    let mut followups = Vec::new();

    for (index, snippet) in snippets.iter().enumerate() {
        let candidate = followup_candidate(snippet).unwrap_or_else(|| lead_segment(snippet));
        if candidate.is_empty() {
            continue;
        }
        let source = sources
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("context#{}", index + 1));
        followups.push(format!("{source}: {candidate}"));
        if followups.len() == 4 {
            break;
        }
    }

    if followups.is_empty() {
        followups.push(
            "Review the retrieved context and identify the next operational step.".to_string(),
        );
    }

    followups
}

fn followup_candidate(snippet: &str) -> Option<String> {
    snippet
        .split(['.', ';'])
        .map(str::trim)
        .find(|segment| {
            let lowered = segment.to_ascii_lowercase();
            [
                "should", "must", "need", "add", "replace", "turn", "feed", "use", "surface",
                "revisit", "expand", "improve",
            ]
            .iter()
            .any(|keyword| lowered.contains(keyword))
        })
        .map(str::to_string)
}

fn lead_segment(snippet: &str) -> String {
    snippet
        .split(['.', ';'])
        .next()
        .unwrap_or(snippet)
        .trim()
        .trim_start_matches("- ")
        .to_string()
}

fn compact_snippet(snippet: &str, max_chars: usize) -> String {
    let mut compact = snippet.trim().replace(['\r', '\n'], " ");
    if compact.chars().count() > max_chars {
        compact = compact.chars().take(max_chars).collect::<String>();
        compact.push_str("...");
    }
    compact
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// LLM-backed variants of the planning/orchestration/research/content_store domain lanes.
///
/// This provider is the bridge between the pure-data `OrdoOpsProvider`
/// (deterministic templates that always work) and the `cloud.*` lane (real
/// outbound HTTP to configured providers). Every capability here is opt-in
/// and degrades gracefully: if no cloud credential is configured, the call
/// returns a structured error rather than panicking.
///
/// Capabilities:
/// - `planning.draft_response` Ã¢â‚¬â€ drafts initiative response from a brief
/// - `orchestration.draft_notes` Ã¢â‚¬â€ drafts reviewer notes or revision rationales
/// - `research.suggest_metadata` Ã¢â‚¬â€ suggests Research title/description/keywords
/// - `content_store.suggest_fields` Ã¢â‚¬â€ suggests Content Store field values for a record
pub struct OrdoLlmProvider {
    credentials: ordo_cloud::CloudCredentialTask,
    http: ordo_cloud::CloudHttp,
    default_service: String,
    /// When set, the provider will hydrate the LLM prompt with RAG
    /// snippets taken from the local retrieval lane. This turns generic
    /// LLM output into operator-consistent output without any caller
    /// coordination.
    bus: Option<Arc<dyn Bus>>,
    rag_top_k: usize,
    /// When set, calls with `review: true` queue the draft for operator
    /// approval and block until a decision arrives. Denied drafts are
    /// returned to the agent as a `Failed` result; edits are
    /// transparently substituted into the response.
    review: Option<ordo_review::ReviewService>,
    /// Maximum time to wait for the operator before expiring the
    /// request. Zero = block forever (not recommended).
    review_wait: std::time::Duration,
}

impl OrdoLlmProvider {
    /// Build an LLM provider that talks to a configured cloud service. The
    /// default service name is `openai` but individual calls can override
    /// it via the `credential` argument, matching the `cloud.*` pattern.
    pub fn new(credentials: ordo_cloud::CloudCredentialTask) -> Self {
        Self {
            credentials,
            http: ordo_cloud::CloudHttp::new(),
            default_service: "openai".to_string(),
            bus: None,
            rag_top_k: 3,
            review: None,
            review_wait: std::time::Duration::from_secs(300),
        }
    }

    pub fn with_default_service(mut self, service: impl Into<String>) -> Self {
        self.default_service = service.into();
        self
    }

    /// Enable human-in-the-loop review. When the caller sets
    /// `review: true`, the provider queues the draft and blocks until
    /// the operator approves / edits / denies.
    pub fn with_review(mut self, service: ordo_review::ReviewService) -> Self {
        self.review = Some(service);
        self
    }

    pub fn with_review_wait(mut self, wait: std::time::Duration) -> Self {
        self.review_wait = wait;
        self
    }

    pub fn with_http(mut self, http: ordo_cloud::CloudHttp) -> Self {
        self.http = http;
        self
    }

    /// Enable automatic RAG context injection. Every call to
    /// `planning.draft_response`, `orchestration.draft_notes`,
    /// `research.suggest_metadata`, and `content_store.suggest_fields` will pre-query
    /// the local retrieval lane using the caller-supplied `rag_query`
    /// (falling back to the prompt itself) and prepend the top-K
    /// snippets to the system message.
    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_rag_top_k(mut self, top_k: usize) -> Self {
        self.rag_top_k = top_k;
        self
    }
}

const PLANNING_DRAFT_RESPONSE: &str = "planning.draft_response";
const ORCHESTRATION_DRAFT_NOTES: &str = "orchestration.draft_notes";
const RESEARCH_SUGGEST_METADATA: &str = "research.suggest_metadata";
const CONTENT_STORE_SUGGEST_FIELDS: &str = "content_store.suggest_fields";

const ORDO_LLM_CAPABILITIES: &[&str] = &[
    PLANNING_DRAFT_RESPONSE,
    ORCHESTRATION_DRAFT_NOTES,
    RESEARCH_SUGGEST_METADATA,
    CONTENT_STORE_SUGGEST_FIELDS,
];

fn planning_llm_description(capability: &str) -> &'static str {
    match capability {
        PLANNING_DRAFT_RESPONSE => {
            "Drafts initiative response from a brief using a configured cloud LLM credential."
        }
        ORCHESTRATION_DRAFT_NOTES => {
            "Drafts reviewer notes or revision rationale using a configured cloud LLM credential."
        }
        RESEARCH_SUGGEST_METADATA => {
            "Suggests Research title/description/keywords using a configured cloud LLM credential."
        }
        CONTENT_STORE_SUGGEST_FIELDS => {
            "Suggests Content Store field values for a record using a configured cloud LLM credential."
        }
        _ => "Ordo LLM capability.",
    }
}

fn planning_llm_system_prompt(capability: &str) -> &'static str {
    match capability {
        PLANNING_DRAFT_RESPONSE => {
            "You are Ordo's planning operations drafter. Return structured, \
             policy-safe initiative response. Keep responses concise and focused on \
             the brief provided. Prefer bullet lists for multiple \
             deliverables."
        }
        ORCHESTRATION_DRAFT_NOTES => {
            "You are Ordo's orchestration reviewer. Draft clear, kind, \
             specific reviewer notes or revision rationale. Cite the brief \
             fields you are responding to. Keep it short."
        }
        RESEARCH_SUGGEST_METADATA => {
            "You are Ordo's Research specialist. Return JSON with keys \
             title (max 60 chars), description (max 155 chars), keywords \
             (array of strings), slug (kebab-case). Only return JSON."
        }
        CONTENT_STORE_SUGGEST_FIELDS => {
            "You are Ordo's Content Store editor. Return JSON mapping each \
             requested field name to a suggested value derived from the \
             provided source material. Only return JSON."
        }
        _ => "You are a helpful assistant.",
    }
}

async fn run_planning_llm_call(
    provider: &OrdoLlmProvider,
    capability: &str,
    arguments: &Value,
) -> Option<ToolCallResult> {
    let service = arguments
        .get("credential")
        .and_then(|value| value.as_str())
        .unwrap_or(&provider.default_service)
        .to_string();

    let credential = match provider.credentials.get(service.clone()).await {
        Ok(Some(credential)) => credential,
        Ok(None) => {
            return Some(ToolCallResult::Failed {
                error: format!(
                    "credential for service '{service}' is not configured; \
                     call cloud.credentials.upsert first to enable {capability}"
                ),
            });
        }
        Err(err) => {
            return Some(ToolCallResult::Failed {
                error: err.to_string(),
            });
        }
    };

    // Build a chat-style prompt. We forward any caller-supplied
    // arguments as the user message body so downstream prompts can
    // pass structured data (briefs, records, etc.) through unchanged.
    let prompt = arguments
        .get("prompt")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(arguments).unwrap_or_default());
    let base_system = planning_llm_system_prompt(capability);

    // Pre-query the local RAG lane and inject the top hits as a second
    // system message so the LLM stays grounded in the operator profile's own
    // corpus. Caller can set `rag_query` explicitly or let the prompt
    // itself be used. `rag=false` disables the prefetch for this call.
    let rag_enabled = arguments
        .get("rag")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let rag_query = arguments
        .get("rag_query")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| prompt.clone());
    let rag_collections = arguments
        .get("rag_collections")
        .and_then(|value| value.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let rag_hits = if rag_enabled {
        fetch_rag_context(provider, &rag_query, &rag_collections).await
    } else {
        Vec::new()
    };

    let mut messages = Vec::new();
    messages.push(json!({ "role": "system", "content": base_system }));
    if !rag_hits.is_empty() {
        let context = render_rag_context(&rag_hits);
        messages.push(json!({
            "role": "system",
            "content": format!(
                "Relevant context from the local Ordo operator corpus. \
                 Use this to stay consistent with existing operator style, \
                 product names, and positioning. Do not invent facts that \
                 are not supported.\n\n{context}"
            ),
        }));
    }
    messages.push(json!({ "role": "user", "content": prompt }));

    let mut chat_args = json!({
        "messages": messages,
        "temperature": arguments.get("temperature").cloned().unwrap_or(json!(0.4)),
    });
    // Honor a per-credential model override (Cloud tab â†’ "model" field,
    // stored in extras). Lets local providers (Ollama / LM Studio) hit
    // whichever model the operator has loaded.
    if let Some(model) = credential.extras.get("model") {
        chat_args["model"] = json!(model);
    }

    // Dispatch to whichever provider is configured. Anthropic credentials
    // get the `messages` endpoint; everything else flows through OpenAI
    // chat.
    let result = if credential.auth_style == "anthropic" {
        ordo_cloud::anthropic::messages(&provider.http, &credential, &chat_args).await
    } else {
        ordo_cloud::openai::chat(&provider.http, &credential, &chat_args).await
    };

    let review_requested = arguments
        .get("review")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Some(match result {
        Ok(mut value) => {
            // Make the domain capability identifiable in the output, and
            // report how many RAG hits grounded the prompt so the
            // operator can see whether the answer was operator-context-
            // aware or fell back to pure LLM output.
            if let Some(object) = value.as_object_mut() {
                object.insert("capability".into(), Value::String(capability.to_string()));
                object.insert("credential_service".into(), Value::String(service.clone()));
                object.insert(
                    "rag_context_hits".into(),
                    Value::Number(serde_json::Number::from(rag_hits.len() as u64)),
                );
                if !rag_hits.is_empty() {
                    object.insert(
                        "rag_context_sources".into(),
                        Value::Array(
                            rag_hits
                                .iter()
                                .map(|hit| {
                                    json!({
                                        "document_id": hit.document_id,
                                        "title": hit.title,
                                        "collection": hit.collection,
                                    })
                                })
                                .collect(),
                        ),
                    );
                }
            }

            // Optional human-in-the-loop review step. We queue the
            // LLM's draft for operator approval and (if the review
            // service is configured) block until a decision arrives.
            // Deny Ã¢â€ â€™ Failed; Edit Ã¢â€ â€™ substitute the edited text in the
            // output so downstream agents see the operator's version.
            if review_requested {
                match (&provider.review, extract_review_draft(capability, &value)) {
                    (Some(review_service), Some(draft)) => {
                        let metadata = std::collections::HashMap::from_iter([
                            (
                                "capability".to_string(),
                                Value::String(capability.to_string()),
                            ),
                            (
                                "credential_service".to_string(),
                                Value::String(service.clone()),
                            ),
                            (
                                "rag_context_hits".to_string(),
                                Value::Number(serde_json::Number::from(rag_hits.len() as u64)),
                            ),
                        ]);
                        let new_request = ordo_review::NewReviewRequest {
                            origin_capability: capability.to_string(),
                            origin_plugin: None,
                            title: review_title(capability, arguments),
                            content_type: review_content_type(capability),
                            content: draft.clone(),
                            metadata,
                        };
                        match review_service
                            .request_and_wait(new_request, provider.review_wait)
                            .await
                        {
                            Ok(resolved) => {
                                use ordo_review::ReviewState::*;
                                match resolved.state {
                                    Approved | EditedAndApproved => {
                                        substitute_review_output(
                                            capability,
                                            &mut value,
                                            resolved.effective_content(),
                                        );
                                        if let Some(object) = value.as_object_mut() {
                                            object.insert(
                                                "review".into(),
                                                json!({
                                                    "state": resolved.state.label(),
                                                    "id": resolved.id,
                                                    "edited": matches!(resolved.state, EditedAndApproved),
                                                    "note": resolved.decision_note,
                                                }),
                                            );
                                        }
                                        ToolCallResult::Completed { result: value }
                                    }
                                    Denied => ToolCallResult::Failed {
                                        error: format!(
                                            "operator denied review {} ({}){}",
                                            resolved.id,
                                            capability,
                                            resolved
                                                .decision_note
                                                .map(|note| format!(": {note}"))
                                                .unwrap_or_default(),
                                        ),
                                    },
                                    Expired => ToolCallResult::Failed {
                                        error: format!(
                                            "review for '{capability}' expired before the operator acted"
                                        ),
                                    },
                                    Open => ToolCallResult::Failed {
                                        error: "review returned in Open state (runtime bug)"
                                            .to_string(),
                                    },
                                }
                            }
                            Err(err) => ToolCallResult::Failed {
                                error: format!("review service error: {err}"),
                            },
                        }
                    }
                    (None, _) => {
                        // Requested review but nothing's wired Ã¢â‚¬â€ be honest
                        // rather than silently skipping.
                        ToolCallResult::Failed {
                            error: "review requested but no review service is configured"
                                .to_string(),
                        }
                    }
                    (Some(_), None) => ToolCallResult::Completed { result: value },
                }
            } else {
                ToolCallResult::Completed { result: value }
            }
        }
        Err(err) => ToolCallResult::Failed {
            error: err.to_string(),
        },
    })
}

/// Extract the reviewable draft from a ordo-llm response. For
/// OpenAI-style chats we prefer `assistant_message`; for Anthropic, we
/// prefer `assistant_text`; otherwise we fall back to the full JSON
/// payload so the operator at least sees something.
fn extract_review_draft(_capability: &str, value: &Value) -> Option<String> {
    if let Some(text) = value.get("assistant_message").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }
    if let Some(text) = value.get("assistant_text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }
    serde_json::to_string_pretty(value).ok()
}

fn substitute_review_output(_capability: &str, value: &mut Value, approved: &str) {
    if let Some(object) = value.as_object_mut() {
        if object.contains_key("assistant_message") {
            object.insert(
                "assistant_message".into(),
                Value::String(approved.to_string()),
            );
        } else if object.contains_key("assistant_text") {
            object.insert("assistant_text".into(), Value::String(approved.to_string()));
        } else {
            object.insert("text".into(), Value::String(approved.to_string()));
        }
    }
}

fn review_title(capability: &str, arguments: &Value) -> String {
    // Prefer a caller-supplied hint so the review panel has a human
    // label. Fall back to the capability name + short prompt excerpt.
    if let Some(title) = arguments.get("review_title").and_then(|v| v.as_str()) {
        return title.to_string();
    }
    if let Some(prompt) = arguments.get("prompt").and_then(|v| v.as_str()) {
        let snippet = prompt.chars().take(64).collect::<String>();
        return format!("{capability}: {snippet}");
    }
    capability.to_string()
}

fn review_content_type(capability: &str) -> String {
    match capability {
        RESEARCH_SUGGEST_METADATA | CONTENT_STORE_SUGGEST_FIELDS => "application/json".to_string(),
        _ => "text/markdown".to_string(),
    }
}

/// Publish a RAG query on the bus and wait briefly for hits. Returns an
/// empty vec if the bus is not configured or if the retrieval lane does
/// not respond in time Ã¢â‚¬â€ this is best-effort grounding, never a hard
/// dependency.
async fn fetch_rag_context(
    provider: &OrdoLlmProvider,
    query: &str,
    collections: &[String],
) -> Vec<RagHit> {
    use futures::StreamExt;
    use std::time::Duration;
    use tokio::time::timeout;

    let Some(bus) = provider.bus.as_ref() else {
        return Vec::new();
    };
    if query.trim().is_empty() || provider.rag_top_k == 0 {
        return Vec::new();
    }

    let correlation_id = CorrelationId::new();
    let envelope = Envelope::new(
        NodeId::new(),
        OrdoMessage::RagQueryRequested {
            query: query.to_string(),
            top_k: provider.rag_top_k,
            collections: collections.to_vec(),
        },
    )
    .with_correlation(correlation_id.clone());

    let mut sub = match bus.subscribe(topics::RAG_QUERY_RESPONSE).await {
        Ok(sub) => sub,
        Err(_) => return Vec::new(),
    };
    if bus
        .publish(topics::RAG_QUERY_REQUEST, envelope)
        .await
        .is_err()
    {
        return Vec::new();
    }

    // Give the RAG lane a short window Ã¢â‚¬â€ we never want to block a user-
    // facing LLM call on a slow retrieval round trip. 750 ms matches the
    // Brain's internal budget for context hydration.
    let wait = Duration::from_millis(750);
    loop {
        match timeout(wait, sub.next()).await {
            Ok(Some(event)) => {
                if event.correlation_id.as_ref() != Some(&correlation_id) {
                    continue;
                }
                if let OrdoMessage::RagQueryCompleted { query: seen, hits } = event.payload {
                    if seen == query {
                        return hits;
                    }
                }
            }
            _ => return Vec::new(),
        }
    }
}

fn render_rag_context(hits: &[RagHit]) -> String {
    let mut out = String::new();
    for (idx, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "[{n}] ({collection}/{doc} #{chunk}, score={score:.2})\n{snippet}\n\n",
            n = idx + 1,
            collection = hit.collection,
            doc = hit.document_id,
            chunk = hit.chunk_index,
            score = hit.score,
            snippet = hit.snippet.trim(),
        ));
    }
    out
}

#[async_trait]
impl CapabilityProvider for OrdoLlmProvider {
    fn name(&self) -> &str {
        "ordo-llm"
    }

    fn capabilities(&self) -> Vec<String> {
        ORDO_LLM_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ORDO_LLM_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    planning_llm_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        if !ORDO_LLM_CAPABILITIES.contains(&capability) {
            return None;
        }
        run_planning_llm_call(self, capability, arguments).await
    }
}

// =========================================================================
// Review provider Ã¢â‚¬â€ exposes the `ordo-review` service as a capability lane
// so agents and plugins call `review.request_approval` like any other tool.
// Lives here (not in `ordo-review`) because it needs `CapabilityProvider`
// and we can't let `ordo-review` depend on `ordo-mcp-host` without a cycle.
// =========================================================================

pub const REVIEW_REQUEST_APPROVAL: &str = "review.request_approval";
pub const REVIEW_LIST_PENDING: &str = "review.list_pending";
pub const REVIEW_APPROVE: &str = "review.approve";
pub const REVIEW_DENY: &str = "review.deny";
pub const REVIEW_EDIT: &str = "review.edit";

const REVIEW_CAPABILITIES: &[&str] = &[
    REVIEW_REQUEST_APPROVAL,
    REVIEW_LIST_PENDING,
    REVIEW_APPROVE,
    REVIEW_DENY,
    REVIEW_EDIT,
];

const REVIEW_DEFAULT_WAIT_SECS: u64 = 300;

fn review_description(capability: &str) -> &'static str {
    match capability {
        REVIEW_REQUEST_APPROVAL => {
            "Queues an artifact for operator review. Optionally waits for the decision and returns the approved (possibly edited) content."
        }
        REVIEW_LIST_PENDING => "Lists every review request still awaiting operator action.",
        REVIEW_APPROVE => "Approves a queued review request by id.",
        REVIEW_DENY => "Denies a queued review request by id.",
        REVIEW_EDIT => "Edits the artifact and approves the request in one call.",
        _ => "Review capability.",
    }
}

pub struct ReviewProvider {
    service: ordo_review::ReviewService,
}

impl ReviewProvider {
    pub fn new(service: ordo_review::ReviewService) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &ordo_review::ReviewService {
        &self.service
    }
}

#[async_trait]
impl CapabilityProvider for ReviewProvider {
    fn name(&self) -> &str {
        "review"
    }

    fn capabilities(&self) -> Vec<String> {
        REVIEW_CAPABILITIES
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        REVIEW_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    review_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let outcome: Result<Value, ordo_review::ReviewError> = match capability {
            REVIEW_REQUEST_APPROVAL => review_do_request(&self.service, arguments).await,
            REVIEW_LIST_PENDING => review_do_list_pending(&self.service),
            REVIEW_APPROVE => review_do_approve(&self.service, arguments),
            REVIEW_DENY => review_do_deny(&self.service, arguments),
            REVIEW_EDIT => review_do_edit(&self.service, arguments),
            _ => return None,
        };
        Some(match outcome {
            Ok(value) => ToolCallResult::Completed { result: value },
            Err(err) => ToolCallResult::Failed {
                error: err.to_string(),
            },
        })
    }
}

async fn review_do_request(
    service: &ordo_review::ReviewService,
    arguments: &Value,
) -> Result<Value, ordo_review::ReviewError> {
    let title = arguments
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_review::ReviewError::InvalidArgument("missing 'title'".into()))?
        .to_string();
    let content = arguments
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_review::ReviewError::InvalidArgument("missing 'content'".into()))?
        .to_string();
    let content_type = arguments
        .get("content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("text/markdown")
        .to_string();
    let origin_capability = arguments
        .get("origin_capability")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let origin_plugin = arguments
        .get("origin_plugin")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let metadata = arguments
        .get("metadata")
        .and_then(|v| v.as_object())
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let wait_seconds = arguments
        .get("wait_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(REVIEW_DEFAULT_WAIT_SECS);
    let async_only = arguments
        .get("async")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let new_request = ordo_review::NewReviewRequest {
        origin_capability,
        origin_plugin,
        title,
        content_type,
        content,
        metadata,
    };

    if async_only || wait_seconds == 0 {
        let queued = service.request(new_request)?;
        return Ok(serialize_review_request(&queued, false));
    }

    let resolved = service
        .request_and_wait(new_request, std::time::Duration::from_secs(wait_seconds))
        .await?;
    Ok(serialize_review_request(&resolved, true))
}

fn review_do_list_pending(
    service: &ordo_review::ReviewService,
) -> Result<Value, ordo_review::ReviewError> {
    let pending = service.pending()?;
    let serialized: Vec<Value> = pending
        .iter()
        .map(|r| serialize_review_request(r, false))
        .collect();
    Ok(json!({
        "count": serialized.len(),
        "pending": serialized,
    }))
}

fn review_do_approve(
    service: &ordo_review::ReviewService,
    arguments: &Value,
) -> Result<Value, ordo_review::ReviewError> {
    let id = parse_review_id(arguments)?;
    let note = arguments
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let resolved = service.decide(id, ordo_review::ReviewDecisionKind::Approve { note })?;
    Ok(serialize_review_request(&resolved, true))
}

fn review_do_deny(
    service: &ordo_review::ReviewService,
    arguments: &Value,
) -> Result<Value, ordo_review::ReviewError> {
    let id = parse_review_id(arguments)?;
    let note = arguments
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let resolved = service.decide(id, ordo_review::ReviewDecisionKind::Deny { note })?;
    Ok(serialize_review_request(&resolved, true))
}

fn review_do_edit(
    service: &ordo_review::ReviewService,
    arguments: &Value,
) -> Result<Value, ordo_review::ReviewError> {
    let id = parse_review_id(arguments)?;
    let content = arguments
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_review::ReviewError::InvalidArgument("missing 'content'".into()))?
        .to_string();
    let note = arguments
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let resolved = service.decide(id, ordo_review::ReviewDecisionKind::Edit { content, note })?;
    Ok(serialize_review_request(&resolved, true))
}

fn parse_review_id(arguments: &Value) -> Result<uuid::Uuid, ordo_review::ReviewError> {
    let raw = arguments
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_review::ReviewError::InvalidArgument("missing 'id'".into()))?;
    uuid::Uuid::parse_str(raw)
        .map_err(|err| ordo_review::ReviewError::InvalidArgument(err.to_string()))
}

fn serialize_review_request(request: &ordo_review::ReviewRequest, include_content: bool) -> Value {
    let mut value = json!({
        "id": request.id,
        "created_at": request.created_at,
        "resolved_at": request.resolved_at,
        "origin_capability": request.origin_capability,
        "origin_plugin": request.origin_plugin,
        "title": request.title,
        "content_type": request.content_type,
        "state": request.state.label(),
        "has_edited_content": request.edited_content.is_some(),
        "decision_note": request.decision_note,
        "metadata": request.metadata,
    });
    if include_content {
        if let Some(object) = value.as_object_mut() {
            object.insert("content".into(), Value::String(request.content.clone()));
            object.insert(
                "effective_content".into(),
                Value::String(request.effective_content().to_string()),
            );
        }
    }
    value
}

// =========================================================================
// Assistant provider Ã¢â‚¬â€ exposes `ordo-assistant` on the capability bus.
// Same "provider-in-mcp, service-in-its-own-crate" pattern as review,
// to avoid a cycle between `ordo-assistant` and `ordo-mcp-host`.
// =========================================================================

pub const ASSISTANT_TURN: &str = "assistant.turn";
pub const ASSISTANT_NEW_SESSION: &str = "assistant.new_session";
pub const ASSISTANT_LIST_SESSIONS: &str = "assistant.list_sessions";
pub const ASSISTANT_GET_SESSION: &str = "assistant.get_session";
pub const ASSISTANT_REMEMBER_FACT: &str = "assistant.remember_fact";
pub const ASSISTANT_FORGET_FACT: &str = "assistant.forget_fact";
pub const ASSISTANT_LIST_FACTS: &str = "assistant.list_facts";
pub const ASSISTANT_RECALL: &str = "assistant.recall";
// Push 3: progressive-disclosure meta-tools + self-knowledge CRUD.
pub const ASSISTANT_RECALL_MEMORY: &str = "assistant.recall_memory";
pub const ASSISTANT_KNOWLEDGE_LOOKUP: &str = "assistant.knowledge_lookup";
pub const ASSISTANT_PARALLEL_LOOKUP: &str = "assistant.parallel_lookup";
pub const ASSISTANT_REMEMBER_KNOWLEDGE: &str = "assistant.remember_knowledge";
pub const ASSISTANT_FORGET_KNOWLEDGE: &str = "assistant.forget_knowledge";
pub const ASSISTANT_LIST_KNOWLEDGE: &str = "assistant.list_knowledge";

const ASSISTANT_CAPABILITIES: &[&str] = &[
    ASSISTANT_TURN,
    ASSISTANT_NEW_SESSION,
    ASSISTANT_LIST_SESSIONS,
    ASSISTANT_GET_SESSION,
    ASSISTANT_REMEMBER_FACT,
    ASSISTANT_FORGET_FACT,
    ASSISTANT_LIST_FACTS,
    ASSISTANT_RECALL,
    ASSISTANT_RECALL_MEMORY,
    ASSISTANT_KNOWLEDGE_LOOKUP,
    ASSISTANT_PARALLEL_LOOKUP,
    ASSISTANT_REMEMBER_KNOWLEDGE,
    ASSISTANT_FORGET_KNOWLEDGE,
    ASSISTANT_LIST_KNOWLEDGE,
];

fn assistant_description(capability: &str) -> &'static str {
    match capability {
        ASSISTANT_TURN => {
            "Process a user turn. Routes through the fact store, the local RAG lane, and the configured cloud LLM; persists the turn. Primary entry point for conversational use."
        }
        ASSISTANT_NEW_SESSION => "Create a new conversation session.",
        ASSISTANT_LIST_SESSIONS => "List recent conversation sessions.",
        ASSISTANT_GET_SESSION => "Load a session with its full turn history.",
        ASSISTANT_REMEMBER_FACT => {
            "Teach the assistant a durable fact about the operator, a client, the operator profile, or a project."
        }
        ASSISTANT_FORGET_FACT => "Remove a stored fact by id.",
        ASSISTANT_LIST_FACTS => "List stored facts, optionally filtered by subject.",
        ASSISTANT_RECALL => {
            "Return the top-K facts most relevant to an arbitrary query, without consulting an LLM."
        }
        ASSISTANT_RECALL_MEMORY => {
            "Meta-tool: semantic recall over persistent fact memory. Returns facts with a read-only preamble describing how to use the memory layer. Called by the assistant itself during a turn; operators can call it directly for debugging."
        }
        ASSISTANT_KNOWLEDGE_LOOKUP => {
            "Meta-tool: semantic recall over the assistant's self-knowledge RAG (skills, personas, tool notes, observations). Optionally filter by kind and/or domain. Results include the self-knowledge-layer preamble."
        }
        ASSISTANT_PARALLEL_LOOKUP => {
            "Meta-tool: fan knowledge_lookup across an explicit list of user-, mode-, or knowledge-selected domains concurrently."
        }
        ASSISTANT_REMEMBER_KNOWLEDGE => {
            "Add an entry to the assistant's self-knowledge RAG (skill card, persona guide, tool note, observation, or free-form note)."
        }
        ASSISTANT_FORGET_KNOWLEDGE => "Remove a stored knowledge entry by id.",
        ASSISTANT_LIST_KNOWLEDGE => {
            "List stored knowledge entries, optionally filtered by kind and/or domain."
        }
        _ => "Assistant capability.",
    }
}

pub struct AssistantProvider {
    service: ordo_assistant::AssistantService,
}

impl AssistantProvider {
    pub fn new(service: ordo_assistant::AssistantService) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &ordo_assistant::AssistantService {
        &self.service
    }
}

#[async_trait]
impl CapabilityProvider for AssistantProvider {
    fn name(&self) -> &str {
        "assistant"
    }

    fn capabilities(&self) -> Vec<String> {
        ASSISTANT_CAPABILITIES
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ASSISTANT_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    assistant_description(capability),
                    CapabilityTier::Core,
                    CapabilityActivation::Eager,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let outcome: Result<Value, ordo_assistant::AssistantError> = match capability {
            ASSISTANT_TURN => assistant_do_turn(&self.service, arguments).await,
            ASSISTANT_NEW_SESSION => assistant_do_new_session(&self.service, arguments),
            ASSISTANT_LIST_SESSIONS => assistant_do_list_sessions(&self.service, arguments),
            ASSISTANT_GET_SESSION => assistant_do_get_session(&self.service, arguments),
            ASSISTANT_REMEMBER_FACT => assistant_do_remember(&self.service, arguments).await,
            ASSISTANT_FORGET_FACT => assistant_do_forget(&self.service, arguments),
            ASSISTANT_LIST_FACTS => assistant_do_list_facts(&self.service, arguments),
            ASSISTANT_RECALL => assistant_do_recall(&self.service, arguments).await,
            ASSISTANT_RECALL_MEMORY => assistant_do_recall_memory(&self.service, arguments).await,
            ASSISTANT_KNOWLEDGE_LOOKUP => {
                assistant_do_knowledge_lookup(&self.service, arguments).await
            }
            ASSISTANT_PARALLEL_LOOKUP => {
                assistant_do_parallel_lookup(&self.service, arguments).await
            }
            ASSISTANT_REMEMBER_KNOWLEDGE => {
                assistant_do_remember_knowledge(&self.service, arguments).await
            }
            ASSISTANT_FORGET_KNOWLEDGE => assistant_do_forget_knowledge(&self.service, arguments),
            ASSISTANT_LIST_KNOWLEDGE => assistant_do_list_knowledge(&self.service, arguments),
            _ => return None,
        };
        Some(match outcome {
            Ok(value) => ToolCallResult::Completed { result: value },
            Err(err) => ToolCallResult::Failed {
                error: err.to_string(),
            },
        })
    }
}

async fn assistant_do_turn(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let request: ordo_assistant::TurnRequest = serde_json::from_value(arguments.clone())
        .map_err(|err| ordo_assistant::AssistantError::InvalidArgument(err.to_string()))?;
    // `assistant.turn` is an untrusted bus/MCP boundary: the caller is not
    // an authenticated session owner. Sanitize the request so a caller
    // cannot target/hijack an existing session id (it always runs on a
    // fresh session) or set the internal isolation fields. Trusted callers
    // (control API, in-process spawns) call `service.turn` directly.
    let request = ordo_assistant::sanitize_untrusted_turn_request(request);
    let result = service.turn(request).await?;
    Ok(serde_json::to_value(&result).unwrap_or(Value::Null))
}

fn assistant_do_new_session(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let title = arguments
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let mode = arguments
        .get("mode")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let session = service.new_session(title, mode)?;
    Ok(serde_json::to_value(&session).unwrap_or(Value::Null))
}

fn assistant_do_list_sessions(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;
    let sessions = service.list_sessions(limit)?;
    Ok(json!({ "count": sessions.len(), "sessions": sessions }))
}

fn assistant_do_get_session(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let id = assistant_parse_id(arguments, "session_id")?;
    let session = service.get_session(id)?;
    Ok(serde_json::to_value(&session).unwrap_or(Value::Null))
}

async fn assistant_do_remember(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let mut new_fact: ordo_assistant::NewFact = serde_json::from_value(arguments.clone())
        .map_err(|err| ordo_assistant::AssistantError::InvalidArgument(err.to_string()))?;

    // Mode-aware bus path: external callers MAY pass an optional
    // `session_id` so the new fact lands in that session's mode
    // scope rather than the legacy global default. The LLM path
    // (dispatch_tool's `assistant.remember_fact` shadow) already
    // does this implicitly; the bus exposes it explicitly because
    // external MCP clients don't have a "current mode" by default.
    //
    // Resolution rules (mirror the meta-tool's):
    //   - If the fact already has an explicit `scope`, that wins.
    //   - Else if `session_id` is supplied AND resolves to a mode,
    //     the fact gets `scope: "mode:<id>"`.
    //   - Else the fact falls through to NewFact's serde default
    //     ("global"), preserving every legacy caller.
    if new_fact.scope.is_none() {
        if let Some(sid_str) = arguments.get("session_id").and_then(|v| v.as_str()) {
            if let Ok(sid) = uuid::Uuid::parse_str(sid_str) {
                if let Some(mode) = service.resolve_session_mode_manifest(sid) {
                    new_fact.scope = Some(format!("mode:{}", mode.id));
                }
            }
        }
    }

    let fact = service.remember_fact(new_fact).await?;
    let summary = ordo_assistant::FactSummary::from(&fact);
    Ok(serde_json::to_value(&summary).unwrap_or(Value::Null))
}

fn assistant_do_forget(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let id = assistant_parse_id(arguments, "id")?;
    let removed = service.forget_fact(id)?;
    Ok(json!({ "id": id, "removed": removed }))
}

fn assistant_do_list_facts(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let subject = arguments
        .get("subject")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let facts = service.list_facts(subject)?;
    Ok(json!({ "count": facts.len(), "facts": facts }))
}

async fn assistant_do_recall(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_assistant::AssistantError::InvalidArgument("missing 'query'".into()))?
        .to_string();
    let top_k = arguments
        .get("top_k")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(50) as usize;
    let recalled = service.recall(&query, top_k).await?;
    Ok(json!({
        "query": query,
        "count": recalled.len(),
        "facts": recalled,
    }))
}

// ---- push 3 meta-tool + knowledge handlers ----------------------------

async fn assistant_do_recall_memory(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_assistant::AssistantError::InvalidArgument("missing 'query'".into()))?
        .to_string();
    let top_k = arguments
        .get("top_k")
        .and_then(|v| v.as_u64())
        .unwrap_or(8)
        .min(50) as usize;
    let facts = service.facts().recall(&query, top_k).await?;
    Ok(json!({
        "preamble": ordo_assistant::MEMORY_PREAMBLE,
        "query": query,
        "top_k": top_k,
        "facts": facts,
    }))
}

async fn assistant_do_knowledge_lookup(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_assistant::AssistantError::InvalidArgument("missing 'query'".into()))?
        .to_string();
    let top_k = arguments
        .get("top_k")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(50) as usize;
    let kind = arguments
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(ordo_assistant::KnowledgeKind::parse);
    let domain = arguments
        .get("domain")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let hits = service
        .knowledge()
        .recall(&query, top_k, kind, domain.as_deref())
        .await?;
    Ok(json!({
        "preamble": ordo_assistant::KNOWLEDGE_PREAMBLE,
        "query": query,
        "top_k": top_k,
        "kind": kind.map(|k| k.as_str()),
        "domain": domain,
        "hits": hits,
    }))
}

async fn assistant_do_parallel_lookup(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_assistant::AssistantError::InvalidArgument("missing 'query'".into()))?
        .to_string();
    let domains: Vec<String> = arguments
        .get("domains")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if domains.is_empty() {
        return Err(ordo_assistant::AssistantError::InvalidArgument(
            "assistant.parallel_lookup requires at least one entry in `domains`".into(),
        ));
    }
    let top_k = arguments
        .get("top_k_per_domain")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .min(50) as usize;
    let kind = arguments
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(ordo_assistant::KnowledgeKind::parse);

    // Run the fanout concurrently Ã¢â‚¬â€ mirrors the in-turn meta-tool.
    let knowledge = service.knowledge().clone();
    let mut handles = Vec::with_capacity(domains.len());
    for domain in &domains {
        let knowledge = knowledge.clone();
        let query = query.clone();
        let domain = domain.clone();
        handles.push(tokio::spawn(async move {
            let hits = knowledge
                .recall(&query, top_k, kind, Some(&domain))
                .await
                .unwrap_or_default();
            (domain, hits)
        }));
    }
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok((domain, hits)) = handle.await {
            results.push(json!({
                "domain": domain,
                "count": hits.len(),
                "hits": hits,
            }));
        }
    }
    Ok(json!({
        "preamble": ordo_assistant::KNOWLEDGE_PREAMBLE,
        "query": query,
        "top_k_per_domain": top_k,
        "kind": kind.map(|k| k.as_str()),
        "domains": domains,
        "results": results,
    }))
}

async fn assistant_do_remember_knowledge(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let new_entry: ordo_assistant::NewKnowledge = serde_json::from_value(arguments.clone())
        .map_err(|err| ordo_assistant::AssistantError::InvalidArgument(err.to_string()))?;
    let entry = service.knowledge().remember(new_entry).await?;
    let summary = ordo_assistant::KnowledgeSummary::from(&entry);
    Ok(serde_json::to_value(&summary).unwrap_or(Value::Null))
}

fn assistant_do_forget_knowledge(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let id = assistant_parse_id(arguments, "id")?;
    let removed = service.knowledge().forget(id)?;
    Ok(json!({ "id": id, "removed": removed }))
}

fn assistant_do_list_knowledge(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let kind = arguments
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(ordo_assistant::KnowledgeKind::parse);
    let domain = arguments
        .get("domain")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let entries = service.knowledge().list(kind, domain.as_deref())?;
    let summaries: Vec<ordo_assistant::KnowledgeSummary> = entries
        .iter()
        .map(ordo_assistant::KnowledgeSummary::from)
        .collect();
    Ok(json!({
        "count": summaries.len(),
        "entries": summaries,
    }))
}

fn assistant_parse_id(
    arguments: &Value,
    field: &str,
) -> Result<uuid::Uuid, ordo_assistant::AssistantError> {
    let raw = arguments
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ordo_assistant::AssistantError::InvalidArgument(format!("missing '{field}'"))
        })?;
    uuid::Uuid::parse_str(raw)
        .map_err(|err| ordo_assistant::AssistantError::InvalidArgument(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{CapabilityProvider, OrdoOpsProvider, ToolCallResult};
    use serde_json::json;

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

    #[tokio::test]
    async fn capture_brief_returns_structured_brief() {
        let result = call(
            "planning.capture_brief",
            json!({
                "title": "Spring Campaign",
                "goal": "Drive awareness",
                "audience": "Planning ops teams",
                "deliverables": ["landing page response", "launch video", "three banner images"],
            }),
        )
        .await;
        assert_eq!(
            result
                .pointer("/brief/deliverable_count")
                .and_then(|value| value.as_u64()),
            Some(3)
        );
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

    #[tokio::test]
    async fn research_audit_readiness_flags_short_title() {
        let result = call(
            "research.audit_readiness",
            json!({
                "title": "short",
                "description": "a description that is long enough to clear the fifty character bar for sure",
                "keywords": ["alpha"],
            }),
        )
        .await;
        assert_eq!(
            result.get("ready").and_then(|value| value.as_bool()),
            Some(false)
        );
        let issues = result
            .get("issues")
            .and_then(|value| value.as_array())
            .expect("issues");
        assert!(!issues.is_empty());
    }

    #[tokio::test]
    async fn content_store_publish_readiness_reports_missing_fields() {
        let result = call(
            "content_store.publish_readiness",
            json!({
                "fields": { "title": "Hello", "body": "world" },
            }),
        )
        .await;
        assert_eq!(
            result.get("ready").and_then(|value| value.as_bool()),
            Some(false)
        );
        let missing = result
            .get("missing")
            .and_then(|value| value.as_array())
            .expect("missing");
        assert_eq!(missing.len(), 2);
    }

    #[tokio::test]
    async fn content_store_field_mapping_canonicalizes_keys() {
        let result = call(
            "content_store.field_mapping",
            json!({
                "source_fields": {
                    "headline": "Hello",
                    "body": "world",
                    "slug": "hello-world",
                },
            }),
        )
        .await;
        let content_store_fields = result
            .get("content_store_fields")
            .and_then(|value| value.as_object())
            .expect("content_store_fields");
        assert!(content_store_fields.contains_key("title"));
        assert!(content_store_fields.contains_key("body"));
        assert!(content_store_fields.contains_key("slug"));
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
    async fn capture_brief_writes_markdown_artifact_to_disk() {
        let root = temp_user_files();
        let result = call_with_root(
            &root,
            "planning.capture_brief",
            json!({
                "title": "Spring Colorway: Trail Runner",
                "goal": "Launch a new spring palette in March",
                "audience": "trail-running customers",
                "deliverables": ["hero video", "landing page", "paid social"],
            }),
        )
        .await;

        let rel = result["artifact_path"]
            .as_str()
            .expect("artifact_path present");
        assert!(
            rel.starts_with("briefs/"),
            "brief should land in briefs/: {rel}"
        );
        assert!(rel.ends_with(".md"));
        let abs = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let body = std::fs::read_to_string(&abs).expect("read artifact");
        assert!(body.contains("# Spring Colorway: Trail Runner"));
        assert!(body.contains("- hero video"));
        assert!(body.contains("- landing page"));
        assert!(body.contains("trail-running customers"));

        let _ = std::fs::remove_dir_all(&root);
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

    #[tokio::test]
    async fn audit_research_readiness_emits_structured_findings() {
        let provider = OrdoOpsProvider::new();
        let result = provider
            .handle_tool_call(
                "research.audit_readiness",
                &json!({
                    "title": "Hi",
                    "description": "short",
                    "keywords": ["trail"],
                    "slug": "Bad Slug!!",
                    "body": "We are launching a new spring color.",
                }),
            )
            .await
            .expect("handled");
        let value = match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("failed: {error}"),
        };
        assert_eq!(value["ready"].as_bool(), Some(false));
        assert!(value["error_count"].as_u64().unwrap_or(0) >= 1);
        let codes: Vec<String> = value["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .filter_map(|f| f["code"].as_str().map(str::to_string))
            .collect();
        assert!(
            codes.contains(&"title_too_short".to_string()),
            "expected title_too_short in {codes:?}"
        );
        assert!(
            codes.contains(&"slug_format".to_string()),
            "expected slug_format in {codes:?}"
        );
        assert!(
            codes.contains(&"keyword_not_covered".to_string()),
            "expected keyword_not_covered in {codes:?}"
        );
    }

    #[tokio::test]
    async fn audit_research_readiness_ready_when_clean() {
        let provider = OrdoOpsProvider::new();
        let result = provider
            .handle_tool_call(
                "research.audit_readiness",
                &json!({
                    "title": "Spring Colorway for Trail Runners",
                    "description": "A new lightweight trail runner colorway engineered for spring weather, built for fast trails and cold mornings.",
                    "keywords": ["trail", "spring", "running"],
                    "slug": "spring-colorway-trail-runner",
                    "body": "Our spring trail running colorway is built for variable weather.",
                }),
            )
            .await
            .expect("handled");
        let value = match result {
            ToolCallResult::Completed { result } => result,
            ToolCallResult::Failed { error } => panic!("failed: {error}"),
        };
        assert_eq!(value["ready"].as_bool(), Some(true));
        assert_eq!(value["error_count"].as_u64(), Some(0));
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
    async fn generate_response_returns_not_configured_without_credential() {
        let provider = OrdoLlmProvider::new(make_task());
        let result = provider
            .handle_tool_call("planning.draft_response", &json!({ "prompt": "hello" }))
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
    async fn generate_response_dispatches_to_openai_when_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-gpt",
                "choices": [
                    {
                        "index": 0,
                        "message": { "role": "assistant", "content": "Draft initiative response." },
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
                "planning.draft_response",
                &json!({
                    "prompt": "launch a spring colorway for the running shoe line",
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
            Some("Draft initiative response.")
        );
        assert_eq!(
            value.get("capability").and_then(|v| v.as_str()),
            Some("planning.draft_response")
        );
        assert_eq!(
            value.get("credential_service").and_then(|v| v.as_str()),
            Some("openai")
        );
    }

    #[tokio::test]
    async fn research_suggest_metadata_routes_to_anthropic_when_credential_style_matches() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-claude",
                "stop_reason": "end_turn",
                "content": [
                    { "type": "text", "text": "{\"title\":\"Spring Run\",\"description\":\"New colorway for spring trails\",\"keywords\":[\"spring\",\"running\"],\"slug\":\"spring-run\"}" }
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
                "research.suggest_metadata",
                &json!({ "prompt": "spring running shoe launch" }),
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
                .map(|s| s.contains("spring-run"))
                .unwrap_or(false),
            "expected assistant_text with Research payload, got: {value:?}"
        );
    }

    /// Stand in for the real RagPeer: listen for `RagQueryRequested` and
    /// reply with a fixed hit on `RAG_QUERY_RESPONSE`.
    async fn fake_rag_peer(bus: Arc<dyn Bus>, fixture_snippet: &'static str) {
        let mut sub = bus
            .subscribe(topics::RAG_QUERY_REQUEST)
            .await
            .expect("subscribe RAG_QUERY_REQUEST");
        while let Some(event) = sub.next().await {
            if let OrdoMessage::RagQueryRequested { query, .. } = &event.payload {
                let mut reply = Envelope::new(
                    NodeId::new(),
                    OrdoMessage::RagQueryCompleted {
                        query: query.clone(),
                        hits: vec![RagHit {
                            document_id: "operator profile-voice".into(),
                            uri: "docs/operator profile-voice.md".into(),
                            title: "Brand Voice".into(),
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
    async fn generate_response_injects_rag_snippets_into_system_prompt() {
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
        tokio::spawn(async move {
            fake_rag_peer(
                rag_bus,
                "BRAND VOICE: Confident, grounded, technical. Never sales-y.",
            )
            .await;
        });

        let provider = OrdoLlmProvider::new(task).with_bus(bus);
        let result = provider
            .handle_tool_call(
                "planning.draft_response",
                &json!({
                    "prompt": "spring trail colorway",
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
    async fn generate_response_skips_rag_when_opted_out() {
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
                "planning.draft_response",
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
    async fn generate_response_routes_through_review_and_substitutes_edit() {
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
                    "planning.draft_response",
                    &json!({
                        "prompt": "spring colorway",
                        "review": true,
                        "review_title": "Spring colorway draft",
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
    async fn generate_response_fails_when_review_denied() {
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
                "planning.draft_response",
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
