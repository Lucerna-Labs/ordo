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
use axum::body::Body;
#[derive(Deserialize)]
pub(crate) struct WebhookListQuery {
    pub(crate) workspace_id: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct RegisterWebhookBody {
    pub(crate) target_url: String,
    #[serde(default)]
    pub(crate) secret: Option<String>,
    #[serde(default)]
    pub(crate) topics: Vec<String>,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) workspace_id: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct UpdateWebhookBody {
    #[serde(default)]
    pub(crate) target_url: Option<String>,
    #[serde(default)]
    pub(crate) topics: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) active: Option<bool>,
}

pub(crate) fn webhooks_service(
    state: &ControlApiState,
) -> Result<ordo_webhooks::WebhookService, ControlApiError> {
    state
        .webhooks
        .clone()
        .ok_or_else(|| ControlApiError::internal("webhooks service not configured"))
}

pub(crate) fn map_webhook_error(err: ordo_webhooks::WebhookError) -> ControlApiError {
    use ordo_webhooks::WebhookError;
    match err {
        WebhookError::NotFound(_) => ControlApiError::not_found(err.to_string()),
        WebhookError::InvalidArgument(_) => ControlApiError::bad_request(err.to_string()),
        WebhookError::Storage(_) => ControlApiError::internal(err.to_string()),
    }
}

pub(crate) async fn list_webhooks_route(
    State(state): State<ControlApiState>,
    Query(q): Query<WebhookListQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = webhooks_service(&state)?;
    let subs = service
        .list(q.workspace_id.as_deref())
        .map_err(map_webhook_error)?;
    Ok(Json(json!({ "subscriptions": subs })))
}

pub(crate) async fn register_webhook_route(
    State(state): State<ControlApiState>,
    Json(body): Json<RegisterWebhookBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = webhooks_service(&state)?;
    let sub = service
        .register(ordo_webhooks::NewSubscription {
            target_url: body.target_url,
            secret: body.secret,
            topics: body.topics,
            description: body.description,
            workspace_id: body.workspace_id,
        })
        .map_err(map_webhook_error)?;
    // Register is the ONE call that returns the real secret so the
    // caller can stash it. All later reads redact.
    Ok(Json(json!({ "subscription": sub })))
}

pub(crate) async fn get_webhook_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = webhooks_service(&state)?;
    let sub = service
        .get(id)
        .map_err(map_webhook_error)?
        .ok_or_else(|| ControlApiError::not_found("subscription not found"))?;
    Ok(Json(json!({ "subscription": sub })))
}

pub(crate) async fn update_webhook_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<UpdateWebhookBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = webhooks_service(&state)?;
    let sub = service
        .update(
            id,
            ordo_webhooks::SubscriptionUpdate {
                target_url: body.target_url,
                topics: body.topics,
                description: body.description,
                active: body.active,
            },
        )
        .map_err(map_webhook_error)?;
    Ok(Json(json!({ "subscription": sub })))
}

pub(crate) async fn delete_webhook_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = webhooks_service(&state)?;
    let deleted = service.delete(id).map_err(map_webhook_error)?;
    Ok(Json(json!({ "deleted": deleted })))
}

pub(crate) async fn assistant_websocket(
    State(state): State<ControlApiState>,
    Path(session): Path<String>,
    ws: axum::extract::WebSocketUpgrade,
) -> axum::response::Response {
    let Some(service) = state.assistant.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "assistant service not configured" })),
        )
            .into_response();
    };
    let session_id = match uuid::Uuid::parse_str(&session) {
        Ok(id) => id,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid session id: {err}") })),
            )
                .into_response();
        }
    };
    ws.on_upgrade(move |socket| assistant_websocket_session(socket, service, session_id))
}

pub(crate) async fn assistant_websocket_session(
    mut socket: axum::extract::ws::WebSocket,
    service: ordo_assistant::AssistantService,
    session_id: uuid::Uuid,
) {
    use axum::extract::ws::Message;
    let mut receiver = service.events().subscribe(session_id);
    // Send a hello so the client knows the subscription is live.
    let hello = json!({
        "event": "subscribed",
        "session_id": session_id,
    });
    let _ = socket.send(Message::Text(hello.to_string())).await;
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
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Text(text))) => {
                        // Push 6: clients can send {"action":"cancel"}
                        // to stop an in-flight turn without closing
                        // the socket. Also accept the bare string
                        // "cancel" as a shortcut.
                        let should_cancel = text.trim() == "cancel"
                            || serde_json::from_str::<Value>(&text)
                                .ok()
                                .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(str::to_string))
                                .as_deref()
                                == Some("cancel");
                        if should_cancel {
                            service.cancel_turn(session_id);
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        // Close â†’ cancel any in-flight turn for this
                        // session. Idempotent if no turn is running.
                        service.cancel_turn(session_id);
                        break;
                    }
                    Some(Err(_)) => {
                        service.cancel_turn(session_id);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

// ---------- ui-extensions ------------------------------------------

const UI_BRIDGE_JS: &str = include_str!("ui_bridge.js");

