use std::sync::Arc;

use std::path::PathBuf;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use parking_lot::Mutex;

mod build_api;
pub mod auth;
pub mod metrics;
pub use auth::AuthConfig;
pub use metrics::{MetricsHandle, RateLimiterHandle};
use ordo_automation::{
    default_diagnostic_automation, default_dreaming_automation, AutomationError,
    AutomationOrchestrator,
};
use ordo_automation_primitives::{AutomationId, AutomationSpec};
use ordo_brain::Brain;
use ordo_bus::Bus;
use ordo_protocol::{
    infer_rag_collections, normalize_rag_collections, rag_collection_label,
    summarize_capability_lanes,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

type DynError = Box<dyn std::error::Error + Send + Sync>;
const DASHBOARD_HTML: &str = include_str!("dashboard.html");

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
    };

    Router::new()
        .route("/", get(dashboard))
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
        .route("/api/builds", get(build_api::list_builds_route).post(build_api::start_build_route))
        .route("/api/builds/:id", get(build_api::get_build_route))
        .route("/api/builds/:id/gate", post(build_api::submit_gate_result_route))
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
        .route("/api/assistant/modes", get(list_assistant_modes))
        .route("/api/assistant/modes/:id", get(get_assistant_mode))
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

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

fn map_automation_error(err: AutomationError) -> ControlApiError {
    match err {
        AutomationError::Validation(err) => ControlApiError::bad_request(err.to_string()),
        AutomationError::AlreadyExists => ControlApiError::bad_request(err.to_string()),
        AutomationError::NotFound => ControlApiError::not_found(err.to_string()),
        AutomationError::ApprovalRequired => ControlApiError::bad_request(err.to_string()),
    }
}

fn seeded_automation_orchestrator() -> AutomationOrchestrator {
    let mut automation = AutomationOrchestrator::new();
    let _ = automation.register(default_diagnostic_automation());
    let _ = automation.register(default_dreaming_automation());
    automation
}

fn build_planner_from_plugins(
    bus: Arc<dyn Bus>,
    plugins_path: &Option<PathBuf>,
) -> ordo_build_planner::BuildPlannerPeer {
    let Some(path) = build_planner_path_from_plugins(plugins_path) else {
        return ordo_build_planner::BuildPlannerPeer::new(bus);
    };

    let ledgers = match ordo_build_planner::BuildLedgerStore::open(&path)
        .and_then(|store| store.list())
    {
        Ok(ledgers) => ledgers,
        Err(err) => {
            tracing::warn!(
                target: "ordo_control::builds",
                error = %err,
                path = %path.display(),
                "failed to load build ledgers; using in-memory build planner"
            );
            return ordo_build_planner::BuildPlannerPeer::new(bus);
        }
    };

    match ordo_build_planner::BuildLedgerTask::open(&path) {
        Ok(task) => ordo_build_planner::BuildPlannerPeer::with_store(bus, task, ledgers),
        Err(err) => {
            tracing::warn!(
                target: "ordo_control::builds",
                error = %err,
                path = %path.display(),
                "failed to open build ledger task; using in-memory build planner"
            );
            ordo_build_planner::BuildPlannerPeer::with_ledgers(bus, ledgers)
        }
    }
}

fn build_planner_path_from_plugins(plugins_path: &Option<PathBuf>) -> Option<PathBuf> {
    let plugins_path = plugins_path.as_ref()?;
    let user_files = plugins_path.parent()?;
    Some(user_files.join("build-ledgers"))
}

fn automation_path_from_plugins(plugins_path: &Option<PathBuf>) -> Option<PathBuf> {
    let plugins_path = plugins_path.as_ref()?;
    let user_files = plugins_path.parent()?;
    Some(user_files.join("automations.json"))
}

fn persist_automations(state: &ControlApiState) -> Result<(), ControlApiError> {
    let Some(path) = &state.automation_path else {
        return Ok(());
    };
    state
        .automation
        .lock()
        .save_to_path(path)
        .map_err(|err| ControlApiError::internal(err.to_string()))
}

async fn list_automations_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let automation = state.automation.lock();
    let automations: Vec<AutomationSpec> = automation.list().into_iter().cloned().collect();
    Ok(Json(json!({
        "automations": automations,
        "events": automation.event_log(),
    })))
}

async fn create_automation_route(
    State(state): State<ControlApiState>,
    Json(spec): Json<AutomationSpec>,
) -> Result<Json<Value>, ControlApiError> {
    let events = {
        let mut automation = state.automation.lock();
        automation.register(spec).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    let automation = state.automation.lock();
    Ok(Json(json!({
        "events": events,
        "automations": automation.list().into_iter().cloned().collect::<Vec<AutomationSpec>>(),
    })))
}

async fn get_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let automation = state.automation.lock();
    let spec = automation
        .get(id)
        .cloned()
        .ok_or_else(|| ControlApiError::not_found("automation not found"))?;
    Ok(Json(json!({ "automation": spec })))
}

async fn approve_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let events = {
        let mut automation = state.automation.lock();
        automation.approve(id).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    Ok(Json(json!({ "events": events })))
}

async fn enable_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let event = {
        let mut automation = state.automation.lock();
        automation.enable(id).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    Ok(Json(json!({ "event": event })))
}

async fn disable_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let event = {
        let mut automation = state.automation.lock();
        automation.disable(id).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    Ok(Json(json!({ "event": event })))
}

async fn delete_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let event = {
        let mut automation = state.automation.lock();
        automation.delete(id).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    Ok(Json(json!({ "event": event })))
}

async fn tick_automations_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let mut automation = state.automation.lock();
    let events = automation.tick(chrono::Utc::now());
    Ok(Json(json!({ "events": events })))
}

async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

/// `/metrics` endpoint. Reads the shared `MetricsHandle` from the
/// request's extensions â€” populated by the traffic layer at
/// `with_traffic` time. When the layer isn't installed (e.g. in unit
/// tests that build a bare router), the handler returns a minimal
/// body with the static build info only.
async fn metrics_endpoint(
    handle: Option<axum::extract::Extension<MetricsHandle>>,
) -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    String,
) {
    let body = match handle {
        Some(axum::extract::Extension(h)) => h.render(),
        None => MetricsHandle::new().render(),
    };
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
}

async fn list_capabilities(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let descriptors = state
        .brain
        .query_capability_descriptors()
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let lanes = summarize_capability_lanes(&descriptors);
    Ok(Json(json!({
        "count": descriptors.len(),
        "lane_count": lanes.len(),
        "lanes": lanes,
        "descriptors": descriptors,
    })))
}

async fn list_rag_collections(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let collections = match state.brain.query_rag_collections().await {
        Ok(collections) => collections,
        Err(err) => {
            return Ok(Json(json!({
                "available": false,
                "count": 0,
                "results": [],
                "error": err.to_string(),
            })));
        }
    };
    Ok(Json(json!({
        "available": true,
        "count": collections.len(),
        "results": collections,
    })))
}

async fn preview_rag(
    State(state): State<ControlApiState>,
    Query(query): Query<RagPreviewQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let raw_query = query.query.unwrap_or_default();
    let trimmed_query = raw_query.trim();
    if trimmed_query.is_empty() {
        return Err(ControlApiError::bad_request("preview query is required"));
    }

    let requested_collections = parse_collection_query(query.collections.as_deref());
    let using_inferred_collections = requested_collections.is_empty();
    let effective_collections = if using_inferred_collections {
        infer_rag_collections(trimmed_query)
    } else {
        requested_collections.clone()
    };
    let top_k = query.top_k.unwrap_or(5).clamp(1, 8);
    let hits = state
        .brain
        .query_rag_in_collections(trimmed_query, &effective_collections, top_k)
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let effective_collection_labels = effective_collections
        .iter()
        .map(|collection| rag_collection_label(collection))
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "query": trimmed_query,
        "top_k": top_k,
        "using_inferred_collections": using_inferred_collections,
        "requested_collections": requested_collections,
        "effective_collections": effective_collections,
        "effective_collection_labels": effective_collection_labels,
        "hit_count": hits.len(),
        "hits": hits,
    })))
}

async fn describe_profile(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "runtime.describe_profile", json!({})).await
}

#[derive(Deserialize)]
struct FindBinaryQuery {
    name: String,
}

/// GET `/api/system/find_binary?name=<exe_name>` — walks a small set
/// of candidate paths anchored on the running runtime's location and
/// returns the first one that exists. The studio's MCP tab uses this
/// to auto-detect `ordo-mcp.exe` so the operator doesn't have to type
/// or browse for a path that's almost always sitting next to the
/// runtime binary it's already talking to.
///
/// Response:
///   { "name": "ordo-mcp.exe", "found": "<abs path or null>",
///     "candidates": ["<path>", "<path>", …] }
///
/// The candidates list is returned even on miss so the studio can
/// surface a "we looked here" hint if the operator has to fix it
/// manually.
async fn find_binary(Query(query): Query<FindBinaryQuery>) -> Result<Json<Value>, ControlApiError> {
    use std::path::PathBuf;

    let raw = query.name.trim();
    if raw.is_empty() {
        return Err(ControlApiError::bad_request(
            "missing required query 'name'".to_string(),
        ));
    }
    // Reject path-traversal: caller specifies a basename, not a path.
    // The whole point of this endpoint is to LOCATE a binary; letting
    // the caller pass `../../etc/passwd` would invert that.
    if raw.contains('/') || raw.contains('\\') || raw.contains("..") {
        return Err(ControlApiError::bad_request(
            "'name' must be a basename (no path separators)".to_string(),
        ));
    }
    // On Windows, normalize to .exe if the caller didn't include it.
    let name = if cfg!(windows) && !raw.to_ascii_lowercase().ends_with(".exe") {
        format!("{raw}.exe")
    } else {
        raw.to_string()
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            // 1. Sibling of the runtime binary (most common — both
            //    built into the same `target/{profile}/` dir).
            candidates.push(dir.join(&name));
            // 2. Walk up looking for sibling `target/release` /
            //    `target/debug` directories. Handles cases where the
            //    runtime is in `target/release` and the caller wants
            //    a binary that only built into `target/debug`.
            let mut walker = dir.parent();
            for _ in 0..4 {
                let Some(up) = walker else { break };
                candidates.push(up.join("release").join(&name));
                candidates.push(up.join("debug").join(&name));
                walker = up.parent();
            }
        }
    }
    // 3. Anything on PATH. `which` would be cleaner but pulling a
    //    new dep for one lookup isn't worth it; walk PATH manually.
    if let Some(path_var) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&path_var) {
            candidates.push(entry.join(&name));
        }
    }

    // De-duplicate while preserving order (operators see candidates
    // in priority order if the search misses).
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let candidates: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect();

    let found = candidates
        .iter()
        .find(|p| p.is_file())
        .map(|p| p.display().to_string());
    let candidate_strs: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();

    Ok(Json(json!({
        "name": name,
        "found": found,
        "candidates": candidate_strs,
    })))
}

async fn describe_storage(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "runtime.describe_storage", json!({})).await
}

async fn describe_settings(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "runtime.describe_settings", json!({})).await
}

async fn update_settings(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "runtime.update_settings", payload).await
}

async fn list_pinned(
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

async fn list_self_heal_cases(
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

async fn list_working(
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

async fn pin_memory(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "memory.pin_note", payload).await
}

async fn unpin_memory(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "memory.unpin_note", payload).await
}

async fn forget_self_heal_case(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "self_heal.forget_case", payload).await
}

async fn pin_self_heal_case(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "self_heal.pin_case", payload).await
}

async fn replay_self_heal_case(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "self_heal.replay_case", payload).await
}

async fn export_self_heal_case(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "self_heal.export_case", payload).await
}

async fn remember_memory(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "memory.remember_note", payload).await
}

async fn list_security_audit(
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

async fn list_security_rules(
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
struct AssistantListQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct AssistantFactsQuery {
    subject: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AssistantNewSessionBody {
    #[serde(default)]
    title: Option<String>,
    /// Mode-scoped workspace for the new session. None = General
    /// Assistant. Validated by the assistant service against its
    /// registered modes; unknown id returns 400.
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AssistantRecallBody {
    query: String,
    #[serde(default = "default_recall_top_k")]
    top_k: usize,
}

fn default_recall_top_k() -> usize {
    5
}

fn require_assistant(
    state: &ControlApiState,
) -> Result<&ordo_assistant::AssistantService, ControlApiError> {
    state.assistant.as_ref().ok_or_else(|| {
        ControlApiError::internal("assistant service not configured on this runtime")
    })
}

fn map_assistant_error(err: ordo_assistant::AssistantError) -> ControlApiError {
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

async fn list_assistant_sessions(
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
async fn list_assistant_modes(
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
async fn get_assistant_mode(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let manifest = service
        .get_mode(&id)
        .ok_or_else(|| ControlApiError::bad_request(format!("mode '{id}' is not registered")))?;
    Ok(Json(serde_json::to_value(manifest).unwrap_or(Value::Null)))
}

async fn create_assistant_session(
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
async fn cancel_assistant_turn(
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
async fn get_session_taint(
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
async fn clear_session_taint(
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

async fn get_assistant_session(
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

async fn post_assistant_turn(
    State(state): State<ControlApiState>,
    Json(body): Json<ordo_assistant::TurnRequest>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let result = service.turn(body).await.map_err(map_assistant_error)?;
    Ok(Json(serde_json::to_value(result).unwrap_or(Value::Null)))
}

async fn list_assistant_facts(
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

async fn remember_assistant_fact(
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

async fn forget_assistant_fact(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_assistant(&state)?;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|err| ControlApiError::bad_request(format!("invalid fact id: {err}")))?;
    let removed = service.forget_fact(uuid).map_err(map_assistant_error)?;
    Ok(Json(json!({ "id": id, "removed": removed })))
}

async fn recall_assistant_facts(
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

async fn post_voice_speech(
    State(state): State<ControlApiState>,
    Json(body): Json<ordo_assistant::SpeechRequest>,
) -> Result<Response, ControlApiError> {
    let service = require_assistant(&state)?;
    let audio = service.speak_text(body).await.map_err(map_assistant_error)?;
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
async fn assistant_sse(
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

// -- files HTTP routes (Phase 1.4) ----------------------------------
//
// Rule 3: handlers serialize in, dispatch to `FilesService`, serialize
// out. No upload/download logic here â€” byte handling lives in the
// service so the MCP bridge and plugin channels reuse the same path.

#[derive(Deserialize)]
struct FilesListQuery {
    workspace_id: Option<String>,
    app_id: Option<uuid::Uuid>,
    limit: Option<u32>,
}

async fn list_files_route(
    State(state): State<ControlApiState>,
    Query(q): Query<FilesListQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = state
        .files
        .clone()
        .ok_or_else(|| ControlApiError::internal("files service not configured"))?;
    let files = service
        .list(ordo_files::FilesQuery {
            workspace_id: q.workspace_id,
            app_id: q.app_id,
            limit: q.limit,
        })
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(json!({ "files": files })))
}

async fn get_file_metadata_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = state
        .files
        .clone()
        .ok_or_else(|| ControlApiError::internal("files service not configured"))?;
    let entry = service
        .get_metadata(id)
        .map_err(|err| ControlApiError::internal(err.to_string()))?
        .ok_or_else(|| ControlApiError::not_found("file not found"))?;
    Ok(Json(json!({ "file": entry })))
}

async fn download_file_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let Some(service) = state.files.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "files service not configured" })),
        )
            .into_response();
    };
    match service.download(id).await {
        Ok((entry, bytes)) => {
            use axum::http::header;
            let mut response = bytes.into_response();
            let headers = response.headers_mut();
            if let Ok(value) = header::HeaderValue::from_str(&entry.content_type) {
                headers.insert(header::CONTENT_TYPE, value);
            }
            let disposition = format!(
                "inline; filename=\"{}\"",
                sanitize_header(&entry.original_name)
            );
            if let Ok(value) = header::HeaderValue::from_str(&disposition) {
                headers.insert(header::CONTENT_DISPOSITION, value);
            }
            response
        }
        Err(ordo_files::FilesError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file not found" })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn delete_file_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = state
        .files
        .clone()
        .ok_or_else(|| ControlApiError::internal("files service not configured"))?;
    let removed = service
        .delete(id)
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(
        json!({ "deleted": removed.is_some(), "file": removed }),
    ))
}

#[derive(Deserialize)]
struct UploadJsonBody {
    original_name: String,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    app_id: Option<uuid::Uuid>,
    #[serde(default)]
    created_by: Option<String>,
    /// Base64-encoded file bytes. Using a JSON body keeps the endpoint
    /// consistent with the MCP provider's `files.upload` tool â€” both
    /// carry bytes as base64. Raw multipart can be added as a second
    /// endpoint when a streaming use case emerges.
    data_base64: String,
}

async fn upload_file_json(
    State(state): State<ControlApiState>,
    Json(body): Json<UploadJsonBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = state
        .files
        .clone()
        .ok_or_else(|| ControlApiError::internal("files service not configured"))?;
    let bytes = base64_decode_minimal(&body.data_base64).map_err(ControlApiError::bad_request)?;
    let entry = service
        .upload(
            ordo_files::NewUpload {
                original_name: body.original_name,
                content_type: body.content_type,
                workspace_id: body.workspace_id,
                created_by: body.created_by,
                app_id: body.app_id,
            },
            bytes,
        )
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(json!({ "file": entry })))
}

/// Minimal base64 decoder â€” same algorithm as
/// `ordo-files/src/provider.rs` so the two stay in lockstep. Local
/// helper keeps the control crate's dep graph unchanged.
fn base64_decode_minimal(input: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in input.chars() {
        if c == '=' {
            break;
        }
        if c.is_whitespace() {
            continue;
        }
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - b'A' as u32,
            'a'..='z' => (c as u32) - b'a' as u32 + 26,
            '0'..='9' => (c as u32) - b'0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid base64 character '{c}'")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Ok(buf)
}

/// Strip anything that would break a `Content-Disposition` value â€”
/// namely double quotes and CRLF. Non-ASCII survives as-is since
/// modern browsers accept UTF-8 filenames in the simple form.
fn sanitize_header(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '"' | '\r' | '\n'))
        .collect()
}

// -- apps HTTP routes (Phase 1.5) -----------------------------------
//
// Mirrors `AppsService`. Status transitions get dedicated POST
// endpoints rather than status-in-PATCH so review gating can be
// applied uniformly (publish + archive are destructive per Rule 5).

#[derive(Deserialize)]
struct AppsListQuery {
    workspace_id: Option<String>,
    status: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct CreateAppBody {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    metadata: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    actor: Option<String>,
}

#[derive(Deserialize)]
struct UpdateAppBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    metadata_patch: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    actor: Option<String>,
}

#[derive(Deserialize)]
struct ActorBody {
    #[serde(default)]
    actor: Option<String>,
}

fn apps_service(state: &ControlApiState) -> Result<ordo_apps::AppsService, ControlApiError> {
    state
        .apps
        .clone()
        .ok_or_else(|| ControlApiError::internal("apps service not configured"))
}

fn map_apps_error(err: ordo_apps::AppsError) -> ControlApiError {
    use ordo_apps::AppsError;
    match err {
        AppsError::NotFound(_) => ControlApiError::not_found(err.to_string()),
        AppsError::InvalidArgument(_)
        | AppsError::InvalidTransition { .. }
        | AppsError::SlugConflict { .. } => ControlApiError::bad_request(err.to_string()),
        AppsError::Storage(_) => ControlApiError::internal(err.to_string()),
    }
}

async fn list_apps_route(
    State(state): State<ControlApiState>,
    Query(q): Query<AppsListQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let status = match q.status.as_deref() {
        None => None,
        Some(label) => Some(
            ordo_protocol::AppStatus::from_label(label)
                .ok_or_else(|| ControlApiError::bad_request(format!("unknown status: {label}")))?,
        ),
    };
    let apps = service
        .list(ordo_apps::AppsQuery {
            workspace_id: q.workspace_id,
            status,
            limit: q.limit,
        })
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "apps": apps })))
}

async fn create_app_route(
    State(state): State<ControlApiState>,
    Json(body): Json<CreateAppBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let actor = body.actor.clone().unwrap_or_else(|| "operator".into());
    let app = service
        .create(
            ordo_apps::NewApp {
                name: body.name,
                description: body.description,
                slug: body.slug,
                workspace_id: body.workspace_id,
                metadata: body.metadata,
            },
            &actor,
        )
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

async fn get_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let app = service
        .get(&ordo_apps::AppRef::Id(id))
        .map_err(map_apps_error)?
        .ok_or_else(|| ControlApiError::not_found("app not found"))?;
    Ok(Json(json!({ "app": app })))
}

async fn update_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<UpdateAppBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let app = service
        .update(
            &ordo_apps::AppRef::Id(id),
            ordo_apps::AppUpdate {
                name: body.name,
                description: body.description,
                metadata_patch: body.metadata_patch,
                actor: body.actor,
            },
        )
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

async fn list_app_events_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let events = service.events(id).map_err(map_apps_error)?;
    Ok(Json(json!({ "events": events })))
}

async fn get_app_state_at_version_route(
    State(state): State<ControlApiState>,
    Path((id, seq)): Path<(uuid::Uuid, u64)>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let app = service.state_at_version(id, seq).map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app, "seq": seq })))
}

async fn publish_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<ActorBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let actor = body.actor.unwrap_or_else(|| "operator".into());
    let app = service
        .publish(&ordo_apps::AppRef::Id(id), &actor)
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

async fn unpublish_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<ActorBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let actor = body.actor.unwrap_or_else(|| "operator".into());
    let app = service
        .unpublish(&ordo_apps::AppRef::Id(id), &actor)
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

async fn archive_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let app = service
        .archive(&ordo_apps::AppRef::Id(id), "operator")
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

async fn unarchive_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<ActorBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let actor = body.actor.unwrap_or_else(|| "operator".into());
    let app = service
        .unarchive(&ordo_apps::AppRef::Id(id), &actor)
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

// -- deployments HTTP routes (Phase 3.3) ----------------------------

#[derive(Deserialize)]
struct CreateDeploymentBody {
    #[serde(default)]
    preview_path: Option<String>,
    #[serde(default)]
    note: String,
}

async fn list_deployments_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let deployments = service.list_deployments(id).map_err(map_apps_error)?;
    Ok(Json(json!({ "deployments": deployments })))
}

async fn create_deployment_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<CreateDeploymentBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let deployment = service
        .create_deployment(id, body.preview_path, &body.note)
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "deployment": deployment })))
}

async fn promote_deployment_route(
    State(state): State<ControlApiState>,
    Path((_app_id, dep_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let deployment = service.promote_deployment(dep_id).map_err(map_apps_error)?;
    Ok(Json(json!({ "deployment": deployment })))
}

async fn fail_deployment_route(
    State(state): State<ControlApiState>,
    Path((_app_id, dep_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let deployment = service.fail_deployment(dep_id).map_err(map_apps_error)?;
    Ok(Json(json!({ "deployment": deployment })))
}

// -- webhooks HTTP routes (Phase 3.1) -------------------------------

#[derive(Deserialize)]
struct WebhookListQuery {
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
struct RegisterWebhookBody {
    target_url: String,
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
struct UpdateWebhookBody {
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    topics: Option<Vec<String>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    active: Option<bool>,
}

fn webhooks_service(
    state: &ControlApiState,
) -> Result<ordo_webhooks::WebhookService, ControlApiError> {
    state
        .webhooks
        .clone()
        .ok_or_else(|| ControlApiError::internal("webhooks service not configured"))
}

fn map_webhook_error(err: ordo_webhooks::WebhookError) -> ControlApiError {
    use ordo_webhooks::WebhookError;
    match err {
        WebhookError::NotFound(_) => ControlApiError::not_found(err.to_string()),
        WebhookError::InvalidArgument(_) => ControlApiError::bad_request(err.to_string()),
        WebhookError::Storage(_) => ControlApiError::internal(err.to_string()),
    }
}

async fn list_webhooks_route(
    State(state): State<ControlApiState>,
    Query(q): Query<WebhookListQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = webhooks_service(&state)?;
    let subs = service
        .list(q.workspace_id.as_deref())
        .map_err(map_webhook_error)?;
    Ok(Json(json!({ "subscriptions": subs })))
}

async fn register_webhook_route(
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

async fn get_webhook_route(
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

async fn update_webhook_route(
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

async fn delete_webhook_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = webhooks_service(&state)?;
    let deleted = service.delete(id).map_err(map_webhook_error)?;
    Ok(Json(json!({ "deleted": deleted })))
}

async fn assistant_websocket(
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

async fn assistant_websocket_session(
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

async fn list_ui_extensions(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let path = match &state.ui_extensions_path {
        Some(path) => path.clone(),
        None => {
            return Ok(Json(json!({
                "extensions_dir": null,
                "extensions": [],
                "errors": [],
            })));
        }
    };
    let report = ordo_ui_extensions::discover_ui_extensions(&path);
    let extensions: Vec<Value> = report
        .loaded
        .iter()
        .map(|loaded| {
            let surfaces: Vec<Value> = loaded
                .manifest
                .surfaces
                .iter()
                .map(|surface| match surface {
                    ordo_ui_extensions::Surface::Tab(tab) => json!({
                        "kind": "tab",
                        "id": tab.id,
                        "label": tab.label,
                        "icon": tab.icon,
                        "description": tab.description,
                        "entry_url": format!(
                            "/api/ui-extensions/{}/files/{}",
                            loaded.manifest.name, tab.entry
                        ),
                    }),
                })
                .collect();
            json!({
                "name": loaded.manifest.name,
                "version": loaded.manifest.version,
                "description": loaded.manifest.description,
                "author": loaded.manifest.author,
                "enabled": loaded.manifest.enabled,
                "surfaces": surfaces,
                "permissions": loaded.manifest.permissions,
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
    Ok(Json(json!({
        "extensions_dir": path.display().to_string(),
        "extensions": extensions,
        "errors": errors,
    })))
}

async fn serve_ui_bridge() -> Response {
    (
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        UI_BRIDGE_JS,
    )
        .into_response()
}

async fn serve_ui_extension_file(
    State(state): State<ControlApiState>,
    Path((name, request_path)): Path<(String, String)>,
) -> Response {
    let root = match &state.ui_extensions_path {
        Some(path) => path.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "ui extensions not configured" })),
            )
                .into_response();
        }
    };
    let report = ordo_ui_extensions::discover_ui_extensions(&root);
    let extension = match report
        .loaded
        .into_iter()
        .find(|ext| ext.manifest.name == name)
    {
        Some(ext) => ext,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("ui extension '{name}' not found") })),
            )
                .into_response();
        }
    };
    if !extension.manifest.enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("ui extension '{name}' is disabled") })),
        )
            .into_response();
    }
    let resolved = match extension.resolve_static(&request_path) {
        Ok(path) => path,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response();
        }
    };
    if !resolved.exists() || !resolved.is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("file not found: {request_path}") })),
        )
            .into_response();
    }
    match std::fs::read(&resolved) {
        Ok(body) => (
            StatusCode::OK,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    ordo_ui_extensions::content_type_for(&resolved),
                ),
                // Extensions load small static assets; a short cache is
                // a reasonable default. Development reloads still work
                // because the manifest list is always fresh.
                (axum::http::header::CACHE_CONTROL, "no-cache"),
            ],
            body,
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

// ---------- review -------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct ReviewRecentQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct ReviewDecisionBody {
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewEditBody {
    content: String,
    #[serde(default)]
    note: Option<String>,
}

fn require_review(state: &ControlApiState) -> Result<&ordo_review::ReviewService, ControlApiError> {
    state
        .review
        .as_ref()
        .ok_or_else(|| ControlApiError::internal("review service not configured on this runtime"))
}

fn parse_review_id(raw: &str) -> Result<uuid::Uuid, ControlApiError> {
    uuid::Uuid::parse_str(raw)
        .map_err(|err| ControlApiError::bad_request(format!("invalid review id: {err}")))
}

fn map_review_error(err: ordo_review::ReviewError) -> ControlApiError {
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

async fn list_review_pending(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let service = require_review(&state)?;
    let pending = service.pending().map_err(map_review_error)?;
    Ok(Json(json!({
        "count": pending.len(),
        "pending": pending,
    })))
}

async fn list_review_recent(
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

async fn get_review_request(
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

async fn approve_review_request(
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

async fn deny_review_request(
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

async fn edit_review_request(
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

async fn review_websocket(
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

async fn review_websocket_session(
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

async fn list_plugins(
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

fn plugin_state_label(state: &ordo_plugins::PluginState) -> &'static str {
    match state {
        ordo_plugins::PluginState::Active => "active",
        ordo_plugins::PluginState::Disabled => "disabled",
        ordo_plugins::PluginState::Failed(_) => "failed",
        ordo_plugins::PluginState::Invalid(_) => "invalid",
    }
}

fn plugin_state_detail(state: &ordo_plugins::PluginState) -> Option<String> {
    match state {
        ordo_plugins::PluginState::Active | ordo_plugins::PluginState::Disabled => None,
        ordo_plugins::PluginState::Failed(err) | ordo_plugins::PluginState::Invalid(err) => {
            Some(err.clone())
        }
    }
}

async fn set_plugin_enabled(
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

async fn disable_plugin(
    State(state): State<ControlApiState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    mutate_plugin_enabled(&state, &name, false)
}

fn mutate_plugin_enabled(
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
async fn invoke_tool_by_name(
    State(state): State<ControlApiState>,
    Path(capability): Path<String>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ControlApiError> {
    let arguments = body.map(|Json(value)| value).unwrap_or(Value::Null);
    invoke_tool(&state.brain, &capability, arguments).await
}

async fn list_cloud_credentials(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "cloud.credentials.list", json!({})).await
}

async fn upsert_cloud_credential(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "cloud.credentials.upsert", payload).await
}

async fn delete_cloud_credential(
    State(state): State<ControlApiState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "cloud.credentials.delete", payload).await
}

async fn invoke_tool(
    brain: &Brain,
    capability: &str,
    arguments: Value,
) -> Result<Json<Value>, ControlApiError> {
    let result = brain
        .invoke_tool(capability, arguments)
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(result))
}

fn parse_collection_query(value: Option<&str>) -> Vec<String> {
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
struct InstallMcpServerBody {
    server_id: String,
    /// Module bytes encoded as base64. Multipart upload is the
    /// alternative; for v1 keep the JSON path simple.
    module_b64: String,
    identity: ordo_protocol::ServerIdentity,
    declaration: ordo_protocol::CapabilityDeclaration,
    tool_catalog: Vec<ordo_protocol::ToolSchema>,
    #[serde(default)]
    limits: Option<ordo_protocol::ResourceLimits>,
}

#[derive(Debug, Deserialize)]
struct McpQuarantineBody {
    reason: String,
}

#[derive(Debug, Deserialize)]
struct McpReAuthorizeBody {
    declaration: ordo_protocol::CapabilityDeclaration,
    tool_catalog: Vec<ordo_protocol::ToolSchema>,
}

fn mcp_registry(
    state: &ControlApiState,
) -> Result<&Arc<ordo_mcp_registry::McpRegistryService>, ControlApiError> {
    state.mcp_registry.as_ref().ok_or_else(|| ControlApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "mcp registry service is not wired into the control API".into(),
    })
}

fn mcp_sandbox(
    state: &ControlApiState,
) -> Result<&Arc<ordo_mcp_sandbox::McpSandboxService>, ControlApiError> {
    state.mcp_sandbox.as_ref().ok_or_else(|| ControlApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "mcp sandbox service is not wired into the control API".into(),
    })
}

async fn list_mcp_servers_route(
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

async fn install_mcp_server_route(
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

async fn uninstall_mcp_server_route(
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

async fn quarantine_mcp_server_route(
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

async fn re_authorize_mcp_server_route(
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

async fn get_mcp_lockfile_route(
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
struct InvokeMcpToolBody {
    #[serde(default)]
    arguments: Value,
}

/// Direct sandbox invocation. The MCP client pipeline (Worker
/// extraction, DRIFT, taint provenance) is the *primary* path â€”
/// this raw-sandbox endpoint exists for development +
/// administration where an operator wants to drive a tool by
/// hand. The sandbox still enforces fuel + memory + rate limits +
/// host-call policy, so the invocation isn't unsafe; it just
/// skips the Planner/Worker structure.
async fn invoke_mcp_tool_route(
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
struct InstallUiExtensionBody {
    name: String,
    /// Map of relative path â†’ base64-encoded file bytes.
    /// `ui.json` is required and validated.
    files: std::collections::BTreeMap<String, String>,
}

async fn install_ui_extension_route(
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

async fn uninstall_ui_extension_route(
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

fn require_connections(
    state: &ControlApiState,
) -> Result<&Arc<ordo_connections::ConnectionService>, ControlApiError> {
    state.connections.as_ref().ok_or_else(|| ControlApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "connections service is not wired into the control API".into(),
    })
}

#[derive(Debug, Deserialize)]
struct CreateConnectionBody {
    type_id: String,
    friendly_name: String,
    #[serde(default)]
    fields: Value,
    /// Sealed in the vault on save; never echoed back. Optional even
    /// for types that require a secret so the field can be marked
    /// missing with a structured error.
    #[serde(default)]
    secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateConnectionBody {
    friendly_name: String,
    #[serde(default)]
    fields: Value,
    /// `Some(...)` rotates the secret. `None` leaves the existing
    /// vault row in place.
    #[serde(default)]
    secret: Option<String>,
}

fn map_connection_err(err: ordo_connections::ConnectionServiceError) -> ControlApiError {
    use ordo_connections::ConnectionServiceError as E;
    match err {
        E::NotFound(msg) => ControlApiError::not_found(msg),
        E::UnknownType(msg) => ControlApiError::bad_request(format!("unknown type: {msg}")),
        E::BadInput(msg) => ControlApiError::bad_request(msg),
        E::Store(inner) => ControlApiError::internal(inner.to_string()),
        E::Vault(inner) => ControlApiError::internal(inner.to_string()),
    }
}

async fn list_connection_types_route() -> Json<Value> {
    let catalog = ordo_connections::catalog();
    Json(json!({
        "count": catalog.len(),
        "types": catalog,
    }))
}

async fn list_connections_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    let rows = svc.list().await.map_err(map_connection_err)?;
    Ok(Json(json!({
        "count": rows.len(),
        "connections": rows,
    })))
}

async fn create_connection_route(
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

async fn get_connection_route(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    let row = svc.get(&id).await.map_err(map_connection_err)?;
    Ok(Json(serde_json::to_value(row).unwrap_or(Value::Null)))
}

async fn update_connection_route(
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

async fn delete_connection_route(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let svc = require_connections(&state)?;
    svc.delete(&id).await.map_err(map_connection_err)?;
    Ok(Json(json!({ "deleted": id })))
}

async fn test_connection_route(
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
                content: "Research metadata includes source titles, descriptions, slugs, and search intent."
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
                content: "Research metadata includes source titles, descriptions, slugs, and keyword intent."
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
                Request::get("/api/rag/preview?query=research%20metadata%20slug")
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
                    "/api/rag/preview?query=research%20metadata%20slug&collections=research",
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
                Request::get("/")
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
