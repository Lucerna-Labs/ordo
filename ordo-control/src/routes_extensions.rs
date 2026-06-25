use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::{ControlApiError, ControlApiState};

// ─── Config persistence ─────────────────────────────────────────

/// A user-configured external MCP server (URL-based, no WASM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalServerEntry {
    pub alias: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    60
}

/// Manages the list of external MCP server connections.
/// Stored as JSON in `<user_files>/mcp-extensions.json`.
pub struct ExtensionRegistry {
    entries: RwLock<Vec<ExternalServerEntry>>,
    config_path: PathBuf,
}

impl ExtensionRegistry {
    pub fn new(user_files: &PathBuf) -> Self {
        let config_path = user_files.join("mcp-extensions.json");
        let entries = if config_path.exists() {
            match std::fs::read_to_string(&config_path) {
                Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        Self {
            entries: RwLock::new(entries),
            config_path,
        }
    }

    async fn persist(&self) {
        let entries = self.entries.read().await;
        if let Ok(json) = serde_json::to_string_pretty(&*entries) {
            let _ = std::fs::write(&self.config_path, json);
        }
    }

    pub async fn list(&self) -> Vec<ExternalServerEntry> {
        self.entries.read().await.clone()
    }

    pub async fn add(&self, entry: ExternalServerEntry) {
        let mut entries = self.entries.write().await;
        // Replace if alias already exists
        entries.retain(|e| e.alias != entry.alias);
        entries.push(entry);
        drop(entries);
        self.persist().await;
    }

    pub async fn remove(&self, alias: &str) -> bool {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|e| e.alias != alias);
        let removed = entries.len() < before;
        drop(entries);
        if removed {
            self.persist().await;
        }
        removed
    }
}

// ─── Request/Response types ─────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct ConnectExtensionBody {
    /// The MCP server URL (e.g. "https://mcp.linear.app/sse")
    pub(crate) url: String,
    /// Optional friendly name. Derived from URL if not provided.
    #[serde(default)]
    pub(crate) alias: Option<String>,
    /// Optional bearer token for authenticated MCP services.
    #[serde(default)]
    pub(crate) auth_token: Option<String>,
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
}

// ─── Route handlers ─────────────────────────────────────────────

/// `GET /api/extensions` — list all configured MCP extensions.
pub(crate) async fn list_extensions(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = state.extension_registry.as_ref().ok_or_else(|| {
        ControlApiError::internal("extension registry not configured")
    })?;
    let entries = registry.list().await;
    Ok(Json(json!({
        "extensions": entries,
        "count": entries.len(),
    })))
}

/// `POST /api/extensions/connect` — connect to an MCP server by URL.
/// This is the "just paste the URL" path — no WASM, no base64.
pub(crate) async fn connect_extension(
    State(state): State<ControlApiState>,
    Json(body): Json<ConnectExtensionBody>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = state.extension_registry.as_ref().ok_or_else(|| {
        ControlApiError::internal("extension registry not configured")
    })?;

    // Validate URL
    let parsed = url::Url::parse(&body.url)
        .map_err(|_| ControlApiError::bad_request(format!("Invalid URL: {}", body.url)))?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ControlApiError::bad_request(
            "URL must be http:// or https://",
        ));
    }

    // Derive alias from URL host if not provided
    let alias = body.alias.unwrap_or_else(|| {
        parsed
            .host_str()
            .unwrap_or("extension")
            .split('.')
            .next()
            .unwrap_or("extension")
            .to_lowercase()
    });

    // Sanitize alias — only alphanumeric + dash
    let alias: String = alias
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .to_lowercase();

    if alias.is_empty() {
        return Err(ControlApiError::bad_request(
            "Could not derive a valid alias from the URL. Please provide an 'alias' field.",
        ));
    }

    // Test the connection by doing a quick MCP handshake
    let timeout_secs = body.timeout_secs.unwrap_or(60);
    let test_result = test_mcp_connection(
        &body.url,
        body.auth_token.as_deref(),
        timeout_secs,
    )
    .await;

    let entry = ExternalServerEntry {
        alias: alias.clone(),
        url: body.url.clone(),
        auth_token: body.auth_token.clone(),
        timeout_secs,
    };

    registry.add(entry).await;

    let connected_tools = match &test_result {
        Ok(tools) => {
            let tool_names: Vec<&str> = tools.iter().take(20).map(|t| t.as_str()).collect();
            json!({
                "discovered": true,
                "tool_count": tools.len(),
                "tools_preview": tool_names,
            })
        }
        Err(e) => json!({
            "discovered": false,
            "warning": e,
            "note": "Server saved but tool discovery failed. It may need authentication or use a different MCP protocol version.",
        }),
    };

    Ok(Json(json!({
        "alias": alias,
        "url": body.url,
        "status": "connected",
        "tools": connected_tools,
        "message": format!("Extension '{}' added. Restart Ordo to activate it in the assistant.", alias),
    })))
}

/// `DELETE /api/extensions/:alias` — remove an MCP extension.
pub(crate) async fn disconnect_extension(
    State(state): State<ControlApiState>,
    AxumPath(alias): AxumPath<String>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = state.extension_registry.as_ref().ok_or_else(|| {
        ControlApiError::internal("extension registry not configured")
    })?;

    let removed = registry.remove(&alias).await;

    if !removed {
        return Err(ControlApiError::not_found(format!(
            "No extension with alias '{}'",
            alias
        )));
    }

    Ok(Json(json!({
        "disconnected": alias,
        "message": "Extension removed. Restart Ordo to fully deactivate it.",
    })))
}

// ─── MCP handshake test ──────────────────────────────────────────

/// Attempts an MCP initialize + tools/list to verify the server
/// is reachable and discover its tools.
async fn test_mcp_connection(
    url: &str,
    auth_token: Option<&str>,
    timeout_secs: u64,
) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let mut req = client.post(url).header(
        "Content-Type",
        "application/json",
    );

    if let Some(token) = auth_token {
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
    }

    // MCP initialize
    let init_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "ordo", "version": env!("CARGO_PKG_VERSION")}
        }
    });

    let resp = req
        .json(&init_body)
        .send()
        .await
        .map_err(|e| format!("Connection failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server returned HTTP {}", resp.status()));
    }

    // tools/list
    let list_body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list"
    });

    let mut req2 = client.post(url).header("Content-Type", "application/json");
    if let Some(token) = auth_token {
        if !token.is_empty() {
            req2 = req2.header("Authorization", format!("Bearer {token}"));
        }
    }

    let list_resp = req2
        .json(&list_body)
        .send()
        .await
        .map_err(|e| format!("tools/list request failed: {e}"))?;

    let list_json: Value = list_resp
        .json()
        .await
        .map_err(|e| format!("Could not parse tools/list response: {e}"))?;

    let tools = list_json["result"]["tools"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(tools)
}
