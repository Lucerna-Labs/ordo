//! Tool gateway Ã¢â‚¬â€ discovers available capabilities and invokes them
//! through the shared bus. Wraps the wire protocol so the rest of
//! `ordo-assistant` can treat tools as plain async functions.
//!
//! We deliberately don't depend on `ordo-brain` here (that would
//! create a cycle because `ordo-mcp-host` now depends on `ordo-assistant`).
//! Instead we implement the same publish-and-correlate pattern Brain
//! uses against the bus topics.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use ordo_bus::Bus;
use ordo_protocol::{topics, CapabilityDescriptor, CorrelationId, Envelope, NodeId, OrdoMessage};
use serde_json::Value;
use tokio::time::timeout;
use uuid::Uuid;

use crate::types::{AssistantError, AssistantResult};

/// Lane prefixes the assistant is allowed to call autonomously.
/// Everything else (cloud.*, runtime.update_*, assistant.*, review.*,
/// self_heal.*, memory pinned writesÃ¢â‚¬Â¦) is off-limits to the LLM by
/// default. Operators can relax this list at construction time.
pub const DEFAULT_ALLOWED_LANES: &[&str] = &[
    "planning.",
    "orchestration.",
    "research.",
    "content_store.",
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
    "rest.",
    "api.",
    "ssh.",
    "brand.",
    "example.",
    // code.* (run code) + workspace.* (read/write files in the confined
    // code workspace) — lets the assistant author and run code. The
    // native runner (code.run_native) is additionally gated at runtime
    // (off unless built with `native-exec` AND ORDO_CODE_ALLOW_NATIVE),
    // so allowing the lane here is safe by default.
    "code.",
    "workspace.",
    // web.strain + web.fetch_and_strain â€” these are the SAFE path
    // for the assistant to read URLs. The strain pipeline is
    // non-skippable on this lane (no raw-fetch capability exists),
    // so allowing the lane just gives the assistant the only door
    // we want it to use.
    "web.",
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
    allowed_lanes: Vec<String>,
    call_timeout: Duration,
    list_timeout: Duration,
}

impl ToolGateway {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self {
            bus,
            allowed_lanes: DEFAULT_ALLOWED_LANES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            call_timeout: Duration::from_secs(45),
            list_timeout: Duration::from_secs(2),
        }
    }

    pub fn with_allowed_lanes<I: IntoIterator<Item = String>>(mut self, lanes: I) -> Self {
        self.allowed_lanes = lanes.into_iter().collect();
        self
    }

    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
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
        Ok(descriptors
            .into_iter()
            .filter(|descriptor| self.is_allowed(&descriptor.capability))
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
            .any(|prefix| capability.starts_with(prefix.as_str()))
    }

    /// Invoke a capability through the bus. Correlated by
    /// `invocation_id`; wraps both the "completed" and "failed"
    /// response variants.
    pub async fn invoke(&self, capability: &str, arguments: Value) -> AssistantResult<Value> {
        if !self.is_allowed(capability) {
            return Err(AssistantError::InvalidArgument(format!(
                "capability '{capability}' is not on the assistant's allow list"
            )));
        }
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
        loop {
            match timeout(self.call_timeout, sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }
                    match event.payload {
                        OrdoMessage::ToolCallCompleted {
                            invocation_id: seen,
                            result,
                            ..
                        } if seen == invocation_id => return Ok(result),
                        OrdoMessage::ToolCallFailed {
                            invocation_id: seen,
                            error,
                            ..
                        } if seen == invocation_id => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_gateway_allows_diagnostic_read_tools() {
        let gateway = ToolGateway {
            bus: Arc::new(ordo_bus::InProcessBus::default()),
            allowed_lanes: DEFAULT_ALLOWED_LANES
                .iter()
                .map(|lane| (*lane).to_string())
                .collect(),
            call_timeout: Duration::from_secs(1),
            list_timeout: Duration::from_secs(1),
        };

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
        let gateway = ToolGateway {
            bus: Arc::new(ordo_bus::InProcessBus::default()),
            allowed_lanes: DEFAULT_ALLOWED_LANES
                .iter()
                .map(|lane| (*lane).to_string())
                .collect(),
            call_timeout: Duration::from_secs(1),
            list_timeout: Duration::from_secs(1),
        };

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
}
