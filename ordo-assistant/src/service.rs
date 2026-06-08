//! `AssistantService` Ã¢â‚¬â€ the turn-loop orchestrator.
//!
//! Responsibilities:
//! - Persist / load sessions + turns
//! - Recall facts from `FactStore`
//! - Route through the local RAG lane (via the shared bus)
//! - Assemble the LLM prompt (system + history + retrieval + user)
//! - **Let the LLM call platform capabilities autonomously** via the
//!   bus-backed `ToolGateway` (push 2)
//! - **Broadcast turn progress** (`ToolCallStarted/Completed`,
//!   `TurnCompleted`) over `EventBroadcaster` so the studio chat UI
//!   can render what the assistant is doing in real time (push 2)
//! - Call the cloud LLM with the configured credential
//! - Persist the final turn Ã¢â‚¬â€ including every tool call it made

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use futures::StreamExt;
use ordo_bus::Bus;
use ordo_cloud::{CloudCredentialTask, CloudHttp};
use ordo_models::EmbeddingClient;
use ordo_protocol::{
    infer_rag_collections, topics, CorrelationId, Envelope, NodeId, OrdoMessage, RagHit,
};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::timeout;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::events::{EventBroadcaster, TurnEvent};
use crate::knowledge::KnowledgeStore;
use crate::recall::FactStore;
use crate::store::AssistantStore;
use crate::tools::ToolGateway;
use crate::types::{
    AssistantError, AssistantResult, AssistantSession, CancelFlag, CancellationRegistry, Fact,
    FactSummary, KnowledgeKind, NewFact, NewKnowledge, RagHitSummary, RecalledFact, ReviewOutcome,
    SessionWithTurns, SpeechRequest, SpeechResponse, ToolInvocation, Turn, TurnContext,
    TurnRequest, TurnResult, MAX_SUBAGENT_DEPTH,
};

// Outer-bound LLM timeout used as a fallback when no credential is
// in scope yet (we still want a finite budget so a runaway awaitable
// can't hang the turn loop forever). For per-call timing we defer to
// `ordo_cloud::timeout_for(&credential)` so operators can extend a
// specific provider via `extras.timeout_secs` without touching code.
//
// 300 s matches `ordo_cloud::DEFAULT_REQUEST_TIMEOUT_SECS` so the
// HTTP-layer timeout (per-request, applied by reqwest) and the
// service-layer timeout (per-call, applied by tokio::time::timeout)
// fire at roughly the same moment instead of one short-circuiting
// the other.
#[allow(dead_code)]
const DEFAULT_LLM_TIMEOUT: Duration = Duration::from_secs(ordo_cloud::DEFAULT_REQUEST_TIMEOUT_SECS);
#[allow(dead_code)]
const DEFAULT_RAG_TIMEOUT: Duration = Duration::from_millis(1500);
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 6;

/// Stop autonomous tool use within a single turn after this many
/// "tool is gated on this conversation" errors. Without a cap a
/// confused model can spin against the cup gate â€” calling write
/// tool, getting blocked, calling it again â€” until the iteration
/// limit eats the whole turn budget. Three tries is enough for the
/// model to notice the pattern; if it hasn't given up by then,
/// we'd rather force a final-answer pass than another tool call.
///
/// Counted across all dispatch iterations in a turn, not per
/// iteration â€” a model emitting 3 gated calls in a single batch
/// trips this on the first iteration boundary, just like 3 over
/// three iterations would.
const MAX_GATE_REJECTIONS_PER_TURN: usize = 3;
const MAX_DUPLICATE_TOOL_CALLS_PER_TURN: usize = 1;
const REASONING_PREVIEW_PREFIX: &str = "(no content emitted; reasoning preview)";

fn visible_assistant_message(response: &Value, invocations: &[ToolInvocation]) -> String {
    let content_raw = response
        .get("content_raw")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if !content_raw.is_empty() {
        return content_raw.to_string();
    }

    let assistant_message = response
        .get("assistant_message")
        .and_then(|value| value.as_str())
        .or_else(|| {
            response
                .get("assistant_text")
                .and_then(|value| value.as_str())
        })
        .unwrap_or("")
        .trim();
    if !assistant_message.is_empty() && !assistant_message.starts_with(REASONING_PREVIEW_PREFIX) {
        return assistant_message.to_string();
    }

    synthesize_tool_result_fallback(invocations)
}

fn synthesize_tool_result_fallback(invocations: &[ToolInvocation]) -> String {
    if invocations.is_empty() {
        return "The model returned no visible answer. It emitted reasoning-only output, so Ordo suppressed that internal reasoning instead of treating it as a response. Try lowering thinking effort or switching to a non-reasoning local model for this turn.".into();
    }

    let mut lines = vec![
        "The model completed tool work but returned no visible final answer. Ordo suppressed the reasoning-only output and is showing the completed tool results instead."
            .to_string(),
        String::new(),
        "Tool results:".to_string(),
    ];
    for invocation in invocations {
        if let Some(error) = &invocation.error {
            lines.push(format!("- {}: failed, {}", invocation.capability, error));
            continue;
        }
        let summary = invocation
            .result
            .as_ref()
            .map(compact_tool_result_summary)
            .unwrap_or_else(|| "completed with no returned payload".to_string());
        lines.push(format!("- {}: {}", invocation.capability, summary));
    }
    lines.join("\n")
}

fn compact_tool_result_summary(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut fields = Vec::new();
            for key in [
                "ok",
                "count",
                "profile",
                "control_api_enabled",
                "rag_enabled",
                "knowledge_enabled",
                "service",
                "error",
            ] {
                if let Some(value) = object.get(key) {
                    fields.push(format!("{key}={}", compact_json_scalar(value)));
                }
            }
            if fields.is_empty() {
                truncate_chars(&serde_json::to_string(value).unwrap_or_default(), 420)
            } else {
                fields.join(", ")
            }
        }
        _ => truncate_chars(&serde_json::to_string(value).unwrap_or_default(), 420),
    }
}

fn compact_json_scalar(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => "null".into(),
        _ => truncate_chars(&serde_json::to_string(value).unwrap_or_default(), 160),
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn tool_call_signature(capability: &str, arguments: &Value) -> String {
    format!(
        "{}\n{}",
        capability,
        serde_json::to_string(arguments).unwrap_or_default()
    )
}

fn is_diagnostic_mode(mode: Option<&ordo_modes::ModeManifest>) -> bool {
    mode.map(|manifest| manifest.id == "diagnostic")
        .unwrap_or(false)
}

fn diagnostic_allows_cloud_models(request: &TurnRequest) -> bool {
    request
        .metadata
        .get("diagnostic")
        .and_then(|value| value.as_object())
        .and_then(|diagnostic| diagnostic.get("allow_cloud_models"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn is_local_llm_credential(credential: &ordo_cloud::CloudCredential) -> bool {
    let service = credential.service.to_ascii_lowercase();
    let provider_kind = credential
        .extras
        .get("provider_kind")
        .map(|value| value.to_ascii_lowercase());
    let base_url = credential
        .base_url
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();

    if matches!(
        provider_kind.as_deref(),
        Some("cloud_model" | "cloud" | "remote_model")
    ) {
        return false;
    }

    matches!(provider_kind.as_deref(), Some("local_model"))
        || service == "ollama"
        || service == "ollama_local"
        || service == "lmstudio"
        || service == "lm-studio"
        || base_url.contains("localhost")
        || base_url.contains("127.0.0.1")
        || base_url.contains("[::1]")
}

#[derive(Clone)]
pub struct AssistantService {
    store: Arc<Mutex<AssistantStore>>,
    facts: FactStore,
    knowledge: KnowledgeStore,
    credentials: CloudCredentialTask,
    http: CloudHttp,
    bus: Option<Arc<dyn Bus>>,
    default_service: String,
    /// Ordered list of credential service names to try when the
    /// primary credential isn't configured or isn't loadable (Phase
    /// 4.5). Empty = no failover. The chain is *resolution-time
    /// only* today Ã¢â‚¬â€ call-time retry across providers is a follow-up
    /// that requires deeper refactor of the turn loop.
    failover_chain: Vec<String>,
    tools: Option<ToolGateway>,
    events: EventBroadcaster,
    max_tool_iterations: usize,
    /// Optional review sink. When present, turns that set
    /// `review: true` route their draft through this service and wait
    /// for a decision before persisting.
    review: Option<ordo_review::ReviewService>,
    /// Optional hierarchical memory log (blueprint v2). When wired,
    /// every turn persists `user.message` + `agent.response` events
    /// into the DPM substrate. Absence is explicit: older deploys
    /// and tests don't need the log to work.
    memory_log: Option<ordo_memory_log::MemoryLogService>,
    cancellations: CancellationRegistry,

    /// Per-session taint state â€” tracks which conversations have
    /// ingested untrusted content (web fetches via the strainer,
    /// MCP outputs, etc.) so sensitive actions on tainted
    /// conversations can be gated.
    ///
    /// Defends against slow-injection: a hostile page plants a fact
    /// in turn 1, the model "remembers" it across turns, and at
    /// turn 5 the model is happy to act on the planted instruction
    /// because the boundary tags from turn 1 are out of sight. The
    /// taint persists across turns within the session, so a
    /// downstream sensitive action sees the tainted ancestry
    /// regardless of how far back the injection sits.
    ///
    /// In-memory only â€” runtime restart wipes session ids anyway,
    /// so persistence would be theater. The studio's auto-recovery
    /// on stale-session creates fresh sessions with clean taint
    /// when needed. If a tainted session needs to be cleared
    /// without restart, the operator hits the "clear taint" action.
    session_taint: Arc<Mutex<HashMap<Uuid, Vec<ordo_protocol::Taint>>>>,

    /// Mode registry. The runtime resolves a session's stored mode
    /// id to a `ModeManifest` here whenever a turn fires. Built
    /// from `<runtime>/user-files/modes/*.json` plus the compiled-in
    /// defaults. Cheap to clone (Arc inside).
    ///
    /// When None (older constructors that haven't been migrated yet,
    /// in-memory tests that don't care about modes), turn behavior
    /// falls back to the pre-mode shape: no scope filtering, all
    /// existing lane allowlists in effect.
    modes: Option<ordo_modes::ModeRegistry>,
}

impl AssistantService {
    pub fn new(
        store: AssistantStore,
        embedder: Arc<dyn EmbeddingClient>,
        credentials: CloudCredentialTask,
    ) -> Self {
        let store = Arc::new(Mutex::new(store));
        let facts = FactStore::new(store.clone(), embedder.clone());
        let knowledge = KnowledgeStore::new(store.clone(), embedder);
        let http = CloudHttp::new();
        let default_service = "openai".to_string();
        Self {
            store,
            facts,
            knowledge,
            credentials,
            http,
            bus: None,
            default_service,
            failover_chain: Vec::new(),
            tools: None,
            events: EventBroadcaster::new(),
            max_tool_iterations: DEFAULT_MAX_TOOL_ITERATIONS,
            review: None,
            memory_log: None,
            cancellations: CancellationRegistry::new(),
            session_taint: Arc::new(Mutex::new(HashMap::new())),
            modes: None,
        }
    }

    /// Attach a mode registry. Without this, the assistant operates
    /// in pre-mode legacy shape (no scope filtering). The runtime's
    /// startup wires this whenever the modes directory is loaded.
    pub fn with_modes(mut self, modes: ordo_modes::ModeRegistry) -> Self {
        self.modes = Some(modes);
        self
    }

    /// All registered modes, sorted by id. Empty when the assistant
    /// has no registry attached. Used by the studio's mode switcher
    /// (`GET /api/modes`) and by the advanced view (full manifest
    /// inspection).
    pub fn list_modes(&self) -> Vec<ordo_modes::ModeManifest> {
        match &self.modes {
            Some(reg) => reg.list(),
            None => Vec::new(),
        }
    }

    /// Look up a single mode by id. None when absent or no registry.
    pub fn get_mode(&self, id: &str) -> Option<ordo_modes::ModeManifest> {
        self.modes.as_ref().and_then(|reg| reg.get(id))
    }

    /// Read access to the registered mode for a session id, falling
    /// back to the General Assistant manifest when the session's
    /// stored mode isn't in the registry. Returns None when the
    /// service has no registry attached at all (legacy shape).
    fn resolve_mode_for_session(&self, session_id: Uuid) -> Option<ordo_modes::ModeManifest> {
        self.resolve_session_mode_manifest(session_id)
    }

    /// Public version of `resolve_mode_for_session` â€” exposed so
    /// external callers (e.g. the MCP-host bus bridge) can resolve
    /// a session's mode when they receive a session_id argument
    /// from a third-party MCP client. Same fallback semantics as
    /// the internal version: unknown id â†’ General; no registry
    /// attached â†’ None.
    pub fn resolve_session_mode_manifest(
        &self,
        session_id: Uuid,
    ) -> Option<ordo_modes::ModeManifest> {
        let registry = self.modes.as_ref()?;
        let session = match self.store.lock().get_session(session_id).ok().flatten() {
            Some(s) => s,
            None => return registry.get(ordo_modes::DEFAULT_MODE_ID),
        };
        registry
            .get(&session.mode)
            .or_else(|| registry.get(ordo_modes::DEFAULT_MODE_ID))
    }

    // â”€â”€â”€ Session-taint surface â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    //
    // Public so the control-API can surface taint state to the
    // studio (`GET /api/assistant/sessions/:id/taint`) and clear it
    // on operator request (`POST .../taint/clear`).

    /// Mark a session as tainted by the supplied source. Idempotent
    /// per source â€” repeated tags from the same URL don't accumulate.
    pub fn taint_session(&self, session_id: Uuid, taint: ordo_protocol::Taint) {
        let mut map = self.session_taint.lock();
        let entry = map.entry(session_id).or_default();
        if !entry.contains(&taint) {
            entry.push(taint);
        }
    }

    /// All taint sources for a session. Empty Vec if untainted.
    pub fn session_taints(&self, session_id: Uuid) -> Vec<ordo_protocol::Taint> {
        self.session_taint
            .lock()
            .get(&session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// True if any taint source on the session would gate sensitive
    /// actions. Currently any `UntrustedWeb` or `UntrustedMcp` ancestor
    /// counts as untrusted.
    pub fn session_is_tainted(&self, session_id: Uuid) -> bool {
        self.session_taint
            .lock()
            .get(&session_id)
            .map(|v| v.iter().any(|t| t.is_untrusted()))
            .unwrap_or(false)
    }

    /// Clear all taint for the session. Operator-driven only â€” the
    /// studio surfaces a "clear" button next to the taint indicator.
    /// Returns true if the session had been tainted.
    pub fn clear_session_taint(&self, session_id: Uuid) -> bool {
        self.session_taint.lock().remove(&session_id).is_some()
    }

    /// Scan a turn's user_message for the strainer's boundary tag.
    /// Used by the turn loop to auto-mark a session tainted when
    /// it ingests `<untrusted_web_content>` content.
    ///
    /// Returns extracted (source_url, fetched_at) when a boundary
    /// tag is detected, or None. Multiple boundary tags in one
    /// message produce one Taint each (the caller iterates).
    pub fn detect_untrusted_web_taints(message: &str) -> Vec<ordo_protocol::Taint> {
        let mut out = Vec::new();
        let mut cursor = 0;
        while let Some(open) = message[cursor..].find("<untrusted_web_content") {
            let abs = cursor + open;
            // Bound the tag to the closing `>` on the open element.
            let tag_end = match message[abs..].find('>') {
                Some(e) => abs + e + 1,
                None => break,
            };
            let tag_str = &message[abs..tag_end];
            let source_url = extract_attr(tag_str, "source").unwrap_or_default();
            let fetched_at = extract_attr(tag_str, "fetched_at").unwrap_or_default();
            out.push(ordo_protocol::Taint::UntrustedWeb {
                source_url,
                fetched_at,
            });
            cursor = tag_end;
        }
        out
    }

    /// Shared cancellation registry. The control-API WebSocket holds a
    /// clone; when the socket closes, it flips the per-session flag so
    /// the turn loop bails out on the next iteration boundary.
    pub fn cancellations(&self) -> CancellationRegistry {
        self.cancellations.clone()
    }

    /// Cancel an in-flight turn for `session_id`. Returns true if a
    /// running turn was found and flagged.
    pub fn cancel_turn(&self, session_id: Uuid) -> bool {
        self.cancellations.cancel(session_id)
    }

    /// Attach a `ReviewService` so turns with `review: true` route
    /// through the operator approval queue before being persisted.
    pub fn with_review(mut self, review: ordo_review::ReviewService) -> Self {
        self.review = Some(review);
        self
    }

    /// Wire the hierarchical memory log (blueprint v2). When set,
    /// every turn appends `user.message` (before the LLM call) and
    /// `agent.response` (after) events. The agent response is chained
    /// to the user message via `parent_id`, building a replay chain.
    pub fn with_memory_log(mut self, log: ordo_memory_log::MemoryLogService) -> Self {
        self.memory_log = Some(log);
        self
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.tools = Some(ToolGateway::new(bus.clone()));
        self.bus = Some(bus);
        self
    }

    /// Configure a fallback chain of credential service names. Tried
    /// in order at credential resolution if the primary isn't
    /// available. Primary picking: `request.credential` (if set) Ã¢â€ â€™
    /// `default_service` Ã¢â€ â€™ each name in `failover_chain` in order.
    pub fn with_failover_chain(mut self, chain: Vec<String>) -> Self {
        self.failover_chain = chain;
        self
    }

    pub fn with_default_service(mut self, service: impl Into<String>) -> Self {
        self.default_service = service.into();
        self
    }

    pub fn with_http(mut self, http: CloudHttp) -> Self {
        self.http = http;
        self
    }

    pub fn knowledge(&self) -> &KnowledgeStore {
        &self.knowledge
    }

    /// Override the default tool allow list.
    pub fn with_tool_gateway(mut self, gateway: ToolGateway) -> Self {
        self.tools = Some(gateway);
        self
    }

    pub fn with_max_tool_iterations(mut self, iterations: usize) -> Self {
        self.max_tool_iterations = iterations;
        self
    }

    pub fn facts(&self) -> &FactStore {
        &self.facts
    }

    /// Shared event broadcaster; the control-API WebSocket subscribes
    /// per session to stream live progress to the studio.
    pub fn events(&self) -> EventBroadcaster {
        self.events.clone()
    }

    /// Append `user.message` to the memory log if one is wired.
    /// Returns the event id so the caller can parent the eventual
    /// `agent.response` event to it. Never fails the turn Ã¢â‚¬â€ the log
    /// is ambient substrate, not a hard dependency. Returns `None`
    /// when no log is wired or when the append failed (logged).
    async fn log_user_message(
        &self,
        session_id: Uuid,
        request: &TurnRequest,
        turn_id: &str,
    ) -> Option<String> {
        let Some(log) = &self.memory_log else {
            return None;
        };
        // Include turn_id in the payload so identical user text
        // across two turns doesn't collide in the dedupe window.
        let payload = json!({
            "session_id": session_id,
            "turn_id": turn_id,
            "text": request.user_message,
            "attachments_count": request.attachments.len(),
        });
        let payload_hash = ordo_memory_log::MemoryLogService::compute_payload_hash(&payload);
        let event = ordo_protocol::MemoryEvent {
            id: ordo_memory_log::MemoryLogService::new_event_id(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            event_type: ordo_protocol::MemoryEventType::UserMessage,
            actor: "operator".into(),
            domain: None,
            category: Some("assistant.turn".into()),
            parent_id: None,
            turn_id: Some(turn_id.to_string()),
            payload,
            payload_hash,
            tier: ordo_protocol::RetentionTier::Hot,
            pinned: false,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        };
        match log.append(event).await {
            Ok(result) => Some(result.event.id),
            Err(err) => {
                tracing::warn!(
                    target: "ordo_assistant::memory_log",
                    error = %err,
                    "user.message append failed (turn continues)"
                );
                None
            }
        }
    }

    /// Append `agent.response` to the memory log, chained to the
    /// parent `user.message` event so replay can reconstruct the
    /// pair.
    async fn log_agent_response(
        &self,
        session_id: Uuid,
        response_text: &str,
        parent_id: Option<&str>,
        turn_id: Option<&str>,
        model: Option<&str>,
        credential_service: &str,
    ) {
        let Some(log) = &self.memory_log else {
            return;
        };
        let payload = json!({
            "session_id": session_id,
            "turn_id": turn_id,
            "text": response_text,
            "model": model,
            "credential_service": credential_service,
        });
        let payload_hash = ordo_memory_log::MemoryLogService::compute_payload_hash(&payload);
        let event = ordo_protocol::MemoryEvent {
            id: ordo_memory_log::MemoryLogService::new_event_id(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            event_type: ordo_protocol::MemoryEventType::AgentResponse,
            actor: "assistant".into(),
            domain: None,
            category: Some("assistant.turn".into()),
            parent_id: parent_id.map(str::to_string),
            turn_id: turn_id.map(str::to_string),
            payload,
            payload_hash,
            tier: ordo_protocol::RetentionTier::Hot,
            pinned: false,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        };
        if let Err(err) = log.append(event).await {
            tracing::warn!(
                target: "ordo_assistant::memory_log",
                error = %err,
                "agent.response append failed"
            );
        }
    }

    /// Build an `AutoExtractor` that shares this service's SQLite
    /// store, fact store, credentials, and HTTP client. Callers
    /// spawn it on a background task with `.run(interval).await`.
    pub fn auto_extractor(&self) -> crate::extractor::AutoExtractor {
        crate::extractor::AutoExtractor::new(
            self.store.clone(),
            self.facts.clone(),
            self.credentials.clone(),
            self.http.clone(),
            self.default_service.clone(),
        )
    }

    // ---- sessions --------------------------------------------------

    /// Create a new conversation session in the requested mode.
    /// Validates `mode` against the registry when one is attached;
    /// unknown ids fail closed (the operator gets a clear error,
    /// not a session silently defaulted to General).
    ///
    /// `mode = None` falls back to the General Assistant default.
    pub fn new_session(
        &self,
        title: Option<&str>,
        mode: Option<&str>,
    ) -> AssistantResult<AssistantSession> {
        let mode_id = if let Some(explicit) = mode {
            // Explicit mode: validate and use.
            if let Some(registry) = &self.modes {
                if registry.get(explicit).is_none() {
                    return Err(AssistantError::InvalidArgument(format!(
                        "mode '{explicit}' is not registered; check the modes directory or pick a known mode"
                    )));
                }
            }
            explicit.to_string()
        } else {
            // No explicit mode: use General. Automatic mode routing is
            // intentionally disabled until it can be developed and audited
            // as a separate subsystem.
            let _ = title;
            ordo_modes::DEFAULT_MODE_ID.to_string()
        };
        self.store.lock().create_session(title, &mode_id)
    }

    pub fn list_sessions(&self, limit: usize) -> AssistantResult<Vec<AssistantSession>> {
        self.store.lock().list_sessions(limit)
    }

    pub fn get_session(&self, id: Uuid) -> AssistantResult<Option<SessionWithTurns>> {
        self.store.lock().load_session_with_turns(id)
    }

    pub fn list_turns(&self, session_id: Uuid) -> AssistantResult<Vec<Turn>> {
        self.store.lock().list_turns(session_id)
    }

    pub async fn speak_text(&self, request: SpeechRequest) -> AssistantResult<SpeechResponse> {
        let mut candidates = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |name: String| {
            if !name.trim().is_empty() && seen.insert(name.clone()) {
                candidates.push(name);
            }
        };
        if let Some(service) = request.service.as_ref() {
            push(service.clone());
        }
        if let Ok(Some(default_service)) = self.credentials.get_default().await {
            push(default_service);
        }
        push(self.default_service.clone());
        if let Ok(all) = self.credentials.list().await {
            for credential in all {
                push(credential.service);
            }
        }

        let mut last_error: Option<AssistantError> = None;
        for service in candidates {
            let credential = match self.credentials.get(service.clone()).await {
                Ok(Some(credential)) => credential,
                Ok(None) => {
                    last_error = Some(AssistantError::NoCredential(service));
                    continue;
                }
                Err(err) => {
                    last_error = Some(AssistantError::Storage(err.to_string()));
                    continue;
                }
            };
            if credential.auth_style == "anthropic" {
                last_error = Some(AssistantError::InvalidArgument(format!(
                    "speech provider '{}' is not OpenAI-compatible",
                    credential.service
                )));
                continue;
            }
            let model = request
                .model
                .clone()
                .or_else(|| credential.extras.get("tts_model").cloned())
                .unwrap_or_else(|| ordo_cloud::openai::DEFAULT_TTS_MODEL.to_string());
            let voice = request
                .voice
                .clone()
                .or_else(|| credential.extras.get("tts_voice").cloned())
                .unwrap_or_else(|| ordo_cloud::openai::DEFAULT_TTS_VOICE.to_string());
            let format = request
                .format
                .clone()
                .or_else(|| credential.extras.get("tts_format").cloned())
                .unwrap_or_else(|| ordo_cloud::openai::DEFAULT_TTS_FORMAT.to_string());
            let args = json!({
                "input": request.input.clone(),
                "model": model,
                "voice": voice,
                "response_format": format,
                "instructions": request.instructions.clone(),
                "speed": request.speed,
            });
            match ordo_cloud::openai::speech(&self.http, &credential, &args).await {
                Ok(audio) => {
                    return Ok(SpeechResponse {
                        bytes: audio.bytes,
                        content_type: audio.content_type,
                        format: audio.format,
                        credential_service: credential.service,
                        model,
                        voice,
                    });
                }
                Err(err) => {
                    last_error = Some(AssistantError::LlmFailed(err.to_string()));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| AssistantError::NoCredential("openai".into())))
    }

    // ---- facts -----------------------------------------------------

    pub async fn remember_fact(&self, new_fact: NewFact) -> AssistantResult<Fact> {
        self.facts.remember(new_fact).await
    }

    pub fn forget_fact(&self, id: Uuid) -> AssistantResult<bool> {
        self.facts.forget(id)
    }

    pub fn list_facts(&self, subject: Option<&str>) -> AssistantResult<Vec<FactSummary>> {
        let facts = self.facts.list(subject)?;
        Ok(facts.iter().map(FactSummary::from).collect())
    }

    pub async fn recall(&self, query: &str, top_k: usize) -> AssistantResult<Vec<RecalledFact>> {
        self.facts.recall(query, top_k).await
    }

    // ---- turn loop -------------------------------------------------

    /// Phase 4.1: spawn a subagent Ã¢â‚¬â€ a scoped assistant turn with its
    /// own fresh session, tighter tool budget, and depth + 1. Returns
    /// the subagent's final response plus the usual turn bookkeeping.
    ///
    /// The provider wrapper in `ordo-mcp-host::AssistantProvider` is what
    /// the LLM calls as `assistant.spawn_subagent`. This method is
    /// the service-side implementation and is also callable directly
    /// by other services (e.g. a planner that wants to delegate).
    pub async fn spawn_subagent(
        &self,
        parent_depth: u32,
        goal: String,
        max_iterations: Option<usize>,
    ) -> AssistantResult<TurnResult> {
        self.spawn_subagent_in_mode(parent_depth, goal, max_iterations, None)
            .await
    }

    pub async fn spawn_subagent_in_mode(
        &self,
        parent_depth: u32,
        goal: String,
        max_iterations: Option<usize>,
        mode: Option<String>,
    ) -> AssistantResult<TurnResult> {
        let child_depth = parent_depth.saturating_add(1);
        if child_depth > MAX_SUBAGENT_DEPTH {
            return Err(AssistantError::SubagentBudgetExceeded(
                child_depth,
                MAX_SUBAGENT_DEPTH,
            ));
        }
        // Subagent turns get a narrower iteration budget. Clones
        // inherit the service's max, but can be tightened per-call to
        // keep nested tool use bounded.
        let sub_service = if let Some(max) = max_iterations {
            let mut s = self.clone();
            s.max_tool_iterations = max;
            s
        } else {
            self.clone()
        };
        let child_request = TurnRequest {
            subagent_depth: child_depth,
            user_message: goal,
            // Fresh session Ã¢â‚¬â€ subagents don't pollute the operator's
            // thread.
            session_id: None,
            // Keep tool use on so the subagent can actually do work;
            // review off (subagents run inside the operator's
            // already-authorized scope).
            use_tools: true,
            review: false,
            stream: false,
            mode,
            ..Default::default()
        };
        sub_service.turn(child_request).await
    }

    pub async fn turn(&self, request: TurnRequest) -> AssistantResult<TurnResult> {
        if request.user_message.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "user_message must not be empty".into(),
            ));
        }
        // Phase 4.1 recursion budget: reject turns past the cap. The
        // spawner is the one that increments depth; operator turns
        // always arrive with depth 0.
        if request.subagent_depth > MAX_SUBAGENT_DEPTH {
            return Err(AssistantError::SubagentBudgetExceeded(
                request.subagent_depth,
                MAX_SUBAGENT_DEPTH,
            ));
        }

        let session = match request.session_id {
            Some(id) => self
                .store
                .lock()
                .get_session(id)?
                .ok_or(AssistantError::SessionNotFound(id))?,
            None => {
                // Auto-create with the requested mode (or General
                // by default). Validate against the registry first
                // so an unknown mode fails fast and clear instead
                // of writing a session that nothing knows how to
                // resolve.
                let mode_id = request
                    .mode
                    .as_deref()
                    .unwrap_or(ordo_modes::DEFAULT_MODE_ID);
                if let Some(registry) = &self.modes {
                    if registry.get(mode_id).is_none() {
                        return Err(AssistantError::InvalidArgument(format!(
                            "mode '{mode_id}' is not registered"
                        )));
                    }
                }
                self.store.lock().create_session(None, mode_id)?
            }
        };
        let session_id = session.id;

        self.events.publish(
            session_id,
            TurnEvent::TurnStarted {
                session_id,
                user_message: request.user_message.clone(),
            },
        );

        // Auto-mark this session as web-tainted when the user_message
        // contains `<untrusted_web_content>` boundary tags (output of
        // ordo-strainer). The taint persists through the rest of the
        // session so subsequent sensitive actions get gated even when
        // the strained content is many turns back â€” defends against
        // slow injection ("plant a fact now, exploit later").
        for taint in Self::detect_untrusted_web_taints(&request.user_message) {
            self.taint_session(session_id, taint);
        }

        // Register (or reuse) a cancellation flag for this session so
        // external callers (e.g. the WS \"stop\" button) can interrupt
        // the loop. Always released when the turn exits, regardless
        // of outcome.
        let cancel = self.cancellations.register(session_id);
        let cancellations = self.cancellations.clone();
        let outcome = self.run_turn(&session, &request, cancel).await;
        cancellations.release(session_id);

        // Run the turn in a helper so we can consistently publish
        // `TurnFailed` on any error path.
        match outcome {
            Ok(result) => {
                self.events.publish(
                    session_id,
                    TurnEvent::TurnCompleted {
                        session_id,
                        turn: result.turn.clone(),
                    },
                );
                Ok(result)
            }
            Err(err) => {
                self.events.publish(
                    session_id,
                    TurnEvent::TurnFailed {
                        session_id,
                        error: err.to_string(),
                    },
                );
                Err(err)
            }
        }
    }

    async fn run_turn(
        &self,
        session: &AssistantSession,
        request: &TurnRequest,
        cancel: CancelFlag,
    ) -> AssistantResult<TurnResult> {
        let check_cancel = || -> AssistantResult<()> {
            if cancel.is_cancelled() {
                Err(AssistantError::Cancelled)
            } else {
                Ok(())
            }
        };
        let session_id = session.id;

        // Blueprint concern 2: every event emitted during this turn
        // shares a `turn_id`. Stamped on user + agent events now,
        // ready for tool-call / router-decision events in future
        // elaborations.
        let turn_id = ordo_memory_log::MemoryLogService::new_event_id();

        // Blueprint v2: append `user.message` to the memory log
        // before the LLM call. Returned id parents the eventual
        // `agent.response`, giving replay a two-link chain per turn.
        let user_event_id = self.log_user_message(session_id, request, &turn_id).await;

        // --- Retrieval -------------------------------------------------
        //
        // Progressive disclosure (push 3): we no longer pre-fetch facts
        // or RAG hits into the prompt. The LLM pulls them on demand via
        // the `assistant.recall_memory` and `assistant.knowledge_lookup`
        // meta-tools. We still emit an
        // (empty) `ContextRetrieved` event for UI continuity and store
        // `tool_calls` in `TurnContext` so the studio side-rail can
        // show what the assistant actually pulled.
        let recalled_facts: Vec<RecalledFact> = Vec::new();
        let rag_summaries: Vec<RagHitSummary> = Vec::new();

        self.events.publish(
            session_id,
            TurnEvent::ContextRetrieved {
                session_id,
                facts: recalled_facts.clone(),
                rag_hits: rag_summaries.clone(),
            },
        );

        // Load conversation history (last N turns before this one).
        let history = self
            .store
            .lock()
            .list_turns(session_id)?
            .into_iter()
            .rev()
            .take(request.history_window)
            .collect::<Vec<_>>();
        let history: Vec<Turn> = history.into_iter().rev().collect();

        // Resolve the active mode before credential selection so mode
        // policy can constrain provider choice. Diagnostic mode
        // denies cloud credentials by default, but the UXI can set
        // metadata.diagnostic.allow_cloud_models for an explicit
        // operator-approved cloud diagnostic turn.
        let active_mode = self.resolve_mode_for_session(session.id);
        let diagnostic_mode = is_diagnostic_mode(active_mode.as_ref());
        let diagnostic_cloud_allowed = diagnostic_allows_cloud_models(request);

        // --- Credential (with Phase 4.5 resolution-time failover) ------
        //
        // Build an ordered candidate list: explicit per-request
        // override Ã¢â€ â€™ default_service Ã¢â€ â€™ each name in failover_chain.
        // Deduplicate preserving order. Walk the list and take the
        // first credential that loads; otherwise return the error
        // from the primary so the operator sees the most relevant
        // misconfiguration.
        let mut candidates: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let push =
            |name: String, dest: &mut Vec<String>, seen: &mut std::collections::HashSet<String>| {
                if !name.is_empty() && seen.insert(name.clone()) {
                    dest.push(name);
                }
            };
        if let Some(explicit) = &request.credential {
            push(explicit.clone(), &mut candidates, &mut seen);
        }
        if let Some(mode_default) = active_mode
            .as_ref()
            .and_then(|mode| mode.default_credential.as_ref())
        {
            push(mode_default.clone(), &mut candidates, &mut seen);
        }
        push(self.default_service.clone(), &mut candidates, &mut seen);
        for name in &self.failover_chain {
            push(name.clone(), &mut candidates, &mut seen);
        }
        // Provider-neutral fallback: if none of the explicit candidates
        // resolved, walk every configured credential. Lets operators
        // ship Ordo with no hardcoded provider preference â€” whichever
        // credential exists is usable.
        if let Ok(all) = self.credentials.list().await {
            for cred in all {
                push(cred.service.clone(), &mut candidates, &mut seen);
            }
        }
        let mut primary_error: Option<AssistantError> = None;
        let mut resolved: Option<(String, ordo_cloud::CloudCredential)> = None;
        for name in &candidates {
            match self.credentials.get(name.clone()).await {
                Ok(Some(cred)) => {
                    if diagnostic_mode
                        && !diagnostic_cloud_allowed
                        && !is_local_llm_credential(&cred)
                    {
                        if request.credential.as_deref() == Some(name.as_str()) {
                            return Err(AssistantError::InvalidArgument(format!(
                                "diagnostic mode can only use local model credentials; '{name}' is not local"
                            )));
                        }
                        if primary_error.is_none() {
                            primary_error = Some(AssistantError::NoCredential(
                                "diagnostic-local-model".into(),
                            ));
                        }
                        continue;
                    }
                    resolved = Some((name.clone(), cred));
                    break;
                }
                Ok(None) => {
                    if primary_error.is_none() {
                        primary_error = Some(AssistantError::NoCredential(name.clone()));
                    }
                }
                Err(err) => {
                    if primary_error.is_none() {
                        primary_error = Some(AssistantError::Storage(err.to_string()));
                    }
                }
            }
        }
        let (credential_service_init, credential_init) = resolved.ok_or_else(|| {
            primary_error.unwrap_or_else(|| {
                AssistantError::NoCredential(
                    candidates
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "<none>".into()),
                )
            })
        })?;
        // Remaining candidates for CALL-TIME failover: everything
        // after the one we just resolved. On LLM transport error or
        // timeout we advance through this list before giving up.
        // (Follow-up 1 of the memory blueprint follow-ups.)
        let resolved_position = candidates
            .iter()
            .position(|n| n == &credential_service_init)
            .unwrap_or(candidates.len());
        let mut failover_remaining: Vec<String> = candidates
            .iter()
            .skip(resolved_position + 1)
            .cloned()
            .collect();
        if diagnostic_mode && !diagnostic_cloud_allowed {
            let mut local_failover = Vec::new();
            for name in failover_remaining {
                if let Ok(Some(credential)) = self.credentials.get(name.clone()).await {
                    if is_local_llm_credential(&credential) {
                        local_failover.push(name);
                    }
                }
            }
            failover_remaining = local_failover;
        }
        let mut credential_service = credential_service_init;
        let mut credential = credential_init;
        let mut is_anthropic = credential.auth_style == "anthropic";

        // --- Tool schema (OpenAI + Anthropic, push 6) ------------------
        //
        // Push 6 extends tool-use to Anthropic via shape translation
        // inside `ordo_cloud::anthropic::messages`. The schemas we
        // build here are always the OpenAI-flavoured `[{type,function}]`
        // shape; the Anthropic wrapper unwraps them before sending.
        // Resolve the active mode for this session (None when the
        // assistant has no registry attached, or â€” defensively â€” if
        // the session's stored mode id has been removed from the
        // registry between session creation and now).
        let active_mode = self.resolve_mode_for_session(session.id);

        // Mode-bound telemetry: one event per turn carrying the
        // mode's active scope summary. The studio's insight trace
        // uses this to render "you're in mode X" without a second
        // API round-trip. Skipped when no mode is resolved (legacy).
        if let Some(mode) = &active_mode {
            self.events.publish(
                session.id,
                TurnEvent::ModeBound {
                    session_id: session.id,
                    mode_id: mode.id.clone(),
                    mode_label: mode.label.clone(),
                    memory_scope: mode.memory_scope.clone(),
                    rag_domains: mode.rag_domains.clone(),
                    allowed_tool_lane_count: mode.allowed_tool_lanes.len(),
                    blocked_tool_capability_count: mode.blocked_tool_capabilities.len(),
                },
            );
        }

        // Build the tool schema AND capture the capability->provider
        // map. We need providers for the MCP-taint hook in the
        // dispatch loop below. The mode (when known) narrows the
        // tool surface to the lanes the mode declares; meta-tools
        // are always exposed regardless.
        let (tools_payload, capability_providers): (Value, HashMap<String, String>) =
            if request.use_tools {
                self.build_tool_schema_with_providers(session.id, active_mode.as_ref())
                    .await
            } else {
                (Value::Null, HashMap::new())
            };
        let mut tool_use_enabled = tools_payload.is_array();

        // --- Prompt assembly (thin bootstrap) -------------------------
        // Phase 1.3: attachments (images today) land on the user-role
        // message as a content array. Default no-attachment turns take
        // the string-content path unchanged.
        // Operator-tunable strictness for the untrusted-content rule.
        // Studio writes `metadata.untrusted_strictness` from the
        // Runtime tab's preset card (off / low / medium / high).
        // Unknown / missing values silently fall back to Medium â€”
        // operator typos shouldn't disable the rule entirely.
        let strictness = request
            .metadata
            .get("untrusted_strictness")
            .and_then(|v| v.as_str())
            .map(crate::prompt::UntrustedStrictness::parse)
            .unwrap_or_default();
        // Per-mode persona + planner-bias preamble. None when no
        // mode is resolved or when the manifest declares neither.
        let mode_preamble = active_mode
            .as_ref()
            .and_then(crate::prompt::render_mode_preamble);

        let mut messages = crate::prompt::build_bootstrap_prompt_with_attachments_and_strictness(
            &request.user_message,
            &history,
            &request.attachments,
            strictness,
        );
        let messages_array = messages
            .as_array_mut()
            .ok_or_else(|| AssistantError::LlmFailed("prompt was not an array".into()))?;
        // Splice the mode preamble in right after the environment
        // map (idempotent on None â€” no-op for unmoded sessions).
        crate::prompt::inject_mode_preamble(messages_array, mode_preamble);
        if let Some(collaboration) = request
            .metadata
            .get("cross_mode_collaboration")
            .and_then(|value| value.as_object())
        {
            let policy = collaboration
                .get("policy")
                .and_then(|value| value.as_str())
                .unwrap_or("off");
            let mechanism = collaboration
                .get("mechanism")
                .and_then(|value| value.as_str())
                .unwrap_or("consult_mode_agent");
            let isolation = collaboration
                .get("isolation")
                .and_then(|value| value.as_str())
                .unwrap_or("no_cross_rag_or_memory_borrow");
            let allowed_modes = collaboration
                .get("allowed_modes")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|joined| !joined.is_empty())
                .unwrap_or_else(|| "none configured".to_string());
            let requested_modes = collaboration
                .get("user_requested_modes")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|joined| !joined.is_empty())
                .unwrap_or_else(|| "none".to_string());
            let max_collaborators = collaboration
                .get("max_collaborators")
                .and_then(|value| value.as_u64())
                .unwrap_or(1)
                .clamp(1, 5);
            let allow_subagents = collaboration
                .get("allow_subagents")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let text = format!(
                "# Cross-mode collaboration\n\n\
                 Policy: {policy}\n\
                 Mechanism: {mechanism}\n\
                 Isolation: {isolation}\n\
                 Allowed collaborator modes: {allowed_modes}\n\
                 Operator-requested modes: {requested_modes}\n\
                 Allow subagents: {allow_subagents}\n\
                 Max collaborators: {max_collaborators}\n\n\
                 If policy is off, do not consult another mode. If policy requires suggestion or approval, explain the proposed collaborator before consulting unless the operator explicitly requested it. \
                 If consultation is allowed, use `assistant.consult_mode_agent`; do not read another mode's RAG or memory directly."
            );
            messages_array.insert(
                messages_array.len().min(3),
                json!({
                    "role": "system",
                    "content": text,
                }),
            );
        }

        // --- Turn loop: LLM Ã¢â€ â€™ maybe tool calls Ã¢â€ â€™ repeat ---------------
        let mut iteration = 0;
        // Running count of tool calls that hit the cup gate in this
        // turn. See MAX_GATE_REJECTIONS_PER_TURN.
        let mut gate_rejections: usize = 0;
        let mut duplicate_tool_rejections: usize = 0;
        let mut tool_call_counts: HashMap<String, usize> = HashMap::new();
        let mut invocations: Vec<crate::types::ToolInvocation> = Vec::new();
        let assistant_text;
        let mut model: Option<String> = None;

        // Push 6: if tools are off and the credential is OpenAI,
        // take the streaming path so the studio gets live token
        // events. Anthropic streaming is deferred. Tool-use loops
        // stay on the non-streaming path because OpenAI's streaming
        // tool-call protocol is significantly more complex and
        // reassembling chunks mid-loop introduces edge cases we
        // don't need yet. Falls back to the non-streaming loop on
        // transport error so the turn still completes.
        let mut streamed = false;
        let mut streamed_text: Option<String> = None;
        let mut streamed_model: Option<String> = None;
        if request.stream && !tool_use_enabled && !is_anthropic {
            match self
                .run_streaming_turn(session_id, messages_array, &credential, &cancel)
                .await
            {
                Ok((text, m)) => {
                    streamed = true;
                    streamed_text = Some(text);
                    streamed_model = m;
                }
                Err(err) => {
                    tracing::warn!(
                        target: "ordo_assistant",
                        error = %err,
                        "streaming chat failed, falling back to non-streaming"
                    );
                }
            }
        }

        loop {
            if streamed {
                assistant_text = streamed_text.take().unwrap_or_default();
                if let Some(m) = streamed_model.take() {
                    model = Some(m);
                }
                break;
            }
            iteration += 1;
            check_cancel()?;
            let mut chat_args = json!({
                "messages": Value::Array(messages_array.clone()),
                "temperature": 0.3,
            });
            // Honor a per-credential model override (set via the Cloud
            // tab's "model" field, stored on `extras.model`). Lets local
            // OpenAI-compatible providers (Ollama, LM Studio, etc.)
            // route to whichever model the operator has loaded.
            if let Some(model) = credential.extras.get("model") {
                chat_args["model"] = json!(model);
            }
            if tool_use_enabled {
                chat_args["tools"] = tools_payload.clone();
                chat_args["tool_choice"] = Value::String("auto".into());
            }

            // Call the LLM with CALL-TIME failover (follow-up 1):
            // on transport error / timeout, try the next credential
            // in the remaining failover list before giving up. The
            // retry swaps `credential`, `credential_service`, and
            // `is_anthropic` so subsequent iterations of the turn
            // loop use the credential that actually answered.
            let mut response: Option<Value> = None;
            let mut last_err: Option<String> = None;
            loop {
                // Read the per-credential timeout from extras so an
                // operator can extend Ollama (or any specific
                // provider) without touching code. The reqwest
                // client layer enforces the same bound, but we
                // wrap here too so the turn loop can fail over to
                // the next credential cleanly on timeout instead
                // of waiting for the inner HTTP call to give up.
                let llm_timeout = ordo_cloud::timeout_for(&credential);
                let call_result = if is_anthropic {
                    tokio::time::timeout(
                        llm_timeout,
                        ordo_cloud::anthropic::messages(&self.http, &credential, &chat_args),
                    )
                    .await
                } else {
                    tokio::time::timeout(
                        llm_timeout,
                        ordo_cloud::openai::chat(&self.http, &credential, &chat_args),
                    )
                    .await
                };
                match call_result {
                    Ok(Ok(value)) => {
                        response = Some(value);
                        break;
                    }
                    Ok(Err(err)) => {
                        last_err = Some(format!("{credential_service}: {err}"));
                    }
                    Err(_) => {
                        last_err = Some(format!(
                            "{credential_service}: LLM call timed out after {}s \
                             (bump extras.timeout_secs on the credential to allow longer)",
                            llm_timeout.as_secs()
                        ));
                    }
                }
                // Failed Ã¢â‚¬â€ try the next candidate if we have one.
                let Some(next_name) = failover_remaining.first().cloned() else {
                    break;
                };
                failover_remaining.remove(0);
                match self.credentials.get(next_name.clone()).await {
                    Ok(Some(next_cred)) => {
                        tracing::warn!(
                            target: "ordo_assistant",
                            from = %credential_service,
                            to = %next_name,
                            error = %last_err.as_deref().unwrap_or(""),
                            "LLM call failed; failing over"
                        );
                        credential = next_cred;
                        credential_service = next_name;
                        is_anthropic = credential.auth_style == "anthropic";
                    }
                    _ => {
                        // Next candidate has no configured
                        // credential Ã¢â‚¬â€ skip it and keep trying.
                        continue;
                    }
                }
            }
            let response = match response {
                Some(value) => value,
                None => {
                    return Err(AssistantError::LlmFailed(last_err.unwrap_or_else(|| {
                        "LLM call failed and no failover available".into()
                    })));
                }
            };
            if model.is_none() {
                model = response
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }

            // Without tool-use the turn is a straight pass-through
            // for both providers Ã¢â‚¬â€ grab the assistant text and exit.
            if !tool_use_enabled {
                assistant_text = visible_assistant_message(&response, &invocations);
                break;
            }

            let tool_calls = response.get("tool_calls").cloned().unwrap_or(Value::Null);
            let tool_calls_array = tool_calls.as_array();
            let has_tool_calls = tool_calls_array.map(|arr| !arr.is_empty()).unwrap_or(false);

            // Raw content from the model (empty when the turn was a
            // pure tool call or when a reasoning model burned its
            // budget on thinking). Distinct from `assistant_message`
            // which carries a UI fallback like "(no content emitted;
            // reasoning preview)â€¦" â€” that fallback is operator-facing
            // ONLY and must never re-enter the prompt as the model's
            // own previous content.
            let content_raw = response
                .get("content_raw")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let reasoning_raw = response
                .get("reasoning")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if !has_tool_calls || iteration > self.max_tool_iterations {
                // If the model produced real content, keep it. If it
                // went silent (qwen3-style: empty content + reasoning
                // only) AND we ran tool calls earlier in this turn,
                // force one explicit final-answer pass â€” the model
                // has the tool results, it just got stuck thinking.
                let needs_finalize = content_raw.trim().is_empty()
                    && !reasoning_raw.trim().is_empty()
                    && !invocations.is_empty()
                    && iteration <= self.max_tool_iterations;

                if needs_finalize {
                    tracing::info!(
                        target: "ordo_assistant",
                        iteration = iteration,
                        reasoning_len = reasoning_raw.len(),
                        "model went silent after tool calls; forcing final-answer pass"
                    );
                    // Don't pollute history with the placeholder â€”
                    // push a real (empty content) assistant turn so
                    // the model sees clean history, then nudge with
                    // a synthetic user message asking for the
                    // final answer in plain text. We loop back to
                    // the top with finalize=true semantics: tools
                    // are turned off for the next call so the model
                    // can't punt with another tool call.
                    messages_array.push(json!({
                        "role": "assistant",
                        "content": "",
                    }));
                    messages_array.push(json!({
                        "role": "user",
                        "content": "Now write your final answer to my original question in plain prose. Don't think aloud, don't call tools, just answer.",
                    }));
                    tool_use_enabled = false;
                    continue;
                }

                // Otherwise take the response's UI-friendly assistant
                // message. This still includes the reasoning preview
                // fallback â€” at the very least the operator gets to
                // see what the model was thinking when it gave up.
                assistant_text = visible_assistant_message(&response, &invocations);
                break;
            }

            // Push the assistant's tool-call turn into the message
            // history so the model sees its own previous request on
            // the next iteration. Use raw content (not the UI
            // fallback) so a thinking model doesn't see "(no content
            // emittedâ€¦)" as if it had said that â€” that placeholder
            // confuses the next iteration into more reasoning.
            let raw_tool_calls = tool_calls.clone();
            messages_array.push(json!({
                "role": "assistant",
                "content": content_raw,
                "tool_calls": raw_tool_calls,
            }));

            // Execute tool calls in parallel. LLMs increasingly emit
            // batches of independent calls in a single turn (Claude
            // "parallel tool use"); running them serially serializes
            // wall-clock time for no reason.
            //
            // Contract (see docs/architecture-contract.md):
            //   - Per-call ToolCallStarted events are published
            //     synchronously in LLM-emitted order so the UI shows
            //     the whole batch kick off at once.
            //   - Per-call ToolCallCompleted / ToolCallFailed events
            //     fire AS EACH TASK RESOLVES Ã¢â‚¬â€ the UI paints the
            //     fastest result first.
            //   - `messages_array` (what the next LLM call sees) and
            //     `invocations` (what gets persisted as TurnContext)
            //     are appended in the ORIGINAL LLM-emitted order after
            //     all tasks complete. The LLM must see tool results in
            //     the same order it asked for them.
            //   - Review gating is per-provider: if a single call
            //     routes through `ReviewProvider`, it blocks its own
            //     task; other calls in the batch proceed. The batch
            //     waits on the slowest, not the sum.
            //   - Cancellation is checked once before dispatch. Fine-
            //     grained mid-batch cancellation can come later; for
            //     now, a cancelled batch runs to completion and the
            //     outer loop exits on the next iteration.
            let calls = tool_calls_array.cloned().unwrap_or_default();
            if cancel.is_cancelled() {
                return Err(AssistantError::Cancelled);
            }

            struct CallRecord {
                idx: usize,
                call_id: String,
                fn_name: String,
                arguments: Value,
                invocation_uuid: Uuid,
                skip_reason: Option<String>,
            }

            let mut records: Vec<CallRecord> = calls
                .iter()
                .enumerate()
                .map(|(idx, call)| {
                    let call_id = call
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let fn_name = call
                        .pointer("/function/name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let arguments_raw = call
                        .pointer("/function/arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}")
                        .to_string();
                    let arguments: Value =
                        serde_json::from_str(&arguments_raw).unwrap_or_else(|_| json!({}));
                    CallRecord {
                        idx,
                        call_id,
                        fn_name,
                        arguments,
                        invocation_uuid: Uuid::new_v4(),
                        skip_reason: None,
                    }
                })
                .collect();

            for rec in &mut records {
                let signature = tool_call_signature(&rec.fn_name, &rec.arguments);
                let count = tool_call_counts.entry(signature).or_insert(0);
                *count += 1;
                if *count > 1 {
                    rec.skip_reason = Some(format!(
                        "duplicate tool call skipped: '{}' was already called with the same arguments in this turn. Use the prior result and write the final answer.",
                        rec.fn_name
                    ));
                }
            }

            for rec in &records {
                self.events.publish(
                    session_id,
                    TurnEvent::ToolCallStarted {
                        session_id,
                        invocation_id: rec.invocation_uuid,
                        capability: rec.fn_name.clone(),
                        arguments: rec.arguments.clone(),
                    },
                );
            }

            // Capture taint state once per iteration (before dispatch).
            // Cheap mutex peek; we use it to gate sensitive
            // capabilities on tainted conversations.
            let session_tainted = self.session_is_tainted(session_id);

            let dispatch_futs = records.into_iter().map(|rec| async move {
                let started = std::time::Instant::now();
                // Slow-injection defense: when the conversation is
                // tainted (web-fetched content has entered context
                // at any prior point), block sensitive capabilities.
                // The model can still read, summarize, recall â€” what
                // it can't do is reach for write/dispatch tools that
                // would actuate on planted instructions.
                let was_duplicate = rec.skip_reason.is_some();
                let was_gated = session_tainted && is_sensitive_capability(&rec.fn_name);
                let result = if let Some(reason) = rec.skip_reason.clone() {
                    Err(AssistantError::InvalidArgument(reason))
                } else if was_gated {
                    Err(AssistantError::InvalidArgument(format!(
                        "tool '{}' is gated on this conversation: untrusted-web content has \
                         entered the context. Clear the conversation's taint (Studio: chat \
                         header â†’ Clear taint) or start a new session to invoke this \
                         capability.",
                        rec.fn_name
                    )))
                } else {
                    self.dispatch_tool(session_id, &rec.fn_name, rec.arguments.clone())
                        .await
                };
                let duration_ms = started.elapsed().as_millis() as u64;
                match &result {
                    Ok(value) => self.events.publish(
                        session_id,
                        TurnEvent::ToolCallCompleted {
                            session_id,
                            invocation_id: rec.invocation_uuid,
                            capability: rec.fn_name.clone(),
                            result: value.clone(),
                        },
                    ),
                    Err(err) => self.events.publish(
                        session_id,
                        TurnEvent::ToolCallFailed {
                            session_id,
                            invocation_id: rec.invocation_uuid,
                            capability: rec.fn_name.clone(),
                            error: err.to_string(),
                        },
                    ),
                }
                (rec, result, duration_ms, was_gated, was_duplicate)
            });

            let mut outputs: Vec<_> = join_all(dispatch_futs).await;
            // join_all preserves future submission order, but sort by
            // the record's original index explicitly so the ordering
            // invariant survives a future switch to FuturesUnordered.
            outputs.sort_by_key(|(rec, _, _, _, _)| rec.idx);

            // Accumulate gate rejections seen this iteration so the
            // outer-turn loop can force-finalize once we cross the
            // per-turn cap (MAX_GATE_REJECTIONS_PER_TURN).
            gate_rejections += outputs
                .iter()
                .filter(|(_, _, _, was_gated, _)| *was_gated)
                .count();
            duplicate_tool_rejections += outputs
                .iter()
                .filter(|(_, _, _, _, was_duplicate)| *was_duplicate)
                .count();

            for (rec, result, duration_ms, _was_gated, _was_duplicate) in outputs {
                match result {
                    Ok(value) => {
                        // MCP auto-taint: a successful tool call routed
                        // through an external MCP server taints the
                        // session. See `mcp_taint_for_provider` for the
                        // rule (provider == "mcp:<id>" â†’ UntrustedMcp).
                        if let Some(taint) = mcp_taint_for_provider(
                            capability_providers.get(&rec.fn_name).map(|s| s.as_str()),
                            rec.invocation_uuid,
                        ) {
                            self.taint_session(session_id, taint);
                        }
                        messages_array.push(json!({
                            "role": "tool",
                            "tool_call_id": rec.call_id,
                            "content": serde_json::to_string(&value).unwrap_or_default(),
                        }));
                        invocations.push(crate::types::ToolInvocation {
                            invocation_id: rec.invocation_uuid,
                            capability: rec.fn_name,
                            arguments: rec.arguments,
                            result: Some(value),
                            error: None,
                            duration_ms,
                        });
                    }
                    Err(err) => {
                        let message = err.to_string();
                        messages_array.push(json!({
                            "role": "tool",
                            "tool_call_id": rec.call_id,
                            "content": format!("error: {message}"),
                        }));
                        invocations.push(crate::types::ToolInvocation {
                            invocation_id: rec.invocation_uuid,
                            capability: rec.fn_name,
                            arguments: rec.arguments,
                            result: None,
                            error: Some(message),
                            duration_ms,
                        });
                    }
                }
            }

            // Loop-break on gate rejection. After the cup has gated
            // MAX_GATE_REJECTIONS_PER_TURN tool calls in this turn,
            // turn off tool use and nudge the model for a final
            // answer. Same finalize shape as the silent-model path
            // above â€” push an empty assistant turn (so the model
            // sees clean history) and a synthetic user nudge that
            // names the constraint, then `continue` back to the top
            // with `tool_use_enabled = false` so the next LLM call
            // can't punt with another gated tool call.
            if gate_rejections >= MAX_GATE_REJECTIONS_PER_TURN && tool_use_enabled {
                tracing::info!(
                    target: "ordo_assistant",
                    iteration = iteration,
                    gate_rejections = gate_rejections,
                    "gate rejection cap reached; forcing final-answer pass"
                );
                messages_array.push(json!({
                    "role": "assistant",
                    "content": "",
                }));
                messages_array.push(json!({
                    "role": "user",
                    "content": format!(
                        "Several of your tool calls were blocked because this \
                         conversation has ingested untrusted web content (the \
                         cup gate is active). Stop calling write/dispatch \
                         tools â€” write your final answer in plain prose using \
                         only the information you already have. If the answer \
                         genuinely needs a blocked tool, say so and tell me to \
                         clear the conversation's taint.",
                    ),
                }));
                tool_use_enabled = false;
                continue;
            }
            if duplicate_tool_rejections >= MAX_DUPLICATE_TOOL_CALLS_PER_TURN && tool_use_enabled {
                tracing::info!(
                    target: "ordo_assistant",
                    iteration = iteration,
                    duplicate_tool_rejections = duplicate_tool_rejections,
                    "duplicate tool-call cap reached; forcing final-answer pass"
                );
                messages_array.push(json!({
                    "role": "assistant",
                    "content": "",
                }));
                messages_array.push(json!({
                    "role": "user",
                    "content": "You repeated a tool call that already returned a result. Stop calling tools now. Write the final answer in plain prose using the tool results already available.",
                }));
                tool_use_enabled = false;
                continue;
            }
        }

        let context = TurnContext {
            facts: recalled_facts.clone(),
            rag_hits: rag_summaries.clone(),
            history_window: history.len(),
            tool_calls: invocations,
        };

        // --- Optional review step -------------------------------------
        //
        // When the caller asks for human review, submit the draft to
        // the `review.*` queue and block until the operator resolves
        // it (or the wait expires). Approvals go through verbatim;
        // edits replace the delivered text; denials are persisted as a
        // short denial note so the session history shows what happened.
        let (delivered_text, review_outcome) = self
            .run_review_step(session_id, &request.user_message, &assistant_text, request)
            .await?;

        let turn = {
            let mut store = self.store.lock();
            store.insert_turn(
                session_id,
                &request.user_message,
                &delivered_text,
                &context,
                model.as_deref(),
                Some(&credential_service),
            )?
        };

        // Blueprint v2: append the `agent.response` event, chained
        // to the `user.message` event we logged at turn start. This
        // gives replay a complete parent Ã¢â€ â€™ child chain for every
        // turn.
        self.log_agent_response(
            session_id,
            &delivered_text,
            user_event_id.as_deref(),
            Some(turn_id.as_str()),
            model.as_deref(),
            &credential_service,
        )
        .await;

        for recalled in &recalled_facts {
            let _ = self.facts.reinforce(recalled.fact.id);
        }

        debug!(
            target: "ordo_assistant",
            session_id = %session_id,
            iterations = iteration,
            tool_calls = context.tool_calls.len(),
            reviewed = review_outcome.is_some(),
            "turn complete"
        );

        Ok(TurnResult {
            session_id,
            turn,
            retrieved_facts: recalled_facts,
            retrieved_rag: rag_summaries,
            review_outcome,
        })
    }

    /// Streaming chat path (push 6). Drives OpenAI's SSE endpoint
    /// and republishes each token delta as a `TurnEvent::TokenDelta`
    /// so the studio can render a live \"typing\" effect. Returns the
    /// concatenated text and the model name. Only used when tools
    /// are disabled for this turn.
    async fn run_streaming_turn(
        &self,
        session_id: Uuid,
        messages_array: &[Value],
        credential: &ordo_cloud::CloudCredential,
        cancel: &CancelFlag,
    ) -> AssistantResult<(String, Option<String>)> {
        use futures::StreamExt;
        use ordo_cloud::openai::ChatStreamEvent;

        let mut chat_args = json!({
            "messages": Value::Array(messages_array.to_vec()),
            "temperature": 0.3,
        });
        if let Some(model) = credential.extras.get("model") {
            chat_args["model"] = json!(model);
        }
        let stream = ordo_cloud::openai::chat_stream(&self.http, credential, &chat_args)
            .await
            .map_err(|err| AssistantError::LlmFailed(err.to_string()))?;
        futures::pin_mut!(stream);

        let mut acc = String::new();
        let model_name: Option<String> = chat_args
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        while let Some(event) = stream.next().await {
            if cancel.is_cancelled() {
                return Err(AssistantError::Cancelled);
            }
            match event {
                ChatStreamEvent::TokenDelta { delta } => {
                    acc.push_str(&delta);
                    self.events
                        .publish(session_id, TurnEvent::TokenDelta { session_id, delta });
                }
                ChatStreamEvent::ToolCallDelta { .. } => {
                    // Streaming path shouldn't see tool calls today
                    // (we disabled the `tools` field on the request),
                    // but if upstream surprises us, bail to the
                    // non-streaming loop.
                    return Err(AssistantError::LlmFailed(
                        "streaming path received unexpected tool_call delta".into(),
                    ));
                }
                ChatStreamEvent::Done { .. } => break,
                ChatStreamEvent::Error { message } => {
                    return Err(AssistantError::LlmFailed(message));
                }
            }
        }
        Ok((acc, model_name))
    }

    /// When `request.review` is true and a `ReviewService` is wired in,
    /// submit the draft and wait for a decision. Returns the (possibly
    /// edited) text to deliver plus a `ReviewOutcome` for persistence.
    /// When review is skipped, echoes the original draft.
    async fn run_review_step(
        &self,
        session_id: Uuid,
        user_message: &str,
        draft: &str,
        request: &TurnRequest,
    ) -> AssistantResult<(String, Option<ReviewOutcome>)> {
        if !request.review {
            return Ok((draft.to_string(), None));
        }
        let Some(review) = self.review.clone() else {
            return Err(AssistantError::InvalidArgument(
                "turn requested review but the assistant has no review service configured".into(),
            ));
        };

        let title: String = user_message
            .chars()
            .take(80)
            .collect::<String>()
            .trim()
            .to_string();
        let title = if title.is_empty() {
            "Assistant draft".to_string()
        } else {
            title
        };
        let new_request = ordo_review::NewReviewRequest {
            origin_capability: "assistant.turn".into(),
            origin_plugin: None,
            title,
            content_type: "text/markdown".into(),
            content: draft.to_string(),
            metadata: std::collections::HashMap::new(),
        };

        // Submit once, fire the event with the real id, then wait
        // on that exact id via `wait_for`.
        let submitted = review
            .request(new_request)
            .map_err(|err| AssistantError::InvalidArgument(err.to_string()))?;
        self.events.publish(
            session_id,
            TurnEvent::ReviewRequested {
                session_id,
                review_request_id: submitted.id,
                draft: draft.to_string(),
            },
        );

        let wait = Duration::from_secs(request.review_wait_secs.max(1));
        let wait_result = review.wait_for(submitted.id, wait).await;

        let resolved = match wait_result {
            Ok(resolved) => resolved,
            Err(err) => {
                // Timed out or couldn't resolve Ã¢â‚¬â€ return a friendly
                // message so the session persists cleanly.
                let outcome = ReviewOutcome {
                    review_request_id: submitted.id,
                    state: "expired".into(),
                    delivered_content: String::new(),
                    note: Some(err.to_string()),
                };
                self.events.publish(
                    session_id,
                    TurnEvent::ReviewResolved {
                        session_id,
                        outcome: outcome.clone(),
                    },
                );
                return Err(AssistantError::InvalidArgument(format!(
                    "review did not resolve in time: {err}"
                )));
            }
        };

        let state_label = resolved.state.label().to_string();
        let delivered = match state_label.as_str() {
            "approved" | "edited_and_approved" => resolved.effective_content().to_string(),
            "denied" => format!(
                "[draft denied by operator]{}",
                resolved
                    .decision_note
                    .as_deref()
                    .map(|n| format!(" Ã¢â‚¬â€ {n}"))
                    .unwrap_or_default()
            ),
            _ => resolved.effective_content().to_string(),
        };
        let outcome = ReviewOutcome {
            review_request_id: resolved.id,
            state: state_label,
            delivered_content: delivered.clone(),
            note: resolved.decision_note.clone(),
        };
        self.events.publish(
            session_id,
            TurnEvent::ReviewResolved {
                session_id,
                outcome: outcome.clone(),
            },
        );
        Ok((delivered, Some(outcome)))
    }

    /// Build the tool schema array advertised to the LLM, AND return
    /// the `capability -> provider` map captured from the descriptor
    /// inventory.
    ///
    /// The provider map is what lets the dispatch loop tell whether a
    /// tool was MCP-routed (provider starts with `mcp:`). That tells
    /// the assistant to mint a `Taint::UntrustedMcp` on the session
    /// after the call returns â€” the cup-shaped defense against
    /// instructions planted in MCP tool output.
    ///
    /// Meta-tools (`assistant.*`) don't appear in the map because they
    /// route locally, not through the bus. The dispatch loop's lookup
    /// returns None for them and skips the taint â€” which is correct,
    /// they aren't untrusted.
    async fn build_tool_schema_with_providers(
        &self,
        session_id: Uuid,
        mode: Option<&ordo_modes::ModeManifest>,
    ) -> (Value, HashMap<String, String>) {
        // Always expose the meta-tools â€” these don't need a bus and
        // they're the LLM's only path into facts/knowledge/routing in
        // the progressive-disclosure architecture. Meta-tools are the
        // architectural baseline; modes don't get to filter them out.
        let mut tools: Vec<Value> = meta_tool_schemas();
        let mut providers: HashMap<String, String> = HashMap::new();

        // Append every bus capability the operator has allow-listed
        // AND that the active mode permits. Mode-side filtering is
        // additive: the gateway's own `is_allowed` already drops
        // capabilities the operator excluded workspace-wide; the
        // mode then narrows that list further to its declared lanes
        // and respects its `blocked_tool_capabilities`.
        //
        // When `mode` is None (legacy / no-registry deploys), we keep
        // every gateway-allowed capability â€” pre-mode behavior.
        let mut mode_filtered = 0usize;
        if let Some(gateway) = &self.tools {
            match gateway.available_tools().await {
                Ok(descriptors) => {
                    for d in descriptors {
                        if let Some(mode) = mode {
                            if !mode.allows_capability(&d.capability) {
                                mode_filtered += 1;
                                continue;
                            }
                        }
                        providers.insert(d.capability.clone(), d.provider.clone());
                        tools.push(json!({
                            "type": "function",
                            "function": {
                                "name": d.capability,
                                "description": d.description,
                                "parameters": {
                                    "type": "object",
                                    "additionalProperties": true,
                                },
                            }
                        }));
                    }
                }
                Err(err) => warn!(
                    target: "ordo_assistant",
                    error = %err,
                    "could not list bus tools; continuing with meta-tools only"
                ),
            }
        }
        if let Some(mode) = mode {
            if mode_filtered > 0 {
                tracing::debug!(
                    target: "ordo_assistant",
                    mode = %mode.id,
                    filtered = mode_filtered,
                    "mode allowlist filtered out non-permitted capabilities"
                );
                // Telemetry event so the studio's insight trace can
                // render "X capabilities were blocked by the active
                // mode" without re-deriving from the manifest. Only
                // emit when filter actually fired â€” silent on
                // matches.
                self.events.publish(
                    session_id,
                    TurnEvent::ModeToolFilterApplied {
                        session_id,
                        mode_id: mode.id.clone(),
                        // tools[0..meta_count] are the meta-tools we
                        // injected at the top; the rest were the
                        // mode-permitted bus capabilities.
                        kept_capabilities: providers.len(),
                        filtered_count: mode_filtered,
                    },
                );
            }
        }
        (Value::Array(tools), providers)
    }

    /// Route a tool call to the meta-tool handlers (fact recall and self-
    /// knowledge lookup) or fall through to the bus-
    /// backed `ToolGateway`. All three meta-tools wrap their results in
    /// a `{preamble, ...}` envelope so the level preamble is visible to
    /// the LLM every time it pulls a new layer.
    async fn dispatch_tool(
        &self,
        session_id: Uuid,
        capability: &str,
        arguments: Value,
    ) -> AssistantResult<Value> {
        match capability {
            "assistant.recall_memory" => self.meta_recall_memory(session_id, arguments).await,
            "assistant.knowledge_lookup" => self.meta_knowledge_lookup(session_id, arguments).await,
            "assistant.parallel_lookup" => self.meta_parallel_lookup(session_id, arguments).await,
            "assistant.consult_mode_agent" => {
                self.meta_consult_mode_agent(session_id, arguments).await
            }
            "assistant.remember_knowledge" => {
                self.meta_remember_knowledge(session_id, arguments).await
            }
            // Shadow the bus's `assistant.remember_fact` so writes
            // from the LLM are auto-tagged with the active mode's
            // scope. External MCP callers hitting the bus directly
            // still get the legacy "global default" behavior â€” this
            // is the assistant's session-aware path only.
            "assistant.remember_fact" => self.meta_remember_fact(session_id, arguments).await,
            "assistant.list_facts" => self.meta_list_facts(session_id, arguments).await,
            "assistant.forget_fact" => self.meta_forget_fact(session_id, arguments).await,
            other => {
                let Some(gateway) = &self.tools else {
                    return Err(AssistantError::InvalidArgument(
                        "assistant has no tool gateway configured".into(),
                    ));
                };
                gateway.invoke(other, arguments).await
            }
        }
    }

    async fn meta_recall_memory(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let top_k = arguments
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(8);
        if query.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.recall_memory requires a non-empty `query`".into(),
            ));
        }
        // When the assistant has a mode registry attached AND the
        // session resolves to a mode, scope the recall to that
        // mode's `memory_scope` list. Otherwise (legacy callers,
        // pre-mode tests), fall back to all-scopes recall â€”
        // backward-compat for anything that hasn't migrated.
        let facts = if let Some(manifest) = self.resolve_mode_for_session(session_id) {
            let scoped = self
                .facts
                .recall_in_scopes(&query, top_k, &manifest.memory_scope)
                .await?;
            // Telemetry: surface the scope filter to the insight
            // trace so an operator inspecting "why didn't fact X
            // surface?" can see the active scope set + visible
            // count without grepping logs.
            self.events.publish(
                session_id,
                TurnEvent::ModeMemoryScopeApplied {
                    session_id,
                    mode_id: manifest.id.clone(),
                    visible_scopes: manifest.memory_scope.clone(),
                    facts_visible: scoped.len(),
                },
            );
            scoped
        } else {
            self.facts.recall(&query, top_k).await?
        };
        // Reinforce on recall so heavily-used facts accrue confidence.
        for recalled in &facts {
            let _ = self.facts.reinforce(recalled.fact.id);
        }
        Ok(json!({
            "preamble": crate::prompt::MEMORY_PREAMBLE,
            "query": query,
            "top_k": top_k,
            "facts": facts,
            "facts_rendered": crate::prompt::render_facts_block(&facts),
        }))
    }

    async fn meta_knowledge_lookup(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let top_k = arguments
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(5);
        let kind = arguments
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(KnowledgeKind::parse);
        let domain = arguments
            .get("domain")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if query.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.knowledge_lookup requires a non-empty `query`".into(),
            ));
        }

        // Mode-scoped RAG: when a mode is active, validate the
        // requested domain against the mode's `rag_domains` list.
        // If the LLM asks for a domain the mode doesn't permit, we
        // fail loudly with a clear message â€” the model can correct
        // and try a domain it IS allowed to use, or recognize that
        // the lookup isn't appropriate for this workspace.
        if let (Some(mode), Some(domain_id)) =
            (self.resolve_mode_for_session(session_id), domain.as_deref())
        {
            if !mode.rag_domains.iter().any(|d| d == domain_id) {
                self.events.publish(
                    session_id,
                    TurnEvent::ToolCallFailed {
                        session_id,
                        invocation_id: Uuid::new_v4(),
                        capability: "assistant.knowledge_lookup".into(),
                        error: format!(
                            "domain '{domain_id}' not in mode '{}' rag_domains",
                            mode.id
                        ),
                    },
                );
                return Err(AssistantError::InvalidArgument(format!(
                    "RAG domain '{domain_id}' is not available in mode '{}'. \
                     This mode permits: {}",
                    mode.id,
                    if mode.rag_domains.is_empty() {
                        "(no RAG domains in this mode)".to_string()
                    } else {
                        mode.rag_domains.join(", ")
                    },
                )));
            }
        }
        let hits = self
            .knowledge
            .recall(&query, top_k, kind, domain.as_deref())
            .await?;
        for hit in &hits {
            let _ = self.knowledge.reinforce(hit.entry.id);
        }
        Ok(json!({
            "preamble": crate::prompt::KNOWLEDGE_PREAMBLE,
            "query": query,
            "top_k": top_k,
            "kind": kind.map(|k| k.as_str()),
            "domain": domain,
            "hits": hits,
        }))
    }

    /// Fan `knowledge_lookup` across an explicit list of domains concurrently.
    /// The domains must come from the user, active mode, or retrieved knowledge;
    /// Ordo no longer exposes an automatic router to pick them.
    async fn meta_parallel_lookup(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if query.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.parallel_lookup requires a non-empty `query`".into(),
            ));
        }
        let mut domains: Vec<String> = arguments
            .get("domains")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if domains.is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.parallel_lookup requires at least one entry in `domains`".into(),
            ));
        }
        let requested_domains = domains.clone();
        let mut used_mode_fallback_domains = false;

        // Mode-scoped RAG: when a mode is active, drop any requested
        // domains that aren't in the mode's `rag_domains` list. If
        // ALL get filtered out, fall back to the active mode's own
        // domains instead of broadening access. The caller still gets
        // `blocked_domains` so it can explain what was denied, while
        // the tool remains useful for smoke tests and general-mode
        // retrieval.
        let mut blocked_domains: Vec<String> = Vec::new();
        if let Some(mode) = self.resolve_mode_for_session(session_id) {
            let allowed: std::collections::HashSet<&String> = mode.rag_domains.iter().collect();
            domains.retain(|d| {
                if allowed.contains(d) {
                    true
                } else {
                    blocked_domains.push(d.clone());
                    false
                }
            });
            if domains.is_empty() {
                domains = mode.rag_domains.clone();
                used_mode_fallback_domains = true;
                if domains.is_empty() {
                    return Err(AssistantError::InvalidArgument(format!(
                        "none of the requested RAG domains are available in mode '{}', \
                         and this mode has no fallback RAG domains",
                        mode.id,
                    )));
                }
            }
            if !blocked_domains.is_empty() {
                tracing::info!(
                    target: "ordo_assistant",
                    mode = %mode.id,
                    blocked = ?blocked_domains,
                    kept = ?domains,
                    "parallel_lookup: dropped domains not in mode rag_domains"
                );
            }
        }
        let top_k = arguments
            .get("top_k_per_domain")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(3);
        let kind = arguments
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(KnowledgeKind::parse);

        let knowledge = self.knowledge.clone();
        let query_cloned = query.clone();
        let mut handles = Vec::with_capacity(domains.len());
        for domain in &domains {
            let knowledge = knowledge.clone();
            let query = query_cloned.clone();
            let domain = domain.clone();
            handles.push(tokio::spawn(async move {
                let hits = knowledge
                    .recall(&query, top_k, kind, Some(&domain))
                    .await
                    .unwrap_or_default();
                for hit in &hits {
                    let _ = knowledge.reinforce(hit.entry.id);
                }
                (domain, hits)
            }));
        }
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok((domain, hits)) => results.push(json!({
                    "domain": domain,
                    "count": hits.len(),
                    "hits": hits,
                })),
                Err(err) => {
                    warn!(
                        target: "ordo_assistant",
                        error = %err,
                        "parallel lookup task panicked"
                    );
                }
            }
        }
        Ok(json!({
            "preamble": crate::prompt::KNOWLEDGE_PREAMBLE,
            "query": query,
            "top_k_per_domain": top_k,
            "kind": kind.map(|k| k.as_str()),
            "domains": domains,
            "requested_domains": requested_domains,
            "blocked_domains": blocked_domains,
            "used_mode_fallback_domains": used_mode_fallback_domains,
            "results": results,
        }))
    }

    /// Mode-aware shadow of `assistant.remember_fact`. When the
    /// session is bound to a mode, NEW facts default to
    /// `scope: "mode:<id>"` instead of `"global"` â€” so the brand
    /// preference the LLM learns in Planning mode doesn't pollute
    /// Vibe Coding's recall.
    ///
    /// The LLM CAN override by passing an explicit `scope` field in
    /// the NewFact JSON: `"global"` for cross-mode visibility,
    /// `"mode:<other_id>"` for cross-tagging (legitimate when an
    /// operator dictates "this is a brand fact even though I'm in
    /// Vibe Coding right now"), or any other valid scope tag.
    ///
    /// When no mode is resolved (legacy / no registry attached), the
    /// fact falls through to "global" â€” same shape as the bus path.
    async fn meta_remember_fact(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let mut new_fact: NewFact = serde_json::from_value(arguments).map_err(|err| {
            AssistantError::InvalidArgument(format!(
                "assistant.remember_fact: invalid fact body â€” {err}"
            ))
        })?;

        if let Some(mode) = self.resolve_mode_for_session(session_id) {
            let mode_scope = format!("mode:{}", mode.id);
            if mode.id == "diagnostic" {
                match new_fact.scope.as_deref() {
                    Some(scope) if scope != mode_scope => {
                        return Err(AssistantError::InvalidArgument(format!(
                            "diagnostic mode memory is self-contained; assistant.remember_fact may only write to '{mode_scope}'"
                        )));
                    }
                    Some(_) => {}
                    None => {
                        new_fact.scope = Some(mode_scope);
                    }
                }
            } else if new_fact.scope.is_none() {
                new_fact.scope = Some(mode_scope);
            }
        }

        let fact = self.facts.remember(new_fact).await?;
        Ok(serde_json::to_value(&fact).unwrap_or(Value::Null))
    }

    async fn meta_list_facts(&self, session_id: Uuid, arguments: Value) -> AssistantResult<Value> {
        let requested_subject = arguments
            .get("subject")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let limit = arguments
            .get("limit")
            .and_then(|value| value.as_u64())
            .map(|value| value.min(500) as usize)
            .unwrap_or(200);
        let mut facts = self.list_facts(requested_subject.as_deref())?;
        if let Some(mode) = self.resolve_mode_for_session(session_id) {
            let allowed_scopes = &mode.memory_scope;
            facts.retain(|fact| allowed_scopes.iter().any(|scope| scope == &fact.scope));
        }
        facts.truncate(limit);
        Ok(json!({
            "facts": facts,
            "count": facts.len(),
            "subject": requested_subject,
            "limit": limit,
        }))
    }

    async fn meta_forget_fact(&self, session_id: Uuid, arguments: Value) -> AssistantResult<Value> {
        let id = arguments
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                AssistantError::InvalidArgument("assistant.forget_fact requires id".into())
            })?;
        let uuid = Uuid::parse_str(id)
            .map_err(|err| AssistantError::InvalidArgument(format!("invalid fact id: {err}")))?;

        if let Some(mode) = self.resolve_mode_for_session(session_id) {
            let visible = self.list_facts(None)?;
            let Some(fact) = visible.iter().find(|fact| fact.id == uuid) else {
                return Err(AssistantError::InvalidArgument(format!(
                    "fact {uuid} is not visible in active mode '{}'",
                    mode.id
                )));
            };
            if !mode.memory_scope.iter().any(|scope| scope == &fact.scope) {
                return Err(AssistantError::InvalidArgument(format!(
                    "fact {uuid} is outside active mode '{}' memory scope",
                    mode.id
                )));
            }
        }

        let removed = self.forget_fact(uuid)?;
        Ok(json!({ "id": uuid, "removed": removed }))
    }

    /// Mode-aware shadow of `assistant.remember_knowledge`. Knowledge writes
    /// are constrained to the active mode's declared RAG domains, so a
    /// diagnostic lesson lands in the diagnostic tree instead of leaking into
    /// the general assistant's self-knowledge.
    async fn meta_remember_knowledge(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        #[derive(Deserialize)]
        struct RememberKnowledgeArgs {
            #[serde(default)]
            kind: Option<String>,
            #[serde(default)]
            domain: Option<String>,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            body: Option<String>,
            #[serde(default)]
            content: Option<String>,
            #[serde(default)]
            note: Option<String>,
            #[serde(default)]
            source: Option<String>,
            #[serde(default)]
            confidence: Option<f32>,
        }

        let args: RememberKnowledgeArgs = serde_json::from_value(arguments).map_err(|err| {
            AssistantError::InvalidArgument(format!(
                "assistant.remember_knowledge: invalid knowledge body - {err}"
            ))
        })?;

        let mode = self.resolve_mode_for_session(session_id).ok_or_else(|| {
            AssistantError::InvalidArgument(
                "assistant.remember_knowledge requires a session bound to a registered mode".into(),
            )
        })?;
        if mode.rag_domains.is_empty() {
            return Err(AssistantError::InvalidArgument(format!(
                "mode '{}' has no writable RAG domains",
                mode.id
            )));
        }

        let requested_domain = args
            .domain
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let domain = if let Some(domain) = requested_domain {
            if !mode.rag_domains.iter().any(|allowed| allowed == domain) {
                return Err(AssistantError::InvalidArgument(format!(
                    "RAG domain '{domain}' is not writable in mode '{}'. This mode permits: {}",
                    mode.id,
                    mode.rag_domains.join(", ")
                )));
            }
            domain.to_string()
        } else if mode.id == "diagnostic"
            && mode
                .rag_domains
                .iter()
                .any(|allowed| allowed == "diagnostic_self_learning_tree")
        {
            "diagnostic_self_learning_tree".to_string()
        } else {
            mode.rag_domains[0].clone()
        };

        let body = args
            .body
            .or(args.content)
            .or(args.note)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AssistantError::InvalidArgument(
                    "assistant.remember_knowledge requires non-empty body, content, or note".into(),
                )
            })?;
        let title = args
            .title
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                body.lines()
                    .next()
                    .unwrap_or("Mode lesson")
                    .chars()
                    .take(96)
                    .collect()
            });
        let kind = args
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                KnowledgeKind::parse(value).ok_or_else(|| {
                    AssistantError::InvalidArgument(format!(
                        "assistant.remember_knowledge kind must be one of skill, persona, tool_note, observation, note; got '{value}'"
                    ))
                })
            })
            .transpose()?
            .unwrap_or(KnowledgeKind::Observation);
        let source = args
            .source
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("assistant:mode:{}:learned", mode.id));
        let confidence = args.confidence.unwrap_or(1.0).clamp(0.0, 1.0);

        let entry = self
            .knowledge
            .remember(NewKnowledge {
                kind,
                domain: Some(domain.clone()),
                title,
                body,
                source,
                confidence,
            })
            .await?;

        Ok(json!({
            "preamble": crate::prompt::KNOWLEDGE_PREAMBLE,
            "mode": mode.id,
            "domain": domain,
            "entry": entry,
        }))
    }

    async fn meta_consult_mode_agent(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let target_mode_id = arguments
            .get("target_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let reason = arguments
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let question = arguments
            .get("question")
            .or_else(|| arguments.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let max_iterations = arguments
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(2)
            .clamp(1, 5);

        if target_mode_id.is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.consult_mode_agent requires a `target_mode`".into(),
            ));
        }
        if reason.is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.consult_mode_agent requires a `reason` for the audit log".into(),
            ));
        }
        if question.is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.consult_mode_agent requires a `question`".into(),
            ));
        }

        let active_manifest = self.resolve_mode_for_session(session_id).ok_or_else(|| {
            AssistantError::InvalidArgument(
                "this assistant has no mode registry attached; cross-mode consultation requires modes to be loaded"
                    .into(),
            )
        })?;
        let active_mode_id = active_manifest.id.clone();
        if target_mode_id == active_mode_id {
            return Err(AssistantError::InvalidArgument(format!(
                "consult target '{target_mode_id}' is the active mode; answer directly instead"
            )));
        }

        let target_manifest = self.get_mode(&target_mode_id).ok_or_else(|| {
            AssistantError::InvalidArgument(format!(
                "consult target '{target_mode_id}' is not a registered mode"
            ))
        })?;

        self.events.publish(
            session_id,
            TurnEvent::CrossModeConsultRequested {
                session_id,
                active_mode: active_mode_id.clone(),
                target_mode: target_mode_id.clone(),
                reason: reason.clone(),
                question: question.clone(),
            },
        );

        if !target_manifest.allows_consult_from() {
            let denial_reason = format!(
                "mode '{}' has cross_mode_consult_policy = deny; consultation requires switching to that mode (start a new chat in {})",
                target_manifest.id, target_manifest.label
            );
            self.events.publish(
                session_id,
                TurnEvent::CrossModeConsultDenied {
                    session_id,
                    active_mode: active_mode_id.clone(),
                    target_mode: target_mode_id.clone(),
                    reason: denial_reason.clone(),
                },
            );
            return Err(AssistantError::InvalidArgument(denial_reason));
        }

        self.events.publish(
            session_id,
            TurnEvent::CrossModeConsultApproved {
                session_id,
                active_mode: active_mode_id.clone(),
                target_mode: target_mode_id.clone(),
            },
        );

        let consult_goal = format!(
            "You are being consulted as the '{}' mode by the active '{}' mode.\n\
             Reason: {}\n\
             Question: {}\n\n\
             Return a concise, bounded answer for the active mode to consider. \
             Do not ask to read or write the active mode's memory. Do not modify durable state \
             unless the operator explicitly requested it.",
            target_manifest.label, active_manifest.label, reason, question
        );
        let result = self
            .spawn_subagent_in_mode(
                0,
                consult_goal,
                Some(max_iterations),
                Some(target_mode_id.clone()),
            )
            .await?;

        self.events.publish(
            session_id,
            TurnEvent::CrossModeConsultCompleted {
                session_id,
                active_mode: active_mode_id,
                target_mode: target_mode_id.clone(),
                turn_id: result.turn.id,
            },
        );

        Ok(json!({
            "preamble": "Cross-mode consultation returns another mode agent's bounded answer. It does not expose that mode's raw RAG or memory.",
            "target_mode": target_mode_id,
            "target_label": target_manifest.label,
            "reason": reason,
            "question": question,
            "max_iterations": max_iterations,
            "consulted_turn": result.turn,
            "answer": result.turn.assistant_response,
        }))
    }

    /// Bus-driven RAG query. Best-effort: returns an empty vec if the
    /// bus isn't configured or the RAG lane doesn't respond in time.
    /// Retained for future progressive-disclosure variants (e.g. the
    /// LLM asking for RAG by collection name) Ã¢â‚¬â€ currently unused now
    /// that the push 3 turn loop pulls context via meta-tools only.
    #[allow(dead_code)]
    async fn fetch_rag_hits(&self, query: &str, top_k_per_collection: usize) -> Vec<RagHit> {
        let Some(bus) = self.bus.as_ref() else {
            return Vec::new();
        };
        if top_k_per_collection == 0 || query.trim().is_empty() {
            return Vec::new();
        }
        let collections = infer_rag_collections(query);
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            NodeId::new(),
            OrdoMessage::RagQueryRequested {
                query: query.to_string(),
                top_k: top_k_per_collection,
                collections: collections.clone(),
            },
        )
        .with_correlation(correlation_id.clone());

        let mut sub = match bus.subscribe(topics::RAG_QUERY_RESPONSE).await {
            Ok(sub) => sub,
            Err(_) => return Vec::new(),
        };
        if bus
            .publish(topics::RAG_QUERY_REQUEST, envelope)
            .await
            .is_err()
        {
            return Vec::new();
        }
        loop {
            match timeout(DEFAULT_RAG_TIMEOUT, sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }
                    if let OrdoMessage::RagQueryCompleted { query: seen, hits } = event.payload {
                        if seen == query {
                            return hits;
                        }
                    }
                }
                _ => return Vec::new(),
            }
        }
    }
}

/// Sensitive-capability allowlist for tainted conversations.
///
/// Returns `true` when the capability is one we refuse to invoke if
/// the conversation has tainted ancestry. The list is conservative
/// on purpose: read/summarize/recall stay open (the operator should
/// still be able to USE strained content; they just shouldn't
/// trigger writes or dispatches based on it).
///
/// Categories blocked:
///   - outbound dispatch: webhooks, network sync (api.sync_resource,
///     api.dispatch_webhook, ssh.prepare_command)
///   - writes that escape the chat: file writes, app publish,
///     credential upserts, vault writes (vault is double-blocked
///     by the security stack anyway â€” listing here documents
///     intent and short-circuits before vault even sees the call)
///   - memory pinning (would persist a planted fact past session
///     lifetime â€” exactly what the slow-injection attack wants)
///   - MCP install (a tainted conversation should not be installing
///     new MCP servers under operator authority)
///   - review approve/edit (would let a tainted turn approve its
///     own draft through the review queue)
///
/// Reads are universally allowed:
///   - assistant.recall_memory, .knowledge_lookup, .parallel_lookup
///   - knowledge.answer_question, .summarize, .compare_sources
///   - filesystem.read_file
///   - apps.list, .state_at_version, files.list, files.get
///   - logic.* (analysis only)
///   - web.strain, web.fetch_and_strain, web.search (transforms /
///     strict-strain reads, not actions â€” the whole point of these
///     is to be the SAFE path for the assistant to read web content;
///     every result already flows through the boundary wrap so the
///     cup gate sees them as untrusted ancestry)
fn is_sensitive_capability(capability: &str) -> bool {
    // Outbound / network-effecting
    matches!(
        capability,
        "api.dispatch_webhook"
            | "api.sync_resource"
            | "api.prepare_request"
            | "ssh.prepare_command"
            | "ssh.sync_workspace"
    // File / state writes
            | "files.upload"
            | "files.delete"
            | "filesystem.write_file"
            | "apps.publish"
            | "apps.unarchive"
    // Credential surface
            | "cloud.credentials.upsert"
            | "cloud.credentials.delete"
    // Memory pinning (slow-injection vehicle)
            | "memory.pin_note"
            | "memory.unpin_note"
            | "assistant.remember_fact"
            | "assistant.remember_knowledge"
            | "assistant.forget_fact"
    // Skill/plugin install / admin
            | "skills.install"
            | "skills.delete"
            | "plugins.install"
            | "plugins.delete"
            | "plugins.set_enabled"
    // MCP install / admin (tainted conv shouldn't be installing servers)
            | "mcp.servers.install"
            | "mcp.servers.uninstall"
            | "mcp.servers.quarantine"
            | "mcp.servers.re_authorize"
            | "mcp.servers.set_trust"
    // Review queue actuation (could approve its own draft)
            | "review.approve"
            | "review.edit"
            | "review.deny"
    // Webhook config writes
            | "webhooks.register"
            | "webhooks.delete"
            | "webhooks.update"
    )
}

/// Decide whether a successful tool call should mint an
/// `UntrustedMcp` taint, based on the descriptor's `provider` field.
///
/// Convention (set by `ordo-mcp-host::ExternalMcpToolsProvider`):
///   - External MCP tools, untrusted state class:
///     `provider = "mcp:<server_id>"` â€” Untrusted / Observed /
///     Validated. Auto-promoted from clean history but the operator
///     hasn't manually vouched for them.
///   - External MCP tools, operator-blessed:
///     `provider = "mcp:trusted:<server_id>"` â€” `ServerTrustState::Trusted`.
///     Long clean history OR explicitly promoted via
///     `mcp.servers.set_trust`. Skipped from auto-taint.
///   - Native bus providers advertise their crate name directly
///     (e.g. `"ordo-strainer"`, `"ordo-files"`, `"ordo-logic"`).
///
/// Only an untrusted-state `mcp:` prefix triggers a taint. The
/// `mcp:trusted:` prefix and native bus output are both treated as
/// first-party for the purposes of the cup gate.
///
/// Returns `None` when:
///   - The capability isn't in the provider map (meta-tools route
///     locally and aren't advertised through the bus).
///   - The provider is `mcp:trusted:*` (operator-blessed server).
///   - The provider isn't `mcp:*` at all (native bus).
fn mcp_taint_for_provider(
    provider: Option<&str>,
    invocation_id: Uuid,
) -> Option<ordo_protocol::Taint> {
    let provider = provider?;
    // Operator-blessed servers skip auto-taint. The check is ordered
    // (trusted prefix first) because `mcp:trusted:` ALSO starts with
    // `mcp:` â€” the more specific match has to fire first.
    if provider.strip_prefix("mcp:trusted:").is_some() {
        return None;
    }
    let server_id = provider.strip_prefix("mcp:")?;
    Some(ordo_protocol::Taint::UntrustedMcp {
        server_id: server_id.to_string(),
        invocation_id: invocation_id.to_string(),
    })
}

/// Pull the value of a single HTML-style attribute out of a tag
/// string. Used by `detect_untrusted_web_taints` to capture
/// `source` and `fetched_at` from the `<untrusted_web_content>`
/// open tag without parsing the whole HTML â€” we already trust the
/// strainer's wrap.rs to escape attribute values, so a permissive
/// substring match is safe here.
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let pattern = format!("{name}=\"");
    let start = tag.find(&pattern)? + pattern.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod taint_helpers_tests {
    use super::*;

    #[test]
    fn extract_attr_finds_quoted_value() {
        let tag = r#"<untrusted_web_content source="https://x.test/a" fetched_at="2026-05-01T00:00:00Z">"#;
        assert_eq!(
            extract_attr(tag, "source").as_deref(),
            Some("https://x.test/a")
        );
        assert_eq!(
            extract_attr(tag, "fetched_at").as_deref(),
            Some("2026-05-01T00:00:00Z")
        );
        assert_eq!(extract_attr(tag, "missing"), None);
    }

    #[test]
    fn detect_untrusted_finds_zero_one_or_many_blocks() {
        let none = AssistantService::detect_untrusted_web_taints(
            "Just a plain user message with no boundary tag.",
        );
        assert!(none.is_empty());

        let one = AssistantService::detect_untrusted_web_taints(
            r#"Summarize: <untrusted_web_content source="https://a.test" fetched_at="now">body</untrusted_web_content>"#,
        );
        assert_eq!(one.len(), 1);
        match &one[0] {
            ordo_protocol::Taint::UntrustedWeb { source_url, .. } => {
                assert_eq!(source_url, "https://a.test");
            }
            other => panic!("expected UntrustedWeb, got {other:?}"),
        }

        let many = AssistantService::detect_untrusted_web_taints(
            r#"Compare:
            <untrusted_web_content source="https://a.test" fetched_at="now">A</untrusted_web_content>
            and
            <untrusted_web_content source="https://b.test" fetched_at="now">B</untrusted_web_content>"#,
        );
        assert_eq!(many.len(), 2);
    }

    #[test]
    fn sensitive_capabilities_block_writes_allow_reads() {
        // Writes / dispatch â€” blocked.
        for cap in [
            "api.dispatch_webhook",
            "files.upload",
            "files.delete",
            "filesystem.write_file",
            "memory.pin_note",
            "assistant.remember_fact",
            "assistant.remember_knowledge",
            "skills.install",
            "skills.delete",
            "plugins.install",
            "plugins.delete",
            "plugins.set_enabled",
            "mcp.servers.install",
            "review.approve",
            "webhooks.register",
            "cloud.credentials.upsert",
        ] {
            assert!(is_sensitive_capability(cap), "{cap} should be sensitive");
        }
        // Reads / analysis â€” allowed.
        for cap in [
            "assistant.recall_memory",
            "assistant.knowledge_lookup",
            "knowledge.summarize",
            "knowledge.answer_question",
            "filesystem.read_file",
            "files.list",
            "files.get",
            "logic.identify_claims",
            "logic.find_fallacies",
            "logic.validate_chain",
            "web.strain",
            "web.fetch_and_strain",
            "web.search",
        ] {
            assert!(!is_sensitive_capability(cap), "{cap} should be allowed");
        }
    }

    #[test]
    fn reasoning_only_response_falls_back_to_tool_summary() {
        let response = json!({
            "assistant_message": "(no content emitted; reasoning preview)\nI should summarize the diagnostics.",
            "content_raw": "",
            "reasoning": "I should summarize the diagnostics.",
        });
        let invocations = vec![ToolInvocation {
            invocation_id: Uuid::new_v4(),
            capability: "runtime.describe_profile".into(),
            arguments: json!({}),
            result: Some(json!({
                "profile": "standard",
                "control_api_enabled": true,
                "rag_enabled": true,
                "knowledge_enabled": true,
            })),
            error: None,
            duration_ms: 12,
        }];

        let visible = visible_assistant_message(&response, &invocations);

        assert!(!visible.contains("reasoning preview"));
        assert!(visible.contains("runtime.describe_profile"));
        assert!(visible.contains("profile=standard"));
    }

    #[test]
    fn mcp_provider_prefix_mints_taint() {
        let invocation_id = Uuid::new_v4();
        let taint = mcp_taint_for_provider(Some("mcp:news-server"), invocation_id)
            .expect("mcp:* provider must mint a taint");
        match taint {
            ordo_protocol::Taint::UntrustedMcp {
                server_id,
                invocation_id: rec_id,
            } => {
                assert_eq!(server_id, "news-server");
                assert_eq!(rec_id, invocation_id.to_string());
            }
            other => panic!("expected UntrustedMcp, got {other:?}"),
        }
    }

    #[test]
    fn native_bus_providers_do_not_taint() {
        // Crate-name providers are first-party; tool output is trusted.
        for provider in [
            "ordo-strainer",
            "ordo-files",
            "ordo-logic",
            "ordo-memory-log",
            "external-mcp-tools", // The PROVIDER NAME of the bridge,
                                  // not the tools it wraps. The bridge advertises its tools
                                  // with `provider = "mcp:<id>"` per descriptor; this
                                  // string never appears in `capability_providers`.
        ] {
            assert!(
                mcp_taint_for_provider(Some(provider), Uuid::new_v4()).is_none(),
                "{provider} should not mint a taint"
            );
        }
    }

    #[test]
    fn missing_provider_does_not_taint() {
        // Meta-tools (assistant.*) aren't in the provider map at all.
        // The lookup returns None â€” must not taint.
        assert!(mcp_taint_for_provider(None, Uuid::new_v4()).is_none());
    }

    #[test]
    fn trusted_mcp_prefix_skips_taint() {
        // Operator-blessed servers advertise as `mcp:trusted:<id>` â€”
        // explicit `mcp.servers.set_trust` to Trusted, or auto-
        // promoted by long clean invocation history. Tool output
        // from these servers does NOT taint the session.
        assert!(
            mcp_taint_for_provider(Some("mcp:trusted:my-content_store"), Uuid::new_v4()).is_none(),
            "trusted MCP must not taint"
        );
    }

    #[test]
    fn trusted_prefix_check_runs_before_generic_mcp_check() {
        // `mcp:trusted:foo` also starts with `mcp:` â€” the more
        // specific branch must fire first. If the order regresses,
        // a Trusted server would be tainted with server_id =
        // "trusted:foo", which would be wrong twice (taint we
        // shouldn't have, server_id mangled).
        let invocation_id = Uuid::new_v4();
        let result = mcp_taint_for_provider(Some("mcp:trusted:my-content_store"), invocation_id);
        assert!(result.is_none());

        // And confirm the untrusted-state branch still works for
        // the bare prefix (regression-resistant against future
        // refactors that might over-strip).
        let untrusted = mcp_taint_for_provider(Some("mcp:my-other-content_store"), invocation_id)
            .expect("untrusted state must taint");
        match untrusted {
            ordo_protocol::Taint::UntrustedMcp { server_id, .. } => {
                assert_eq!(server_id, "my-other-content_store");
            }
            other => panic!("expected UntrustedMcp, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod cross_mode_consult_tests {
    use super::*;
    use ordo_cloud::{CloudCredentialStore, CloudCredentialTask, CloudCredentialUpdate};
    use ordo_models::HashingEmbedder;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_mode(
        id: &str,
        label: &str,
        borrow_policy: Option<&str>,
        consult_policy: Option<&str>,
    ) -> ordo_modes::ModeManifest {
        let mut manifest = ordo_modes::ModeManifest {
            id: id.to_string(),
            label: label.to_string(),
            description: format!("{label} test mode"),
            memory_scope: vec!["global".to_string(), format!("mode:{id}")],
            rag_domains: vec![format!("{id}_rag")],
            allowed_tool_lanes: vec!["knowledge.".to_string(), "memory.list_".to_string()],
            blocked_tool_capabilities: Vec::new(),
            policies: Vec::new(),
            planner_bias: vec![format!("Answer as {label}.")],
            persona: vec![format!("{id}_persona")],
            default_timeout_secs: None,
            default_strictness: None,
            default_credential: None,
            cross_mode_borrow_policy: borrow_policy.map(str::to_string),
            cross_mode_consult_policy: consult_policy.map(str::to_string),
        };
        manifest.normalize_and_validate().expect("valid test mode");
        manifest
    }

    fn registry_with(modes: Vec<ordo_modes::ModeManifest>) -> ordo_modes::ModeRegistry {
        let registry = ordo_modes::ModeRegistry::empty();
        for mode in modes {
            registry.upsert(mode).expect("upsert test mode");
        }
        registry
    }

    async fn credentials_for(base_url: &str) -> CloudCredentialTask {
        let store = CloudCredentialStore::in_memory().expect("credential store");
        let task = CloudCredentialTask::start(store);
        task.upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{base_url}/")),
            ..Default::default()
        })
        .await
        .expect("upsert test credential");
        task
    }

    #[tokio::test]
    async fn consult_mode_agent_returns_target_answer_without_raw_target_context() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "mock-target",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Research mode says: verify sources and separate claims from evidence."
                    },
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;

        let registry = registry_with(vec![
            test_mode("general", "General", None, None),
            test_mode("research", "Research", Some("deny"), Some("allow")),
        ]);
        let service = AssistantService::new(
            AssistantStore::in_memory().expect("store"),
            Arc::new(HashingEmbedder::new(96)),
            credentials_for(&server.uri()).await,
        )
        .with_modes(registry);
        let session = service
            .new_session(Some("active general"), Some("general"))
            .expect("create active session");

        let output = service
            .meta_consult_mode_agent(
                session.id,
                json!({
                    "target_mode": "research",
                    "reason": "Need source-checking expertise.",
                    "question": "How should this claim be assessed?",
                    "max_iterations": 1
                }),
            )
            .await
            .expect("consult should succeed");

        assert_eq!(output["target_mode"], "research");
        assert_eq!(
            output["answer"],
            "Research mode says: verify sources and separate claims from evidence."
        );
        assert!(output["preamble"]
            .as_str()
            .expect("preamble")
            .contains("does not expose that mode's raw RAG or memory"));

        let consulted = &output["consulted_turn"];
        assert_eq!(consulted["context"]["facts"].as_array().unwrap().len(), 0);
        assert_eq!(
            consulted["context"]["rag_hits"].as_array().unwrap().len(),
            0
        );

        let received = server.received_requests().await.expect("requests");
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).expect("request body");
        let serialized = serde_json::to_string(&body["messages"]).expect("serialize messages");
        assert!(serialized.contains("You are being consulted as the 'Research' mode"));
        assert!(serialized.contains("Need source-checking expertise."));
        assert!(!serialized.contains("research_rag"));
    }

    #[tokio::test]
    async fn consult_mode_agent_respects_target_deny_policy() {
        let registry = registry_with(vec![
            test_mode("general", "General", None, None),
            test_mode("diagnostic", "Diagnostic", None, Some("deny")),
        ]);
        let service = AssistantService::new(
            AssistantStore::in_memory().expect("store"),
            Arc::new(HashingEmbedder::new(96)),
            CloudCredentialTask::start(
                CloudCredentialStore::in_memory().expect("credential store"),
            ),
        )
        .with_modes(registry);
        let session = service
            .new_session(Some("active general"), Some("general"))
            .expect("create active session");

        let err = service
            .meta_consult_mode_agent(
                session.id,
                json!({
                    "target_mode": "diagnostic",
                    "reason": "Need system diagnosis.",
                    "question": "What is wrong?"
                }),
            )
            .await
            .expect_err("diagnostic should deny consultation");

        match err {
            AssistantError::InvalidArgument(message) => {
                assert!(message.contains("cross_mode_consult_policy = deny"));
                assert!(message.contains("start a new chat in Diagnostic"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }
}

/// OpenAI-style function-calling schemas for the three meta-tools. The
/// descriptions are deliberately long Ã¢â‚¬â€ they double as the read-only
/// instructions for how to use each memory layer and reach the LLM
/// without needing to reserialize the system prompt.
fn meta_tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "assistant.recall_memory",
                "description": "Persistent fact memory about the operator, brand, clients, and projects. Use this to pull facts before answering anything that touches preferences, history, or domain context. Facts are subject-predicate-object triples with confidence scores; operator-authored facts outrank auto-extracted ones. If no fact matches, the result set is empty Ã¢â‚¬â€ say so rather than invent.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Natural-language query the fact store embeds and ranks by cosine similarity."},
                        "top_k": {"type": "integer", "description": "Maximum number of facts to return. Defaults to 8.", "default": 8}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "assistant.knowledge_lookup",
                "description": "The assistant's self-knowledge RAG Ã¢â‚¬â€ skill cards, persona guides, capability notes, and observations about what worked or didn't. Call this to discover what you can do and how. Optionally scope by `kind` ('skill', 'persona', 'tool_note', 'observation', 'note') or by `domain` (one of the ten domain slots: planning, orchestration, research, content_store, domain_slot_5Ã¢â‚¬Â¦domain_slot_10).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Natural-language query Ã¢â‚¬â€ typically the task you're trying to accomplish."},
                        "top_k": {"type": "integer", "description": "Maximum number of entries to return. Defaults to 5.", "default": 5},
                        "kind": {"type": "string", "description": "Optional filter. One of skill, persona, tool_note, observation, note."},
                        "domain": {"type": "string", "description": "Optional domain slot to scope the lookup."}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "assistant.parallel_lookup",
                "description": "Run `knowledge_lookup` concurrently across an explicit list of domains. Use this only when the user, active mode, or retrieved knowledge has already identified the domains; Ordo does not auto-route domains here. Returns {domain, hits[]} entries. Same `kind`/`top_k` semantics as the single-domain variant.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Natural-language query, shared across all fanned-out lookups."},
                        "domains": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Domain names to query in parallel. These must be explicit choices from the user, active mode, or retrieved knowledge."
                        },
                        "top_k_per_domain": {"type": "integer", "description": "Max hits per domain. Defaults to 3.", "default": 3},
                        "kind": {"type": "string", "description": "Optional filter. One of skill, persona, tool_note, observation, note."}
                    },
                    "required": ["query", "domains"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "assistant.list_facts",
                "description": "List persistent fact/persona memory visible to the active mode. Diagnostic mode only sees global plus mode:diagnostic scoped facts. Use subject to filter operator, agent, project, client, or another subject.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string", "description": "Optional fact subject filter, such as operator, agent, project, or client."},
                        "limit": {"type": "integer", "description": "Maximum facts to return. Defaults to 200, capped at 500."}
                    },
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "assistant.remember_fact",
                "description": "Write a persistent fact/persona memory into the active mode scope. Diagnostic mode writes are automatically limited to mode:diagnostic and cannot write global facts.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string", "description": "Fact subject, such as operator, agent, project, client, or diagnostic."},
                        "predicate": {"type": "string", "description": "Relationship or fact type, such as note, persona, preference, constraint, or finding."},
                        "object": {"type": "string", "description": "The fact body."},
                        "source": {"type": "string", "description": "Optional source label."},
                        "confidence": {"type": "number", "description": "Confidence 0.0 to 1.0. Defaults to 1.0."},
                        "scope": {"type": "string", "description": "Optional memory scope. Diagnostic mode may only use mode:diagnostic."}
                    },
                    "required": ["subject", "predicate", "object"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "assistant.forget_fact",
                "description": "Delete a persistent fact/persona memory by id, limited to facts visible inside the active mode's memory scope.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "description": "UUID of the fact to delete."}
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "assistant.remember_knowledge",
                "description": "Write a verified lesson, tool note, skill card, persona note, or observation into the active mode's own self-knowledge RAG. The write is automatically limited to domains declared by the active mode. Diagnostic mode defaults to diagnostic_self_learning_tree and cannot write outside its private diagnostic RAG domains.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Short title for the lesson. Defaults to the first line of the body."},
                        "body": {"type": "string", "description": "Lesson text to remember. Use only after the outcome has been verified."},
                        "content": {"type": "string", "description": "Alias for body."},
                        "note": {"type": "string", "description": "Alias for body."},
                        "kind": {"type": "string", "description": "One of skill, persona, tool_note, observation, note. Defaults to observation."},
                        "domain": {"type": "string", "description": "Optional active-mode RAG domain. Omit in diagnostic mode to write to diagnostic_self_learning_tree."},
                        "source": {"type": "string", "description": "Optional source label. Defaults to assistant:mode:<mode>:learned."},
                        "confidence": {"type": "number", "description": "Confidence 0.0 to 1.0. Defaults to 1.0."}
                    },
                    "anyOf": [
                        {"required": ["body"]},
                        {"required": ["content"]},
                        {"required": ["note"]}
                    ],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "assistant.consult_mode_agent",
                "description": "Cross-mode consultation. Use this ONLY when the active mode needs another mode's expertise for THIS response. This starts a bounded subagent in the target mode and returns only that agent's answer. It never exposes the target mode's raw RAG or memory to the active mode.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "target_mode": {"type": "string", "description": "Mode id to consult. Must be a registered mode and not the active mode."},
                        "reason": {"type": "string", "description": "One-sentence justification. Recorded in the audit log."},
                        "question": {"type": "string", "description": "The narrow question to ask the target mode agent."},
                        "max_iterations": {"type": "integer", "description": "Maximum tool-call iterations for the consulted mode agent. Defaults to 2, capped at 5."}
                    },
                    "required": ["target_mode", "reason", "question"],
                    "additionalProperties": false
                }
            }
        }),
    ]
}
