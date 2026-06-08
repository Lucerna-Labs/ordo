//! `LogicProvider` — the loose-coupling seam.
//!
//! Every internal caller (planner, recall, assistant tool gateway)
//! depends on this trait, never on a concrete impl. That lets us
//! swap `LlmLogicProvider` for an MCP-backed proxy or a hybrid
//! resolver later without touching call sites.
//!
//! Method signatures intentionally take borrowed inputs and return
//! owned outputs — no fancy lifetime juggling at the seam, easy to
//! mock in tests, easy to wrap for capability-level adapters.

use async_trait::async_trait;

use crate::types::{ChainValidation, Claim, ClaimClassification, Fallacy, LogicResult};

#[async_trait]
pub trait LogicProvider: Send + Sync {
    /// Identify the explicit claims a passage makes. Returns one
    /// entry per distinct assertion. Does not infer claims the
    /// passage doesn't actually state — that's `assumption_audit`.
    async fn identify_claims(&self, text: &str) -> LogicResult<Vec<Claim>>;

    /// Find logical fallacies in an argument. Empty Vec is a normal
    /// outcome (clean argument); never returns an error just because
    /// nothing was found.
    async fn find_fallacies(&self, argument: &str) -> LogicResult<Vec<Fallacy>>;

    /// Validate that a conclusion follows from a set of premises.
    /// On `holds: false`, `gaps` lists what's missing — the planner
    /// uses this to know whether to gather more evidence before
    /// committing.
    async fn validate_chain(
        &self,
        premises: &[String],
        conclusion: &str,
    ) -> LogicResult<ChainValidation>;

    /// Steel-man — return the strongest, most charitable version of
    /// the input argument. Useful for debate prep, draft review, and
    /// for the planner when it's about to push back on operator
    /// input (better to argue against the strong form).
    async fn steel_man(&self, argument: &str) -> LogicResult<String>;

    /// Classify the domain + stakes of a claim (Phase C Layer 1 of
    /// the Grounding Floor). Returns the domain tags, a stakes
    /// bucket, and whether the operator should require an
    /// authoritative source before acting on it.
    ///
    /// The intended usage flow (paired system prompt rule):
    ///
    ///   1. Assistant is about to commit to a factual claim.
    ///   2. Calls `logic.classify_claim_domain(claim)`.
    ///   3. If `requires_authoritative_source` is true and the
    ///      conversation's only source is strained web content,
    ///      hedge the wording ("the article asserts...") or decline
    ///      and ask the operator for an authoritative source.
    ///
    /// Default impl on [`LlmLogicProvider`] uses the LLM with a
    /// structured-output prompt.
    async fn classify_claim_domain(&self, claim: &str) -> LogicResult<ClaimClassification>;
}
