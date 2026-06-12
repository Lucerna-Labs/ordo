//! Adapters that expose `ordo-apps::AppsProvider` and
//! `ordo-files::FilesProvider` as `CapabilityProvider` implementations
//! so the `McpHost` + `ToolGateway` can dispatch to them alongside
//! the native providers. Follow-up 3 of the memory blueprint
//! follow-ups.
//!
//! Why adapters live here (not in ordo-apps / ordo-files):
//!   - `CapabilityProvider` is defined in ordo-mcp-host.
//!   - ordo-apps / ordo-files intentionally don't depend on ordo-mcp-host
//!     (keeps the dep graph shallow â€” those crates are usable
//!     headless).
//!   - ordo-mcp-host is the natural seam for "stuff exposed to the tool
//!     gateway," so adapters here keep the dependency direction
//!     clean.

use async_trait::async_trait;
use ordo_protocol::{CapabilityDescriptor, RagHit};
use serde_json::Value;

use crate::{CapabilityMatch, CapabilityProvider, ProviderRun, ToolCallResult};

/// Bridges `ordo_apps::AppsProvider` into the `CapabilityProvider`
/// surface.
pub struct AppsCapabilityAdapter {
    inner: ordo_apps::AppsProvider,
}

impl AppsCapabilityAdapter {
    pub fn new(inner: ordo_apps::AppsProvider) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl CapabilityProvider for AppsCapabilityAdapter {
    fn name(&self) -> &str {
        "ordo-apps"
    }

    fn capabilities(&self) -> Vec<String> {
        ordo_apps::AppsProvider::capabilities_list()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ordo_apps::AppsProvider::descriptors()
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
        if !capability.starts_with("apps.") {
            return None;
        }
        match self.inner.invoke(capability, arguments).await {
            Ok(result) => Some(ToolCallResult::Completed { result }),
            Err(error) => Some(ToolCallResult::Failed { error }),
        }
    }
}

/// Bridges `ordo_files::FilesProvider` into the `CapabilityProvider`
/// surface.
pub struct FilesCapabilityAdapter {
    inner: ordo_files::FilesProvider,
}

impl FilesCapabilityAdapter {
    pub fn new(inner: ordo_files::FilesProvider) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl CapabilityProvider for FilesCapabilityAdapter {
    fn name(&self) -> &str {
        "ordo-files"
    }

    fn capabilities(&self) -> Vec<String> {
        ordo_files::FilesProvider::capabilities_list()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ordo_files::FilesProvider::descriptors()
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
        if !capability.starts_with("files.") {
            return None;
        }
        match self.inner.invoke(capability, arguments).await {
            Ok(result) => Some(ToolCallResult::Completed { result }),
            Err(error) => Some(ToolCallResult::Failed { error }),
        }
    }
}

/// Bridges `ordo_code::CodeProvider` into the `CapabilityProvider`
/// surface. Unlike the single-namespace adapters above, this one owns
/// BOTH the `code.*` and `workspace.*` namespaces, so the prefix check
/// accepts either.
pub struct CodeCapabilityAdapter {
    inner: ordo_code::CodeProvider,
}

impl CodeCapabilityAdapter {
    pub fn new(inner: ordo_code::CodeProvider) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl CapabilityProvider for CodeCapabilityAdapter {
    fn name(&self) -> &str {
        "ordo-code"
    }

    fn capabilities(&self) -> Vec<String> {
        ordo_code::CodeProvider::capabilities_list()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ordo_code::CodeProvider::descriptors()
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
        // Owns two namespaces — return None for anything else so the
        // host falls through to other providers.
        if !(capability.starts_with("code.") || capability.starts_with("workspace.")) {
            return None;
        }
        match self.inner.invoke(capability, arguments).await {
            Ok(result) => Some(ToolCallResult::Completed { result }),
            Err(error) => Some(ToolCallResult::Failed { error }),
        }
    }
}

/// Bridges `ordo-strainer` into the `CapabilityProvider` surface.
/// Stateless — every call goes through the strainer's pure
/// transform pipeline.
pub struct StrainerCapabilityAdapter;

impl Default for StrainerCapabilityAdapter {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl CapabilityProvider for StrainerCapabilityAdapter {
    fn name(&self) -> &str {
        "ordo-strainer"
    }
    fn capabilities(&self) -> Vec<String> {
        ordo_strainer::capability_descriptors()
            .into_iter()
            .map(|d| d.capability)
            .collect()
    }
    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ordo_strainer::capability_descriptors()
    }
    async fn handle_requirement(&self, _: &str) -> Option<CapabilityMatch> {
        None
    }
    async fn handle_run(&self, _: &str, _: &[RagHit]) -> Option<ProviderRun> {
        None
    }
    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        if !capability.starts_with("web.") {
            return None;
        }
        match ordo_strainer::invoke_capability(capability, arguments).await {
            Ok(Some(value)) => Some(ToolCallResult::Completed { result: value }),
            Ok(None) => Some(ToolCallResult::Failed {
                error: format!("unknown web capability: {capability}"),
            }),
            Err(error) => Some(ToolCallResult::Failed { error }),
        }
    }
}

/// Bridges any `ordo_logic::LogicProvider` impl (the default
/// `LlmLogicProvider`, or a future MCP-backed variant) into the
/// `CapabilityProvider` surface.
///
/// Stored as `Arc<dyn LogicProvider>` so the runtime can swap
/// implementations later without re-wiring the adapter — the
/// loose-coupling promise that motivated `ordo-logic` as a crate
/// in the first place.
pub struct LogicCapabilityAdapter {
    inner: std::sync::Arc<dyn ordo_logic::LogicProvider>,
}

impl LogicCapabilityAdapter {
    pub fn new(inner: std::sync::Arc<dyn ordo_logic::LogicProvider>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl CapabilityProvider for LogicCapabilityAdapter {
    fn name(&self) -> &str {
        "ordo-logic"
    }

    fn capabilities(&self) -> Vec<String> {
        ordo_logic::capability_descriptors()
            .into_iter()
            .map(|d| d.capability)
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ordo_logic::capability_descriptors()
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
        if !capability.starts_with("logic.") {
            return None;
        }
        match ordo_logic::invoke_capability(&*self.inner, capability, arguments).await {
            Ok(Some(result)) => Some(ToolCallResult::Completed { result }),
            // None means the prefix matched but no capability did —
            // unknown logic.* capability. Surface as a failure so the
            // caller sees a clear error rather than silent fallthrough.
            Ok(None) => Some(ToolCallResult::Failed {
                error: format!("unknown logic capability: {capability}"),
            }),
            Err(error) => Some(ToolCallResult::Failed { error }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apps_adapter_routes_apps_calls_only() {
        use ordo_apps::{AppsService, AppsStore};
        let store = AppsStore::in_memory().expect("store");
        let service = AppsService::new(store);
        let provider = ordo_apps::AppsProvider::new(service);
        let adapter = AppsCapabilityAdapter::new(provider);
        // Non-apps capability returns None (doesn't claim it).
        assert!(adapter
            .handle_tool_call("files.list", &serde_json::json!({}))
            .await
            .is_none());
        // apps.list should be handled.
        let out = adapter
            .handle_tool_call("apps.list", &serde_json::json!({}))
            .await;
        assert!(matches!(out, Some(ToolCallResult::Completed { .. })));
        // Descriptors are non-empty and every one starts with apps.
        let descs = adapter.capability_descriptors();
        assert!(!descs.is_empty());
        assert!(descs.iter().all(|d| d.capability.starts_with("apps.")));
    }

    #[tokio::test]
    async fn files_adapter_routes_files_calls_only() {
        use ordo_files::{FilesService, FilesStore};
        let tmp = tempfile::tempdir().expect("tmp");
        let store = FilesStore::in_memory().expect("store");
        let service = FilesService::new(store, tmp.path().to_path_buf());
        let provider = ordo_files::FilesProvider::new(service);
        let adapter = FilesCapabilityAdapter::new(provider);
        assert!(adapter
            .handle_tool_call("apps.list", &serde_json::json!({}))
            .await
            .is_none());
        let out = adapter
            .handle_tool_call("files.list", &serde_json::json!({}))
            .await;
        assert!(matches!(out, Some(ToolCallResult::Completed { .. })));
    }

    #[tokio::test]
    async fn code_adapter_routes_code_and_workspace_only() {
        use ordo_code::{CodePolicy, CodeProvider, CodeService};
        use ordo_sandbox::NullSandbox;
        use std::sync::Arc;
        let tmp = tempfile::tempdir().expect("tmp");
        let service = CodeService::new(
            tmp.path().to_path_buf(),
            Arc::new(NullSandbox),
            Arc::new(NullSandbox),
            CodePolicy::default(),
        );
        let adapter = CodeCapabilityAdapter::new(CodeProvider::new(service));
        // Not owned -> None (falls through to other providers).
        assert!(adapter
            .handle_tool_call("files.list", &serde_json::json!({}))
            .await
            .is_none());
        // workspace.write_file is owned and handled.
        let out = adapter
            .handle_tool_call(
                "workspace.write_file",
                &serde_json::json!({"path": "a.txt", "content": "hi"}),
            )
            .await;
        assert!(matches!(out, Some(ToolCallResult::Completed { .. })));
        // code.* is owned too.
        let descs = adapter.capability_descriptors();
        assert!(descs.iter().any(|d| d.capability == "code.run_native"));
    }
}
