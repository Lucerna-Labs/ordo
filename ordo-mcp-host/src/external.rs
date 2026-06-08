//! `ExternalMcpProvider` â€” lets the Ordo Assistant consume
//! third-party MCP servers (GitHub, Linear, Playwright, Figma, etc.)
//! as additional tool lanes.
//!
//! Design
//! ------
//! - Each configured external server gets an `alias`. Every remote
//!   tool is re-exported as `ext.<alias>.<remote_name>` so aliases
//!   can't collide with our native lanes.
//! - Tool discovery happens at `connect()`-time: we send `initialize`
//!   then `tools/list` once per server and cache the result. A refresh
//!   helper is exposed for re-discovery; we don't poll by default.
//! - Tool invocation is a fresh JSON-RPC POST per call. MCP servers
//!   that insist on strict session sequencing can be adapted later
//!   by extending the `Session` type.
//!
//! Security / review
//! -----------------
//! Per the architecture contract, external MCP tool calls are NOT
//! automatically trusted. At runtime wiring, this provider should be
//! wrapped in a `SecurityStack.gate(..., "external_mcp")`. A persistent
//! "first-use approval" layer is deferred â€” today's rollout grants
//! approval for the session; tomorrow we promote that to SQLite-
//! persisted state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ordo_protocol::{CapabilityActivation, CapabilityDescriptor, CapabilityTier, RagHit};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{CapabilityMatch, CapabilityProvider, ProviderRun, ToolCallResult};

const PROTOCOL_VERSION: &str = "2024-11-05";
/// Local prefix stamped onto every remote tool. Guaranteed to not
/// collide with any Ordo native capability namespace
/// (`apps.*`, `files.*`, `assistant.*`, `cloud.*`, etc.).
pub const EXT_PREFIX: &str = "ext.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalMcpServerConfig {
    /// Stable short name used to disambiguate tools across servers.
    /// Becomes the second segment of every re-exported capability
    /// (`ext.<alias>.<remote>`).
    pub alias: String,
    /// Base URL of the remote MCP server. Expected to accept a
    /// JSON-RPC POST at its root or at an explicit path; we send to
    /// the URL as given.
    pub url: String,
    /// Optional bearer token. Sent as `Authorization: Bearer ...` on
    /// every request â€” hosted MCP services (e.g. Linear, Notion)
    /// require this.
    #[serde(default)]
    pub auth_token: Option<String>,
    /// Per-call timeout in seconds. Defaults to 60.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    60
}

#[derive(Debug, Clone)]
struct RemoteToolEntry {
    /// Local capability string (`ext.<alias>.<remote_name>`).
    local: String,
    /// Alias of the server this tool lives on.
    alias: String,
    /// Remote tool name as the server declared it.
    remote: String,
    /// Description echoed from the remote, or a generated one if the
    /// server didn't give us a description.
    description: String,
    /// Optional input schema reported by the remote. Passed through
    /// verbatim (providers trust the remote's own schema).
    input_schema: Option<Value>,
}

struct Server {
    config: ExternalMcpServerConfig,
    http: reqwest::Client,
}

pub struct ExternalMcpProvider {
    servers: Arc<HashMap<String, Arc<Server>>>,
    tools: Arc<Mutex<Vec<RemoteToolEntry>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalMcpError {
    #[error("transport error talking to {alias}: {cause}")]
    Transport { alias: String, cause: String },
    #[error("{alias} rejected request: {code} {message}")]
    Rpc {
        alias: String,
        code: i64,
        message: String,
    },
    #[error("{alias} returned malformed response: {message}")]
    Malformed { alias: String, message: String },
    #[error("alias '{0}' is not configured")]
    UnknownAlias(String),
}

impl ExternalMcpProvider {
    /// Build and eagerly connect. Any server that fails to initialize
    /// is logged as a warning and skipped â€” its tools will be absent
    /// from `capabilities()` until `refresh()` succeeds. This keeps a
    /// single broken remote from taking down the Assistant.
    pub async fn connect(configs: Vec<ExternalMcpServerConfig>) -> Self {
        let mut servers = HashMap::new();
        for cfg in configs {
            let http = match reqwest::Client::builder()
                .timeout(Duration::from_secs(cfg.timeout_secs))
                .build()
            {
                Ok(client) => client,
                Err(err) => {
                    tracing::warn!(
                        target: "ordo_mcp_host::external",
                        alias = %cfg.alias,
                        error = %err,
                        "failed to build HTTP client; skipping external mcp server"
                    );
                    continue;
                }
            };
            servers.insert(cfg.alias.clone(), Arc::new(Server { config: cfg, http }));
        }
        let provider = Self {
            servers: Arc::new(servers),
            tools: Arc::new(Mutex::new(Vec::new())),
        };
        provider.refresh().await;
        provider
    }

    /// Rediscover tools on every configured server. Called once at
    /// construction; callers can invoke it again to pick up newly-
    /// added remote tools without a restart.
    pub async fn refresh(&self) {
        let mut collected: Vec<RemoteToolEntry> = Vec::new();
        for server in self.servers.values() {
            match discover_tools(server.clone()).await {
                Ok(mut entries) => collected.append(&mut entries),
                Err(err) => {
                    tracing::warn!(
                        target: "ordo_mcp_host::external",
                        alias = %server.config.alias,
                        error = %err,
                        "tool discovery failed"
                    );
                }
            }
        }
        *self.tools.lock() = collected;
    }

    /// Snapshot of currently known tools. Cheap clone â€” the underlying
    /// storage is an `Arc<Mutex<Vec<_>>>`.
    pub fn tool_count(&self) -> usize {
        self.tools.lock().len()
    }

    async fn dispatch_remote(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Result<Value, ExternalMcpError> {
        let (alias, remote_name) = parse_capability(capability)
            .ok_or_else(|| ExternalMcpError::UnknownAlias(capability.to_string()))?;
        let server = self
            .servers
            .get(&alias)
            .ok_or_else(|| ExternalMcpError::UnknownAlias(alias.clone()))?
            .clone();
        call_tool(&server, &remote_name, arguments).await
    }
}

async fn discover_tools(server: Arc<Server>) -> Result<Vec<RemoteToolEntry>, ExternalMcpError> {
    // Minimal handshake: initialize, then tools/list. Ignore the
    // initialize response â€” we only need the tool inventory.
    let _ = rpc(
        &server,
        "initialize",
        Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "clientInfo": {"name": "ordo-assistant", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {}
        })),
    )
    .await?;
    let list = rpc(&server, "tools/list", None).await?;
    let tools = list
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut entries = Vec::with_capacity(tools.len());
    for tool in tools {
        let Some(remote_name) = tool.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let description = tool
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("External MCP tool.")
            .to_string();
        let input_schema = tool.get("inputSchema").cloned();
        let local = format!("{EXT_PREFIX}{}.{remote_name}", server.config.alias);
        entries.push(RemoteToolEntry {
            local,
            alias: server.config.alias.clone(),
            remote: remote_name.to_string(),
            description,
            input_schema,
        });
    }
    Ok(entries)
}

async fn call_tool(
    server: &Server,
    remote_name: &str,
    arguments: &Value,
) -> Result<Value, ExternalMcpError> {
    let result = rpc(
        server,
        "tools/call",
        Some(json!({
            "name": remote_name,
            "arguments": arguments,
        })),
    )
    .await?;
    Ok(result)
}

async fn rpc(
    server: &Server,
    method: &str,
    params: Option<Value>,
) -> Result<Value, ExternalMcpError> {
    let id = uuid::Uuid::new_v4().to_string();
    let mut body = serde_json::Map::new();
    body.insert("jsonrpc".into(), json!("2.0"));
    body.insert("id".into(), json!(id));
    body.insert("method".into(), json!(method));
    if let Some(params) = params {
        body.insert("params".into(), params);
    }

    let mut req = server.http.post(&server.config.url);
    if let Some(token) = &server.config.auth_token {
        req = req.bearer_auth(token);
    }
    req = req.json(&Value::Object(body));
    let response = req
        .send()
        .await
        .map_err(|err| ExternalMcpError::Transport {
            alias: server.config.alias.clone(),
            cause: err.to_string(),
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| ExternalMcpError::Transport {
            alias: server.config.alias.clone(),
            cause: err.to_string(),
        })?;
    if !status.is_success() {
        return Err(ExternalMcpError::Transport {
            alias: server.config.alias.clone(),
            cause: format!("HTTP {}: {}", status.as_u16(), text),
        });
    }
    let value: Value = serde_json::from_str(&text).map_err(|err| ExternalMcpError::Malformed {
        alias: server.config.alias.clone(),
        message: format!("{err}: {text}"),
    })?;
    if let Some(error) = value.get("error") {
        let code = error.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
        let message = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        return Err(ExternalMcpError::Rpc {
            alias: server.config.alias.clone(),
            code,
            message,
        });
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

fn parse_capability(capability: &str) -> Option<(String, String)> {
    let rest = capability.strip_prefix(EXT_PREFIX)?;
    let (alias, remote) = rest.split_once('.')?;
    if alias.is_empty() || remote.is_empty() {
        return None;
    }
    Some((alias.to_string(), remote.to_string()))
}

#[async_trait]
impl CapabilityProvider for ExternalMcpProvider {
    fn name(&self) -> &str {
        "external-mcp"
    }

    fn capabilities(&self) -> Vec<String> {
        self.tools.lock().iter().map(|t| t.local.clone()).collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        self.tools
            .lock()
            .iter()
            .map(|entry| {
                let desc = CapabilityDescriptor::new(
                    entry.local.clone(),
                    format!("external-mcp:{}", entry.alias),
                    format!(
                        "[ext:{}] {} â€” remote tool `{}`",
                        entry.alias, entry.description, entry.remote
                    ),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                );
                match &entry.input_schema {
                    Some(schema) => desc.with_input_schema(schema.clone()),
                    None => desc,
                }
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        // External MCP tools surface explicitly via their advertised
        // names; we don't try to pattern-match requirements to remote
        // tools (that would invite accidental routing to a remote
        // the operator didn't intend).
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
        if !capability.starts_with(EXT_PREFIX) {
            return None;
        }
        match self.dispatch_remote(capability, arguments).await {
            Ok(result) => Some(ToolCallResult::Completed { result }),
            Err(err) => Some(ToolCallResult::Failed {
                error: err.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parse_capability_splits_alias_and_remote() {
        let (alias, remote) = parse_capability("ext.github.create_issue").unwrap();
        assert_eq!(alias, "github");
        assert_eq!(remote, "create_issue");

        assert!(parse_capability("apps.list").is_none());
        assert!(parse_capability("ext.").is_none());
        assert!(parse_capability("ext.foo").is_none());
        assert!(parse_capability("ext..bar").is_none());
    }

    fn initialize_response() -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": "anything",
            "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": {"name": "mock", "version": "0.0"},
                "capabilities": {"tools": {"listChanged": false}}
            }
        })
    }

    fn tools_list_response(tools: Vec<serde_json::Value>) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": "anything",
            "result": {"tools": tools}
        })
    }

    #[tokio::test]
    async fn discover_populates_capabilities_with_prefix() {
        let server = MockServer::start().await;
        // wiremock returns the first matching response per mount, so
        // we mount two mocks matched on body contents to differentiate
        // initialize vs tools/list. Simpler: return a single response
        // that answers initialize once, then tools/list. Since we POST
        // twice, we can just match both to the same mock if we
        // return something that works for either â€” but tools/list
        // needs {"tools": [...]} and initialize just needs a result.
        // Use two mocks matching on the JSON body.
        Mock::given(method("POST"))
            .and(path("/"))
            .and(wiremock::matchers::body_string_contains("\"initialize\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(initialize_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(wiremock::matchers::body_string_contains("\"tools/list\""))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(tools_list_response(vec![json!({
                    "name": "echo",
                    "description": "Echo a string back.",
                    "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}
                })])),
            )
            .mount(&server)
            .await;

        let provider = ExternalMcpProvider::connect(vec![ExternalMcpServerConfig {
            alias: "mock".into(),
            url: server.uri(),
            auth_token: None,
            timeout_secs: 10,
        }])
        .await;

        let caps = provider.capabilities();
        assert_eq!(caps, vec!["ext.mock.echo"]);
        let descriptors = provider.capability_descriptors();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].capability, "ext.mock.echo");
        assert!(descriptors[0].input_schema.is_some());
    }

    #[tokio::test]
    async fn tool_call_forwards_to_remote_and_returns_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(wiremock::matchers::body_string_contains("\"initialize\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(initialize_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(wiremock::matchers::body_string_contains("\"tools/list\""))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(tools_list_response(vec![
                    json!({"name": "echo", "description": "e", "inputSchema": null}),
                ])),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(wiremock::matchers::body_string_contains("\"tools/call\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": "x",
                "result": {"content": [{"type": "text", "text": "hello"}]}
            })))
            .mount(&server)
            .await;

        let provider = ExternalMcpProvider::connect(vec![ExternalMcpServerConfig {
            alias: "mock".into(),
            url: server.uri(),
            auth_token: None,
            timeout_secs: 10,
        }])
        .await;

        let result = provider
            .handle_tool_call("ext.mock.echo", &json!({"text": "hi"}))
            .await
            .expect("completed");
        match result {
            ToolCallResult::Completed { result } => {
                assert_eq!(result["content"][0]["text"], "hello");
            }
            ToolCallResult::Failed { error } => panic!("unexpected failure: {error}"),
        }
    }

    #[tokio::test]
    async fn unknown_alias_is_rejected_distinctly() {
        let provider = ExternalMcpProvider::connect(vec![]).await;
        assert_eq!(provider.tool_count(), 0);
        let result = provider
            .handle_tool_call("ext.ghost.anything", &json!({}))
            .await;
        match result {
            Some(ToolCallResult::Failed { error }) => {
                assert!(error.contains("ghost"), "got: {error}");
            }
            other => panic!("expected failed variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_ext_capability_is_not_handled() {
        let provider = ExternalMcpProvider::connect(vec![]).await;
        let result = provider.handle_tool_call("apps.list", &json!({})).await;
        assert!(result.is_none(), "should not claim non-ext capabilities");
    }
}
