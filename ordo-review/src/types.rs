//! Shared types for the review surface.
//!
//! These travel across the bus, the control-API JSON, and the WebSocket
//! event stream â€” so they are `serde`-serialisable and purposefully
//! minimal.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle of a review request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    /// Still in the queue, waiting for operator action.
    Open,
    /// Operator approved without edits.
    Approved,
    /// Operator approved after editing the content in place.
    EditedAndApproved,
    /// Operator explicitly declined the artifact.
    Denied,
    /// The waiter gave up (timeout) before a decision arrived.
    Expired,
}

impl ReviewState {
    pub fn label(self) -> &'static str {
        match self {
            ReviewState::Open => "open",
            ReviewState::Approved => "approved",
            ReviewState::EditedAndApproved => "edited_and_approved",
            ReviewState::Denied => "denied",
            ReviewState::Expired => "expired",
        }
    }

    pub fn from_label(label: &str) -> Option<ReviewState> {
        match label {
            "open" => Some(ReviewState::Open),
            "approved" => Some(ReviewState::Approved),
            "edited_and_approved" => Some(ReviewState::EditedAndApproved),
            "denied" => Some(ReviewState::Denied),
            "expired" => Some(ReviewState::Expired),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, ReviewState::Open)
    }
}

/// A single queued review request â€” an artifact Ordo produced
/// that wants operator sign-off.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    /// Capability that queued this request (e.g. `orchestration.draft_notes`).
    pub origin_capability: String,
    /// Optional plugin name when the producer was an external plugin.
    pub origin_plugin: Option<String>,
    pub title: String,
    /// MIME-style content type. The studio switches the preview renderer
    /// on this value. Accepted today: `text/markdown`, `text/plain`,
    /// `text/html`, `application/json`, `image/png`, `image/jpeg`,
    /// `image/svg+xml`.
    pub content_type: String,
    /// Inline content. For images this can be a data URL.
    pub content: String,
    /// Free-form metadata the caller wants to show in the review panel
    /// (source prompt, model used, RAG hits, etc.).
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    pub state: ReviewState,
    /// Populated when the operator edited the content before approving.
    #[serde(default)]
    pub edited_content: Option<String>,
    /// Operator note recorded alongside the decision.
    #[serde(default)]
    pub decision_note: Option<String>,
}

impl ReviewRequest {
    /// Content the downstream agent should act on â€” the edited version
    /// if the operator rewrote it, otherwise the original draft.
    pub fn effective_content(&self) -> &str {
        self.edited_content
            .as_deref()
            .unwrap_or(self.content.as_str())
    }
}

/// Fields supplied when submitting a new request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewReviewRequest {
    pub origin_capability: String,
    #[serde(default)]
    pub origin_plugin: Option<String>,
    pub title: String,
    pub content_type: String,
    pub content: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Outcome an operator records against a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewDecisionKind {
    Approve {
        #[serde(default)]
        note: Option<String>,
    },
    Deny {
        #[serde(default)]
        note: Option<String>,
    },
    Edit {
        content: String,
        #[serde(default)]
        note: Option<String>,
    },
    /// Server-side: a waiter timed out.
    Expire,
}

/// Broadcast events pushed over the review WebSocket.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ReviewEvent {
    /// A new request joined the queue.
    Opened { request: ReviewRequest },
    /// An existing request reached a terminal state.
    Resolved { request: ReviewRequest },
    /// Full snapshot of the pending queue â€” sent on WS connect so the
    /// studio can catch up without polling.
    QueueSnapshot {
        pending: Vec<ReviewRequest>,
        total: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("review request '{0}' not found")]
    NotFound(Uuid),
    #[error("review request '{0}' is already resolved (state: {1})")]
    AlreadyResolved(Uuid, &'static str),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("local storage error: {0}")]
    Storage(String),
}

pub type ReviewResult<T> = Result<T, ReviewError>;
