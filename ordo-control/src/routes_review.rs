use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

use axum::http::HeaderValue;
pub(crate) fn require_review(state: &ControlApiState) -> Result<&ordo_review::ReviewService, ControlApiError> {
    state
        .review
        .as_ref()
        .ok_or_else(|| ControlApiError::internal("review service not configured on this runtime"))
}

pub(crate) fn parse_review_id(raw: &str) -> Result<uuid::Uuid, ControlApiError> {
    uuid::Uuid::parse_str(raw)
        .map_err(|err| ControlApiError::bad_request(format!("invalid review id: {err}")))
}

pub(crate) fn map_review_error(err: ordo_review::ReviewError) -> ControlApiError {
    use ordo_review::ReviewError::*;
    match err {
        NotFound(id) => ControlApiError::bad_request(format!("review request '{id}' not found")),
        AlreadyResolved(id, state) => ControlApiError::bad_request(format!(
            "review request '{id}' already resolved ({state})"
        )),
        InvalidArgument(msg) => ControlApiError::bad_request(msg),
        Storage(msg) => ControlApiError::internal(msg),
    }
}

pub(crate) async fn list_review_pending(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_review(&state)?;
    let pending = service.pending().map_err(map_review_error)?;
    Ok(Json(json!({
        "count": pending.len(),
        "pending": pending,
    })))
}

pub(crate) async fn list_review_recent(
    State(state): State<ControlApiState>,
    Query(query): Query<ReviewRecentQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_review(&state)?;
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let recent = service.recent(limit).map_err(map_review_error)?;
    Ok(Json(json!({
        "count": recent.len(),
        "recent": recent,
    })))
}

pub(crate) async fn get_review_request(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_review(&state)?;
    let uuid = parse_review_id(&id)?;
    let request = service
        .get(uuid)
        .map_err(map_review_error)?
        .ok_or_else(|| ControlApiError::bad_request(format!("review request '{id}' not found")))?;
    Ok(Json(serde_json::to_value(request).unwrap_or(Value::Null)))
}

pub(crate) async fn approve_review_request(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
    body: Option<Json<ReviewDecisionBody>>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_review(&state)?;
    let uuid = parse_review_id(&id)?;
    let note = body.and_then(|Json(payload)| payload.note);
    let resolved = service
        .decide(uuid, ordo_review::ReviewDecisionKind::Approve { note })
        .map_err(map_review_error)?;
    Ok(Json(serde_json::to_value(resolved).unwrap_or(Value::Null)))
}

pub(crate) async fn deny_review_request(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
    body: Option<Json<ReviewDecisionBody>>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_review(&state)?;
    let uuid = parse_review_id(&id)?;
    let note = body.and_then(|Json(payload)| payload.note);
    let resolved = service
        .decide(uuid, ordo_review::ReviewDecisionKind::Deny { note })
        .map_err(map_review_error)?;
    Ok(Json(serde_json::to_value(resolved).unwrap_or(Value::Null)))
}

pub(crate) async fn edit_review_request(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
    Json(body): Json<ReviewEditBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_review(&state)?;
    let uuid = parse_review_id(&id)?;
    let resolved = service
        .decide(
            uuid,
            ordo_review::ReviewDecisionKind::Edit {
                content: body.content,
                note: body.note,
            },
        )
        .map_err(map_review_error)?;
    Ok(Json(serde_json::to_value(resolved).unwrap_or(Value::Null)))
}

pub(crate) async fn review_websocket(
    State(state): State<ControlApiState>,
    ws: axum::extract::WebSocketUpgrade,
) -> axum::response::Response {
    let Some(service) = state.review.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "review service not configured" })),
        )
            .into_response();
    };
    ws.on_upgrade(move |socket| review_websocket_session(socket, service))
}

pub(crate) async fn review_websocket_session(
    mut socket: axum::extract::ws::WebSocket,
    service: ordo_review::ReviewService,
) {
    use axum::extract::ws::Message;
    let mut receiver = service.subscribe();

    // Send an initial snapshot so the client has zero-latency catch-up.
    if let Ok(pending) = service.pending() {
        let total = pending.len();
        let snapshot = ordo_review::ReviewEvent::QueueSnapshot { pending, total };
        if let Ok(payload) = serde_json::to_string(&snapshot) {
            if socket.send(Message::Text(payload)).await.is_err() {
                return;
            }
        }
    }

    loop {
        tokio::select! {
            event = receiver.recv() => {
                match event {
                    Ok(event) => {
                        if let Ok(payload) = serde_json::to_string(&event) {
                            if socket.send(Message::Text(payload)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        let notice = json!({
                            "event": "lagged",
                            "skipped": skipped,
                        });
                        let _ = socket.send(Message::Text(notice.to_string())).await;
                        // After a lag, push a fresh snapshot so the
                        // client is back in sync.
                        if let Ok(pending) = service.pending() {
                            let total = pending.len();
                            let snapshot = ordo_review::ReviewEvent::QueueSnapshot { pending, total };
                            if let Ok(payload) = serde_json::to_string(&snapshot) {
                                if socket.send(Message::Text(payload)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    // Any other client-originated message is ignored on
                    // purpose: decisions must flow through REST so we
                    // have a single auditable mutation path.
                    _ => {}
                }
            }
        }
    }
}

