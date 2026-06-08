//! Shared types — claims, fallacies, validation outcomes, errors.
//!
//! Wire format: every type is `Serialize + Deserialize` so the bus
//! adapter can return them directly as JSON to `/api/tools/logic.*`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type LogicResult<T> = Result<T, LogicError>;

#[derive(Debug, Error)]
pub enum LogicError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// LLM upstream returned an error or the response couldn't be
    /// parsed into the expected shape. Carries a short reason so
    /// callers (and operators reading the audit log) can tell whether
    /// to retry, switch providers, or rephrase the input.
    #[error("LLM call failed: {0}")]
    LlmFailed(String),

    /// No cloud credential satisfied the call. The assistant pattern
    /// already hints toward the Cloud tab; we mirror its language so
    /// the operator-facing messages stay consistent.
    #[error("no cloud credential configured for logic; configure one in the Cloud tab")]
    NoCredential,

    #[error("internal error: {0}")]
    Internal(String),
}

/// A single explicit claim extracted from a passage. `weight` is a
/// 0..1 confidence the LLM assigns based on how directly the passage
/// asserts it (vs. implies it). `support` is verbatim spans from the
/// source that anchor the claim — operator can audit attribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Claim {
    pub statement: String,
    #[serde(default)]
    pub weight: f32,
    #[serde(default)]
    pub support: Vec<String>,
}

/// A logical fallacy found in an argument. `kind` is a free-form
/// label ("ad hominem", "straw man", "false dichotomy", …) — we don't
/// constrain to an enum because new categories arrive as the field
/// evolves and the LLM's labeling is what operators read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fallacy {
    pub kind: String,
    pub explanation: String,
    /// Verbatim quote from the input where the fallacy sits.
    #[serde(default)]
    pub quote: String,
    #[serde(default)]
    pub severity: FallacySeverity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FallacySeverity {
    /// Stylistic — argument still holds.
    Minor,
    /// Materially weakens but doesn't sink the argument.
    #[default]
    Moderate,
    /// Argument cannot be salvaged without addressing this.
    Critical,
}

/// How confident the validation result is. The architectural point of
/// this field: tell the operator (and the planner) whether they have a
/// proof or an opinion. Pure-LLM analysis returns `Rhetorical`; a
/// formal SAT proof returns `Formal`; an attempt that couldn't even
/// formalize cleanly returns `Unknown` so callers know to ask the
/// operator for clarification rather than treat the answer as gospel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Certainty {
    /// Mechanically proved (truth-table or SAT). Trust deterministically.
    Formal,
    /// LLM judgment with no formal underpinning. Reasonable but
    /// fallible — cite as "the assistant thinks", not "the assistant
    /// proved".
    #[default]
    Rhetorical,
    /// Could not assess — formalization failed and the LLM declined
    /// to commit to a rhetorical read either. Caller should rephrase
    /// or ask the operator.
    Unknown,
}

/// Outcome of `validate_chain`. `holds` says whether the conclusion
/// follows from the premises under standard rules; `gaps` lists
/// missing premises the chain would need to be valid; `notes` carries
/// any nuance worth surfacing (modal scope, equivocation, …);
/// `certainty` says whether `holds` is a formal proof or a rhetorical
/// judgment — the signal the planner reads to know whether to cite
/// or persuade.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChainValidation {
    pub holds: bool,
    #[serde(default)]
    pub gaps: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub certainty: Certainty,
}

/// Two facts (or claims) that contradict, with a one-line reason
/// the LLM gives for why they conflict. Used by future planner /
/// recall integration to flag inconsistent state before it spreads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Contradiction {
    pub a: String,
    pub b: String,
    pub reason: String,
}

/// Domain classification for `logic.classify_claim_domain` (Phase C
/// Layer 1). The LLM tags a claim with one or more domains, a
/// stakes level, and whether the operator should require an
/// authoritative source before the assistant commits to it.
///
/// The system prompt rule paired with this capability tells the
/// assistant: when a claim's `requires_authoritative_source` is
/// true and the only source in the conversation is strained web
/// content, hedge the wording or decline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaimClassification {
    /// Zero or more domain tags. Free-form strings — the doc's
    /// recommended set is `legal`, `medical`, `financial`,
    /// `safety`, `scientific_consensus`, `general`. We don't
    /// constrain to an enum because new high-stakes categories
    /// (privacy law, election rules, …) arrive faster than schema
    /// changes should.
    pub domains: Vec<String>,

    /// Operator-facing stakes label. Three buckets that map cleanly
    /// to "would acting on this be revocable?".
    #[serde(default)]
    pub stakes: ClaimStakes,

    /// True when the claim falls in a domain where the operator
    /// should not act on bare untrusted-web content. Layer-1
    /// hedging language fires on this flag.
    #[serde(default)]
    pub requires_authoritative_source: bool,

    /// One-sentence rationale the LLM gives for the classification.
    /// Surfaces in the studio so the operator can see WHY a claim
    /// got flagged.
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClaimStakes {
    /// Acting on a wrong belief here is cheaply reversible.
    #[default]
    Low,
    /// Acting on a wrong belief here costs time, money, or
    /// reputation but is recoverable.
    Medium,
    /// Acting on a wrong belief here causes irreversible harm —
    /// legal liability, medical injury, financial ruin, safety
    /// incident.
    High,
}
