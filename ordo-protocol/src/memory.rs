//! Hierarchical memory architecture types (v2 blueprint).
//!
//! These are the constitutional additions for the
//! `ordo-memory-log` / `ordo-memory-router` / `ordo-memory-projection`
//! crates. Rule 11: all wire-shared shapes live here, changes go
//! through the CHANGELOG.
//!
//! The dense comments below are load-bearing â€” they document the
//! invariants the three memory crates rely on. Future refactors:
//! read them first.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// ULID (Crockford-base32, 26 chars, time-sortable). Event ids MUST
/// be ULIDs per the protocol constitution â€” sortable, globally
/// unique, no exceptions. Stored as `String` on the wire so we
/// don't force consumers to pull a ULID crate; validation happens
/// at the log layer.
pub type Ulid = String;

// -----------------------------------------------------------------------------
// Event log (Crate 1: ordo-memory-log)
// -----------------------------------------------------------------------------

/// The canonical event taxonomy. Extensible â€” new variants are
/// additive. Existing labels are FROZEN because they persist in
/// SQLite rows and are consumed by downstream projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEventType {
    // Interaction
    UserMessage,
    AgentResponse,
    ToolInvocation,
    ToolResult,
    // Memory
    MemoryWrite,
    MemoryCorrection,
    // Identity & workflow
    IdentityAssertion,
    WorkflowCheckpoint,
    // Feedback (v2)
    FeedbackSignal,
    // System
    SystemRescue,
    SystemTreeChange,
    SystemProtocolViolation,
    RetentionTransition,
    /// Canary write from the memory-log health task. Auto-pinned so
    /// tier transitions don't churn on them; auto-cleaned after 24h
    /// by the same task. Prove the full write path works.
    SystemHealthProbe,
}

impl MemoryEventType {
    pub fn label(self) -> &'static str {
        match self {
            Self::UserMessage => "user.message",
            Self::AgentResponse => "agent.response",
            Self::ToolInvocation => "tool.invocation",
            Self::ToolResult => "tool.result",
            Self::MemoryWrite => "memory.write",
            Self::MemoryCorrection => "memory.correction",
            Self::IdentityAssertion => "identity.assertion",
            Self::WorkflowCheckpoint => "workflow.checkpoint",
            Self::FeedbackSignal => "feedback.signal",
            Self::SystemRescue => "system.rescue",
            Self::SystemTreeChange => "system.tree_change",
            Self::SystemProtocolViolation => "system.protocol_violation",
            Self::RetentionTransition => "retention.transition",
            Self::SystemHealthProbe => "system.health_probe",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Some(match label {
            "user.message" => Self::UserMessage,
            "agent.response" => Self::AgentResponse,
            "tool.invocation" => Self::ToolInvocation,
            "tool.result" => Self::ToolResult,
            "memory.write" => Self::MemoryWrite,
            "memory.correction" => Self::MemoryCorrection,
            "identity.assertion" => Self::IdentityAssertion,
            "workflow.checkpoint" => Self::WorkflowCheckpoint,
            "feedback.signal" => Self::FeedbackSignal,
            "system.rescue" => Self::SystemRescue,
            "system.tree_change" => Self::SystemTreeChange,
            "system.protocol_violation" => Self::SystemProtocolViolation,
            "retention.transition" => Self::RetentionTransition,
            "system.health_probe" => Self::SystemHealthProbe,
            _ => return None,
        })
    }

    /// Events that auto-pin on append. Identity + protocol
    /// violations are evidence; workflow checkpoints get warm-tier
    /// treatment (handled elsewhere; they don't hard-pin). Health
    /// probes auto-pin so the retention tier manager doesn't churn
    /// on them â€” the health task cleans them up directly.
    pub fn auto_pins(self) -> bool {
        matches!(
            self,
            Self::IdentityAssertion | Self::SystemProtocolViolation | Self::SystemHealthProbe
        )
    }
}

/// Retention tier. Hot/warm live in the primary SQLite; cold lives
/// in an attached archive DB, queryable only with `include_cold:
/// true` on the query (per the blueprint's explicit cold semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionTier {
    Hot,
    Warm,
    Cold,
    /// `Pinned` is orthogonal to hot/warm/cold â€” a pinned event
    /// ignores tier transitions. Stored as `tier='hot', pinned=1`
    /// on disk; `Pinned` is the semantic representation.
    Pinned,
}

impl RetentionTier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Pinned => "pinned",
        }
    }
}

/// A canonical memory log event. `id` is a ULID; `payload_hash` is
/// blake3-hex of `payload_json`; `tier` reflects storage tier at
/// write time (transitions emit their own events).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEvent {
    pub id: Ulid,
    pub timestamp_ms: i64,
    pub event_type: MemoryEventType,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Ulid>,
    /// First-class turn grouping primitive (concern 2). Stamped by
    /// the turn loop on every event it emits so
    /// `memory.log.query.by_turn` can return all events for a single
    /// turn without walking parent chains or inferring from
    /// timestamp proximity. `None` is legal for events emitted
    /// outside a turn (retention transitions, tree changes, health
    /// probes) and for events predating the introduction of this
    /// field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Ulid>,
    pub payload: serde_json::Value,
    /// Lowercase hex blake3 of canonical JSON bytes of `payload`.
    /// The log validates this on append; append fails if it doesn't
    /// match what the log computes itself.
    pub payload_hash: String,
    #[serde(default = "default_tier")]
    pub tier: RetentionTier,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub soft_deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_deleted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_deleted_reason: Option<String>,
}

fn default_tier() -> RetentionTier {
    RetentionTier::Hot
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLogFilter {
    Domain(String),
    Category(String),
    EventType(MemoryEventType),
    Actor(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryLogQueryByRange {
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub filters: Vec<MemoryLogFilter>,
    /// Must be explicit. Hot+warm is default; `true` attaches the
    /// cold archive for this query and emits a cold-access audit
    /// event. See "Cold tier access semantics" in the blueprint.
    #[serde(default)]
    pub include_cold: bool,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryLogQueryResult {
    pub events: Vec<MemoryEvent>,
    pub truncated: bool,
    pub cold_queried: bool,
}

/// Memory log health snapshot (concern 1). Counters are cumulative
/// from process start; `last_success_ms` is wall-clock ms since
/// epoch; `last_failure_reason` is the most recent append error,
/// if any. `appends_failed_last_hour` is a rolling window so short
/// outages show up even when the totals look healthy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryLogHealth {
    pub appends_attempted: u64,
    pub appends_succeeded: u64,
    pub appends_failed_last_hour: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_successful_append_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_reason: Option<String>,
    pub events_total: u64,
}

/// Startup integrity-sweep report. `passed == true` iff
/// `mismatches_found == 0`. Mismatches are counted, not collected â€”
/// a single corrupted row is a soft signal; hundreds mean something
/// structural is wrong and rescue should engage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryLogIntegrityReport {
    pub passed: bool,
    pub mismatches_found: u64,
    pub checked_count: u64,
}

/// Feedback signal (v2). Required field on its own event type so
/// the router's confidence calibration has a real data source
/// rather than implicit downstream signals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedbackSignal {
    pub target_event_id: Ulid,
    pub polarity: FeedbackPolarity,
    pub reason: String,
    pub source: FeedbackSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackPolarity {
    Positive,
    Negative,
    Corrective,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackSource {
    User,
    Automated,
    DownstreamFailure,
}

// -----------------------------------------------------------------------------
// Router (Crate 2: ordo-memory-router)
// -----------------------------------------------------------------------------

/// Retrieval semantics advertised by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalSemantics {
    Lexical,
    Dense,
    Hybrid,
    Exact,
}

/// Rough cost signal â€” the router uses this to prefer cheap
/// providers when multiple can serve a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostHint {
    Cheap,
    Moderate,
    Expensive,
}

/// Which route mode to use. `Auto` lets the router pick based on
/// fast-mode confidence threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteMode {
    Fast,
    Classify,
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderRegistration {
    /// ULID.
    pub provider_id: String,
    pub serves_paths: Vec<String>,
    pub retrieval_semantics: RetrievalSemantics,
    pub cost_hint: CostHint,
    /// `true` = provider returns provenance metadata natively.
    /// `false` = router wraps results with synthetic provenance
    /// (provider_id + timestamp + input query hash) so downstream
    /// consumers never see a provenance-less result.
    pub provenance_guarantee: bool,
    pub heartbeat_interval_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TreeNode {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_hint: Option<RetrievalSemantics>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub tombstoned: bool,
}

/// The output of a classifier LLM call, cached on the routing
/// decision event so replay never re-calls the model (DPM v2
/// extension: capture non-deterministic outputs, never recompute).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassifierOutput {
    /// Model id that produced this (e.g. `gpt-4o-2024-08-06` or
    /// `claude-3-5-sonnet-20241022`). Load-bearing: swapping the
    /// model between emit and replay would silently change
    /// behaviour.
    pub model: String,
    pub nodes: Vec<ClassifierNodeChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassifierNodeChoice {
    pub path: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteDecided {
    pub query_id: String,
    pub mode_used: RouteMode,
    pub nodes_selected: Vec<String>,
    pub providers_dispatched: Vec<String>,
    pub confidence: f32,
    /// Present iff `mode_used == Classify`. Absent for Fast routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier_output_cache: Option<ClassifierOutput>,
}

// -----------------------------------------------------------------------------
// Projection (Crate 3: ordo-memory-projection)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectionRequest {
    pub query: String,
    pub routing_decision_id: String,
    pub token_budget: u32,
    /// `None` for live projection; `Some(timestamp_ms)` for replay.
    #[serde(default)]
    pub replay_timestamp: Option<i64>,
    /// Default false â€” fail loudly if identity exceeds budget
    /// rather than silently truncating identity blocks.
    #[serde(default)]
    pub allow_identity_truncation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectionBuilt {
    pub projection_id: String,
    pub context_window: String,
    pub provenance: serde_json::Value,
    /// Blake3-hex of the context_window + deterministic inputs.
    /// Replay recomputes this and fails loudly on mismatch.
    pub output_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayDegradedReason {
    /// Cached classifier output missing; replay refuses to re-call
    /// LLM because that would silently diverge from the original.
    MissingClassifierOutput,
    HashMismatch,
    Impossible,
}

// -----------------------------------------------------------------------------
// Protocol violations (v2)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolViolationType {
    ClassifierHallucination,
    ProvenanceMissing,
    ParentReferenceInvalid,
    PayloadHashInvalid,
    HardDeleteAttempted,
    CrossTierWithoutFlag,
    IdentityOverBudget,
    ReplayDegraded,
    DuplicateEventId,
    InvalidEventId,
    // MCP security architecture (invariants 25â€“34). Every category
    // below maps to a specific enforcement point in the MCP crates.
    McpSandboxEscape,
    McpLockfileTampered,
    McpDriftUnauthorized,
    McpEgressDenied,
    McpInstructionInjection,
    McpSchemaViolation,
    McpResourceLimitExceeded,
    McpWorkerContamination,
    McpPrivilegeTierViolation,
    McpProvenanceTaintViolation,
    McpSensitiveActionBlocked,
    McpCapabilityWideningAttempted,
    McpDpopReplay,
    McpAuthDegraded,
    McpNonWasmBinary,
    McpSignatureInvalid,
    McpAttestationInvalid,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Error,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtocolViolation {
    pub violation_type: ProtocolViolationType,
    /// Offending message / event id, when identifiable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offending_id: Option<String>,
    pub details: String,
    pub severity: Severity,
}

// -----------------------------------------------------------------------------
// Topics â€” single source of truth for bus routing. Match on these
// string constants instead of inlining topic names at callsites.
// -----------------------------------------------------------------------------

pub mod memory_topics {
    // Log
    pub const LOG_APPEND_REQUEST: &str = "ordo.memory.log.append.request";
    pub const LOG_APPEND_RESPONSE: &str = "ordo.memory.log.append.response";
    pub const LOG_APPENDED: &str = "ordo.memory.log.appended";
    pub const LOG_QUERY_REQUEST: &str = "ordo.memory.log.query.request";
    pub const LOG_QUERY_RESPONSE: &str = "ordo.memory.log.query.response";
    pub const LOG_COLD_QUERY: &str = "ordo.memory.log.cold_query";
    pub const LOG_RETENTION_TRANSITION: &str = "ordo.memory.log.retention";
    // Health (concern 1)
    pub const LOG_HEALTH_REQUEST: &str = "ordo.memory.log.health.request";
    pub const LOG_HEALTH_RESPONSE: &str = "ordo.memory.log.health.response";
    pub const LOG_HEALTH_OK: &str = "ordo.memory.log.health.ok";
    pub const LOG_HEALTH_DEGRADED: &str = "ordo.memory.log.health.degraded";
    pub const LOG_INTEGRITY_RESULT: &str = "ordo.memory.log.integrity.result";
    // Turn grouping (concern 2)
    pub const LOG_QUERY_BY_TURN_REQUEST: &str = "ordo.memory.log.query.by_turn.request";
    pub const LOG_QUERY_BY_TURN_RESPONSE: &str = "ordo.memory.log.query.by_turn.response";

    // Router
    pub const ROUTE_REQUEST: &str = "ordo.memory.route.request";
    pub const ROUTE_RESPONSE: &str = "ordo.memory.route.response";
    pub const ROUTE_DECIDED: &str = "ordo.memory.route.decided";
    pub const ROUTE_LOW_CONFIDENCE: &str = "ordo.memory.route.low_confidence_classify";
    pub const PROVIDER_REGISTER: &str = "ordo.memory.provider.register";
    pub const PROVIDER_DEREGISTER: &str = "ordo.memory.provider.deregister";
    pub const PROVIDER_HEARTBEAT: &str = "ordo.memory.provider.heartbeat";
    pub const TREE_CHANGE: &str = "ordo.memory.tree.change";

    // Projection
    pub const PROJECTION_BUILD_REQUEST: &str = "ordo.memory.projection.build.request";
    pub const PROJECTION_BUILD_RESPONSE: &str = "ordo.memory.projection.build.response";
    pub const PROJECTION_BUILT: &str = "ordo.memory.projection.built";
    pub const PROJECTION_IDENTITY_OVER_BUDGET: &str = "ordo.memory.projection.identity_over_budget";
    pub const PROJECTION_REPLAY_DEGRADED: &str = "ordo.memory.projection.replay_degraded";

    // Feedback
    pub const FEEDBACK_SIGNAL: &str = "ordo.memory.feedback.signal";

    // Protocol violations
    pub const PROTOCOL_VIOLATION: &str = "ordo.memory.protocol_violation";
}
