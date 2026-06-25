use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

use parking_lot::Mutex;
use axum::body::Body;
pub(crate) fn classify_brain_error(err: &ordo_brain::BrainError) -> ControlApiError {
    use ordo_brain::BrainError::{
        CapabilityInventoryTimedOut, CapabilityResponseTimedOut, GoalPlanningFailed,
        RagCollectionsTimedOut, RagIngestTimedOut, RagQueryTimedOut, RunTimedOut, SelfHealTimedOut,
        ToolCallFailed, ToolCallRateLimited, ToolCallTimedOut,
    };
    match err {
        // The runaway guard tripped — a client-rate problem, not a server fault.
        ToolCallRateLimited { .. } => ControlApiError::too_many_requests(err.to_string()),
        // A provider ran and reported a failure. The message tells us whether
        // it was the caller's fault (bad input) or ours.
        ToolCallFailed { error, .. } => classify_tool_failure(error, err.to_string()),
        // Anything timing out on the bus is a gateway timeout from HTTP's view.
        ToolCallTimedOut { .. }
        | CapabilityResponseTimedOut
        | CapabilityInventoryTimedOut
        | RunTimedOut { .. }
        | RagIngestTimedOut { .. }
        | RagQueryTimedOut { .. }
        | RagCollectionsTimedOut
        | SelfHealTimedOut { .. } => ControlApiError::gateway_timeout(err.to_string()),
        // Planning failed for an internal reason — genuine 5xx.
        GoalPlanningFailed { .. } => ControlApiError::internal(err.to_string()),
    }
}

/// Classify the free-text error string a provider returned in
/// [`ordo_brain::BrainError::ToolCallFailed`]. The error type is erased to a
/// `String` on the bus, so we key on stable message fragments — the same
/// signals the exhaustive harness keys on — preferring 4xx for anything the
/// caller can fix. When in doubt we keep 500 (a too-low status is worse than a
/// too-high one for a genuine fault).
pub(crate) fn classify_tool_failure(inner: &str, full: String) -> ControlApiError {
    let low = inner.to_ascii_lowercase();
    // Capability genuinely owned by no provider (post-fix this only happens for
    // a truly unknown capability, since providers now report bad args as
    // validation failures rather than declining).
    if low.contains("no provider handled") {
        return ControlApiError::not_found(full);
    }
    // A required credential / external precondition is not configured.
    if low.contains("not configured")
        || low.contains("no compatible credential")
        || low.contains("credential for service")
    {
        return ControlApiError::precondition_failed(full);
    }
    // Client-side validation: missing/invalid arguments.
    if low.contains("invalid argument")
        || low.contains("missing")
        || low.contains("required")
        || low.contains("requires")
        || low.contains("must be")
        || low.contains("must not be empty")
        || low.contains("expected")
        || low.contains("unknown field")
    {
        return ControlApiError::bad_request(full);
    }
    // A referenced entity does not exist.
    if low.contains("not found") || low.contains("no remembered") || low.contains("does not exist")
    {
        return ControlApiError::not_found(full);
    }
    // Unrecognised provider error — treat as a genuine server fault.
    ControlApiError::internal(full)
}

pub(crate) fn parse_collection_query(value: Option<&str>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };

    let collections = value
        .split(',')
        .map(str::trim)
        .filter(|collection| !collection.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    normalize_rag_collections(&collections)
}

// ---------- mcp install/uninstall surface ---------------------------
//
// Wire-level: every operation runs through `McpRegistryService` +
// `McpSandboxService`. The HTTP layer is a thin mirror â€” it never
// invents trust-state transitions or signs lockfiles itself.

#[derive(Debug, Deserialize)]
pub(crate) struct InstallMcpServerBody {
    pub(crate) server_id: String,
    /// Module bytes encoded as base64. Multipart upload is the
    /// alternative; for v1 keep the JSON path simple.
    pub(crate) module_b64: String,
    pub(crate) identity: ordo_protocol::ServerIdentity,
    pub(crate) declaration: ordo_protocol::CapabilityDeclaration,
    pub(crate) tool_catalog: Vec<ordo_protocol::ToolSchema>,
    #[serde(default)]
    pub(crate) limits: Option<ordo_protocol::ResourceLimits>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct McpQuarantineBody {
    pub(crate) reason: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct McpReAuthorizeBody {
    pub(crate) declaration: ordo_protocol::CapabilityDeclaration,
    pub(crate) tool_catalog: Vec<ordo_protocol::ToolSchema>,
}

pub(crate) fn mcp_registry(
    state: &ControlApiState,
) -> Result<&Arc<ordo_mcp_registry::McpRegistryService>, ControlApiError> {
    state.mcp_registry.as_ref().ok_or_else(|| ControlApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "mcp registry service is not wired into the control API".into(),
    })
}

pub(crate) fn mcp_sandbox(
    state: &ControlApiState,
) -> Result<&Arc<ordo_mcp_sandbox::McpSandboxService>, ControlApiError> {
    state.mcp_sandbox.as_ref().ok_or_else(|| ControlApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "mcp sandbox service is not wired into the control API".into(),
    })
}

pub(crate) async fn list_mcp_servers_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = mcp_registry(&state)?;
    let servers: Vec<Value> = registry
        .list()
        .into_iter()
        .map(|s| {
            json!({
                "server_id": s.lockfile.server_id,
                "trust_state": s.trust_state.label(),
                "installed_at": s.installed_at.to_rfc3339(),
                "clean_invocation_count": s.clean_invocation_count,
                "last_clean_invocation_at": s.last_clean_invocation_at.map(|t| t.to_rfc3339()),
                "tool_catalog": s.tool_catalog,
                "declared_capabilities": s.lockfile.declared_capabilities,
                "resource_limits": s.lockfile.resource_limits,
            })
        })
        .collect();
    Ok(Json(json!({ "servers": servers })))
}

pub(crate) async fn install_mcp_server_route(
    State(state): State<ControlApiState>,
    Json(body): Json<InstallMcpServerBody>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = mcp_registry(&state)?;
    let sandbox = mcp_sandbox(&state)?;

    // Decode the WASM module bytes from base64.
    use base64_decoder::decode_b64_standard as decode_b64;
    let module_bytes = decode_b64(&body.module_b64).map_err(|err| {
        ControlApiError::bad_request(format!("module_b64 is not valid base64: {err}"))
    })?;
    if module_bytes.is_empty() {
        return Err(ControlApiError::bad_request("module bytes empty"));
    }

    let limits = body.limits.unwrap_or_default();

    // Sandbox install validates the module is real WASM.
    sandbox
        .install(
            body.server_id.clone(),
            module_bytes,
            body.declaration.clone(),
            limits.clone(),
        )
        .map_err(|err| ControlApiError::bad_request(err.to_string()))?;

    // Registry install signs the lockfile.
    let lockfile = registry
        .install(
            body.server_id.clone(),
            body.identity,
            &body.tool_catalog,
            body.declaration,
            limits,
        )
        .await
        .map_err(|err| ControlApiError::bad_request(err.to_string()))?;

    Ok(Json(json!({
        "server_id": body.server_id,
        "lockfile": lockfile,
    })))
}

pub(crate) async fn uninstall_mcp_server_route(
    State(state): State<ControlApiState>,
    Path(server_id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = mcp_registry(&state)?;
    let sandbox = mcp_sandbox(&state)?;
    sandbox.uninstall(&server_id);
    registry
        .uninstall(&server_id)
        .await
        .map_err(|err| ControlApiError::bad_request(err.to_string()))?;
    Ok(Json(json!({ "uninstalled": server_id })))
}

pub(crate) async fn quarantine_mcp_server_route(
    State(state): State<ControlApiState>,
    Path(server_id): Path<String>,
    Json(body): Json<McpQuarantineBody>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = mcp_registry(&state)?;
    registry
        .quarantine(&server_id, body.reason)
        .await
        .map_err(|err| ControlApiError::bad_request(err.to_string()))?;
    Ok(Json(json!({ "quarantined": server_id })))
}

pub(crate) async fn re_authorize_mcp_server_route(
    State(state): State<ControlApiState>,
    Path(server_id): Path<String>,
    Json(body): Json<McpReAuthorizeBody>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = mcp_registry(&state)?;
    let sandbox = mcp_sandbox(&state)?;
    // Update the live sandbox policy in-place so the new
    // declaration takes effect immediately.
    if !sandbox.update_policy(&server_id, body.declaration.clone()) {
        return Err(ControlApiError::not_found(format!(
            "server {server_id} not present in sandbox; can't re-authorize"
        )));
    }
    let lockfile = registry
        .re_authorize(&server_id, &body.tool_catalog, body.declaration)
        .await
        .map_err(|err| ControlApiError::bad_request(err.to_string()))?;
    Ok(Json(json!({
        "server_id": server_id,
        "lockfile": lockfile,
    })))
}

pub(crate) async fn get_mcp_lockfile_route(
    State(state): State<ControlApiState>,
    Path(server_id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let registry = mcp_registry(&state)?;
    let installed = registry.get(&server_id).ok_or_else(|| {
        ControlApiError::not_found(format!("mcp server {server_id} not installed"))
    })?;
    Ok(Json(json!({
        "lockfile": installed.lockfile,
        "trust_state": installed.trust_state.label(),
    })))
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct InvokeMcpToolBody {
    #[serde(default)]
    pub(crate) arguments: Value,
}

/// Direct sandbox invocation. The MCP client pipeline (Worker
/// extraction, DRIFT, taint provenance) is the *primary* path â€”
/// this raw-sandbox endpoint exists for development +
/// administration where an operator wants to drive a tool by
/// hand. The sandbox still enforces fuel + memory + rate limits +
/// host-call policy, so the invocation isn't unsafe; it just
/// skips the Planner/Worker structure.
pub(crate) async fn invoke_mcp_tool_route(
    State(state): State<ControlApiState>,
    Path((server_id, tool_name)): Path<(String, String)>,
    Json(body): Json<InvokeMcpToolBody>,
) -> Result<Json<Value>, ControlApiError> {
    let sandbox = mcp_sandbox(&state)?;
    let registry = mcp_registry(&state)?;
    if registry.get(&server_id).is_none() {
        return Err(ControlApiError::not_found(format!(
            "mcp server {server_id} not installed"
        )));
    }
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let arguments = if body.arguments.is_null() {
        json!({})
    } else {
        body.arguments
    };
    let (raw_response, usage) = sandbox
        .invoke(&server_id, &invocation_id, &tool_name, arguments)
        .await
        .map_err(|err| ControlApiError::bad_request(err.to_string()))?;
    Ok(Json(json!({
        "server_id": server_id,
        "tool": tool_name,
        "invocation_id": invocation_id,
        "raw_response": raw_response,
        "resource_usage": usage,
    })))
}

// ---------- ui extension install/uninstall --------------------------
//
// Install copies a directory tree (delivered as a JSON map of
// relative path â†’ base64 bytes) into `<ui_extensions_path>/<name>/`.
// Uninstall removes the directory. The list / serve routes above
// pick up the new tree automatically â€” no separate registration.

#[derive(Debug, Deserialize)]
pub(crate) struct InstallUiExtensionBody {
    pub(crate) name: String,
    /// Map of relative path â†’ base64-encoded file bytes.
    /// `ui.json` is required and validated.
    pub(crate) files: std::collections::BTreeMap<String, String>,
}

pub(crate) async fn install_ui_extension_route(
    State(state): State<ControlApiState>,
    Json(body): Json<InstallUiExtensionBody>,
) -> Result<Json<Value>, ControlApiError> {
    let root = state
        .ui_extensions_path
        .as_ref()
        .ok_or_else(|| ControlApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "ui_extensions_path is not configured".into(),
        })?;
    if body.name.is_empty()
        || body.name.contains('/')
        || body.name.contains('\\')
        || body.name.contains("..")
    {
        return Err(ControlApiError::bad_request(
            "extension name must be non-empty and free of path separators",
        ));
    }
    if !body.files.contains_key("ui.json") {
        return Err(ControlApiError::bad_request(
            "extension bundle must include ui.json at the root",
        ));
    }

    let ext_root = root.join(&body.name);
    if ext_root.exists() {
        return Err(ControlApiError::bad_request(format!(
            "extension `{}` already installed; uninstall first",
            body.name
        )));
    }
    std::fs::create_dir_all(&ext_root).map_err(|err| ControlApiError::internal(err.to_string()))?;

    use base64_decoder::decode_b64_standard as decode_b64;
    for (rel, data_b64) in &body.files {
        if rel.contains("..") || rel.starts_with('/') || rel.starts_with('\\') {
            return Err(ControlApiError::bad_request(format!(
                "file path {rel} escapes the extension root"
            )));
        }
        let bytes =
            decode_b64(data_b64).map_err(|err| ControlApiError::bad_request(err.to_string()))?;
        let target = ext_root.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ControlApiError::internal(err.to_string()))?;
        }
        std::fs::write(&target, &bytes)
            .map_err(|err| ControlApiError::internal(err.to_string()))?;
    }

    Ok(Json(json!({
        "installed": body.name,
        "files": body.files.len(),
    })))
}

pub(crate) async fn uninstall_ui_extension_route(
    State(state): State<ControlApiState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let root = state
        .ui_extensions_path
        .as_ref()
        .ok_or_else(|| ControlApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "ui_extensions_path is not configured".into(),
        })?;
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(ControlApiError::bad_request(
            "extension name must be non-empty and free of path separators",
        ));
    }
    let ext_root = root.join(&name);
    if !ext_root.exists() {
        return Err(ControlApiError::not_found(format!(
            "extension `{name}` not installed"
        )));
    }
    std::fs::remove_dir_all(&ext_root).map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(json!({ "uninstalled": name })))
}

// ---------- operator connections (Bluesky, WordPress, OpenAI, ...) -
//
// The studio's Connections tab is the only consumer. Routes are thin
// mirrors over `ordo_connections::ConnectionService`. Secrets only
// flow IN through create/update; they are never returned to the
// caller. Test runs the live tester against the configured backend
// and persists status to the row.

