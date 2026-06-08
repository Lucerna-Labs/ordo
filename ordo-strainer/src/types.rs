//! Public types exposed at the crate boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type StrainResult<T> = Result<T, StrainError>;

#[derive(Debug, Error)]
pub enum StrainError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Indicates the HTML parser bailed entirely. Shouldn't happen
    /// with html5ever (which is permissive) but surfaces cleanly if
    /// it ever does.
    #[error("parse failure: {0}")]
    Parse(String),

    #[error("internal: {0}")]
    Internal(String),
}

/// What the strainer emits. The wrapped string is what goes into
/// the assistant's context; everything else is metadata for
/// telemetry, audit, and the future taint-propagation layer.
///
/// `source` is the origin URL; the runtime reads this when minting
/// a `Taint::UntrustedWeb` event on a conversation that ingests
/// strained content (Stage 5 wiring, in `ordo-mcp-provenance`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrainedContent {
    /// Boundary-wrapped markdown — what enters the prompt. Includes
    /// the `<untrusted_web_content>` opening tag, the markdown body,
    /// and the closing tag. The system prompt rule (paired into
    /// [`ordo-assistant`'s bootstrap prompt]) makes the LLM treat
    /// this region as data, not instructions.
    pub wrapped: String,

    /// Same body without the boundary tags — useful for telemetry,
    /// rendering previews in the UI, or feeding into RAG indexing
    /// where the boundary tags would be noise.
    pub markdown: String,

    /// Source URL the content came from. Recorded verbatim in the
    /// boundary wrapper.
    pub source: String,

    /// When the fetch happened (caller's clock). Recorded in the
    /// boundary wrapper for audit.
    pub fetched_at: DateTime<Utc>,

    /// SHA-256 of the markdown body. Useful for dedupe and audit
    /// trail. Do NOT use as a trust signal — content with a known
    /// hash is still untrusted.
    pub sha256: String,
}
