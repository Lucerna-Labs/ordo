use ordo_protocol::{MemoryEvent, RouteDecided, TreeNode};
use serde::{Deserialize, Serialize};

/// A single retrieved item from a provider. Provenance is REQUIRED
/// â€” without it, the projection drops the item and emits a
/// `ProvenanceMissing` violation (blueprint constitution: "there is
/// never a provenance-less result in the pipeline").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievedItem {
    pub provider_id: String,
    pub score: f32,
    pub text: String,
    /// Arbitrary provenance metadata. The presence of a non-null
    /// value is what the projection checks; semantics are provider-
    /// specific.
    pub provenance: serde_json::Value,
}

/// Token-like budget. We approximate tokens as `chars / 4`
/// (OpenAI-ish) so the crate doesn't need a tokenizer dep. Callers
/// that need strict counts pass a smaller budget.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_tokens: u32,
}

impl Budget {
    pub fn tokens_for(text: &str) -> u32 {
        // Safe-low approximation: 4 chars â‰ˆ 1 token for prose.
        // Underestimates for code / foreign languages but never
        // overcommits budget.
        ((text.chars().count() as f32) / 4.0).ceil() as u32
    }
}

/// Everything the projection needs to build a context window. All
/// inputs are explicit so the function stays pure.
#[derive(Debug, Clone)]
pub struct BuildInputs {
    pub query: String,
    pub routing_decision: RouteDecided,
    pub tree_state: Vec<TreeNode>,
    pub pinned_events: Vec<MemoryEvent>,
    pub recent_events: Vec<MemoryEvent>,
    pub retrieved: Vec<RetrievedItem>,
    pub budget: Budget,
    pub allow_identity_truncation: bool,
    pub replay_timestamp_ms: Option<i64>,
}
