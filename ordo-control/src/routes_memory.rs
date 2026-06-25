use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) async fn describe_storage(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "runtime.describe_storage", json!({})).await
}

pub(crate) async fn describe_settings(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "runtime.describe_settings", json!({})).await
}

pub(crate) async fn update_settings(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "runtime.update_settings", payload).await
}

pub(crate) async fn list_pinned(
    State(state): State<ControlApiState>,
    Query(query): Query<ListMemoryQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let limit = query.limit.unwrap_or(10);
    invoke_tool(
        &state.brain,
        "memory.list_pinned",
        json!({ "limit": limit }),
    )
    .await
}

pub(crate) async fn list_self_heal_cases(
    State(state): State<ControlApiState>,
    Query(query): Query<ListMemoryQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let limit = query.limit.unwrap_or(10);
    invoke_tool(
        &state.brain,
        "self_heal.list_cases",
        json!({ "limit": limit }),
    )
    .await
}

pub(crate) async fn list_working(
    State(state): State<ControlApiState>,
    Query(query): Query<ListMemoryQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let limit = query.limit.unwrap_or(10);
    invoke_tool(
        &state.brain,
        "memory.list_working",
        json!({ "limit": limit }),
    )
    .await
}

pub(crate) async fn pin_memory(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "memory.pin_note", payload).await
}

pub(crate) async fn unpin_memory(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "memory.unpin_note", payload).await
}

pub(crate) async fn forget_self_heal_case(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "self_heal.forget_case", payload).await
}

pub(crate) async fn pin_self_heal_case(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "self_heal.pin_case", payload).await
}

pub(crate) async fn replay_self_heal_case(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "self_heal.replay_case", payload).await
}

pub(crate) async fn export_self_heal_case(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "self_heal.export_case", payload).await
}

pub(crate) async fn remember_memory(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "memory.remember_note", payload).await
}

pub(crate) async fn list_security_audit(
    State(state): State<ControlApiState>,
    Query(query): Query<ListMemoryQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let limit = query.limit.unwrap_or(50).min(500);
    let Some(security) = &state.security else {
        return Ok(Json(json!({
            "available": false,
            "count": 0,
            "events": [],
        })));
    };
    let events = security.audit.recent(limit);
    Ok(Json(json!({
        "available": true,
        "count": events.len(),
        "events": events,
    })))
}

pub(crate) async fn list_security_rules(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let Some(security) = &state.security else {
        return Ok(Json(json!({
            "available": false,
            "rules": [],
        })));
    };
    let inventory = security.pipeline.rule_inventory();
    Ok(Json(json!({
        "available": true,
        "count": inventory.len(),
        "rules": inventory,
    })))
}

// ---------- assistant ----------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub(crate) struct AssistantListQuery {
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct AssistantFactsQuery {
    pub(crate) subject: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AssistantNewSessionBody {
    #[serde(default)]
    pub(crate) title: Option<String>,
    /// Mode-scoped workspace for the new session. None = General
    /// Assistant. Validated by the assistant service against its
    /// registered modes; unknown id returns 400.
    #[serde(default)]
    pub(crate) mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AssistantRecallBody {
    pub(crate) query: String,
    #[serde(default = "default_recall_top_k")]
    pub(crate) top_k: usize,
}

pub(crate) fn default_recall_top_k() -> usize {
    5
}

