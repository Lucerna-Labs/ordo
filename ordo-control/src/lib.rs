pub mod auth;
mod build_api;
pub mod metrics;

mod routes_studio;
mod routes_automation;
mod routes_system;
mod routes_memory;
mod routes_assistant;
mod routes_avatar;
mod routes_files;
mod routes_apps;
mod routes_webhooks;
mod routes_ui;
mod routes_review;
mod routes_plugins;
mod routes_mcp;
mod routes_connections;

pub(crate) use routes_studio::*;
pub(crate) use routes_automation::*;
pub(crate) use routes_system::*;
pub(crate) use routes_memory::*;
pub(crate) use routes_assistant::*;
pub(crate) use routes_avatar::*;
pub(crate) use routes_files::*;
pub(crate) use routes_apps::*;
pub(crate) use routes_webhooks::*;
pub(crate) use routes_ui::*;
pub(crate) use routes_review::*;
pub(crate) use routes_plugins::*;
pub(crate) use routes_mcp::*;
pub(crate) use routes_connections::*;

pub use auth::AuthConfig;
pub use metrics::{MetricsHandle, RateLimiterHandle};
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::Mutex;
use axum::routing::{delete, get, post, put};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::Router;
use ordo_automation::{
    default_diagnostic_automation, default_dreaming_automation, AutomationError,
    AutomationOrchestrator,
};
use ordo_automation_primitives::{AutomationId, AutomationSpec};
use ordo_brain::Brain;
use ordo_bus::Bus;
use ordo_protocol::{
    avatar_topics, infer_rag_collections, normalize_rag_collections, rag_collection_label,
    summarize_capability_lanes, OrdoMessage,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

type DynError = Box<dyn std::error::Error + Send + Sync>;
const DASHBOARD_HTML: &str = include_str!("dashboard.html");
pub(crate) const STUDIO_DIST_DIR: &str = "ordo-studio/dist";
pub(crate) const STUDIO_INDEX: &str = "index.html";

// Avatar pop-out window assets, embedded so the runtime can serve the
// page + sprite atlas itself (same origin as `/sse/avatar`, so the
// page's relative URLs resolve without any CORS dance). The avatar
// renders in its own resizable window — see `OrdoShell` for the
// pop-out trigger. Single source of truth lives under
// `ordo-studio/public/`; embedded here at compile time.
const AVATAR_HTML: &str = include_str!("../../ordo-studio/public/avatar.html");
const AVATAR_ATLAS_JSON: &str = include_str!("../../ordo-studio/public/avatar/avatar.json");
const AVATAR_MOUTH_PNG: &[u8] = include_bytes!("../../ordo-studio/public/avatar/mouth.png");
const AVATAR_EXPRESSION_PNG: &[u8] =
    include_bytes!("../../ordo-studio/public/avatar/expression.png");
const AVATAR_GLITCH_PNG: &[u8] = include_bytes!("../../ordo-studio/public/avatar/glitch.png");

#[derive(Clone)]
struct ControlApiState {
    brain: Arc<Brain>,
    plugins_path: Option<PathBuf>,
    plugin_statuses: Arc<Vec<ordo_plugins::PluginLoadStatus>>,
    security: Option<ordo_security::SecurityStack>,
    review: Option<ordo_review::ReviewService>,
    ui_extensions_path: Option<PathBuf>,
    assistant: Option<ordo_assistant::AssistantService>,
    /// Apps + files services (Phase 1.1 / 1.4). `None` until wired in
    /// the runtime; HTTP routes return 503 in that case rather than
    /// panicking, which keeps the router buildable for unit tests that
    /// don't need these services.
    apps: Option<ordo_apps::AppsService>,
    files: Option<ordo_files::FilesService>,
    webhooks: Option<ordo_webhooks::WebhookService>,
    /// MCP security stack handles. Same `None` discipline as apps/files
    /// so the router builds in tests that don't construct them.
    /// `mcp_client` is held for future invoke/test routes; current
    /// HTTP surface only needs registry + sandbox.
    mcp_registry: Option<Arc<ordo_mcp_registry::McpRegistryService>>,
    mcp_sandbox: Option<Arc<ordo_mcp_sandbox::McpSandboxService>>,
    #[allow(dead_code)]
    mcp_client: Option<Arc<ordo_mcp_client::McpClientService>>,
    /// Operator-facing Connections service (Phase Connections). Holds
    /// metadata + vault-sealed credentials for each configured backend
    /// (Bluesky, WordPress, OpenAI, etc.). `None` until wired in by
    /// the runtime; HTTP routes return 503 in that case rather than
    /// panicking, keeping the router buildable for unit tests.
    connections: Option<Arc<ordo_connections::ConnectionService>>,
    automation: Arc<Mutex<AutomationOrchestrator>>,
    automation_path: Option<PathBuf>,
    build_planner: Arc<AsyncMutex<ordo_build_planner::BuildPlannerPeer>>,
    /// MiniMax-style multi-agent orchestrator. `Some` only when
    /// `ORDO_ENABLE_ORCHESTRATOR` is set AND an assistant service is wired;
    /// the `/api/orchestrate` route returns 503 otherwise.
    orchestrator: Option<Arc<ordo_orchestrator::Orchestrator>>,
    /// Direct bus handle. Used by `/sse/avatar` to subscribe to
    /// `ordo.avatar.frame.emitted` envelopes and forward them to the
    /// avatar pop-out window. Held separately from `Brain` (which wraps
    /// the bus internally) because the SSE route needs raw
    /// `subscribe(topic)` access.
    bus: Arc<dyn Bus>,
    /// Stub TTS service wired against the same bus. Drives
    /// `POST /api/avatar/speak` — the caller posts text, the service
    /// publishes the `ordo.tts.*` stream, and the avatar driver reacts
    /// and emits frames on `/sse/avatar`. The agnostic voice provider
    /// (Stage 2) layers on top of this same publish path.
    tts: ordo_tts::TtsService,
}

#[derive(Debug)]
struct ControlApiError {
    status: StatusCode,
    message: String,
}

impl ControlApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    /// A precondition the caller/operator must satisfy first — e.g. a tool
    /// whose required credential is not configured. Honest 4xx, not a fault.
    fn precondition_failed(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PRECONDITION_FAILED,
            message: message.into(),
        }
    }

    /// The runaway guard rejected the call as too-frequent. Mirror it as 429
    /// rather than burying a client-rate problem in a 500.
    fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: message.into(),
        }
    }

    /// A downstream capability/provider timed out on the bus. The HTTP layer
    /// is the gateway, so 504 is the faithful status.
    fn gateway_timeout(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: message.into(),
        }
    }

    /// The request conflicts with current state — e.g. creating a mode whose id
    /// already exists.
    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    /// The action is refused by policy — e.g. deleting a protected core mode.
    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }

    /// A required subsystem isn't wired in (e.g. no mode registry attached).
    fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }
}

/// Map a mode-lifecycle error to the most honest HTTP status.
fn map_mode_mutation_error(err: ordo_modes::ModeMutationError) -> ControlApiError {
    use ordo_modes::ModeMutationError::*;
    match err {
        Unavailable => ControlApiError::service_unavailable(err.to_string()),
        AlreadyExists(_) => ControlApiError::conflict(err.to_string()),
        NotFound(_) => ControlApiError::not_found(err.to_string()),
        Protected(_) => ControlApiError::forbidden(err.to_string()),
        Invalid(_) => ControlApiError::bad_request(err.to_string()),
        Persist { .. } => ControlApiError::internal(err.to_string()),
    }
}

impl IntoResponse for ControlApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": self.message,
            })),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize, Default)]
struct ListMemoryQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct RagPreviewQuery {
    query: Option<String>,
    collections: Option<String>,
    top_k: Option<usize>,
}

pub fn build_router(bus: Arc<dyn Bus>) -> Router {
    build_router_with_plugins(
        bus,
        None,
        Vec::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Wrap a built router in the bearer-token middleware when
/// `auth_config` is not `Off`. A no-op when auth is off â€” Rule 3:
/// the HTTP layer mirrors state without owning it, and "auth off" is
/// the default mirror.
pub fn with_auth(router: Router, auth_config: AuthConfig) -> Router {
    let handle: auth::AuthHandle = Arc::new(auth_config);
    router.layer(axum::middleware::from_fn_with_state(
        handle,
        auth::require_auth,
    ))
}

/// Wrap a built router in the traffic middleware (metrics recording
/// plus per-IP rate limiting). Also attaches the `MetricsHandle` as an
/// extension so the `/metrics` endpoint renders the live counters
/// rather than the empty default.
pub fn with_traffic(router: Router, metrics: MetricsHandle, limiter: RateLimiterHandle) -> Router {
    router
        .layer(axum::Extension(metrics.clone()))
        .layer(axum::middleware::from_fn_with_state(
            (metrics, limiter),
            metrics::traffic_middleware,
        ))
}

/// Wrap a router in permissive CORS for the local operator surface.
///
/// The control API binds to `127.0.0.1` by default, but the desktop
/// shell's webview runs at a *different* origin (`tauri.localhost` in
/// the packaged app, `localhost:1420` under vite), so every studio
/// fetch is cross-origin. Browsers block cross-origin reads — and the
/// preflight `OPTIONS` for a JSON `POST` — unless the server echoes the
/// matching CORS headers. Without this, the studio's provider **Test**
/// and **Discover Models** buttons (which `POST /api/tools/...` with no
/// native fallback) fail with "Failed to fetch", and even GETs only
/// survive through the shell's native-command fallback (so a configured
/// provider appears in the list but is never actually persisted to the
/// runtime).
///
/// Auth (opt-in bearer token via `ORDO_AUTH_TOKENS`) stays the real
/// access boundary; CORS only decides which browser origins may *read*
/// replies. This layer is applied OUTSIDE auth + rate-limiting so the
/// credential-free preflight is answered before either runs, and so
/// 401/429 responses still carry headers the browser can read.
pub fn with_cors(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(cors_middleware))
}

async fn cors_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use axum::http::{header, HeaderValue, Method};

    // Only loopback / Tauri-webview origins may read replies. The
    // control API is a local operator surface (default bind 127.0.0.1)
    // that can invoke powerful capabilities, so we deliberately do NOT
    // reflect arbitrary internet origins — that would let any web page
    // the operator happens to visit read from (or drive) their local
    // runtime. Unknown origins get no CORS headers and stay blocked by
    // the browser, exactly as before this layer existed.
    let allowed_origin: Option<HeaderValue> = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .filter(|origin| cors_origin_allowed(origin))
        .and_then(|origin| HeaderValue::from_str(origin).ok());

    // Reflect the headers the browser announces for the upcoming real
    // request; fall back to the set the studio actually sends.
    let allow_headers = request
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("content-type, authorization"));

    // Answer the preflight here — before auth / rate-limit / routing,
    // none of which handle OPTIONS (the matched route would 405 it).
    let mut response = if request.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };

    if let Some(origin) = allowed_origin {
        let headers = response.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"),
        );
        headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, allow_headers);
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("86400"),
        );
        // Caches must key on Origin since ACAO varies per caller.
        headers.append(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

/// True for loopback HTTP(S) origins and the Tauri webview's custom-
/// protocol origin. The `.localhost` TLD is reserved for loopback
/// (RFC 6761); the packaged shell serves from `http://tauri.localhost`
/// and vite dev from `http://localhost:1420` / `http://127.0.0.1:1420`.
fn cors_origin_allowed(origin: &str) -> bool {
    if origin == "tauri://localhost" {
        return true;
    }
    let rest = match origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    {
        Some(rest) => rest,
        None => return false,
    };
    // Drop any path, then any trailing :port (keeping bracketed IPv6).
    let host = rest.split('/').next().unwrap_or(rest);
    let hostname = match host.find(':') {
        Some(idx) if !host.starts_with('[') => &host[..idx],
        _ => host,
    };
    hostname == "localhost"
        || hostname.ends_with(".localhost")
        || hostname == "127.0.0.1"
        || hostname == "::1"
        || host.starts_with("[::1]")
}

#[allow(clippy::too_many_arguments)]
pub fn build_router_with_plugins(
    bus: Arc<dyn Bus>,
    plugins_path: Option<PathBuf>,
    plugin_statuses: Vec<ordo_plugins::PluginLoadStatus>,
    security: Option<ordo_security::SecurityStack>,
    review: Option<ordo_review::ReviewService>,
    ui_extensions_path: Option<PathBuf>,
    assistant: Option<ordo_assistant::AssistantService>,
    apps: Option<ordo_apps::AppsService>,
    files: Option<ordo_files::FilesService>,
    webhooks: Option<ordo_webhooks::WebhookService>,
    mcp_registry: Option<Arc<ordo_mcp_registry::McpRegistryService>>,
    mcp_sandbox: Option<Arc<ordo_mcp_sandbox::McpSandboxService>>,
    mcp_client: Option<Arc<ordo_mcp_client::McpClientService>>,
    connections: Option<Arc<ordo_connections::ConnectionService>>,
) -> Router {
    let automation_path = automation_path_from_plugins(&plugins_path);
    let automation = match &automation_path {
        Some(path) => AutomationOrchestrator::load_or_seed(
            path,
            vec![
                default_diagnostic_automation(),
                default_dreaming_automation(),
            ],
        )
        .unwrap_or_else(|err| {
            tracing::warn!(
                target: "ordo_control::automations",
                error = %err,
                path = %path.display(),
                "failed to load automation store; using seeded defaults"
            );
            seeded_automation_orchestrator()
        }),
        None => seeded_automation_orchestrator(),
    };

    let build_planner = build_planner_from_plugins(bus.clone(), &plugins_path);
    let orchestrator = build_orchestrator(&assistant);

    // Keep a raw bus handle for `/sse/avatar` and wire the stub TTS
    // producer against the same bus before `Brain::new` consumes its
    // clone. The avatar driver subscribes to the TTS stream inside the
    // runtime; here we only need to publish onto it.
    let avatar_bus = bus.clone();
    let tts = ordo_tts::TtsService::new().with_bus(bus.clone());

    let state = ControlApiState {
        brain: Arc::new(Brain::new(bus)),
        plugins_path,
        plugin_statuses: Arc::new(plugin_statuses),
        security,
        review,
        ui_extensions_path,
        assistant,
        apps,
        files,
        webhooks,
        mcp_registry,
        mcp_sandbox,
        mcp_client,
        connections,
        automation: Arc::new(Mutex::new(automation)),
        automation_path,
        build_planner: Arc::new(AsyncMutex::new(build_planner)),
        orchestrator,
        bus: avatar_bus,
        tts,
    };

    Router::new()
        .route("/", get(studio_index_or_dashboard))
        .route("/index.html", get(studio_index_or_dashboard))
        .route("/dashboard", get(dashboard))
        .route("/health", get(health))
        .route("/metrics", get(metrics_endpoint))
        .route("/api/capabilities", get(list_capabilities))
        .route("/api/rag/collections", get(list_rag_collections))
        .route("/api/rag/preview", get(preview_rag))
        .route("/api/runtime/profile", get(describe_profile))
        .route("/api/runtime/storage", get(describe_storage))
        .route("/api/system/find_binary", get(find_binary))
        .route(
            "/api/runtime/settings",
            get(describe_settings).post(update_settings),
        )
        .route(
            "/api/self-heal/cases",
            get(list_self_heal_cases).delete(forget_self_heal_case),
        )
        .route("/api/self-heal/cases/pin", post(pin_self_heal_case))
        .route("/api/self-heal/cases/replay", post(replay_self_heal_case))
        .route("/api/self-heal/cases/export", post(export_self_heal_case))
        .route(
            "/api/memory/pinned",
            get(list_pinned).post(pin_memory).delete(unpin_memory),
        )
        .route(
            "/api/memory/working",
            get(list_working).post(remember_memory),
        )
        .route(
            "/api/cloud/credentials",
            get(list_cloud_credentials)
                .post(upsert_cloud_credential)
                .delete(delete_cloud_credential),
        )
        .route(
            "/api/builds",
            get(build_api::list_builds_route).post(build_api::start_build_route),
        )
        .route("/api/builds/:id", get(build_api::get_build_route))
        .route(
            "/api/builds/:id/gate",
            post(build_api::submit_gate_result_route),
        )
        .route(
            "/api/automations",
            get(list_automations_route).post(create_automation_route),
        )
        .route("/api/automations/tick", post(tick_automations_route))
        .route(
            "/api/automations/:id",
            get(get_automation_route).delete(delete_automation_route),
        )
        .route(
            "/api/automations/:id/approve",
            post(approve_automation_route),
        )
        .route("/api/automations/:id/enable", post(enable_automation_route))
        .route(
            "/api/automations/:id/disable",
            post(disable_automation_route),
        )
        .route(
            "/api/agent-teams",
            get(list_agent_teams_route).put(save_agent_teams_route),
        )
        .route("/api/agent-teams/active", post(set_active_agent_team_route))
        .route("/api/tools/:capability", post(invoke_tool_by_name))
        .route("/api/plugins", get(list_plugins))
        .route(
            "/api/plugins/:name/enabled",
            post(set_plugin_enabled).delete(disable_plugin),
        )
        .route("/api/security/audit", get(list_security_audit))
        .route("/api/security/rules", get(list_security_rules))
        .route("/api/review/pending", get(list_review_pending))
        .route("/api/review/recent", get(list_review_recent))
        .route("/api/review/:id", get(get_review_request))
        .route("/api/review/:id/approve", post(approve_review_request))
        .route("/api/review/:id/deny", post(deny_review_request))
        .route("/api/review/:id/edit", post(edit_review_request))
        .route("/ws/review", get(review_websocket))
        .route("/api/ui-extensions", get(list_ui_extensions))
        .route("/api/ui-extensions/_bridge.js", get(serve_ui_bridge))
        .route(
            "/api/ui-extensions/:name/files/*path",
            get(serve_ui_extension_file),
        )
        .route(
            "/api/assistant/sessions",
            get(list_assistant_sessions).post(create_assistant_session),
        )
        .route("/api/assistant/sessions/:id", get(get_assistant_session))
        .route("/api/assistant/turn", post(post_assistant_turn))
        .route("/api/orchestrate", post(orchestrate_route))
        .route(
            "/api/assistant/facts",
            get(list_assistant_facts).post(remember_assistant_fact),
        )
        .route(
            "/api/assistant/facts/:id",
            axum::routing::delete(forget_assistant_fact),
        )
        .route("/api/assistant/recall", post(recall_assistant_facts))
        .route("/api/voice/speech", post(post_voice_speech))
        .route("/api/voice/transcribe", post(post_voice_transcribe))
        .route(
            "/api/assistant/modes",
            get(list_assistant_modes).post(create_assistant_mode),
        )
        .route(
            "/api/assistant/modes/:id",
            get(get_assistant_mode)
                .patch(update_assistant_mode)
                .delete(delete_assistant_mode),
        )
        .route("/api/assistant/sessions/:id/taint", get(get_session_taint))
        .route(
            "/api/assistant/sessions/:id/taint/clear",
            post(clear_session_taint),
        )
        .route(
            "/api/assistant/sessions/:id/cancel",
            post(cancel_assistant_turn),
        )
        .route("/ws/assistant/:session", get(assistant_websocket))
        // HTTP SSE mirror of `/ws/assistant/:session`. Same broadcast
        // source (Rule 3: HTTP mirrors the bus, never owns logic) â€”
        // SSE is a one-way stream and gives HTTP-only clients
        // (ordo-mcp, webhooks, plain curl) live turn events
        // without needing WebSocket.
        .route(
            "/api/assistant/sessions/:session/stream",
            get(assistant_sse),
        )
        // Avatar performance frames. The `ordo-avatar` driver publishes
        // one `AvatarFrameEmitted` per tick (~30Hz); this SSE route
        // mirrors that stream to the resizable avatar pop-out window
        // (and to `curl -N http://127.0.0.1:4141/sse/avatar` for debug).
        .route("/sse/avatar", get(avatar_sse))
        // The avatar pop-out (or any caller) posts the text it wants
        // spoken; ordo-tts publishes the `ordo.tts.*` envelopes, the
        // avatar driver reacts, and frames flow back over `/sse/avatar`.
        .route("/api/avatar/speak", post(post_avatar_speak))
        // Avatar pop-out window page + sprite atlas, served from the
        // runtime so the window is self-contained at the control-API
        // origin (its relative `/sse/avatar` + `/api/avatar/speak`
        // calls then resolve without CORS).
        .route("/avatar.html", get(avatar_page))
        .route("/avatar/avatar.json", get(avatar_atlas_descriptor))
        .route("/avatar/mouth.png", get(avatar_mouth_png))
        .route("/avatar/expression.png", get(avatar_expression_png))
        .route("/avatar/glitch.png", get(avatar_glitch_png))
        // Behavior-clip avatar: the manifest + clip files are served from a
        // DISK directory (not embedded) so clips can be swapped without a
        // runtime rebuild. See `avatar_clips_dir`.
        .route("/avatar/clips.json", get(avatar_clips_manifest))
        .route("/avatar/clips/:name", get(avatar_clip_file))
        // Files primitive (Phase 1.4). Rule 3: HTTP mirrors the
        // service â€” no business logic in these handlers.
        .route("/api/files", get(list_files_route).post(upload_file_json))
        .route(
            "/api/files/:id",
            get(get_file_metadata_route).delete(delete_file_route),
        )
        .route("/api/files/:id/content", get(download_file_route))
        // Apps primitive (Phase 1.5). Thin mirrors over `AppsService`
        // â€” status transitions are dedicated endpoints so the review
        // layer can gate them consistently.
        .route("/api/apps", get(list_apps_route).post(create_app_route))
        .route(
            "/api/apps/:id",
            get(get_app_route)
                .patch(update_app_route)
                .delete(archive_app_route),
        )
        .route("/api/apps/:id/events", get(list_app_events_route))
        .route(
            "/api/apps/:id/state-at/:seq",
            get(get_app_state_at_version_route),
        )
        .route("/api/apps/:id/publish", post(publish_app_route))
        .route("/api/apps/:id/unpublish", post(unpublish_app_route))
        .route("/api/apps/:id/archive", post(archive_app_route))
        .route("/api/apps/:id/unarchive", post(unarchive_app_route))
        // Webhooks (Phase 3.1). Subscriptions are workspace-scoped;
        // secrets are redacted from list/read responses.
        .route(
            "/api/webhooks",
            get(list_webhooks_route).post(register_webhook_route),
        )
        .route(
            "/api/webhooks/:id",
            get(get_webhook_route)
                .patch(update_webhook_route)
                .delete(delete_webhook_route),
        )
        // App deployments (Phase 3.3).
        .route(
            "/api/apps/:id/deployments",
            get(list_deployments_route).post(create_deployment_route),
        )
        .route(
            "/api/apps/:id/deployments/:dep_id/promote",
            post(promote_deployment_route),
        )
        .route(
            "/api/apps/:id/deployments/:dep_id/fail",
            post(fail_deployment_route),
        )
        // MCP security architecture: signed-lockfile install,
        // drift detection, trust state machine. Multipart upload
        // path lives at the dedicated /install endpoint because
        // axum's standard JSON extractor doesn't carry a binary
        // module body cleanly.
        .route("/api/mcp/servers", get(list_mcp_servers_route))
        .route("/api/mcp/servers/install", post(install_mcp_server_route))
        .route(
            "/api/mcp/servers/:server_id",
            axum::routing::delete(uninstall_mcp_server_route),
        )
        .route(
            "/api/mcp/servers/:server_id/quarantine",
            post(quarantine_mcp_server_route),
        )
        .route(
            "/api/mcp/servers/:server_id/re-authorize",
            post(re_authorize_mcp_server_route),
        )
        .route(
            "/api/mcp/servers/:server_id/lockfile",
            get(get_mcp_lockfile_route),
        )
        .route(
            "/api/mcp/servers/:server_id/invoke/:tool_name",
            post(invoke_mcp_tool_route),
        )
        // UI extensions install/uninstall surfaces. List + serve
        // routes are above; these complete the lifecycle.
        .route(
            "/api/ui-extensions/install",
            post(install_ui_extension_route),
        )
        .route(
            "/api/ui-extensions/:name",
            axum::routing::delete(uninstall_ui_extension_route),
        )
        // Operator-facing Connections (Bluesky, WordPress, OpenAI,
        // etc.). The studio's Connections tab calls these. Secrets
        // never travel back through these routes â€” only metadata +
        // status. The :id/test endpoint re-runs the live tester.
        .route("/api/connections/types", get(list_connection_types_route))
        .route(
            "/api/connections",
            get(list_connections_route).post(create_connection_route),
        )
        .route(
            "/api/connections/:id",
            get(get_connection_route)
                .patch(update_connection_route)
                .delete(delete_connection_route),
        )
        .route("/api/connections/:id/test", post(test_connection_route))
        .route("/proxy/ollama/*path", get(proxy_ollama_route))
        .route("/proxy/lmstudio/*path", get(proxy_lmstudio_route))
        .fallback(get(studio_asset_fallback))
        .with_state(state)
}

pub async fn run_control_api(bus: Arc<dyn Bus>, bind_addr: &str) -> Result<(), DynError> {
    run_control_api_with_plugins(
        bus,
        bind_addr,
        None,
        Vec::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_control_api_with_plugins(
    bus: Arc<dyn Bus>,
    bind_addr: &str,
    plugins_path: Option<PathBuf>,
    plugin_statuses: Vec<ordo_plugins::PluginLoadStatus>,
    security: Option<ordo_security::SecurityStack>,
    review: Option<ordo_review::ReviewService>,
    ui_extensions_path: Option<PathBuf>,
    assistant: Option<ordo_assistant::AssistantService>,
    apps: Option<ordo_apps::AppsService>,
    files: Option<ordo_files::FilesService>,
    webhooks: Option<ordo_webhooks::WebhookService>,
    mcp_registry: Option<Arc<ordo_mcp_registry::McpRegistryService>>,
    mcp_sandbox: Option<Arc<ordo_mcp_sandbox::McpSandboxService>>,
    mcp_client: Option<Arc<ordo_mcp_client::McpClientService>>,
    connections: Option<Arc<ordo_connections::ConnectionService>>,
) -> Result<(), DynError> {
    // Auth is env-driven: `ORDO_AUTH_TOKENS` unset â†’ off (the
    // default, matching the pre-Phase-2.5 behavior). Non-empty â†’
    // bearer-token enforcement. The `with_auth(router, Off)` call is a
    // no-op, so calling it unconditionally is safe.
    let auth_config = AuthConfig::from_env();
    if auth_config.is_enforced() {
        tracing::info!(
            target: "ordo_control",
            "auth: bearer-token enforcement ON (ORDO_AUTH_TOKENS set)"
        );
    }
    let router = build_router_with_plugins(
        bus,
        plugins_path,
        plugin_statuses,
        security,
        review,
        ui_extensions_path,
        assistant,
        apps,
        files,
        webhooks,
        mcp_registry,
        mcp_sandbox,
        mcp_client,
        connections,
    );
    let router = with_auth(router, auth_config);
    // Phase 4.6: metrics + per-IP rate limit applied to the
    // outermost layer so dashboard / health / metrics (the rate-limit
    // exempt paths) still get counted.
    let metrics_handle = MetricsHandle::new();
    let rate_limiter = RateLimiterHandle::from_env();
    let router = with_traffic(router, metrics_handle, rate_limiter);
    // CORS is the OUTERMOST layer: the desktop shell's webview is a
    // different origin than 127.0.0.1:4141, so the preflight must be
    // answered (and ACAO echoed) before auth / rate-limit / routing run.
    let router = with_cors(router);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    println!("[control] API listening on http://{}", local_addr);
    // Phase 4.6: ConnectInfo is needed by the rate limiter to see
    // per-peer IPs. `into_make_service_with_connect_info` is the
    // standard hook for that.
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}


mod base64_decoder {
    /// Tiny base64 decoder so the control API doesn't drag in a
    /// dedicated crate. Standard alphabet, accepts padded input.
    pub fn decode_b64_standard(input: &str) -> Result<Vec<u8>, String> {
        let trimmed: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        let trimmed = trimmed.trim_end_matches('=');
        let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
        let mut buf: u32 = 0;
        let mut bits: u32 = 0;
        for c in trimmed.chars() {
            let v = match c {
                'A'..='Z' => (c as u32) - ('A' as u32),
                'a'..='z' => (c as u32) - ('a' as u32) + 26,
                '0'..='9' => (c as u32) - ('0' as u32) + 52,
                '+' => 62,
                '/' => 63,
                _ => return Err(format!("invalid base64 character `{c}`")),
            };
            buf = (buf << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buf >> bits) & 0xff) as u8);
            }
        }
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::decode_b64_standard;

        #[test]
        fn round_trip_simple() {
            assert_eq!(decode_b64_standard("SGVsbG8=").unwrap(), b"Hello");
        }

        #[test]
        fn round_trip_padded() {
            assert_eq!(decode_b64_standard("Zm9v").unwrap(), b"foo");
            assert_eq!(decode_b64_standard("Zm9vYg==").unwrap(), b"foob");
        }

        #[test]
        fn rejects_invalid_char() {
            assert!(decode_b64_standard("hi$").is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Method, http::Request, http::StatusCode};
    use ordo_bus::{Bus, InProcessBus};
    use ordo_cloud::{CloudCredentialStore, CloudCredentialTask};
    use ordo_heal::{SelfHealPeer, SelfHealStorageTask, SelfHealStore};
    use ordo_mcp_host::{
        CloudOpsProvider, FilesystemProvider, KnowledgeProvider, McpHost, MemoryToolsProvider,
        RuntimeInfoProvider, RuntimePolicySnapshot, SelfHealToolsProvider,
    };
    use ordo_memory::MemoryPeer;
    use ordo_protocol::RagDocument;
    use ordo_rag::{RagPeer, RagStore};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::{build_router, build_router_with_plugins};
    use std::{path::PathBuf, sync::Arc, time::Duration};

    #[tokio::test]
    async fn capabilities_endpoint_returns_lane_summaries() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(FilesystemProvider::default()));
        host.add_provider(Arc::new(RuntimeInfoProvider::new(RuntimePolicySnapshot {
            profile: "standard".to_string(),
            control_api_bind: Some("127.0.0.1:4141".to_string()),
            rag_enabled: true,
            knowledge_enabled: true,
            rag_activation_mode: "lazy".to_string(),
            knowledge_activation_mode: "lazy".to_string(),
            rag_budget_bytes: 1024,
            memory_working_budget_bytes: 2048,
            memory_pinned_budget_bytes: 4096,
            self_heal_history_budget_bytes: 512,
            self_heal_llama_cpp_binary: None,
            self_heal_model_path: None,
            self_heal_model_context_size: 4096,
            self_heal_model_max_tokens: 384,
            self_heal_model_temperature: 0.1,
            llama_cpp_configured: false,
            embedding_backend: "hashing".to_string(),
            embedding_dimensions: 96,
            embedding_llama_cpp_binary: None,
            embedding_model_path: None,
            embedding_context_size: 512,
            embedding_ollama_url: None,
            embedding_ollama_model: None,
        })));
        host.add_provider(Arc::new(KnowledgeProvider));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let app = build_router(bus);
        let response = app
            .oneshot(
                Request::get("/api/capabilities")
                    .body(axum::body::Body::empty())
                    .expect("capabilities request"),
            )
            .await
            .expect("capabilities response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("capabilities body");
        let json: Value = serde_json::from_slice(&body).expect("capabilities json");
        assert_eq!(json["count"].as_u64(), Some(8));
        assert_eq!(json["lane_count"].as_u64(), Some(3));
        assert!(json["lanes"]
            .as_array()
            .expect("lanes array")
            .iter()
            .any(|lane| {
                lane["name"].as_str() == Some("knowledge")
                    && lane["group"].as_str() == Some("system")
                    && lane["count"].as_u64() == Some(4)
            }));

        host_task.abort();
    }

    #[tokio::test]
    async fn rag_collections_endpoint_lists_live_collection_inventory() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut rag_store = RagStore::in_memory();
        rag_store
            .upsert_document(&RagDocument {
                document_id: "design-basics".to_string(),
                uri: "docs/rag/main/design-basics.md".to_string(),
                title: "Design Basics".to_string(),
                tags: vec!["docs".to_string(), "design".to_string()],
                collection: "main".to_string(),
                content: "Design basics cover hierarchy, composition, and spacing.".to_string(),
            })
            .expect("seed main rag document");
        rag_store
            .upsert_document(&RagDocument {
                document_id: "research-domain".to_string(),
                uri: "docs/domains/research.md".to_string(),
                title: "Research Domain".to_string(),
                tags: vec!["docs".to_string(), "research".to_string()],
                collection: "research".to_string(),
                content:
                    "Research notes track cited sources, evidence summaries, and review decisions."
                        .to_string(),
            })
            .expect("seed research rag document");
        let mut rag = RagPeer::with_store(bus.clone(), rag_store);
        let rag_task = tokio::spawn(async move {
            let _ = rag.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let app = build_router(bus);
        let response = app
            .oneshot(
                Request::get("/api/rag/collections")
                    .body(axum::body::Body::empty())
                    .expect("rag collections request"),
            )
            .await
            .expect("rag collections response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("rag collections body");
        let json: Value = serde_json::from_slice(&body).expect("rag collections json");
        assert_eq!(json["available"].as_bool(), Some(true));
        assert_eq!(json["count"].as_u64(), Some(2));
        assert!(json["results"]
            .as_array()
            .expect("rag collections results")
            .iter()
            .any(|entry| {
                entry["name"].as_str() == Some("main")
                    && entry["group"].as_str() == Some("shared")
                    && entry["document_count"].as_u64() == Some(1)
            }));
        assert!(json["results"]
            .as_array()
            .expect("rag collections results")
            .iter()
            .any(|entry| {
                entry["name"].as_str() == Some("research")
                    && entry["group"].as_str() == Some("domain")
                    && entry["chunk_count"].as_u64() == Some(1)
            }));

        rag_task.abort();
    }

    #[tokio::test]
    async fn rag_preview_endpoint_supports_inferred_and_manual_collections() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut rag_store = RagStore::in_memory();
        rag_store
            .upsert_document(&RagDocument {
                document_id: "design-basics".to_string(),
                uri: "docs/rag/main/design-basics.md".to_string(),
                title: "Design Basics".to_string(),
                tags: vec!["docs".to_string(), "design".to_string()],
                collection: "main".to_string(),
                content: "Campaign design work needs hierarchy and visual contrast.".to_string(),
            })
            .expect("seed main rag document");
        rag_store
            .upsert_document(&RagDocument {
                document_id: "research-domain".to_string(),
                uri: "docs/domains/research.md".to_string(),
                title: "Research Domain".to_string(),
                tags: vec!["docs".to_string(), "research".to_string()],
                collection: "research".to_string(),
                content:
                    "Research notes track cited sources, evidence summaries, and review decisions."
                        .to_string(),
            })
            .expect("seed research rag document");
        let mut rag = RagPeer::with_store(bus.clone(), rag_store);
        let rag_task = tokio::spawn(async move {
            let _ = rag.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let app = build_router(bus);
        let inferred_response = app
            .clone()
            .oneshot(
                Request::get("/api/rag/preview?query=research%20evidence%20sources")
                    .body(axum::body::Body::empty())
                    .expect("rag preview inferred request"),
            )
            .await
            .expect("rag preview inferred response");
        assert_eq!(inferred_response.status(), StatusCode::OK);
        let inferred_body = to_bytes(inferred_response.into_body(), usize::MAX)
            .await
            .expect("rag preview inferred body");
        let inferred_json: Value =
            serde_json::from_slice(&inferred_body).expect("rag preview inferred json");
        assert_eq!(
            inferred_json["using_inferred_collections"].as_bool(),
            Some(true)
        );
        assert!(inferred_json["effective_collections"]
            .as_array()
            .expect("effective collections")
            .iter()
            .any(|entry| entry.as_str() == Some("main")));
        assert!(inferred_json["effective_collections"]
            .as_array()
            .expect("effective collections")
            .iter()
            .any(|entry| entry.as_str() == Some("research")));

        let manual_response = app
            .oneshot(
                Request::get(
                    "/api/rag/preview?query=research%20evidence%20sources&collections=research",
                )
                .body(axum::body::Body::empty())
                .expect("rag preview manual request"),
            )
            .await
            .expect("rag preview manual response");
        assert_eq!(manual_response.status(), StatusCode::OK);
        let manual_body = to_bytes(manual_response.into_body(), usize::MAX)
            .await
            .expect("rag preview manual body");
        let manual_json: Value =
            serde_json::from_slice(&manual_body).expect("rag preview manual json");
        assert_eq!(
            manual_json["using_inferred_collections"].as_bool(),
            Some(false)
        );
        assert_eq!(
            manual_json["effective_collections"]
                .as_array()
                .expect("manual collections")
                .len(),
            1
        );
        assert!(manual_json["hits"]
            .as_array()
            .expect("manual hits")
            .iter()
            .all(|hit| hit["collection"].as_str() == Some("research")));

        rag_task.abort();
    }

    #[tokio::test]
    async fn pinned_memory_endpoint_lists_results() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut memory = MemoryPeer::new(bus.clone());
        let memory_task = tokio::spawn(async move {
            let _ = memory.run().await;
        });

        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(MemoryToolsProvider::new(bus.clone())));
        host.add_provider(Arc::new(RuntimeInfoProvider::new(RuntimePolicySnapshot {
            profile: "standard".to_string(),
            control_api_bind: Some("127.0.0.1:4141".to_string()),
            rag_enabled: true,
            knowledge_enabled: true,
            rag_activation_mode: "lazy".to_string(),
            knowledge_activation_mode: "lazy".to_string(),
            rag_budget_bytes: 1024,
            memory_working_budget_bytes: 2048,
            memory_pinned_budget_bytes: 4096,
            self_heal_history_budget_bytes: 512,
            self_heal_llama_cpp_binary: None,
            self_heal_model_path: None,
            self_heal_model_context_size: 4096,
            self_heal_model_max_tokens: 384,
            self_heal_model_temperature: 0.1,
            llama_cpp_configured: false,
            embedding_backend: "hashing".to_string(),
            embedding_dimensions: 96,
            embedding_llama_cpp_binary: None,
            embedding_model_path: None,
            embedding_context_size: 512,
            embedding_ollama_url: None,
            embedding_ollama_model: None,
        })));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let app = build_router(bus);
        let pin_response = app
            .clone()
            .oneshot(
                Request::post("/api/memory/pinned")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"content":"UI should be able to pin important notes."}"#,
                    ))
                    .expect("pin request"),
            )
            .await
            .expect("pin response");
        assert_eq!(pin_response.status(), StatusCode::OK);

        let list_response = app
            .clone()
            .oneshot(
                Request::get("/api/memory/pinned?limit=5")
                    .body(axum::body::Body::empty())
                    .expect("list request"),
            )
            .await
            .expect("list response");
        assert_eq!(list_response.status(), StatusCode::OK);
        let body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .expect("list body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        assert!(json["results"]
            .as_array()
            .expect("results array")
            .iter()
            .any(|entry| entry.as_str() == Some("UI should be able to pin important notes.")));

        let unpin_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/memory/pinned")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"content":"UI should be able to pin important notes."}"#,
                    ))
                    .expect("unpin request"),
            )
            .await
            .expect("unpin response");
        assert_eq!(unpin_response.status(), StatusCode::OK);

        let list_after_unpin = app
            .oneshot(
                Request::get("/api/memory/pinned?limit=5")
                    .body(axum::body::Body::empty())
                    .expect("list after unpin request"),
            )
            .await
            .expect("list after unpin response");
        assert_eq!(list_after_unpin.status(), StatusCode::OK);
        let body = to_bytes(list_after_unpin.into_body(), usize::MAX)
            .await
            .expect("list after unpin body");
        let json: Value = serde_json::from_slice(&body).expect("json body after unpin");
        assert!(!json["results"]
            .as_array()
            .expect("results array after unpin")
            .iter()
            .any(|entry| entry.as_str() == Some("UI should be able to pin important notes.")));

        host_task.abort();
        memory_task.abort();
    }

    #[tokio::test]
    async fn self_heal_cases_endpoint_lists_results() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let temp_db_path = std::env::temp_dir().join("codex-ordo-control-self-heal-test.db");
        let _ = std::fs::remove_file(&temp_db_path);
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-wal", temp_db_path.display())));
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-shm", temp_db_path.display())));

        let mut memory = MemoryPeer::with_store(
            bus.clone(),
            ordo_memory::MemoryStore::open(temp_db_path.clone()).expect("memory sqlite store"),
        );
        let memory_task = tokio::spawn(async move {
            let _ = memory.run().await;
        });

        let mut self_heal_store =
            SelfHealStore::open(temp_db_path.clone()).expect("self-heal sqlite store");
        self_heal_store
            .record_plan(
                &ordo_protocol::SelfHealIncident {
                    incident_id: uuid::Uuid::new_v4(),
                    component: "ordo-mcp-host/filesystem".to_string(),
                    symptom: "filesystem.read_file escaped the root".to_string(),
                    fingerprint: "filesystem-root-escape".to_string(),
                    urgency: ordo_protocol::SelfHealUrgency::Medium,
                    logs: vec!["path '../outside.txt' escapes root".to_string()],
                },
                &ordo_protocol::SelfHealPlan {
                    summary: "Repair filesystem root handling".to_string(),
                    why: "Repeat rooted path validation before escalating.".to_string(),
                    actions: vec![
                        "Check root".to_string(),
                        "Normalize path".to_string(),
                        "Retry read".to_string(),
                    ],
                    source: ordo_protocol::SelfHealSource::DeterministicFallback,
                    reused_previous_fix: false,
                    memory_hits: 0,
                },
            )
            .expect("seed self-heal case");
        let self_heal_storage = SelfHealStorageTask::from_store(self_heal_store);
        let mut self_heal =
            SelfHealPeer::with_storage_and_model(bus.clone(), self_heal_storage.clone(), None);
        let self_heal_task = tokio::spawn(async move {
            let _ = self_heal.run().await;
        });

        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(MemoryToolsProvider::new(bus.clone())));
        host.add_provider(Arc::new(SelfHealToolsProvider::new(
            self_heal_storage,
            bus.clone(),
        )));
        host.add_provider(Arc::new(RuntimeInfoProvider::new(RuntimePolicySnapshot {
            profile: "standard".to_string(),
            control_api_bind: Some("127.0.0.1:4141".to_string()),
            rag_enabled: true,
            knowledge_enabled: true,
            rag_activation_mode: "lazy".to_string(),
            knowledge_activation_mode: "lazy".to_string(),
            rag_budget_bytes: 1024,
            memory_working_budget_bytes: 2048,
            memory_pinned_budget_bytes: 4096,
            self_heal_history_budget_bytes: 512,
            self_heal_llama_cpp_binary: None,
            self_heal_model_path: None,
            self_heal_model_context_size: 4096,
            self_heal_model_max_tokens: 384,
            self_heal_model_temperature: 0.1,
            llama_cpp_configured: false,
            embedding_backend: "hashing".to_string(),
            embedding_dimensions: 96,
            embedding_llama_cpp_binary: None,
            embedding_model_path: None,
            embedding_context_size: 512,
            embedding_ollama_url: None,
            embedding_ollama_model: None,
        })));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let app = build_router(bus);
        let list_response = app
            .clone()
            .oneshot(
                Request::get("/api/self-heal/cases?limit=5")
                    .body(axum::body::Body::empty())
                    .expect("self-heal list request"),
            )
            .await
            .expect("self-heal list response");
        assert_eq!(list_response.status(), StatusCode::OK);
        let body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .expect("self-heal list body");
        let json: Value = serde_json::from_slice(&body).expect("self-heal list json");
        assert_eq!(json["count"].as_u64(), Some(1));
        assert!(json["results"]
            .as_array()
            .expect("self-heal results")
            .iter()
            .any(|entry| entry["fingerprint"].as_str() == Some("filesystem-root-escape")));

        let seed_stale_pinned = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/memory/pinned")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"content":"Self-heal fix: filesystem-root-escape\nOld stale fix"}"#,
                    ))
                    .expect("seed stale pinned request"),
            )
            .await
            .expect("seed stale pinned response");
        assert_eq!(seed_stale_pinned.status(), StatusCode::OK);

        let pin_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/self-heal/cases/pin")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"fingerprint":"filesystem-root-escape"}"#,
                    ))
                    .expect("self-heal pin request"),
            )
            .await
            .expect("self-heal pin response");
        assert_eq!(pin_response.status(), StatusCode::OK);
        let pin_body = to_bytes(pin_response.into_body(), usize::MAX)
            .await
            .expect("self-heal pin body");
        let pin_json: Value = serde_json::from_slice(&pin_body).expect("self-heal pin json");
        assert_eq!(pin_json["stored"].as_bool(), Some(true));

        let pinned_response = app
            .clone()
            .oneshot(
                Request::get("/api/memory/pinned?limit=5")
                    .body(axum::body::Body::empty())
                    .expect("pinned list request"),
            )
            .await
            .expect("pinned list response");
        assert_eq!(pinned_response.status(), StatusCode::OK);
        let pinned_body = to_bytes(pinned_response.into_body(), usize::MAX)
            .await
            .expect("pinned list body");
        let pinned_json: Value = serde_json::from_slice(&pinned_body).expect("pinned list json");
        let matching_entries = pinned_json["results"]
            .as_array()
            .expect("pinned results")
            .iter()
            .filter(|entry| {
                entry
                    .as_str()
                    .is_some_and(|value| value.starts_with("Self-heal fix: filesystem-root-escape"))
            })
            .count();
        assert_eq!(matching_entries, 1);
        assert!(pinned_json["results"]
            .as_array()
            .expect("pinned results")
            .iter()
            .any(|entry| {
                entry
                    .as_str()
                    .is_some_and(|value| value.contains("Self-heal fix: filesystem-root-escape"))
            }));
        assert!(!pinned_json["results"]
            .as_array()
            .expect("pinned results")
            .iter()
            .any(|entry| {
                entry
                    .as_str()
                    .is_some_and(|value| value.contains("Old stale fix"))
            }));

        let export_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/self-heal/cases/export")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"fingerprint":"filesystem-root-escape"}"#,
                    ))
                    .expect("self-heal export request"),
            )
            .await
            .expect("self-heal export response");
        assert_eq!(export_response.status(), StatusCode::OK);
        let export_body = to_bytes(export_response.into_body(), usize::MAX)
            .await
            .expect("self-heal export body");
        let export_json: Value =
            serde_json::from_slice(&export_body).expect("self-heal export json");
        assert_eq!(
            export_json["filename"].as_str(),
            Some("self-heal-filesystem-root-escape.md")
        );
        assert!(export_json["markdown"]
            .as_str()
            .is_some_and(|value| value.contains("Repair filesystem root handling")));

        let replay_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/self-heal/cases/replay")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"fingerprint":"filesystem-root-escape"}"#,
                    ))
                    .expect("self-heal replay request"),
            )
            .await
            .expect("self-heal replay response");
        assert_eq!(replay_response.status(), StatusCode::OK);
        let replay_body = to_bytes(replay_response.into_body(), usize::MAX)
            .await
            .expect("self-heal replay body");
        let replay_json: Value =
            serde_json::from_slice(&replay_body).expect("self-heal replay json");
        assert_eq!(replay_json["replayed"].as_bool(), Some(true));
        assert_eq!(replay_json["plan"]["source"].as_str(), Some("MemoryReuse"));
        assert_eq!(
            replay_json["plan"]["reused_previous_fix"].as_bool(),
            Some(true)
        );

        let delete_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/self-heal/cases")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"fingerprint":"filesystem-root-escape"}"#,
                    ))
                    .expect("self-heal delete request"),
            )
            .await
            .expect("self-heal delete response");
        assert_eq!(delete_response.status(), StatusCode::OK);

        host_task.abort();
        self_heal_task.abort();
        memory_task.abort();
        let _ = std::fs::remove_file(&temp_db_path);
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-wal", temp_db_path.display())));
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-shm", temp_db_path.display())));
    }

    #[tokio::test]
    async fn cloud_credentials_endpoint_supports_upsert_list_delete() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let store = CloudCredentialStore::in_memory().expect("cloud credentials in-memory store");
        let cloud_task = CloudCredentialTask::start(store);

        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(CloudOpsProvider::new(cloud_task)));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let app = build_router(bus);

        let upsert_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/cloud/credentials")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"service":"openai","label":"OpenAI","auth_style":"bearer","secret":"sk-control-test"}"#,
                    ))
                    .expect("upsert request"),
            )
            .await
            .expect("upsert response");
        assert_eq!(upsert_response.status(), StatusCode::OK);
        let upsert_body = to_bytes(upsert_response.into_body(), usize::MAX)
            .await
            .expect("upsert body");
        let upsert_json: Value = serde_json::from_slice(&upsert_body).expect("upsert json");
        assert_eq!(
            upsert_json["credential"]["service"].as_str(),
            Some("openai")
        );
        assert_eq!(upsert_json["credential"]["label"].as_str(), Some("OpenAI"));
        assert_eq!(
            upsert_json["credential"]["auth_style"].as_str(),
            Some("bearer")
        );
        // Secret must not round-trip back to the client.
        assert!(upsert_json["credential"]["secret"].is_null());

        let list_response = app
            .clone()
            .oneshot(
                Request::get("/api/cloud/credentials")
                    .body(axum::body::Body::empty())
                    .expect("list request"),
            )
            .await
            .expect("list response");
        assert_eq!(list_response.status(), StatusCode::OK);
        let list_body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .expect("list body");
        let list_json: Value = serde_json::from_slice(&list_body).expect("list json");
        let credentials = list_json["credentials"]
            .as_array()
            .expect("credentials array");
        assert_eq!(credentials.len(), 1);
        assert_eq!(credentials[0]["service"].as_str(), Some("openai"));
        assert!(credentials[0]["secret"].is_null());

        let delete_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/cloud/credentials")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"service":"openai"}"#))
                    .expect("delete request"),
            )
            .await
            .expect("delete response");
        assert_eq!(delete_response.status(), StatusCode::OK);
        let delete_body = to_bytes(delete_response.into_body(), usize::MAX)
            .await
            .expect("delete body");
        let delete_json: Value = serde_json::from_slice(&delete_body).expect("delete json");
        assert_eq!(delete_json["service"].as_str(), Some("openai"));
        assert_eq!(delete_json["removed"].as_bool(), Some(true));

        let list_after = app
            .oneshot(
                Request::get("/api/cloud/credentials")
                    .body(axum::body::Body::empty())
                    .expect("list after delete request"),
            )
            .await
            .expect("list after delete response");
        assert_eq!(list_after.status(), StatusCode::OK);
        let list_after_body = to_bytes(list_after.into_body(), usize::MAX)
            .await
            .expect("list after delete body");
        let list_after_json: Value =
            serde_json::from_slice(&list_after_body).expect("list after delete json");
        assert!(list_after_json["credentials"]
            .as_array()
            .expect("credentials array after delete")
            .is_empty());

        host_task.abort();
    }

    #[tokio::test]
    async fn dashboard_endpoint_serves_html() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let app = build_router(bus);
        let response = app
            .oneshot(
                Request::get("/dashboard")
                    .body(axum::body::Body::empty())
                    .expect("dashboard request"),
            )
            .await
            .expect("dashboard response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("dashboard body");
        let html = String::from_utf8(body.to_vec()).expect("dashboard utf8");
        assert!(html.contains("Ordo Control"));
    }

    #[test]
    fn studio_content_type_maps_module_assets() {
        assert_eq!(
            super::studio_content_type(std::path::Path::new("assets/app.js")),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            super::studio_content_type(std::path::Path::new("assets/app.css")),
            "text/css; charset=utf-8"
        );
        assert_eq!(
            super::studio_content_type(std::path::Path::new("index.html")),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn automations_endpoint_lists_core_defaults() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let app = build_router(bus);
        let response = app
            .oneshot(
                Request::get("/api/automations")
                    .body(axum::body::Body::empty())
                    .expect("automations request"),
            )
            .await
            .expect("automations response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("automations body");
        let json: Value = serde_json::from_slice(&body).expect("automations json");
        let names: Vec<&str> = json["automations"]
            .as_array()
            .expect("automations list")
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            .collect();
        assert!(names.contains(&"Diagnostic Sweep"));
        assert!(names.contains(&"Dreaming Review"));
    }

    #[tokio::test]
    async fn ui_extensions_list_and_serve_static_files() {
        // Set up a temp directory that looks like a plugins root with
        // one extension inside it.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("ordo-ui-ext-{stamp}"));
        let ext_dir = root.join("hello");
        std::fs::create_dir_all(&ext_dir).expect("mkdir ext");
        std::fs::write(
            ext_dir.join("ui.json"),
            r#"{
  "name": "hello",
  "version": "0.1.0",
  "core_override": true,
  "surfaces": [
    { "kind": "tab", "id": "main", "label": "Hello", "entry": "index.html" }
  ],
  "permissions": { "mcp_tools": ["filesystem.write_file"] }
}"#,
        )
        .expect("write manifest");
        std::fs::write(
            ext_dir.join("index.html"),
            "<html><body>hello world</body></html>",
        )
        .expect("write index");

        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let app = build_router_with_plugins(
            bus,
            None,
            Vec::new(),
            None,
            None,
            Some(root.clone()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        // GET /api/ui-extensions should list the manifest.
        let list_response = app
            .clone()
            .oneshot(
                Request::get("/api/ui-extensions")
                    .body(axum::body::Body::empty())
                    .expect("list request"),
            )
            .await
            .expect("list response");
        assert_eq!(list_response.status(), StatusCode::OK);
        let body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .expect("list body");
        let json: Value = serde_json::from_slice(&body).expect("list json");
        let extensions = json["extensions"].as_array().expect("extensions array");
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0]["name"].as_str(), Some("hello"));
        assert_eq!(
            extensions[0]["surfaces"][0]["entry_url"].as_str(),
            Some("/api/ui-extensions/hello/files/index.html")
        );

        // Fetching the entry file should succeed with text/html.
        let file_response = app
            .clone()
            .oneshot(
                Request::get("/api/ui-extensions/hello/files/index.html")
                    .body(axum::body::Body::empty())
                    .expect("file request"),
            )
            .await
            .expect("file response");
        assert_eq!(file_response.status(), StatusCode::OK);
        let content_type = file_response
            .headers()
            .get("content-type")
            .and_then(|v: &axum::http::HeaderValue| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.starts_with("text/html"),
            "got content-type: {content_type}"
        );
        let file_body = to_bytes(file_response.into_body(), usize::MAX)
            .await
            .expect("file body");
        assert!(String::from_utf8_lossy(&file_body).contains("hello world"));

        // Path traversal must be rejected before we even try the file
        // system.
        let escape_response = app
            .clone()
            .oneshot(
                Request::get("/api/ui-extensions/hello/files/../../outside")
                    .body(axum::body::Body::empty())
                    .expect("escape request"),
            )
            .await
            .expect("escape response");
        // axum routing normalises `..`, so the request may land with
        // either a 400 from our guard or a 404 from the router. Both
        // are acceptable â€” what matters is we don't return 200 with
        // foreign content.
        let escape_status = escape_response.status();
        assert!(
            escape_status == StatusCode::BAD_REQUEST
                || escape_status == StatusCode::NOT_FOUND
                || escape_status == StatusCode::METHOD_NOT_ALLOWED,
            "got: {escape_status}"
        );

        // Bridge JS should serve as application/javascript.
        let bridge_response = app
            .oneshot(
                Request::get("/api/ui-extensions/_bridge.js")
                    .body(axum::body::Body::empty())
                    .expect("bridge request"),
            )
            .await
            .expect("bridge response");
        assert_eq!(bridge_response.status(), StatusCode::OK);
        let bridge_ct = bridge_response
            .headers()
            .get("content-type")
            .and_then(|v: &axum::http::HeaderValue| v.to_str().ok())
            .unwrap_or("");
        assert!(
            bridge_ct.starts_with("application/javascript"),
            "got content-type: {bridge_ct}"
        );
        let bridge_body = to_bytes(bridge_response.into_body(), usize::MAX)
            .await
            .expect("bridge body");
        let bridge_text = String::from_utf8_lossy(&bridge_body);
        assert!(bridge_text.contains("window.ordo"));
        assert!(bridge_text.contains("tools.call") || bridge_text.contains("\"call\""));

        let _ = std::fs::remove_dir_all(&root);
    }
}
