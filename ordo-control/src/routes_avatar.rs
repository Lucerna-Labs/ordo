use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) async fn avatar_sse(State(state): State<ControlApiState>) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::stream::{self, StreamExt};

    let bus = state.bus.clone();
    let sub = match bus.subscribe(avatar_topics::FRAME_EMITTED).await {
        Ok(s) => s,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("avatar subscribe failed: {err}") })),
            )
                .into_response();
        }
    };

    let hello = Event::default()
        .event("subscribed")
        .data(json!({ "topic": avatar_topics::FRAME_EMITTED }).to_string());

    // Move `sub` into unfold's state slot so the future doesn't borrow
    // across iterations (same trick as `assistant_sse`).
    let tail = stream::unfold(sub, |mut sub| async move {
        let envelope = sub.next().await?;
        let event = match envelope.payload {
            OrdoMessage::AvatarFrameEmitted(frame) => {
                let data = serde_json::to_string(&frame).unwrap_or_else(|_| "{}".to_string());
                Event::default().event("frame").data(data)
            }
            _ => {
                // Topic filter at subscribe-time should mean we never
                // see other variants here, but if the bus ever changes
                // filter semantics we keep the stream open rather than
                // tear it down.
                Event::default()
                    .event("ignored")
                    .data(json!({ "reason": "unexpected_variant" }).to_string())
            }
        };
        Some((Ok::<_, std::convert::Infallible>(event), sub))
    });

    let head = stream::once(async move { Ok::<_, std::convert::Infallible>(hello) });
    let combined = head.chain(tail);

    Sse::new(combined)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Body for `POST /api/avatar/speak`.
#[derive(Deserialize)]
pub(crate) struct AvatarSpeakRequest {
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) voice_id: Option<String>,
}

/// Trigger the TTS producer. Returns the freshly-minted
/// `utterance_id` so the caller can correlate with the phoneme frame
/// stream if it subscribes to the raw TTS topics. Most callers only
/// care about the avatar driver — they subscribe to `/sse/avatar` and
/// ignore the id.
pub(crate) async fn post_avatar_speak(
    State(state): State<ControlApiState>,
    Json(body): Json<AvatarSpeakRequest>,
) -> Result<Json<Value>, ControlApiError> {
    if body.text.trim().is_empty() {
        return Err(ControlApiError::bad_request("text is required"));
    }
    let utterance_id = state
        .tts
        .speak_with_options(
            body.text,
            ordo_tts::SpeakOptions {
                voice_id: body.voice_id,
                ..ordo_tts::SpeakOptions::default()
            },
        )
        .await;
    Ok(Json(json!({ "utterance_id": utterance_id })))
}

/// The avatar pop-out window page. Framework-free HTML+canvas that
/// connects to `/sse/avatar` and posts to `/api/avatar/speak`. Served
/// from the control API so those relative URLs stay same-origin.
pub(crate) async fn avatar_page() -> Html<&'static str> {
    Html(AVATAR_HTML)
}

/// Sprite-atlas layout descriptor consumed by [`avatar_page`].
pub(crate) async fn avatar_atlas_descriptor() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        AVATAR_ATLAS_JSON,
    )
}

/// Serve one of the embedded avatar sprite-atlas PNGs.
pub(crate) fn png_response(bytes: &'static [u8]) -> Response {
    ([(axum::http::header::CONTENT_TYPE, "image/png")], bytes).into_response()
}

pub(crate) async fn avatar_mouth_png() -> Response {
    png_response(AVATAR_MOUTH_PNG)
}

pub(crate) async fn avatar_expression_png() -> Response {
    png_response(AVATAR_EXPRESSION_PNG)
}

pub(crate) async fn avatar_glitch_png() -> Response {
    png_response(AVATAR_GLITCH_PNG)
}

/// Directory the avatar's behavior clips + manifest are served from.
/// Disk-served (NOT embedded) so an artist can drop in / swap clips without a
/// runtime rebuild. Override with `ORDO_AVATAR_CLIPS_DIR`; defaults to the
/// studio's public dir (the runtime's CWD is the repo root under the launcher).
pub(crate) fn avatar_clips_dir() -> PathBuf {
    std::env::var_os("ORDO_AVATAR_CLIPS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ordo-studio/public/avatar/clips"))
}

/// Fallback manifest used when the clips dir has no `clips.json` yet, so the
/// page still boots (the clip files simply 404 until present).
const DEFAULT_CLIPS_JSON: &str = r#"{
  "clips": {
    "idle": "avatar/clips/watching.mp4",
    "working": "avatar/clips/working.mp4",
    "watching": "avatar/clips/watching.mp4",
    "thinking": "avatar/clips/working.mp4",
    "listening": "avatar/clips/watching.mp4",
    "found": "avatar/clips/found.mp4",
    "pleased": "avatar/clips/happy.mp4",
    "speaking": "avatar/clips/speaking.mp4"
  },
  "idle_rotation": ["working", "watching"]
}"#;

/// GET `/avatar/clips.json` — the behavior-clip manifest, read from the clips
/// dir (falls back to the built-in default so the avatar always boots).
pub(crate) async fn avatar_clips_manifest() -> Response {
    let json_ct = [(axum::http::header::CONTENT_TYPE, "application/json")];
    match std::fs::read(avatar_clips_dir().join("clips.json")) {
        Ok(bytes) => (json_ct, bytes).into_response(),
        Err(_) => (json_ct, DEFAULT_CLIPS_JSON).into_response(),
    }
}

/// GET `/avatar/clips/:name` — serve one behavior clip from the clips dir.
/// `name` must be a basename (no path separators / traversal).
pub(crate) async fn avatar_clip_file(Path(name): Path<String>) -> Response {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return (StatusCode::BAD_REQUEST, "clip name must be a basename").into_response();
    }
    let content_type = match name.rsplit('.').next() {
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    match std::fs::read(avatar_clips_dir().join(&name)) {
        Ok(bytes) => ([(axum::http::header::CONTENT_TYPE, content_type)], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "clip not found").into_response(),
    }
}

// -- files HTTP routes (Phase 1.4) ----------------------------------
//
// Rule 3: handlers serialize in, dispatch to `FilesService`, serialize
// out. No upload/download logic here â€” byte handling lives in the
// service so the MCP bridge and plugin channels reuse the same path.

