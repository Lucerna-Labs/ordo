//! Ordo Logic — core reasoning module.
//!
//! Logic is a runtime property, not a domain lane. It sits next to
//! `ordo-memory`, `ordo-planner`, `ordo-router` rather than next to
//! product-domain crates. The planner uses it to
//! validate plans, the recall layer uses it to find contradictions in
//! the fact store, the assistant exposes a subset of its capabilities
//! directly to the operator (Skills tab → `logic.*`), and a future
//! `logic-mcp` SAT/SMT external server can extend it without changing
//! the runtime binary.
//!
//! ## Architecture
//!
//! - [`LogicProvider`] is the loose-coupling seam. Every internal
//!   caller depends on this trait, never on a concrete impl. That
//!   lets us swap implementations (LLM-only, MCP-backed, hybrid)
//!   without touching call sites.
//! - [`LlmLogicProvider`] is the default. Each capability is a
//!   structured prompt against the configured LLM, with the response
//!   parsed back into typed Rust structs. No native deps, no model
//!   files — costs single-digit MB on the runtime binary.
//! - [`LogicCapabilityProvider`] wraps a [`LogicProvider`] as a
//!   `ordo-mcp-host::CapabilityProvider`-compatible surface (via
//!   `descriptors` + `invoke`). The runtime adapter in
//!   `ordo-mcp-host::LogicCapabilityAdapter` bridges the two so the
//!   bus + assistant tool gateway pick logic capabilities up like
//!   any other provider.
//!
//! ## What lives here, what lives elsewhere
//!
//! In: argument analysis, fallacy detection, premise validation,
//! steel-manning, decomposition, contradiction-finding, assumption
//! audit. All LLM-shaped — no native deps, no solver.
//!
//! Out: SAT / SMT / formal proof. That goes in a separate `logic-mcp`
//! binary the operator opts into via the MCP tab. The runtime stays
//! provider-neutral and dep-light.

pub mod capabilities;
pub mod fol;
pub mod hybrid;
pub mod llm;
pub mod prompts;
pub mod propositional;
pub mod provider;
pub mod types;

pub use capabilities::{
    capability_descriptors, invoke_capability, LOGIC_CLASSIFY_CLAIM_DOMAIN, LOGIC_FIND_FALLACIES,
    LOGIC_IDENTIFY_CLAIMS, LOGIC_STEEL_MAN, LOGIC_VALIDATE_CHAIN,
};
pub use hybrid::HybridLogicProvider;
pub use llm::LlmLogicProvider;
pub use provider::LogicProvider;
pub use types::{
    Certainty, ChainValidation, Claim, ClaimClassification, ClaimStakes, Contradiction, Fallacy,
    LogicError, LogicResult,
};
