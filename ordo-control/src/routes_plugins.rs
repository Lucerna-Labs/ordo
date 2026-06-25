use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) async fn list_plugins(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let path = match &state.plugins_path {
        Some(path) => path.clone(),
        None => {
            return Ok(Json(json!({
                "plugins_dir": null,
                "loaded": [],
                "errors": [],
                "live": [],
            })));
        }
    };
    let report = ordo_plugins::discover_plugins(&path);
    let loaded: Vec<Value> = report
        .loaded
        .iter()
        .map(|loaded| {
            json!({
                "name": loaded.manifest.name,
                "version": loaded.manifest.version,
                "enabled": loaded.manifest.enabled,
                "description": loaded.manifest.description,
                "expected_lanes": loaded.manifest.expected_lanes,
                "manifest_path": loaded.manifest_path.display().to_string(),
            })
        })
        .collect();
    let errors: Vec<Value> = report
        .errors
        .iter()
        .map(|err| {
            json!({
                "manifest_path": err.path.display().to_string(),
                "error": err.error,
            })
        })
        .collect();
    let live: Vec<Value> = state
        .plugin_statuses
        .iter()
        .map(|status| {
            json!({
                "name": status.name,
                "version": status.version,
                "tool_count": status.tool_count,
                "capabilities": status.capabilities,
                "manifest_path": status.manifest_path,
                "state": plugin_state_label(&status.state),
                "state_detail": plugin_state_detail(&status.state),
            })
        })
        .collect();
    Ok(Json(json!({
        "plugins_dir": path.display().to_string(),
        "loaded": loaded,
        "errors": errors,
        "live": live,
    })))
}

pub(crate) fn plugin_state_label(state: &ordo_plugins::PluginState) -> &'static str {
    match state {
        ordo_plugins::PluginState::Active => "active",
        ordo_plugins::PluginState::Disabled => "disabled",
        ordo_plugins::PluginState::Failed(_) => "failed",
        ordo_plugins::PluginState::Invalid(_) => "invalid",
    }
}

pub(crate) fn plugin_state_detail(state: &ordo_plugins::PluginState) -> Option<String> {
    match state {
        ordo_plugins::PluginState::Active | ordo_plugins::PluginState::Disabled => None,
        ordo_plugins::PluginState::Failed(err) | ordo_plugins::PluginState::Invalid(err) => {
            Some(err.clone())
        }
    }
}

pub(crate) async fn set_plugin_enabled(
    State(state): State<ControlApiState>,
    Path(name): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    let enabled = payload
        .get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| ControlApiError::bad_request("missing boolean 'enabled' field"))?;
    mutate_plugin_enabled(&state, &name, enabled)
}

pub(crate) async fn disable_plugin(
    State(state): State<ControlApiState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    mutate_plugin_enabled(&state, &name, false)
}

pub(crate) fn mutate_plugin_enabled(
    state: &ControlApiState,
    name: &str,
    enabled: bool,
) -> Result<Json<Value>, ControlApiError> {
    let path = state.plugins_path.as_ref().ok_or_else(|| {
        ControlApiError::internal("control API was started without a plugins path")
    })?;
    let report = ordo_plugins::discover_plugins(path);
    let manifest_path = report
        .loaded
        .iter()
        .find(|m| m.manifest.name == name)
        .map(|m| m.manifest_path.clone())
        .ok_or_else(|| ControlApiError::bad_request(format!("no plugin named '{name}'")))?;

    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let mut manifest: ordo_plugins::PluginManifest =
        serde_json::from_str(&raw).map_err(|err| ControlApiError::internal(err.to_string()))?;
    manifest.enabled = enabled;
    let updated = serde_json::to_string_pretty(&manifest)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    std::fs::write(&manifest_path, updated)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(json!({
        "name": name,
        "enabled": enabled,
        "manifest_path": manifest_path.display().to_string(),
        "note": "restart the runtime (or call runtime.reload_plugins when available) to apply",
    })))
}

/// Generic capability invocation. Lets the UI and operators reach every
/// registered capability (api.*, runtime.*, knowledge.*, memory.*, cloud.*,
/// and anything else wired into the host) without adding a bespoke route
/// per capability. The capability is a URL path segment so the router
/// stays boring; the body is forwarded unchanged as the argument JSON.
pub(crate) async fn invoke_tool_by_name(
    State(state): State<ControlApiState>,
    Path(capability): Path<String>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ControlApiError> {
    let arguments = body.map(|Json(value)| value).unwrap_or(Value::Null);
    invoke_tool(&state.brain, &capability, arguments).await
}

pub(crate) async fn list_cloud_credentials(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "cloud.credentials.list", json!({})).await
}

pub(crate) async fn upsert_cloud_credential(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "cloud.credentials.upsert", payload).await
}

pub(crate) async fn delete_cloud_credential(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "cloud.credentials.delete", payload).await
}

pub(crate) async fn invoke_tool(
    brain: &Brain,
    capability: &str,
    arguments: Value,
) -> Result<Json<Value>, ControlApiError> {
    match brain.invoke_tool(capability, arguments).await {
        Ok(result) => Ok(Json(result)),
        Err(err) => Err(match err.downcast_ref::<ordo_brain::BrainError>() {
            Some(brain_err) => classify_brain_error(brain_err),
            None => ControlApiError::internal(err.to_string()),
        }),
    }
}

