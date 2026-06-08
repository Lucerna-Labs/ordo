//! Adapters that expose the **external MCP server stack** to the
//! assistant's tool gateway.
//!
//! Two providers live here:
//!
//! 1. `ExternalMcpToolsProvider` â€” enumerates every installed
//!    MCP server's tool catalog as platform capabilities. When
//!    the assistant calls one of these tools, the call routes
//!    through `McpClientService::invoke` so the full Tier-5
//!    pipeline runs: Worker extraction (untrusted-content
//!    quarantine), DRIFT custody check, taint provenance, the
//!    ToolRiskLevelâ†’ServerTrustState gate, sandbox fuel/memory/
//!    rate caps, and host-call mediation. The Planner never
//!    sees raw tool responses (invariant 25).
//!
//! 2. `McpManagementProvider` â€” exposes `mcp.servers.list /
//!    install / uninstall / quarantine / re_authorize / inspect /
//!    invoke_raw` so the assistant can manage the package layer
//!    itself. These are administrative, run at platform tier
//!    (the assistant is trusted to drive them), and emit
//!    audit-grade bus events the same way the HTTP/CLI surfaces
//!    do.
//!
//! Together they close the loop: an operator (or the assistant
//! on their behalf) can install a new MCP server and the
//! installed server's tools immediately appear as capabilities
//! the assistant can call.

use std::sync::Arc;

use async_trait::async_trait;
use ordo_mcp_client::McpClientService;
use ordo_mcp_registry::{InstalledServer, McpRegistryService};
use ordo_mcp_sandbox::McpSandboxService;
use ordo_protocol::{
    CapabilityActivation, CapabilityDescriptor, CapabilityTier, PrivilegeTier, RagHit,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{CapabilityMatch, CapabilityProvider, ProviderRun, ToolCallResult};

// =================================================================
// ExternalMcpToolsProvider â€” surface installed servers' tools
// =================================================================

/// Wraps the registry + client services so the assistant's tool
/// gateway can dispatch to any installed MCP server's tool catalog.
pub struct ExternalMcpToolsProvider {
    registry: Arc<McpRegistryService>,
    client: Arc<McpClientService>,
}

impl ExternalMcpToolsProvider {
    pub fn new(registry: Arc<McpRegistryService>, client: Arc<McpClientService>) -> Self {
        Self { registry, client }
    }

    /// Find which installed server owns a given tool name.
    /// Tool names are unique within a workspace by convention; if
    /// two servers happen to declare the same tool, the
    /// alphabetically-first server_id wins. The Worker extraction
    /// pipeline then enforces the declared output schema, so the
    /// caller still gets a typed payload â€” but logging the
    /// collision lets the operator address it.
    fn server_for_tool(
        &self,
        tool_name: &str,
    ) -> Option<(InstalledServer, ordo_protocol::ToolSchema)> {
        let mut hits: Vec<(InstalledServer, ordo_protocol::ToolSchema)> = self
            .registry
            .list()
            .into_iter()
            .filter_map(|server| {
                server
                    .tool_catalog
                    .iter()
                    .find(|t| t.name == tool_name)
                    .cloned()
                    .map(|tool| (server, tool))
            })
            .collect();
        hits.sort_by(|a, b| a.0.lockfile.server_id.cmp(&b.0.lockfile.server_id));
        if hits.len() > 1 {
            tracing::warn!(
                target: "ordo_mcp_host::external_mcp",
                tool = tool_name,
                servers = ?hits.iter().map(|(s, _)| &s.lockfile.server_id).collect::<Vec<_>>(),
                "tool name collision across installed servers; picking the first by id"
            );
        }
        hits.into_iter().next()
    }
}

#[async_trait]
impl CapabilityProvider for ExternalMcpToolsProvider {
    fn name(&self) -> &str {
        "external-mcp-tools"
    }

    fn capabilities(&self) -> Vec<String> {
        self.registry
            .list()
            .into_iter()
            .flat_map(|server| {
                server
                    .tool_catalog
                    .into_iter()
                    .map(|t| t.name)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        self.registry
            .list()
            .into_iter()
            .flat_map(|server| {
                let server_id = server.lockfile.server_id.clone();
                // Encode the trust state into the routing identifier
                // so the assistant's taint hook can skip operator-
                // blessed servers without a second registry lookup.
                //
                //   mcp:trusted:<id>  → ServerTrustState::Trusted
                //                       (long clean history OR operator
                //                        explicitly promoted via
                //                        `mcp.servers.set_trust`)
                //   mcp:<id>          → everything else
                //                       (Untrusted / Observed / Validated)
                //
                // Quarantined servers never reach this code — the
                // registry rejects them at invoke time. We only want
                // operator-blessed Trusted to skip auto-taint; the
                // auto-promoted Validated state is not enough — that
                // tier is monitor-only, not "the operator vouched."
                let provider =
                    if matches!(server.trust_state, ordo_protocol::ServerTrustState::Trusted) {
                        format!("mcp:trusted:{server_id}")
                    } else {
                        format!("mcp:{server_id}")
                    };
                server.tool_catalog.into_iter().map(move |tool| {
                    let activation = match tool.risk_level {
                        ordo_protocol::ToolRiskLevel::ReadOnly
                        | ordo_protocol::ToolRiskLevel::Mutating => CapabilityActivation::Eager,
                        ordo_protocol::ToolRiskLevel::Sensitive
                        | ordo_protocol::ToolRiskLevel::HighRisk => CapabilityActivation::Lazy,
                    };
                    let tier = match tool.risk_level {
                        ordo_protocol::ToolRiskLevel::ReadOnly => CapabilityTier::Core,
                        ordo_protocol::ToolRiskLevel::Mutating => CapabilityTier::Optional,
                        _ => CapabilityTier::Heavy,
                    };
                    let mut desc = CapabilityDescriptor::new(
                        tool.name.clone(),
                        provider.clone(),
                        tool.description.clone(),
                        tier,
                        activation,
                    );
                    if !tool.input_schema.is_null() {
                        desc = desc.with_input_schema(tool.input_schema.clone());
                    }
                    desc
                })
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
        let (server, tool) = self.server_for_tool(capability)?;

        // Route through the *full* MCP client pipeline â€” Worker
        // extraction, trust gate, optional provenance check.
        // Tier-5 (UntrustedMcp) is the privilege the Planner sees
        // for the extracted result.
        let result = self
            .client
            .invoke(
                &server.lockfile.server_id,
                &tool,
                arguments.clone(),
                PrivilegeTier::UntrustedMcp,
                None,
            )
            .await;

        match result {
            Ok(invocation) => Some(ToolCallResult::Completed {
                result: invocation.extracted_data,
            }),
            Err(err) => Some(ToolCallResult::Failed {
                error: format!("mcp invocation failed: {err}"),
            }),
        }
    }
}

// =================================================================
// McpManagementProvider â€” assistant-callable mcp.* admin tools
// =================================================================

/// Lets the assistant install / list / uninstall / quarantine /
/// re-authorize / invoke MCP servers programmatically. Each tool
/// is a thin wrapper over the registry + sandbox + client
/// services.
///
/// These are intentionally NOT routed through the external-MCP
/// pipeline (Worker extraction etc.) â€” they're operating ON the
/// MCP layer, not THROUGH it. The output is platform-typed JSON,
/// not untrusted-tool-output, so Tier-5 framing would be wrong.
pub struct McpManagementProvider {
    registry: Arc<McpRegistryService>,
    sandbox: Arc<McpSandboxService>,
}

impl McpManagementProvider {
    pub fn new(registry: Arc<McpRegistryService>, sandbox: Arc<McpSandboxService>) -> Self {
        Self { registry, sandbox }
    }

    pub const CAPABILITY_LIST: &'static [&'static str] = &[
        "mcp.servers.list",
        "mcp.servers.inspect",
        "mcp.servers.install",
        "mcp.servers.uninstall",
        "mcp.servers.quarantine",
        "mcp.servers.re_authorize",
        "mcp.servers.set_trust",
        "mcp.servers.invoke_raw",
    ];
}

#[derive(Deserialize)]
struct InstallArgs {
    server_id: String,
    /// Path to the WASM module on disk OR base64-encoded module
    /// bytes. The path form is convenient when the assistant has
    /// just downloaded a module to user-files.
    #[serde(default)]
    module_path: Option<String>,
    #[serde(default)]
    module_b64: Option<String>,
    identity: ordo_protocol::ServerIdentity,
    declaration: ordo_protocol::CapabilityDeclaration,
    tool_catalog: Vec<ordo_protocol::ToolSchema>,
    #[serde(default)]
    limits: Option<ordo_protocol::ResourceLimits>,
}

#[derive(Deserialize)]
struct ServerIdArgs {
    server_id: String,
}

#[derive(Deserialize)]
struct QuarantineArgs {
    server_id: String,
    reason: String,
}

#[derive(Deserialize)]
struct ReAuthorizeArgs {
    server_id: String,
    declaration: ordo_protocol::CapabilityDeclaration,
    tool_catalog: Vec<ordo_protocol::ToolSchema>,
}

#[derive(Deserialize)]
struct SetTrustArgs {
    server_id: String,
    /// One of "untrusted", "observed", "validated", "trusted",
    /// "quarantined". Always paired with a reason for the audit
    /// trail.
    state: String,
    reason: String,
}

#[derive(Deserialize)]
struct InvokeRawArgs {
    server_id: String,
    tool_name: String,
    #[serde(default)]
    arguments: Value,
}

#[async_trait]
impl CapabilityProvider for McpManagementProvider {
    fn name(&self) -> &str {
        "mcp-management"
    }

    fn capabilities(&self) -> Vec<String> {
        Self::CAPABILITY_LIST
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor::new(
                "mcp.servers.list",
                "mcp-management",
                "List installed MCP servers with their trust state and tool catalogs.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "mcp.servers.inspect",
                "mcp-management",
                "Fetch one server's signed lockfile and current trust state.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "mcp.servers.install",
                "mcp-management",
                "Install an MCP server from a WASM module + manifest. Pass module_path \
                 (file on disk) or module_b64 (base64 bytes), plus identity, declaration, \
                 and tool_catalog. Limits optional.",
                CapabilityTier::Heavy,
                CapabilityActivation::Lazy,
            ),
            CapabilityDescriptor::new(
                "mcp.servers.uninstall",
                "mcp-management",
                "Remove an installed MCP server.",
                CapabilityTier::Optional,
                CapabilityActivation::Lazy,
            ),
            CapabilityDescriptor::new(
                "mcp.servers.quarantine",
                "mcp-management",
                "Block all invocations to a server pending operator review.",
                CapabilityTier::Optional,
                CapabilityActivation::Lazy,
            ),
            CapabilityDescriptor::new(
                "mcp.servers.re_authorize",
                "mcp-management",
                "Approve a drift-detected change by re-signing the lockfile with new \
                 capability declaration + tool catalog. Trust state demotes to Observed.",
                CapabilityTier::Heavy,
                CapabilityActivation::Lazy,
            ),
            CapabilityDescriptor::new(
                "mcp.servers.set_trust",
                "mcp-management",
                "Administratively set a server's trust state. Bypass the time-gated \
                 automatic graduation when the operator vouches for a server (e.g. \
                 their own org's package). Records the reason in the audit trail.",
                CapabilityTier::Heavy,
                CapabilityActivation::Lazy,
            ),
            CapabilityDescriptor::new(
                "mcp.servers.invoke_raw",
                "mcp-management",
                "Invoke a tool directly through the sandbox WITHOUT Worker extraction. \
                 Administrative path â€” prefer the auto-surfaced capability if you want \
                 the full Tier-5 pipeline.",
                CapabilityTier::Heavy,
                CapabilityActivation::Lazy,
            ),
        ]
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
        let result: Result<Value, String> = match capability {
            "mcp.servers.list" => Ok(list_servers_value(&self.registry)),
            "mcp.servers.inspect" => match parse::<ServerIdArgs>(arguments) {
                Ok(args) => match self.registry.get(&args.server_id) {
                    Some(installed) => Ok(json!({
                        "lockfile": installed.lockfile,
                        "trust_state": installed.trust_state.label(),
                        "tool_catalog": installed.tool_catalog,
                        "installed_at": installed.installed_at.to_rfc3339(),
                        "clean_invocation_count": installed.clean_invocation_count,
                    })),
                    None => Err(format!("server {} not installed", args.server_id)),
                },
                Err(e) => Err(e),
            },
            "mcp.servers.install" => install_server(&self.registry, &self.sandbox, arguments).await,
            "mcp.servers.uninstall" => match parse::<ServerIdArgs>(arguments) {
                Ok(args) => {
                    self.sandbox.uninstall(&args.server_id);
                    self.registry
                        .uninstall(&args.server_id)
                        .await
                        .map(|()| json!({ "uninstalled": args.server_id }))
                        .map_err(|e| e.to_string())
                }
                Err(e) => Err(e),
            },
            "mcp.servers.quarantine" => match parse::<QuarantineArgs>(arguments) {
                Ok(args) => self
                    .registry
                    .quarantine(&args.server_id, args.reason)
                    .await
                    .map(|()| json!({ "quarantined": args.server_id }))
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e),
            },
            "mcp.servers.re_authorize" => match parse::<ReAuthorizeArgs>(arguments) {
                Ok(args) => {
                    // Update the sandbox's policy in-place too so
                    // subsequent invocations enforce the new
                    // declared-capability allowlist (otherwise
                    // re-authorization would only update the
                    // signed lockfile while the live policy
                    // continued to enforce the install-time
                    // declaration).
                    let updated_policy = self
                        .sandbox
                        .update_policy(&args.server_id, args.declaration.clone());
                    if !updated_policy {
                        return Some(ToolCallResult::Failed {
                            error: format!(
                                "server {} not present in sandbox; can't re-authorize",
                                args.server_id
                            ),
                        });
                    }
                    self.registry
                        .re_authorize(&args.server_id, &args.tool_catalog, args.declaration)
                        .await
                        .map(|lockfile| json!({ "lockfile": lockfile }))
                        .map_err(|e| e.to_string())
                }
                Err(e) => Err(e),
            },
            "mcp.servers.set_trust" => match parse::<SetTrustArgs>(arguments) {
                Ok(args) => {
                    let target = match args.state.as_str() {
                        "untrusted" => ordo_protocol::ServerTrustState::Untrusted,
                        "observed" => ordo_protocol::ServerTrustState::Observed,
                        "validated" => ordo_protocol::ServerTrustState::Validated,
                        "trusted" => ordo_protocol::ServerTrustState::Trusted,
                        "quarantined" => ordo_protocol::ServerTrustState::Quarantined,
                        other => {
                            return Some(ToolCallResult::Failed {
                                error: format!("unknown trust state `{other}`"),
                            });
                        }
                    };
                    self.registry
                        .set_trust_state(&args.server_id, target, args.reason)
                        .await
                        .map(|()| {
                            json!({ "server_id": args.server_id, "trust_state": target.label() })
                        })
                        .map_err(|e| e.to_string())
                }
                Err(e) => Err(e),
            },
            "mcp.servers.invoke_raw" => match parse::<InvokeRawArgs>(arguments) {
                Ok(args) => {
                    let invocation_id = ulid::Ulid::new().to_string();
                    let arguments_value = if args.arguments.is_null() {
                        json!({})
                    } else {
                        args.arguments
                    };
                    self.sandbox
                        .invoke(
                            &args.server_id,
                            &invocation_id,
                            &args.tool_name,
                            arguments_value,
                        )
                        .await
                        .map(|(raw_response, usage)| {
                            json!({
                                "server_id": args.server_id,
                                "tool": args.tool_name,
                                "invocation_id": invocation_id,
                                "raw_response": raw_response,
                                "resource_usage": usage,
                            })
                        })
                        .map_err(|e| e.to_string())
                }
                Err(e) => Err(e),
            },
            _ => return None,
        };

        Some(match result {
            Ok(value) => ToolCallResult::Completed { result: value },
            Err(error) => ToolCallResult::Failed { error },
        })
    }
}

fn list_servers_value(registry: &McpRegistryService) -> Value {
    let servers: Vec<Value> = registry
        .list()
        .into_iter()
        .map(|s| {
            json!({
                "server_id": s.lockfile.server_id,
                "trust_state": s.trust_state.label(),
                "installed_at": s.installed_at.to_rfc3339(),
                "tool_count": s.tool_catalog.len(),
                "tools": s
                    .tool_catalog
                    .iter()
                    .map(|t| t.name.clone())
                    .collect::<Vec<_>>(),
                "declared_capabilities": s.lockfile.declared_capabilities,
                "clean_invocation_count": s.clean_invocation_count,
            })
        })
        .collect();
    json!({ "servers": servers, "count": servers.len() })
}

async fn install_server(
    registry: &McpRegistryService,
    sandbox: &McpSandboxService,
    arguments: &Value,
) -> Result<Value, String> {
    let args: InstallArgs = parse(arguments)?;

    let module_bytes = match (args.module_path.as_deref(), args.module_b64.as_deref()) {
        (Some(path), _) => std::fs::read(path).map_err(|err| format!("read {path}: {err}"))?,
        (None, Some(b64)) => decode_base64(b64)?,
        (None, None) => return Err("install requires module_path or module_b64".into()),
    };
    if module_bytes.is_empty() {
        return Err("module bytes are empty".into());
    }
    let limits = args.limits.unwrap_or_default();

    sandbox
        .install(
            args.server_id.clone(),
            module_bytes,
            args.declaration.clone(),
            limits.clone(),
        )
        .map_err(|err| err.to_string())?;

    let lockfile = registry
        .install(
            args.server_id.clone(),
            args.identity,
            &args.tool_catalog,
            args.declaration,
            limits,
        )
        .await
        .map_err(|err| err.to_string())?;

    Ok(json!({
        "server_id": args.server_id,
        "lockfile": lockfile,
    }))
}

fn parse<T: for<'de> Deserialize<'de>>(value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|err| format!("invalid arguments: {err}"))
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let trimmed: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let trimmed = trimmed.trim_end_matches('=');
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in trimmed.chars() {
        let v = match c {
            'A'..='Z' => (c as u32) - ('A' as u32),
            'a'..='z' => (c as u32) - ('a' as u32) + 26,
            '0'..='9' => (c as u32) - ('0' as u32) + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid base64 character `{c}`")),
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use ordo_mcp_registry::McpRegistryService;
    use ordo_mcp_sandbox::{McpSandboxService, NullHost};
    use ordo_protocol::{
        CapabilityDeclaration, ResourceLimits, ServerIdentity, ToolRiskLevel, ToolSchema,
    };
    use rand::rngs::OsRng;

    fn test_identity() -> ServerIdentity {
        ServerIdentity {
            name: "test-server".into(),
            version: "0.1".into(),
            publisher: "test".into(),
            sigstore_cert: vec![1, 2, 3],
            identity_hash: [0u8; 32],
        }
    }

    fn tool(name: &str) -> ToolSchema {
        ToolSchema {
            name: name.into(),
            description: format!("tool {name}"),
            input_schema: serde_json::json!({ "type": "object" }),
            output_schema: serde_json::json!({ "type": "object" }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    fn minimal_wat() -> Vec<u8> {
        let wat = r#"
            (module
              (memory (export "memory") 1)
              (global $bump (mut i32) (i32.const 1024))
              (func (export "alloc") (param $n i32) (result i32)
                (local $p i32)
                (local.set $p (global.get $bump))
                (global.set $bump (i32.add (global.get $bump) (local.get $n)))
                (local.get $p))
              (func (export "noop") (param $inp i32) (param $len i32) (result i64)
                (i64.or
                  (i64.shl (i64.extend_i32_u (local.get $inp)) (i64.const 32))
                  (i64.extend_i32_u (local.get $len)))))
        "#;
        wat::parse_str(wat).expect("valid wat")
    }

    #[tokio::test]
    async fn external_tools_provider_advertises_installed_server_tools() {
        let registry = Arc::new(McpRegistryService::new(SigningKey::generate(&mut OsRng)));
        let sandbox = Arc::new(McpSandboxService::new(Arc::new(NullHost)).unwrap());
        sandbox
            .install(
                "srv-x",
                minimal_wat(),
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .unwrap();
        registry
            .install(
                "srv-x".into(),
                test_identity(),
                &[tool("noop"), tool("other")],
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .await
            .unwrap();

        let worker_pool = Arc::new(ordo_mcp_worker::WorkerPool::new(Arc::new(
            ordo_mcp_worker::DeterministicExtractor::default(),
        )));
        let client = Arc::new(ordo_mcp_client::McpClientService::new(
            registry.clone(),
            sandbox.clone(),
            worker_pool,
            SigningKey::generate(&mut OsRng),
        ));
        let provider = ExternalMcpToolsProvider::new(registry, client);

        let caps = provider.capabilities();
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&"noop".to_string()));
        assert!(caps.contains(&"other".to_string()));

        let descs = provider.capability_descriptors();
        assert_eq!(descs.len(), 2);
        assert!(descs.iter().all(|d| d.provider == "mcp:srv-x"));

        // Tool that doesn't exist returns None (caller routes elsewhere)
        let no = provider
            .handle_tool_call("does-not-exist", &serde_json::json!({}))
            .await;
        assert!(no.is_none());
    }

    #[tokio::test]
    async fn management_provider_lists_installed_servers() {
        let registry = Arc::new(McpRegistryService::new(SigningKey::generate(&mut OsRng)));
        let sandbox = Arc::new(McpSandboxService::new(Arc::new(NullHost)).unwrap());
        registry
            .install(
                "srv-a".into(),
                test_identity(),
                &[tool("a")],
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .await
            .unwrap();

        let provider = McpManagementProvider::new(registry, sandbox);
        let result = provider
            .handle_tool_call("mcp.servers.list", &serde_json::json!({}))
            .await
            .expect("handled");
        match result {
            ToolCallResult::Completed { result } => {
                assert_eq!(result["count"].as_u64(), Some(1));
                assert_eq!(result["servers"][0]["server_id"].as_str(), Some("srv-a"));
            }
            ToolCallResult::Failed { error } => panic!("failed: {error}"),
        }
    }

    #[tokio::test]
    async fn management_provider_install_requires_module_source() {
        let registry = Arc::new(McpRegistryService::new(SigningKey::generate(&mut OsRng)));
        let sandbox = Arc::new(McpSandboxService::new(Arc::new(NullHost)).unwrap());
        let provider = McpManagementProvider::new(registry, sandbox);
        let result = provider
            .handle_tool_call(
                "mcp.servers.install",
                &serde_json::json!({
                    "server_id": "srv-b",
                    "identity": {
                        "name": "x", "version": "0.1", "publisher": "p",
                        "sigstore_cert": [1,2,3],
                        "identity_hash": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
                    },
                    "declaration": { "host_functions": [], "domains": [], "filesystem_paths": [], "bus_topics": [], "secret_classes": [] },
                    "tool_catalog": []
                }),
            )
            .await
            .expect("handled");
        match result {
            ToolCallResult::Failed { error } => {
                assert!(error.contains("module_path") || error.contains("module_b64"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }
}
