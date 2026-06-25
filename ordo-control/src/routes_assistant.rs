use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) fn require_assistant(
    state: &ControlApiState,
) -> Result<&ordo_assistant::AssistantService, ControlApiError> {
    state.assistant.as_ref().ok_or_else(|| {
        ControlApiError::internal("assistant service not configured on this runtime")
    })
}

pub(crate) fn map_assistant_error(err: ordo_assistant::AssistantError) -> ControlApiError {
    use ordo_assistant::AssistantError::*;
    match err {
        SessionNotFound(id) => ControlApiError::bad_request(format!("session '{id}' not found")),
        FactNotFound(id) => ControlApiError::bad_request(format!("fact '{id}' not found")),
        InvalidArgument(msg) => ControlApiError::bad_request(msg),
        Storage(msg) | Embedding(msg) | Bus(msg) => ControlApiError::internal(msg),
        LlmFailed(msg) | NoCredential(msg) => ControlApiError::bad_request(msg),
        Cancelled => ControlApiError::bad_request("turn was cancelled".to_string()),
        SubagentBudgetExceeded(depth, max) => ControlApiError::bad_request(format!(
            "subagent recursion budget exceeded: depth {depth} > max {max}"
        )),
    }
}

pub(crate) async fn list_assistant_sessions(
    State(state): State<ControlApiState>,
    Query(query): Query<AssistantListQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let sessions = service.list_sessions(limit).map_err(map_assistant_error)?;
    Ok(Json(json!({
        "count": sessions.len(),
        "sessions": sessions,
    })))
}

/// GET `/api/assistant/modes` — list all registered modes for the
/// studio's mode switcher. Returns a sorted array of manifests
/// (full bodies — they're tiny, no need for a separate detail
/// endpoint for the picker).
///
/// Empty array when the runtime has no registry attached (config
/// path misconfiguration or first-boot failure). The studio
/// degrades gracefully: shows just "General" as a fallback.
pub(crate) async fn list_assistant_modes(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let modes = service.list_modes();
    Ok(Json(json!({
        "count": modes.len(),
        "modes": modes,
    })))
}

/// GET `/api/assistant/modes/:id` — full manifest for one mode.
/// Used by the studio's advanced view (step 10) to render the
/// "this is what's in scope" panel.
pub(crate) async fn get_assistant_mode(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let manifest = service
        .get_mode(&id)
        .ok_or_else(|| ControlApiError::bad_request(format!("mode '{id}' is not registered")))?;
    Ok(Json(serde_json::to_value(manifest).unwrap_or(Value::Null)))
}

/// POST `/api/assistant/modes` — create a new mode. Body is either
/// `{ "name": "My Mode" }` (the studio "Create" path — safe defaults, id
/// slugified from the name) or a full manifest carrying an `id` (advanced
/// create). See `docs/mode-lifecycle.md`.
pub(crate) async fn create_assistant_mode(
    State(state): State<ControlApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let manifest = if body.get("id").and_then(Value::as_str).is_some() {
        serde_json::from_value::<ordo_modes::ModeManifest>(body)
            .map_err(|err| ControlApiError::bad_request(format!("invalid mode manifest: {err}")))?
    } else {
        let name = body.get("name").and_then(Value::as_str).ok_or_else(|| {
            ControlApiError::bad_request(
                "create mode requires a 'name' (or a full manifest with 'id')",
            )
        })?;
        ordo_modes::ModeManifest::new_user_mode(name)
            .map_err(|err| ControlApiError::bad_request(err.to_string()))?
    };
    let created = service
        .create_mode(manifest)
        .map_err(map_mode_mutation_error)?;
    Ok(Json(serde_json::to_value(created).unwrap_or(Value::Null)))
}

/// PATCH `/api/assistant/modes/:id` — update a mode's config. The path id wins
/// over any `id` in the body. Protectedness is immutable (M2).
pub(crate) async fn update_assistant_mode(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    if let Some(obj) = body.as_object_mut() {
        obj.insert("id".to_string(), Value::String(id.clone()));
    }
    let manifest = serde_json::from_value::<ordo_modes::ModeManifest>(body)
        .map_err(|err| ControlApiError::bad_request(format!("invalid mode manifest: {err}")))?;
    let updated = service
        .update_mode(manifest)
        .map_err(map_mode_mutation_error)?;
    Ok(Json(serde_json::to_value(updated).unwrap_or(Value::Null)))
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct DeleteModeQuery {
    /// Required to delete a PROTECTED core mode. Defaults false.
    #[serde(default)]
    pub(crate) force: bool,
}

/// DELETE `/api/assistant/modes/:id` — delete a mode. Protected core modes
/// require `?force=true`.
pub(crate) async fn delete_assistant_mode(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
    Query(query): Query<DeleteModeQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    service
        .delete_mode(&id, query.force)
        .map_err(map_mode_mutation_error)?;
    Ok(Json(json!({ "deleted": id })))
}

pub(crate) async fn create_assistant_session(
    State(state): State<ControlApiState>,
    body: Option<Json<AssistantNewSessionBody>>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let (title, mode) = match body {
        Some(Json(b)) => (b.title.filter(|s| !s.is_empty()), b.mode),
        None => (None, None),
    };
    let session = service
        .new_session(title.as_deref(), mode.as_deref())
        .map_err(map_assistant_error)?;
    Ok(Json(serde_json::to_value(session).unwrap_or(Value::Null)))
}

/// Push 6: operator \"stop\" button for an in-flight turn. Flips the
/// session's cancellation flag; the turn loop picks it up on the next
/// iteration boundary and returns `AssistantError::Cancelled`.
pub(crate) async fn cancel_assistant_turn(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|err| ControlApiError::bad_request(format!("invalid session id: {err}")))?;
    let cancelled = service.cancel_turn(uuid);
    Ok(Json(serde_json::json!({
        "session_id": uuid,
        "cancelled": cancelled,
    })))
}

/// GET `/api/assistant/sessions/:id/taint` — operator-facing read of
/// the conversation's taint state. Returns:
///
///   { "session_id": "...", "tainted": bool, "sources": [Taint, ...] }
///
/// Each entry in `sources` is a `Taint` value (`UntrustedWeb {
/// source_url, fetched_at }`, etc.). Studio renders a small badge in
/// the chat header when `tainted: true`, with the URLs in a tooltip.
pub(crate) async fn get_session_taint(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|err| ControlApiError::bad_request(format!("invalid session id: {err}")))?;
    let taints = service.session_taints(uuid);
    let tainted = taints.iter().any(|t| t.is_untrusted());
    Ok(Json(serde_json::json!({
        "session_id": uuid,
        "tainted": tainted,
        "sources": taints,
    })))
}

/// POST `/api/assistant/sessions/:id/taint/clear` — operator
/// explicitly clears the conversation's taint. Removes every Taint
/// source attached to the session. Subsequent turns start fresh
/// until the operator reads new untrusted content.
///
/// Response: `{ "session_id": "...", "cleared": bool }` — `cleared`
/// is true when the session had tainted state to remove.
pub(crate) async fn clear_session_taint(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|err| ControlApiError::bad_request(format!("invalid session id: {err}")))?;
    let cleared = service.clear_session_taint(uuid);
    Ok(Json(serde_json::json!({
        "session_id": uuid,
        "cleared": cleared,
    })))
}

pub(crate) async fn get_assistant_session(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|err| ControlApiError::bad_request(format!("invalid session id: {err}")))?;
    let session = service
        .get_session(uuid)
        .map_err(map_assistant_error)?
        .ok_or_else(|| ControlApiError::bad_request(format!("session '{id}' not found")))?;
    Ok(Json(serde_json::to_value(session).unwrap_or(Value::Null)))
}

pub(crate) fn orchestrator_enabled() -> bool {
    matches!(
        std::env::var("ORDO_ENABLE_ORCHESTRATOR").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

/// Build the multi-agent orchestrator from the wired assistant service,
/// gated behind `ORDO_ENABLE_ORCHESTRATOR` (default off). Returns `None`
/// when disabled or when no assistant service is wired.
pub(crate) fn build_orchestrator(
    assistant: &Option<ordo_assistant::AssistantService>,
) -> Option<Arc<ordo_orchestrator::Orchestrator>> {
    if !orchestrator_enabled() {
        return None;
    }
    let service = assistant.as_ref()?;
    let glue = Arc::new(ordo_assistant::AssistantOrchestration::new(service.clone()));
    let planner: Arc<dyn ordo_orchestrator::GoalPlanner> = glue.clone();
    let runner: Arc<dyn ordo_orchestrator::SubagentRunner> = glue.clone();
    let critic: Arc<dyn ordo_orchestrator::Critic> = glue;
    Some(Arc::new(ordo_orchestrator::Orchestrator::new(
        planner,
        runner,
        Some(critic),
        ordo_orchestrator::OrchestratorBudget::default(),
    )))
}

#[derive(Deserialize)]
pub(crate) struct OrchestrateRequest {
    pub(crate) goal: String,
}

/// `POST /api/orchestrate` — submit a goal to the multi-agent orchestrator
/// and return the outcome (accepted/failed subtasks, rounds, terminal
/// phase). Bounded by the orchestrator's wall-clock budget. Returns 503
/// when the orchestrator is disabled.
pub(crate) async fn orchestrate_route(
    State(state): State<ControlApiState>,
    Json(body): Json<OrchestrateRequest>,
) -> Result<Json<ordo_orchestrator::OrchestrationOutcome>, ControlApiError> {
    let Some(orchestrator) = state.orchestrator.clone() else {
        return Err(ControlApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message:
                "orchestrator is disabled (set ORDO_ENABLE_ORCHESTRATOR=1 and wire an assistant)"
                    .into(),
        });
    };
    let goal = body.goal.trim();
    if goal.is_empty() {
        return Err(ControlApiError::bad_request("goal must not be empty"));
    }
    let outcome = orchestrator.run_bounded(goal).await;
    Ok(Json(outcome))
}

pub(crate) async fn post_assistant_turn(
    State(state): State<ControlApiState>,
    Json(body): Json<ordo_assistant::TurnRequest>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let result = service.turn(body).await.map_err(map_assistant_error)?;
    Ok(Json(serde_json::to_value(result).unwrap_or(Value::Null)))
}

pub(crate) async fn list_assistant_facts(
    State(state): State<ControlApiState>,
    Query(query): Query<AssistantFactsQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let facts = service
        .list_facts(query.subject.as_deref())
        .map_err(map_assistant_error)?;
    Ok(Json(json!({
        "count": facts.len(),
        "facts": facts,
    })))
}

pub(crate) async fn remember_assistant_fact(
    State(state): State<ControlApiState>,
    Json(body): Json<ordo_assistant::NewFact>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let fact = service
        .remember_fact(body)
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(
        serde_json::to_value(ordo_assistant::FactSummary::from(&fact)).unwrap_or(Value::Null),
    ))
}

pub(crate) async fn forget_assistant_fact(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|err| ControlApiError::bad_request(format!("invalid fact id: {err}")))?;
    let removed = service.forget_fact(uuid).map_err(map_assistant_error)?;
    Ok(Json(json!({ "id": id, "removed": removed })))
}

pub(crate) async fn recall_assistant_facts(
    State(state): State<ControlApiState>,
    Json(body): Json<AssistantRecallBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let recalled = service
        .recall(&body.query, body.top_k)
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(json!({
        "query": body.query,
        "count": recalled.len(),
        "facts": recalled,
    })))
}

pub(crate) async fn post_voice_speech(
    State(state): State<ControlApiState>,
    Json(body): Json<ordo_assistant::SpeechRequest>,
) -> Result<Response, ControlApiError> {
    let service = require_assistant(&state)?;
    let audio = service
        .speak_text(body)
        .await
        .map_err(map_assistant_error)?;
    let mut response = Response::new(axum::body::Body::from(audio.bytes));
    let headers = response.headers_mut();
    let content_type = axum::http::HeaderValue::from_str(&audio.content_type)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    headers.insert(axum::http::header::CONTENT_TYPE, content_type);
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    let model = axum::http::HeaderValue::from_str(&audio.model)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let voice = axum::http::HeaderValue::from_str(&audio.voice)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let provider = axum::http::HeaderValue::from_str(&audio.credential_service)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let format = axum::http::HeaderValue::from_str(&audio.format)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    headers.insert("x-ordo-tts-model", model);
    headers.insert("x-ordo-tts-voice", voice);
    headers.insert("x-ordo-tts-provider", provider);
    headers.insert("x-ordo-tts-format", format);
    Ok(response)
}

/// Body for `POST /api/voice/transcribe`. Audio rides as base64 so the
/// control API stays JSON (only the provider call is multipart). `format`
/// is the container hint (e.g. "webm", "wav", "mp3").
#[derive(Deserialize)]
pub(crate) struct VoiceTranscribeRequest {
    pub(crate) audio_base64: String,
    #[serde(default)]
    pub(crate) format: Option<String>,
    #[serde(default)]
    pub(crate) service: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) language: Option<String>,
}

/// Speech-to-text: decode the base64 audio and transcribe it via an
/// OpenAI-compatible provider (agnostic — `base_url` can be local or cloud).
/// Returns `{ text, provider, model }`.
pub(crate) async fn post_voice_transcribe(
    State(state): State<ControlApiState>,
    Json(body): Json<VoiceTranscribeRequest>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    if body.audio_base64.trim().is_empty() {
        return Err(ControlApiError::bad_request("audio_base64 is required"));
    }
    let audio = base64_decode_minimal(&body.audio_base64).map_err(ControlApiError::bad_request)?;
    let format = body.format.unwrap_or_else(|| "webm".to_string());
    let transcript = service
        .transcribe_audio(
            audio,
            format,
            ordo_assistant::TranscribeRequest {
                service: body.service,
                model: body.model,
                language: body.language,
            },
        )
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(json!({
        "text": transcript.text,
        "provider": transcript.credential_service,
        "model": transcript.model,
    })))
}

/// Server-Sent Events mirror of the assistant-event WebSocket.
///
/// One-way stream (server â†’ client). Used by HTTP-only consumers
/// (the standalone `ordo-mcp` bridge, webhooks, and plain
/// curl). Subscribes to the exact same per-session broadcast channel
/// as the WebSocket handler â€” no new logic, no side-effects.
///
/// Event format: each `TurnEvent` is emitted as an SSE event whose
/// `event:` line is the `TurnEvent` discriminant (`turn_started`,
/// `tool_call_started`, etc.) and whose `data:` line is the full
/// JSON-serialized event.
pub(crate) async fn assistant_sse(
    State(state): State<ControlApiState>,
    Path(session): Path<String>,
) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::stream::{self, StreamExt};
    use tokio::sync::broadcast::error::RecvError;

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

    let receiver = service.events().subscribe(session_id);
    let hello = Event::default()
        .event("subscribed")
        .data(json!({ "session_id": session_id }).to_string());

    let tail = stream::unfold(receiver, |mut rx| async move {
        match rx.recv().await {
            Ok(turn_event) => {
                let value = serde_json::to_value(&turn_event).unwrap_or(serde_json::Value::Null);
                let name = value
                    .get("event")
                    .and_then(|v| v.as_str())
                    .unwrap_or("message")
                    .to_string();
                let data = serde_json::to_string(&value).unwrap_or_default();
                let sse_event = Event::default().event(name).data(data);
                Some((Ok::<_, std::convert::Infallible>(sse_event), rx))
            }
            Err(RecvError::Lagged(skipped)) => {
                let notice = Event::default()
                    .event("lagged")
                    .data(json!({ "skipped": skipped }).to_string());
                Some((Ok::<_, std::convert::Infallible>(notice), rx))
            }
            Err(RecvError::Closed) => None,
        }
    });

    let head = stream::once(async move { Ok::<_, std::convert::Infallible>(hello) });
    let combined = head.chain(tail);

    Sse::new(combined)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// -- avatar HTTP routes ---------------------------------------------
//
// The avatar lives in a resizable pop-out window (a spare-monitor
// surface). It subscribes to `/sse/avatar` for performance frames and
// posts text to `/api/avatar/speak`. Rule 3: these handlers only
// mirror the bus — phoneme scheduling lives in `ordo-tts`, frame
// composition in `ordo-avatar`.

