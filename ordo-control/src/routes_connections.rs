use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) fn require_connections(
    state: &ControlApiState,
) -> Result<&Arc<ordo_connections::ConnectionService>, ControlApiError> {
    state.connections.as_ref().ok_or_else(|| ControlApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "connections service is not wired into the control API".into(),
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateConnectionBody {
    pub(crate) type_id: String,
    pub(crate) friendly_name: String,
    #[serde(default)]
    pub(crate) fields: Value,
    /// Sealed in the vault on save; never echoed back. Optional even
    /// for types that require a secret so the field can be marked
    /// missing with a structured error.
    #[serde(default)]
    pub(crate) secret: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateConnectionBody {
    pub(crate) friendly_name: String,
    #[serde(default)]
    pub(crate) fields: Value,
    /// `Some(...)` rotates the secret. `None` leaves the existing
    /// vault row in place.
    #[serde(default)]
    pub(crate) secret: Option<String>,
}

pub(crate) fn map_connection_err(err: ordo_connections::ConnectionServiceError) -> ControlApiError {
    use ordo_connections::ConnectionServiceError as E;
    match err {
        E::NotFound(msg) => ControlApiError::not_found(msg),
        E::UnknownType(msg) => ControlApiError::bad_request(format!("unknown type: {msg}")),
        E::BadInput(msg) => ControlApiError::bad_request(msg),
        E::Store(inner) => ControlApiError::internal(inner.to_string()),
        E::Vault(inner) => ControlApiError::internal(inner.to_string()),
    }
}

pub(crate) async fn list_connection_types_route() -> Json<Value> {
    let catalog = ordo_connections::catalog();
    Json(json!({
        "count": catalog.len(),
        "types": catalog,
    }))
}

pub(crate) async fn list_connections_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    let rows = svc.list().await.map_err(map_connection_err)?;
    Ok(Json(json!({
        "count": rows.len(),
        "connections": rows,
    })))
}

pub(crate) async fn create_connection_route(
    State(state): State<ControlApiState>,
    Json(body): Json<CreateConnectionBody>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    let row = svc
        .create(&body.type_id, &body.friendly_name, body.fields, body.secret)
        .await
        .map_err(map_connection_err)?;
    Ok(Json(serde_json::to_value(row).unwrap_or(Value::Null)))
}

pub(crate) async fn get_connection_route(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    let row = svc.get(&id).await.map_err(map_connection_err)?;
    Ok(Json(serde_json::to_value(row).unwrap_or(Value::Null)))
}

pub(crate) async fn update_connection_route(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateConnectionBody>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    let row = svc
        .update(&id, &body.friendly_name, body.fields, body.secret)
        .await
        .map_err(map_connection_err)?;
    Ok(Json(serde_json::to_value(row).unwrap_or(Value::Null)))
}

pub(crate) async fn delete_connection_route(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    svc.delete(&id).await.map_err(map_connection_err)?;
    Ok(Json(json!({ "deleted": id })))
}

pub(crate) async fn test_connection_route(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    let report = svc.test(&id).await.map_err(map_connection_err)?;
    // Re-read the row so the studio gets the persisted status +
    // last_test_at_ms in the same response â€” saves a follow-up GET.
    let row = svc.get(&id).await.map_err(map_connection_err)?;
    Ok(Json(json!({
        "report": report,
        "connection": row,
    })))
}

