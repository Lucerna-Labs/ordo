//! Tool gateway Ã¢â‚¬â€ discovers available capabilities and invokes them
//! through the shared bus. Wraps the wire protocol so the rest of
//! `ordo-assistant` can treat tools as plain async functions.
//!
//! We deliberately don't depend on `ordo-brain` here (that would
//! create a cycle because `ordo-mcp-host` now depends on `ordo-assistant`).
//! Instead we implement the same publish-and-correlate pattern Brain
//! uses against the bus topics.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ordo_bus::Bus;
use ordo_protocol::{topics, CapabilityDescriptor, CorrelationId, Envelope, NodeId, OrdoMessage};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::time::timeout;
use tool_lightcone_mcp::{ActionRequest, ExecutionEvidence, HostPolicy, ToolCausalEngine};
use uuid::Uuid;

use crate::types::{AssistantError, AssistantResult};

/// Capability lanes the assistant may call. Entries ending in `.` or `_`
/// are prefixes; all other entries are exact capability names.
/// Everything else (cloud.*, runtime.update_*, assistant.*, review.*,
/// self_heal.*, memory pinned writesÃ¢â‚¬Â¦) is off-limits to the LLM by
/// default. Operators can relax this list at construction time.
pub const DEFAULT_ALLOWED_LANES: &[&str] = &[
    "planning.",
    "orchestration.",
    "research.",
    "knowledge.",
    "memory.list_",
    "memory.remember_",
    "filesystem.read_",
    "runtime.describe_",
    "mcp.servers.list",
    "mcp.servers.inspect",
    "mcp.servers.install",
    "mcp.servers.uninstall",
    "mcp.servers.quarantine",
    "mcp.servers.re_authorize",
    "mcp.servers.set_trust",
    "skills.list",
    "skills.get",
    "skills.install",
    "skills.delete",
    "plugins.list",
    "plugins.install",
    "plugins.delete",
    "plugins.set_enabled",
    "agent_teams.list",
    "agent_teams.get",
    "agent_teams.upsert",
    "agent_teams.delete",
    "agent_teams.set_active",
    "automation.list",
    "automation.inspect",
    "logs.system_tail",
    "cloud.credentials.list",
    "cloud.credentials.test",
    "cloud.credentials.models",
    "self_heal.list_cases",
    "rest.describe_endpoint",
    "rest.prepare_request",
    "rest.validate_response",
    "rest.sync_resource",
    "api.describe_client",
    "api.prepare_auth",
    "api.dispatch_webhook",
    "ssh.describe_host",
    "ssh.prepare_command",
    "ssh.sync_workspace",
    "brand.",
    "example.",
    // code.* (run code) + workspace.* (read/write files in the confined
    // code workspace) — lets the assistant author and run code. The
    // native runner (code.run_native) is additionally gated at runtime
    // (off unless built with `native-exec` AND ORDO_CODE_ALLOW_NATIVE),
    // so allowing the lane here is safe by default.
    "code.run",
    "code.run_native",
    "workspace.write_file",
    "workspace.read_file",
    "workspace.list",
    // web.strain + web.fetch_and_strain â€” these are the SAFE path
    // for the assistant to read URLs. The strain pipeline is
    // non-skippable on this lane (no raw-fetch capability exists),
    // so allowing the lane just gives the assistant the only door
    // we want it to use.
    "web.strain",
    "web.fetch_and_strain",
    "web.search",
];

/// Prefixes that are ALWAYS off-limits to the assistant, regardless
/// of what the operator adds to the allow list. These are either
/// recursion traps (assistant.*) or trust-sensitive write / execution
/// surfaces (cloud model calls, generic REST, runtime updates, and
/// self-heal mutations).
pub const RESERVED_FROM_ASSISTANT: &[&str] = &[
    "assistant.",
    "cloud.openai.",
    "cloud.anthropic.",
    "cloud.rest.",
    "cloud.credentials.upsert",
    "cloud.credentials.delete",
    "runtime.update",
    "review.",
    "self_heal.pin",
    "self_heal.forget",
    "self_heal.replay",
];

#[derive(Clone)]
pub struct ToolGateway {
    bus: Arc<dyn Bus>,
    trusted_host: Option<NodeId>,
    allowed_lanes: Vec<String>,
    call_timeout: Duration,
    list_timeout: Duration,
    causal_engine: Arc<tokio::sync::Mutex<ToolCausalEngine>>,
    execution_lock: Arc<tokio::sync::Mutex<()>>,
}

/// Opaque, single-use authorization for one exact mutating call.
///
/// The fields are private and the type is intentionally not `Clone`: only
/// trusted host code in this crate can mint a ticket after operator review,
/// and handing it to `invoke_authorized` consumes it.
pub(crate) struct ToolAuthorization {
    id: Uuid,
    capability: String,
    arguments_hash: [u8; 32],
    expires_at: Instant,
}

/// Capabilities that must never run from an autonomous model response alone.
///
/// This is deliberately prefix-aware for administrative namespaces so adding
/// a new mutator does not silently make it autonomous. Read-only members are
/// carved out explicitly.
pub(crate) fn capability_requires_authorization(capability: &str) -> bool {
    if capability.starts_with("mcp.servers.") {
        return !matches!(capability, "mcp.servers.list" | "mcp.servers.inspect");
    }
    if capability.starts_with("skills.") {
        return !matches!(
            capability,
            "skills.list" | "skills.get" | "skills.audit_routing"
        );
    }
    if capability.starts_with("plugins.") {
        return capability != "plugins.list";
    }
    if capability.starts_with("agent_teams.") {
        return !matches!(capability, "agent_teams.list" | "agent_teams.get");
    }
    if capability.starts_with("automation.") {
        return !matches!(capability, "automation.list" | "automation.inspect");
    }
    if capability.starts_with("self_heal.") {
        return capability != "self_heal.list_cases";
    }

    matches!(
        capability,
        "code.run_native"
            | "api.dispatch_webhook"
            | "api.sync_resource"
            | "rest.sync_resource"
            | "ssh.sync_workspace"
            | "files.delete"
            | "apps.publish"
            | "apps.unpublish"
            | "apps.archive"
            | "apps.unarchive"
            | "cloud.credentials.upsert"
            | "cloud.credentials.delete"
            | "runtime.update"
            | "runtime.update_settings"
            | "review.approve"
            | "review.edit"
            | "review.deny"
            | "webhooks.register"
            | "webhooks.delete"
            | "webhooks.update"
    )
}

pub(crate) fn canonical_arguments_hash(arguments: &Value) -> [u8; 32] {
    // serde_json's default map representation is deterministically ordered.
    // Hash the complete typed value so an approval for one argument set cannot
    // be replayed for another.
    let bytes = serde_json::to_vec(arguments).unwrap_or_default();
    Sha256::digest(bytes).into()
}

impl ToolGateway {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        let combined_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("ordo-assistant lives under the combined workspace")
            .to_path_buf();
        let state_path = std::env::var_os("ORDO_TOOL_LIGHTCONE_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|| combined_root.join(".runtime/tool-lightcone/ordo-state.json"));
        let causal_engine = ToolCausalEngine::load(
            ToolCausalEngine::installed_world_dir(),
            state_path,
            HostPolicy::delegated_to_host(),
        )
        .unwrap_or_else(|error| panic!("Tool-Lightcone startup gate failed: {error}"));
        let causal_status = causal_engine.status();
        tracing::info!(
            target: "ordo_assistant",
            world = %causal_status.world_id,
            world_hash = %causal_status.world_hash,
            rules = causal_status.rule_count,
            transitions = causal_status.edge_count,
            observations = causal_status.training_observations,
            "exclusive Tool-Lightcone gateway ready"
        );
        Self {
            bus,
            trusted_host: None,
            allowed_lanes: DEFAULT_ALLOWED_LANES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            call_timeout: Duration::from_secs(45),
            list_timeout: Duration::from_secs(2),
            causal_engine: Arc::new(tokio::sync::Mutex::new(causal_engine)),
            execution_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(bus: Arc<dyn Bus>) -> Self {
        let causal_engine = ToolCausalEngine::in_memory(
            ToolCausalEngine::installed_world_dir(),
            HostPolicy::delegated_to_host(),
        )
        .expect("load Tool-Lightcone test world");
        Self {
            bus,
            trusted_host: None,
            allowed_lanes: DEFAULT_ALLOWED_LANES
                .iter()
                .map(|lane| (*lane).to_string())
                .collect(),
            call_timeout: Duration::from_secs(1),
            list_timeout: Duration::from_secs(1),
            causal_engine: Arc::new(tokio::sync::Mutex::new(causal_engine)),
            execution_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn with_trusted_host(mut self, node_id: NodeId) -> Self {
        self.trusted_host = Some(node_id);
        self
    }

    pub fn with_allowed_lanes<I: IntoIterator<Item = String>>(mut self, lanes: I) -> Self {
        self.allowed_lanes = lanes.into_iter().collect();
        self
    }

    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    pub(crate) fn authorize_once(
        &self,
        capability: &str,
        arguments: &Value,
        ttl: Duration,
    ) -> AssistantResult<ToolAuthorization> {
        if ttl.is_zero() {
            return Err(AssistantError::InvalidArgument(
                "authorization ticket ttl must be non-zero".into(),
            ));
        }
        Ok(ToolAuthorization {
            id: Uuid::new_v4(),
            capability: capability.to_string(),
            arguments_hash: canonical_arguments_hash(arguments),
            expires_at: Instant::now() + ttl,
        })
    }

    /// Ask the `McpHost` for every registered capability, then filter
    /// down to the ones the assistant is permitted to call. Returns an
    /// empty vec if the inventory is unreachable Ã¢â‚¬â€ the assistant still
    /// functions, just without autonomous tool use.
    pub async fn available_tools(&self) -> AssistantResult<Vec<CapabilityDescriptor>> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(NodeId::new(), OrdoMessage::CapabilityInventoryRequested)
            .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::CAPABILITY_INVENTORY_RESPONSE)
            .await
            .map_err(|err| AssistantError::Bus(err.to_string()))?;
        self.bus
            .publish(topics::CAPABILITY_INVENTORY_REQUEST, envelope)
            .await
            .map_err(|err| AssistantError::Bus(err.to_string()))?;
        let start = tokio::time::Instant::now();
        let mut descriptors: Vec<CapabilityDescriptor> = Vec::new();
        // Multiple providers respond; collect snapshots for a short
        // window, dedupe by capability name.
        while start.elapsed() < self.list_timeout {
            match timeout(
                self.list_timeout.saturating_sub(start.elapsed()),
                sub.next(),
            )
            .await
            {
                Ok(Some(event)) => {
                    if self
                        .trusted_host
                        .as_ref()
                        .is_some_and(|expected| &event.sender != expected)
                    {
                        continue;
                    }
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }
                    if let OrdoMessage::CapabilityInventorySnapshot {
                        descriptors: snapshot,
                        ..
                    } = event.payload
                    {
                        descriptors.extend(snapshot);
                    }
                }
                _ => break,
            }
        }
        descriptors.sort_by(|a, b| a.capability.cmp(&b.capability));
        descriptors.dedup_by(|a, b| a.capability == b.capability);
        let causal_engine = self.causal_engine.lock().await;
        Ok(descriptors
            .into_iter()
            .filter(|descriptor| {
                self.is_allowed(&descriptor.capability)
                    && causal_engine.supports(&descriptor.capability)
            })
            .collect())
    }

    pub fn is_allowed(&self, capability: &str) -> bool {
        if RESERVED_FROM_ASSISTANT
            .iter()
            .any(|reserved| capability.starts_with(reserved))
        {
            return false;
        }
        self.allowed_lanes
            .iter()
            .any(|lane| capability_matches_lane(capability, lane))
    }

    /// Invoke a capability through the bus. Correlated by
    /// `invocation_id`; wraps both the "completed" and "failed"
    /// response variants.
    pub async fn invoke(&self, capability: &str, arguments: Value) -> AssistantResult<Value> {
        if capability_requires_authorization(capability) {
            return Err(AssistantError::InvalidArgument(format!(
                "capability '{capability}' requires an exact, one-use operator authorization"
            )));
        }
        self.invoke_inner(capability, arguments).await
    }

    pub(crate) async fn invoke_authorized(
        &self,
        authorization: ToolAuthorization,
        capability: &str,
        arguments: Value,
    ) -> AssistantResult<Value> {
        if Instant::now() >= authorization.expires_at {
            return Err(AssistantError::InvalidArgument(format!(
                "authorization {} expired before dispatch",
                authorization.id
            )));
        }
        if authorization.capability != capability
            || authorization.arguments_hash != canonical_arguments_hash(&arguments)
        {
            return Err(AssistantError::InvalidArgument(format!(
                "authorization {} does not match the exact capability and arguments",
                authorization.id
            )));
        }
        self.invoke_inner(capability, arguments).await
    }

    async fn invoke_inner(&self, capability: &str, arguments: Value) -> AssistantResult<Value> {
        if !self.is_allowed(capability) {
            return Err(AssistantError::InvalidArgument(format!(
                "capability '{capability}' is not on the assistant's allow list"
            )));
        }
        self.invoke_guarded(capability, arguments, |arguments| async move {
            self.invoke_bus_raw(capability, arguments).await
        })
        .await
    }

    pub(crate) async fn invoke_local<F, Fut>(
        &self,
        capability: &str,
        arguments: Value,
        operation: F,
    ) -> AssistantResult<Value>
    where
        F: FnOnce(Value) -> Fut,
        Fut: Future<Output = AssistantResult<Value>>,
    {
        self.invoke_guarded(capability, arguments, operation).await
    }

    pub async fn causal_status(&self) -> tool_lightcone_mcp::EngineStatus {
        self.causal_engine.lock().await.status()
    }

    async fn invoke_guarded<F, Fut>(
        &self,
        capability: &str,
        arguments: Value,
        operation: F,
    ) -> AssistantResult<Value>
    where
        F: FnOnce(Value) -> Fut,
        Fut: Future<Output = AssistantResult<Value>>,
    {
        let _serial_execution = self.execution_lock.lock().await;
        let permit = {
            let mut engine = self.causal_engine.lock().await;
            let revision = engine.status().state_revision;
            let proposal = engine
                .propose(ActionRequest {
                    goal: format!(
                        "Invoke {capability} through Ordo's exclusive model-facing tool gateway"
                    ),
                    capability: capability.to_string(),
                    arguments: arguments.clone(),
                    expected_state_revision: Some(revision),
                })
                .map_err(|error| {
                    AssistantError::InvalidArgument(format!(
                        "Tool-Lightcone proposal refused '{capability}': {error}"
                    ))
                })?;
            let authorization = engine.authorize(&proposal.proposal_id).map_err(|error| {
                AssistantError::InvalidArgument(format!(
                    "Tool-Lightcone authorization refused '{capability}': {error}"
                ))
            })?;
            engine
                .begin_execution(&authorization.authorization_id, capability, &arguments)
                .map_err(|error| {
                    AssistantError::InvalidArgument(format!(
                        "Tool-Lightcone execution refused '{capability}': {error}"
                    ))
                })?
        };

        match operation(arguments).await {
            Ok(result) => {
                let postcondition_met = provider_result_is_credible(&result);
                let receipt = self
                    .causal_engine
                    .lock()
                    .await
                    .finish_success(
                        &permit.permit_id,
                        ExecutionEvidence {
                            result: result.clone(),
                            postcondition_met,
                            verification_strength: "ordo-provider-receipt".into(),
                            observations: serde_json::json!({
                                "provider_returned_nonempty_result": postcondition_met,
                                "bus_correlation_enforced": true,
                            }),
                        },
                    )
                    .map_err(|error| {
                        AssistantError::LlmFailed(format!(
                            "Tool-Lightcone postcondition refused '{capability}': {error}"
                        ))
                    })?;
                Ok(serde_json::json!({
                    "result": result,
                    "tool_lightcone_receipt": receipt,
                }))
            }
            Err(error) => {
                let failure = self
                    .causal_engine
                    .lock()
                    .await
                    .finish_failure(&permit.permit_id, error.to_string())
                    .map_err(AssistantError::LlmFailed)?;
                Err(AssistantError::LlmFailed(format!(
                    "{error}; Tool-Lightcone verified failure receipt: {}",
                    serde_json::to_string(&failure).unwrap_or_else(|_| "unavailable".into())
                )))
            }
        }
    }

    async fn invoke_bus_raw(&self, capability: &str, arguments: Value) -> AssistantResult<Value> {
        let invocation_id = Uuid::new_v4();
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            NodeId::new(),
            OrdoMessage::ToolCallRequested {
                invocation_id,
                capability: capability.to_string(),
                arguments,
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::TOOL_RESPONSE)
            .await
            .map_err(|err| AssistantError::Bus(err.to_string()))?;
        self.bus
            .publish(topics::TOOL_REQUEST, envelope)
            .await
            .map_err(|err| AssistantError::Bus(err.to_string()))?;
        let deadline = Instant::now() + self.call_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AssistantError::LlmFailed(format!(
                    "tool call '{capability}' timed out"
                )));
            }
            match timeout(remaining, sub.next()).await {
                Ok(Some(event)) => {
                    if self
                        .trusted_host
                        .as_ref()
                        .is_some_and(|expected| &event.sender != expected)
                    {
                        continue;
                    }
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }
                    match event.payload {
                        OrdoMessage::ToolCallCompleted {
                            invocation_id: seen,
                            capability: seen_capability,
                            result,
                        } if seen == invocation_id && seen_capability == capability => {
                            return Ok(result)
                        }
                        OrdoMessage::ToolCallFailed {
                            invocation_id: seen,
                            capability: seen_capability,
                            error,
                        } if seen == invocation_id && seen_capability == capability => {
                            return Err(AssistantError::LlmFailed(error));
                        }
                        _ => continue,
                    }
                }
                Ok(None) => return Err(AssistantError::Bus("tool response stream closed".into())),
                Err(_) => {
                    return Err(AssistantError::LlmFailed(format!(
                        "tool call '{capability}' timed out"
                    )));
                }
            }
        }
    }
}

fn provider_result_is_credible(result: &Value) -> bool {
    match result {
        Value::Null => false,
        Value::Object(object) => !object.contains_key("error"),
        Value::String(text) => !text.trim().is_empty(),
        _ => true,
    }
}

fn capability_matches_lane(capability: &str, lane: &str) -> bool {
    if let Some(namespace) = lane.strip_suffix('.') {
        capability == namespace || capability.starts_with(lane)
    } else if lane.ends_with('_') {
        capability.starts_with(lane)
    } else {
        capability == lane
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_bus::Bus;
    use ordo_protocol::{topics, Envelope, NodeId, OrdoMessage};
    use serde_json::json;

    #[test]
    fn default_gateway_allows_diagnostic_read_tools() {
        let gateway = ToolGateway::for_test(Arc::new(ordo_bus::InProcessBus::default()));

        for capability in [
            "runtime.describe_profile",
            "runtime.describe_storage",
            "runtime.describe_settings",
            "mcp.servers.list",
            "mcp.servers.inspect",
            "mcp.servers.install",
            "mcp.servers.uninstall",
            "mcp.servers.quarantine",
            "mcp.servers.re_authorize",
            "mcp.servers.set_trust",
            "skills.list",
            "skills.install",
            "skills.delete",
            "plugins.list",
            "plugins.install",
            "plugins.delete",
            "plugins.set_enabled",
            "automation.list",
            "automation.inspect",
            "logs.system_tail",
            "cloud.credentials.list",
            "cloud.credentials.test",
            "cloud.credentials.models",
            "self_heal.list_cases",
        ] {
            assert!(
                gateway.is_allowed(capability),
                "{capability} should be allowed"
            );
        }
    }

    #[test]
    fn default_gateway_allows_diagnostic_maintenance_tools_but_blocks_raw_mcp() {
        let gateway = ToolGateway::for_test(Arc::new(ordo_bus::InProcessBus::default()));

        for capability in [
            "mcp.servers.install",
            "mcp.servers.uninstall",
            "mcp.servers.quarantine",
            "mcp.servers.re_authorize",
            "mcp.servers.set_trust",
            "skills.list",
            "skills.install",
            "skills.delete",
            "plugins.list",
            "plugins.install",
            "plugins.delete",
            "plugins.set_enabled",
        ] {
            assert!(
                gateway.is_allowed(capability),
                "{capability} should be allowed for diagnostic maintenance"
            );
        }

        for capability in [
            "runtime.update_settings",
            "automation.create",
            "automation.delete",
            "automation.tick",
            "logs.clear",
            "mcp.servers.invoke_raw",
            "cloud.openai.chat",
            "cloud.anthropic.messages",
            "cloud.rest.request",
            "cloud.credentials.upsert",
            "cloud.credentials.delete",
            "self_heal.pin_case",
            "self_heal.forget_case",
            "self_heal.replay_case",
            "self_heal.export_case",
        ] {
            assert!(
                !gateway.is_allowed(capability),
                "{capability} should be blocked"
            );
        }
    }

    #[test]
    fn administrative_calls_require_exact_authorization() {
        let gateway = ToolGateway::for_test(Arc::new(ordo_bus::InProcessBus::default()));
        assert!(capability_requires_authorization("mcp.servers.install"));
        assert!(capability_requires_authorization("skills.delete"));
        assert!(capability_requires_authorization("code.run_native"));
        assert!(!capability_requires_authorization("mcp.servers.list"));
        assert!(!capability_requires_authorization("workspace.write_file"));
        assert!(!gateway.is_allowed("skills.list_and_delete"));
        assert!(!gateway.is_allowed("mcp.servers.list_secrets"));
        assert!(gateway.is_allowed("skills.list"));
        assert!(gateway.is_allowed("knowledge.answer_question"));

        let approved = json!({"id": "one"});
        let unauthorised =
            futures::executor::block_on(gateway.invoke("skills.delete", approved.clone()))
                .expect_err("privileged call without ticket must fail");
        assert!(unauthorised.to_string().contains("requires"));
        let ticket = gateway
            .authorize_once("skills.delete", &approved, Duration::from_secs(1))
            .unwrap();
        let err = futures::executor::block_on(gateway.invoke_authorized(
            ticket,
            "skills.delete",
            json!({"id": "two"}),
        ))
        .expect_err("ticket must bind exact arguments");
        assert!(err.to_string().contains("does not match"));
    }

    #[tokio::test]
    async fn model_facing_invoke_is_causally_gated_and_receipted() {
        let bus = Arc::new(ordo_bus::InProcessBus::default());
        let gateway = ToolGateway::for_test(bus.clone());
        let responder_bus = bus.clone();
        let responder = tokio::spawn(async move {
            let mut requests = responder_bus
                .subscribe(topics::TOOL_REQUEST)
                .await
                .expect("subscribe");
            let request = requests.next().await.expect("tool request");
            let correlation = request.correlation_id.clone();
            let OrdoMessage::ToolCallRequested {
                invocation_id,
                capability,
                arguments: _,
            } = request.payload
            else {
                panic!("unexpected request");
            };
            let response = Envelope::new(
                NodeId::new(),
                OrdoMessage::ToolCallCompleted {
                    invocation_id,
                    capability,
                    result: json!({"path":"README.md", "bytes":42}),
                },
            );
            let response = match correlation {
                Some(correlation) => response.with_correlation(correlation),
                None => response,
            };
            responder_bus
                .publish(topics::TOOL_RESPONSE, response)
                .await
                .expect("publish result");
        });
        tokio::task::yield_now().await;

        let result = gateway
            .invoke("filesystem.read_file", json!({"path":"README.md"}))
            .await
            .expect("verified invocation");
        responder.await.expect("responder");
        assert_eq!(result["result"]["bytes"], 42);
        assert_eq!(result["tool_lightcone_receipt"]["success"], true);
        assert_eq!(
            result["tool_lightcone_receipt"]["verification_strength"],
            "ordo-provider-receipt"
        );
        assert_eq!(
            result["tool_lightcone_receipt"]["output_cannot_grant_permission"],
            true
        );
        assert_eq!(gateway.causal_status().await.state_revision, 1);
    }

    #[tokio::test]
    async fn allowed_lane_cannot_bypass_an_unknown_causality() {
        let gateway = ToolGateway::for_test(Arc::new(ordo_bus::InProcessBus::default()))
            .with_allowed_lanes(["workspace.".to_string()]);
        let error = gateway
            .invoke("workspace.untrained_raw_action", json!({}))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("admitted no computer-use causality"));
        assert_eq!(gateway.causal_status().await.state_revision, 0);
    }

    #[tokio::test]
    async fn response_must_match_requested_capability() {
        let bus = Arc::new(ordo_bus::InProcessBus::default());
        let trusted_host = NodeId::new();
        let response_host = trusted_host.clone();
        let mut requests = bus.subscribe(topics::TOOL_REQUEST).await.unwrap();
        let responder = bus.clone();
        tokio::spawn(async move {
            let request = requests.next().await.unwrap();
            let correlation = request.correlation_id.unwrap();
            let OrdoMessage::ToolCallRequested { invocation_id, .. } = request.payload else {
                panic!("unexpected request");
            };
            let response = Envelope::new(
                response_host,
                OrdoMessage::ToolCallCompleted {
                    invocation_id,
                    capability: "wrong.capability".into(),
                    result: json!({"forged": true}),
                },
            )
            .with_correlation(correlation);
            responder
                .publish(topics::TOOL_RESPONSE, response)
                .await
                .unwrap();
        });

        let gateway = ToolGateway::for_test(bus)
            .with_trusted_host(trusted_host)
            .with_call_timeout(Duration::from_millis(40));
        let err = gateway
            .invoke("filesystem.read_file", json!({"path":"README.md"}))
            .await
            .expect_err("wrong-capability response must not be accepted");
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn response_must_come_from_bound_host() {
        let bus = Arc::new(ordo_bus::InProcessBus::default());
        let trusted_host = NodeId::new();
        let mut requests = bus.subscribe(topics::TOOL_REQUEST).await.unwrap();
        let responder = bus.clone();
        tokio::spawn(async move {
            let request = requests.next().await.unwrap();
            let correlation = request.correlation_id.unwrap();
            let OrdoMessage::ToolCallRequested {
                invocation_id,
                capability,
                ..
            } = request.payload
            else {
                panic!("unexpected request");
            };
            let response = Envelope::new(
                NodeId::new(),
                OrdoMessage::ToolCallCompleted {
                    invocation_id,
                    capability,
                    result: json!({"forged": true}),
                },
            )
            .with_correlation(correlation);
            responder
                .publish(topics::TOOL_RESPONSE, response)
                .await
                .unwrap();
        });

        let gateway = ToolGateway::for_test(bus)
            .with_trusted_host(trusted_host)
            .with_call_timeout(Duration::from_millis(40));
        let err = gateway
            .invoke("filesystem.read_file", json!({"path":"README.md"}))
            .await
            .expect_err("untrusted sender must not be accepted");
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn irrelevant_bus_traffic_does_not_extend_absolute_deadline() {
        let bus = Arc::new(ordo_bus::InProcessBus::default());
        let spammer = bus.clone();
        tokio::spawn(async move {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let response = Envelope::new(
                    NodeId::new(),
                    OrdoMessage::ToolCallCompleted {
                        invocation_id: Uuid::new_v4(),
                        capability: "irrelevant.capability".into(),
                        result: json!({}),
                    },
                )
                .with_correlation(CorrelationId::new());
                let _ = spammer.publish(topics::TOOL_RESPONSE, response).await;
            }
        });

        let gateway = ToolGateway::for_test(bus).with_call_timeout(Duration::from_millis(80));
        let started = Instant::now();
        let _ = gateway
            .invoke("filesystem.read_file", json!({"path":"README.md"}))
            .await
            .expect_err("call must time out");
        assert!(
            started.elapsed() < Duration::from_millis(180),
            "irrelevant traffic extended the absolute deadline"
        );
    }
}
