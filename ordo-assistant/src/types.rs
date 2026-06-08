//! Core types for the assistant layer â€” sessions, turns, facts,
//! retrieval records.
//!
//! Kept deliberately small: these travel across the bus, the control
//! API, and the persistent SQLite store, so they need to be
//! serde-serialisable without surprise.

use chrono::{DateTime, Utc};
use ordo_protocol::{RagHit, UserAttachment};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// A persisted conversation thread. Each turn the operator has with
/// the assistant lives under one of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantSession {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Mode-scoped workspace this session is bound to. Resolved to
    /// a `ModeManifest` at turn time. Fixed at session creation —
    /// switching modes in the UXI creates a new session, never
    /// rewrites this field.
    ///
    /// Defaults to `"general"` for sessions created before the
    /// mode column existed; new sessions specify a mode id the
    /// registry knows.
    #[serde(default = "default_mode_id")]
    pub mode: String,
    /// Optional human-readable title. Populated the first time the
    /// session produces a turn â€” defaults to the first few words of
    /// the user's opening message.
    pub title: Option<String>,
    pub turn_count: u32,
}

fn default_mode_id() -> String {
    "general".to_string()
}

/// Every exchange is persisted as one turn row: what the operator
/// said, what the assistant replied, and *why* it said what it said
/// (retrieved facts, RAG snippets, model used).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub id: Uuid,
    pub session_id: Uuid,
    pub index: u32,
    pub created_at: DateTime<Utc>,
    pub user_message: String,
    pub assistant_response: String,
    pub context: TurnContext,
    pub model: Option<String>,
    pub credential_service: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechRequest {
    pub input: String,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct SpeechResponse {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub format: String,
    pub credential_service: String,
    pub model: String,
    pub voice: String,
}

/// Everything the router pulled into the prompt for this turn. Stored
/// alongside the turn so the studio can surface "here's what the
/// assistant consulted before it answered you."
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnContext {
    #[serde(default)]
    pub facts: Vec<RecalledFact>,
    #[serde(default)]
    pub rag_hits: Vec<RagHitSummary>,
    #[serde(default)]
    pub history_window: usize,
    /// Tool calls the assistant made during this turn (push 2+).
    #[serde(default)]
    pub tool_calls: Vec<ToolInvocation>,
}

/// One autonomous tool call the assistant made while composing a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub invocation_id: Uuid,
    pub capability: String,
    pub arguments: serde_json::Value,
    /// `Some` when the call succeeded; `None` when it failed (see
    /// `error` for the reason).
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Lightweight reference to a RAG hit â€” we keep the metadata but not
/// the full text, since the RAG store already owns the body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagHitSummary {
    pub collection: String,
    pub document_id: String,
    pub title: String,
    pub chunk_index: usize,
    pub score: f32,
    pub snippet: String,
}

impl From<&RagHit> for RagHitSummary {
    fn from(hit: &RagHit) -> Self {
        Self {
            collection: hit.collection.clone(),
            document_id: hit.document_id.clone(),
            title: hit.title.clone(),
            chunk_index: hit.chunk_index,
            score: hit.score,
            snippet: hit.snippet.clone(),
        }
    }
}

/// A durable fact the assistant remembers about someone or something.
/// Subject-predicate-object lets us store both key/value preferences
/// and free-form biographical notes in a uniform shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub id: Uuid,
    /// Who or what this fact is about. Conventions:
    /// - `"user"` for the operator themselves
    /// - `"client:<name>"` for a client
    /// - `"brand"` for brand-wide preferences
    /// - `"project:<slug>"` for a specific project
    pub subject: String,
    /// Relationship type. Conventions: `prefers`, `avoids`, `location`,
    /// `role`, `fact`. Free-form â€” the LLM reads it as text.
    pub predicate: String,
    pub object: String,
    pub source: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub reinforced_at: DateTime<Utc>,
    /// Memory scope this fact lives under. Conventions:
    /// - `"global"` — visible from every mode (default for legacy
    ///   facts and for facts inserted without an explicit scope).
    /// - `"mode:<id>"` — only visible to the named mode.
    /// - `"project:<slug>"` / `"session:<uuid>"` — narrower scopes
    ///   reserved for future use.
    ///
    /// Recall reads facts whose scope is in the active mode's
    /// `memory_scope` list. The list always includes `"global"`,
    /// so global facts surface everywhere.
    #[serde(default = "default_fact_scope")]
    pub scope: String,
    /// Embedding of `format!("{subject} {predicate} {object}")`. Used
    /// for semantic recall at turn time; never returned to the API.
    #[serde(skip)]
    pub embedding: Vec<f32>,
}

fn default_fact_scope() -> String {
    "global".to_string()
}

impl Fact {
    pub fn semantic_form(&self) -> String {
        format!("{} {} {}", self.subject, self.predicate, self.object)
    }
}

/// Fact plus its recall score, so the UI can show "this is why I
/// thought this was relevant."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecalledFact {
    pub fact: FactSummary,
    pub score: f32,
}

/// API-safe fact view (drops the embedding bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactSummary {
    pub id: Uuid,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub source: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub reinforced_at: DateTime<Utc>,
    /// Memory scope this fact is tagged with. See [`Fact::scope`]
    /// for the conventions.
    #[serde(default = "default_fact_scope")]
    pub scope: String,
}

impl From<&Fact> for FactSummary {
    fn from(fact: &Fact) -> Self {
        Self {
            id: fact.id,
            subject: fact.subject.clone(),
            predicate: fact.predicate.clone(),
            object: fact.object.clone(),
            source: fact.source.clone(),
            confidence: fact.confidence,
            created_at: fact.created_at,
            reinforced_at: fact.reinforced_at,
            scope: fact.scope.clone(),
        }
    }
}

/// Constructor for a new fact â€” `id`, timestamps, and embedding are
/// filled in by the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewFact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    /// Memory scope. None = `"global"` (visible from every mode).
    /// Set to `"mode:<id>"` to scope a fact to a single workspace.
    #[serde(default)]
    pub scope: Option<String>,
}

fn default_source() -> String {
    "operator".to_string()
}

fn default_confidence() -> f32 {
    1.0
}

/// The final result of a `turn()` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    pub session_id: Uuid,
    pub turn: Turn,
    /// Echo of the retrieved context for the caller's convenience
    /// (already embedded in `turn.context`).
    pub retrieved_facts: Vec<RecalledFact>,
    pub retrieved_rag: Vec<RagHitSummary>,
    /// Outcome of the review step when `review: true` was requested.
    /// `None` when review was skipped.
    #[serde(default)]
    pub review_outcome: Option<ReviewOutcome>,
}

/// Outcome of routing an assistant draft through the `review.*` lane.
/// Recorded on the turn so the studio can show \"this draft was
/// approved/edited/denied before delivery.\"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOutcome {
    pub review_request_id: Uuid,
    pub state: String,
    /// Whatever the operator landed on as the final text. Matches
    /// `turn.assistant_response` on approve/edit; differs on deny
    /// (contains the denial note) and expired paths.
    pub delivered_content: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionWithTurns {
    pub session: AssistantSession,
    pub turns: Vec<Turn>,
}

/// Raw arguments from the caller. `session_id` is optional â€” when
/// omitted a new session is created.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnRequest {
    #[serde(default)]
    pub session_id: Option<Uuid>,
    pub user_message: String,
    /// Override the default credential (same pattern as the cloud lane).
    #[serde(default)]
    pub credential: Option<String>,
    /// When false, skip RAG retrieval entirely. Defaults to true.
    #[serde(default = "default_true")]
    pub use_rag: bool,
    /// When false, skip fact recall. Defaults to true.
    #[serde(default = "default_true")]
    pub use_memory: bool,
    /// When false, skip autonomous tool use for this turn. Defaults
    /// to true.
    #[serde(default = "default_true")]
    pub use_tools: bool,
    /// When true, route the assistant's final draft through the
    /// `review.*` queue before the operator sees it. Requires the
    /// service to have been built with `with_review(...)`. Defaults
    /// to false â€” review is opt-in so everyday conversations stay
    /// fast.
    #[serde(default)]
    pub review: bool,
    /// How long to wait for an operator decision when `review` is
    /// true. Defaults to 300 s (five minutes); the request expires if
    /// nobody acts in time and the turn fails cleanly.
    #[serde(default = "default_review_wait_secs")]
    pub review_wait_secs: u64,
    /// Push 6: when true (and the turn is not using tools), stream
    /// tokens back as `TurnEvent::TokenDelta` for live studio
    /// \"typing\" UX. Defaults to false so existing callers keep the
    /// one-shot REST shape.
    #[serde(default)]
    pub stream: bool,
    /// How many prior turns from this session to include in the prompt.
    #[serde(default = "default_history_window")]
    pub history_window: usize,
    /// Top-K facts to consider during recall.
    #[serde(default = "default_fact_top_k")]
    pub fact_top_k: usize,
    /// Top-K RAG hits per inferred collection.
    #[serde(default = "default_rag_top_k")]
    pub rag_top_k: usize,
    /// Optional extra metadata caller wants echoed into audit.
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, Value>,
    /// Multimodal attachments (Phase 1.3). When non-empty, the turn's
    /// user-role message is built as a content array with the text
    /// alongside each attachment. When empty, the existing string-only
    /// path is used unchanged (zero impact on default turns).
    #[serde(default)]
    pub attachments: Vec<UserAttachment>,
    /// Subagent recursion depth (Phase 4.1). Increments each time an
    /// assistant spawns another assistant turn. The service rejects
    /// any turn with depth > `MAX_SUBAGENT_DEPTH` so a bug in a tool
    /// call can't fork infinitely. External callers set this to 0
    /// (the default); internal `spawn_subagent` sets it to parent+1.
    #[serde(default)]
    pub subagent_depth: u32,
    /// Mode-scoped workspace for THIS request. Only consulted when
    /// the request creates a new session (session_id is None and
    /// the runtime auto-creates one). For an existing session, the
    /// session's stored mode wins; this field is ignored — the
    /// architecture doesn't allow mid-session mode changes (see
    /// `ordo_modes` rationale).
    ///
    /// When None and a new session is being created, defaults to
    /// `"general"`.
    #[serde(default)]
    pub mode: Option<String>,
}

/// Hard cap on nested assistant invocations. Three levels is
/// generous â€” one operator turn, one planner subagent, one leaf
/// subagent. Beyond that, an agent is almost certainly looping.
pub const MAX_SUBAGENT_DEPTH: u32 = 3;

fn default_true() -> bool {
    true
}

impl Default for TurnRequest {
    fn default() -> Self {
        Self {
            session_id: None,
            user_message: String::new(),
            credential: None,
            use_rag: default_true(),
            use_memory: default_true(),
            use_tools: default_true(),
            review: false,
            review_wait_secs: default_review_wait_secs(),
            stream: false,
            history_window: default_history_window(),
            fact_top_k: default_fact_top_k(),
            rag_top_k: default_rag_top_k(),
            metadata: std::collections::HashMap::new(),
            attachments: Vec::new(),
            subagent_depth: 0,
            mode: None,
        }
    }
}

fn default_history_window() -> usize {
    6
}

fn default_fact_top_k() -> usize {
    8
}

fn default_rag_top_k() -> usize {
    3
}

fn default_review_wait_secs() -> u64 {
    300
}

// ---- self-knowledge (push 3) -----------------------------------------

/// A snippet in the assistant's self-knowledge RAG: a skill card,
/// persona guide, capability note, or observation. The LLM pulls these
/// via `assistant.knowledge_lookup` to decide *how* to act.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: Uuid,
    pub kind: KnowledgeKind,
    /// Optional domain slot this entry is scoped to. When set, the
    /// router can filter lookups to a specific domain.
    #[serde(default)]
    pub domain: Option<String>,
    pub title: String,
    pub body: String,
    pub source: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub reinforced_at: DateTime<Utc>,
    #[serde(skip)]
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeKind {
    /// A capability / skill the assistant can perform.
    Skill,
    /// A voice or persona profile.
    Persona,
    /// Notes on how to use a specific tool or capability.
    ToolNote,
    /// An observation about what worked or didn't on past turns.
    Observation,
    /// Free-form note.
    Note,
}

impl KnowledgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Persona => "persona",
            Self::ToolNote => "tool_note",
            Self::Observation => "observation",
            Self::Note => "note",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "skill" => Some(Self::Skill),
            "persona" => Some(Self::Persona),
            "tool_note" => Some(Self::ToolNote),
            "observation" => Some(Self::Observation),
            "note" => Some(Self::Note),
            _ => None,
        }
    }
}

/// API-safe view of a knowledge entry (drops embedding).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSummary {
    pub id: Uuid,
    pub kind: KnowledgeKind,
    pub domain: Option<String>,
    pub title: String,
    pub body: String,
    pub source: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub reinforced_at: DateTime<Utc>,
}

impl From<&KnowledgeEntry> for KnowledgeSummary {
    fn from(entry: &KnowledgeEntry) -> Self {
        Self {
            id: entry.id,
            kind: entry.kind,
            domain: entry.domain.clone(),
            title: entry.title.clone(),
            body: entry.body.clone(),
            source: entry.source.clone(),
            confidence: entry.confidence,
            created_at: entry.created_at,
            updated_at: entry.updated_at,
            reinforced_at: entry.reinforced_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecalledKnowledge {
    pub entry: KnowledgeSummary,
    pub score: f32,
}

/// Constructor for a new knowledge entry â€” `id`, timestamps, and
/// embedding are filled in by the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewKnowledge {
    pub kind: KnowledgeKind,
    #[serde(default)]
    pub domain: Option<String>,
    pub title: String,
    pub body: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

// ---- cancellation registry (push 6) ----------------------------------

/// Per-session cancellation flag registry. The control-API WebSocket
/// registers a flag when a turn starts; calling `cancel(session_id)`
/// flips it, and the turn loop checks between tool-call iterations to
/// bail out quickly. Cheap to clone (`Arc` + `parking_lot::Mutex`).
#[derive(Clone, Default)]
pub struct CancellationRegistry {
    inner: std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<Uuid, CancelFlag>>>,
}

#[derive(Clone, Default)]
pub struct CancelFlag(pub std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelFlag {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl CancellationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or reuse) a flag for `session_id`. Returned handle is
    /// cloneable â€” both the turn loop and any external cancel caller
    /// hold their own reference.
    pub fn register(&self, session_id: Uuid) -> CancelFlag {
        let mut guard = self.inner.lock();
        guard.entry(session_id).or_default().clone()
    }

    pub fn cancel(&self, session_id: Uuid) -> bool {
        let guard = self.inner.lock();
        if let Some(flag) = guard.get(&session_id) {
            flag.cancel();
            true
        } else {
            false
        }
    }

    /// Remove the flag once the turn is done so the registry doesn't
    /// grow forever. Called from the turn loop's exit path.
    pub fn release(&self, session_id: Uuid) {
        self.inner.lock().remove(&session_id);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AssistantError {
    #[error("subagent recursion budget exceeded (depth {0}; max {1})")]
    SubagentBudgetExceeded(u32, u32),
    #[error("assistant session '{0}' not found")]
    SessionNotFound(Uuid),
    #[error("fact '{0}' not found")]
    FactNotFound(Uuid),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("local storage error: {0}")]
    Storage(String),
    #[error("embedding error: {0}")]
    Embedding(String),
    #[error("LLM call failed: {0}")]
    LlmFailed(String),
    #[error("turn was cancelled by the operator")]
    Cancelled,
    #[error("assistant has no cloud credential configured: {0}")]
    NoCredential(String),
    #[error("bus error: {0}")]
    Bus(String),
}

pub type AssistantResult<T> = Result<T, AssistantError>;

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[test]
    fn cancel_flag_reflects_state() {
        let flag = CancelFlag::default();
        assert!(!flag.is_cancelled());
        flag.cancel();
        assert!(flag.is_cancelled());
    }

    #[test]
    fn registry_cancels_registered_session() {
        let registry = CancellationRegistry::new();
        let id = Uuid::new_v4();
        let flag = registry.register(id);
        assert!(!flag.is_cancelled());
        assert!(registry.cancel(id));
        assert!(flag.is_cancelled());
        registry.release(id);
        // After release the registry returns false for the same id.
        assert!(!registry.cancel(id));
    }

    #[test]
    fn registry_returns_false_for_unknown_session() {
        let registry = CancellationRegistry::new();
        assert!(!registry.cancel(Uuid::new_v4()));
    }
}
