//! `PluginProvider` is the bridge between an MCP plugin subprocess and
//! the Ordo bus. It implements `CapabilityProvider` so the
//! rest of the runtime sees plugin-advertised tools as just another
//! capability lane.

use std::sync::Arc;

use async_trait::async_trait;
use ordo_mcp_host::{CapabilityMatch, CapabilityProvider, ProviderRun, ToolCallResult};
use ordo_protocol::{CapabilityActivation, CapabilityDescriptor, CapabilityTier, RagHit};
use serde_json::Value;

use crate::client::McpClient;
use crate::manifest::LoadedManifest;
use crate::protocol::McpToolDescriptor;

/// A capability provider that forwards every `handle_tool_call` into an
/// MCP plugin and returns the structured result.
pub struct PluginProvider {
    pub(crate) manifest: LoadedManifest,
    pub(crate) client: Arc<McpClient>,
    pub(crate) tools: Vec<McpToolDescriptor>,
    provider_name: String,
}

impl PluginProvider {
    pub fn new(
        manifest: LoadedManifest,
        client: Arc<McpClient>,
        tools: Vec<McpToolDescriptor>,
    ) -> Self {
        let provider_name = format!("plugin:{}", manifest.manifest.name);
        Self {
            manifest,
            client,
            tools,
            provider_name,
        }
    }

    pub fn plugin_name(&self) -> &str {
        &self.manifest.manifest.name
    }

    pub fn tools(&self) -> &[McpToolDescriptor] {
        &self.tools
    }
}

#[async_trait]
impl CapabilityProvider for PluginProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn capabilities(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name.clone()).collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        self.tools
            .iter()
            .map(|tool| {
                CapabilityDescriptor::new(
                    &tool.name,
                    self.name(),
                    tool.description
                        .as_deref()
                        .unwrap_or("Plugin-provided capability."),
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
        if !self.tools.iter().any(|t| t.name == capability) {
            return None;
        }
        let outcome = self.client.call_tool(capability, arguments.clone()).await;
        Some(match outcome {
            Ok(result) => {
                let mut value = serde_json::to_value(&result)
                    .unwrap_or_else(|_| serde_json::json!({"error": "serialize_failed"}));
                // Hoist text blocks to a convenient top-level field so
                // operators don't have to crack open the content array
                // for simple text replies.
                if let Some(text) = result
                    .content
                    .iter()
                    .find_map(|block| block.as_text().map(str::to_string))
                {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("text".into(), Value::String(text));
                    }
                }
                if result.is_error {
                    ToolCallResult::Failed {
                        error: format!("plugin '{}' reported tool error", self.plugin_name()),
                    }
                } else {
                    ToolCallResult::Completed { result: value }
                }
            }
            Err(err) => ToolCallResult::Failed {
                error: format!("plugin '{}' call failed: {err}", self.plugin_name()),
            },
        })
    }
}
