//! Plugin host â€” spawns, initialises, tracks, and shuts down plugin
//! subprocesses. Produces `PluginProvider`s the runtime can register
//! on the capability bus.

use std::process::Stdio;
use std::sync::Arc;

use ordo_mcp_host::CapabilityProvider;
use tokio::process::Command;
use tracing::{info, warn};

use crate::client::McpClient;
use crate::manifest::{DiscoveryReport, LoadedManifest};
use crate::provider::PluginProvider;
use crate::transport::StdioTransport;

/// Summary of a plugin load attempt â€” surfaced to the CLI and UI so
/// operators can see which plugins came online, which are disabled,
/// and which failed to load (and why).
#[derive(Debug, Clone)]
pub struct PluginLoadStatus {
    pub name: String,
    pub version: String,
    pub state: PluginState,
    pub tool_count: usize,
    pub manifest_path: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum PluginState {
    /// Plugin spawned, handshook, and advertised tools.
    Active,
    /// Manifest was loaded but the operator disabled the plugin.
    Disabled,
    /// Something went wrong during spawn, handshake, or tools/list.
    Failed(String),
    /// Manifest itself failed to parse.
    Invalid(String),
}

/// Bundles a live plugin subprocess with the things the host needs to
/// manage it.
pub struct RunningPlugin {
    pub provider: Arc<PluginProvider>,
    pub transport: Arc<StdioTransport>,
}

pub struct PluginHost {
    pub plugins: Vec<RunningPlugin>,
    pub statuses: Vec<PluginLoadStatus>,
}

impl PluginHost {
    /// Load every manifest in `report`, spawning the ones that pass
    /// validation and are enabled. Never panics and never blocks the
    /// caller on a slow plugin â€” a misbehaving plugin simply lands in
    /// `Failed` state.
    pub async fn from_discovery(report: DiscoveryReport) -> Self {
        let mut plugins = Vec::new();
        let mut statuses = Vec::new();

        for err in report.errors {
            statuses.push(PluginLoadStatus {
                name: err
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                version: String::new(),
                state: PluginState::Invalid(err.error),
                tool_count: 0,
                manifest_path: err.path.display().to_string(),
                capabilities: Vec::new(),
            });
        }

        for loaded in report.loaded {
            if !loaded.manifest.enabled {
                statuses.push(PluginLoadStatus {
                    name: loaded.manifest.name.clone(),
                    version: loaded.manifest.version.clone(),
                    state: PluginState::Disabled,
                    tool_count: 0,
                    manifest_path: loaded.manifest_path.display().to_string(),
                    capabilities: Vec::new(),
                });
                continue;
            }
            match spawn_plugin(&loaded).await {
                Ok(running) => {
                    let capabilities = running.provider.capabilities();
                    statuses.push(PluginLoadStatus {
                        name: loaded.manifest.name.clone(),
                        version: loaded.manifest.version.clone(),
                        state: PluginState::Active,
                        tool_count: running.provider.tools().len(),
                        manifest_path: loaded.manifest_path.display().to_string(),
                        capabilities,
                    });
                    plugins.push(running);
                }
                Err(err) => {
                    warn!(
                        target: "ordo_plugins",
                        plugin = %loaded.manifest.name,
                        error = %err,
                        "plugin failed to load"
                    );
                    statuses.push(PluginLoadStatus {
                        name: loaded.manifest.name.clone(),
                        version: loaded.manifest.version.clone(),
                        state: PluginState::Failed(err),
                        tool_count: 0,
                        manifest_path: loaded.manifest_path.display().to_string(),
                        capabilities: Vec::new(),
                    });
                }
            }
        }

        Self { plugins, statuses }
    }

    /// Kill every spawned subprocess. Idempotent.
    pub async fn shutdown(self) {
        for plugin in self.plugins {
            plugin.transport.shutdown().await;
        }
    }

    /// Consume the host and return the live providers so the runtime
    /// can register each one with the capability host.
    pub fn into_providers(
        self,
    ) -> (
        Vec<Arc<PluginProvider>>,
        Vec<PluginLoadStatus>,
        Vec<Arc<StdioTransport>>,
    ) {
        let mut providers = Vec::new();
        let mut transports = Vec::new();
        for plugin in self.plugins {
            providers.push(plugin.provider);
            transports.push(plugin.transport);
        }
        (providers, self.statuses, transports)
    }
}

async fn spawn_plugin(loaded: &LoadedManifest) -> Result<RunningPlugin, String> {
    let command_path = loaded.resolved_command();
    let mut cmd = Command::new(&command_path);
    cmd.args(&loaded.manifest.args)
        .env_clear()
        .current_dir(&loaded.directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Forward allowlisted env vars verbatim.
    for key in &loaded.manifest.required_env {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    // Apply literal env overrides declared in the manifest (for things
    // like API base URLs the plugin needs).
    for (key, value) in &loaded.manifest.env {
        cmd.env(key, value);
    }
    // Always forward a minimal PATH so the plugin can find e.g. its
    // own interpreter (python, node) if it doesn't resolve the command
    // itself.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(system_root) = std::env::var("SystemRoot") {
        cmd.env("SystemRoot", system_root);
    }

    let child = cmd
        .spawn()
        .map_err(|err| format!("spawn '{}': {err}", command_path.display()))?;
    let transport =
        Arc::new(StdioTransport::new(child).map_err(|err| format!("stdio setup: {err}"))?);
    let client = Arc::new(McpClient::new(transport.clone()));
    client.start_dispatcher().await;

    let init_result = client
        .initialize("ordo", env!("CARGO_PKG_VERSION"))
        .await
        .map_err(|err| format!("initialize handshake: {err}"))?;
    info!(
        target: "ordo_plugins",
        plugin = %loaded.manifest.name,
        server_info = ?init_result.get("serverInfo"),
        "plugin initialized"
    );

    let tools = client
        .list_tools()
        .await
        .map_err(|err| format!("tools/list: {err}"))?;

    // Enforce the manifest's `expected_lanes` claim: every advertised
    // tool must live under one of the declared prefixes. This blocks a
    // plugin from sneaking in extra capabilities the operator never
    // approved.
    if !loaded.manifest.expected_lanes.is_empty() {
        for tool in &tools {
            let allowed = loaded
                .manifest
                .expected_lanes
                .iter()
                .any(|prefix| tool.name.starts_with(prefix));
            if !allowed {
                return Err(format!(
                    "plugin advertised tool '{}' which is outside expected_lanes {:?}",
                    tool.name, loaded.manifest.expected_lanes
                ));
            }
        }
    }

    // Also enforce the reserved-lane guard again (manifest validate
    // already covered expected_lanes; this covers actual advertised
    // names too). A plugin cannot claim `cloud.*` etc. at runtime even
    // if the manifest omitted them.
    if !loaded.manifest.core_override {
        for tool in &tools {
            if crate::manifest::RESERVED_CORE_LANES
                .iter()
                .any(|prefix| tool.name.starts_with(prefix))
            {
                return Err(format!(
                    "plugin tried to register reserved tool '{}'",
                    tool.name
                ));
            }
        }
    }

    let provider = Arc::new(PluginProvider::new(loaded.clone(), client, tools));
    Ok(RunningPlugin {
        provider,
        transport,
    })
}
