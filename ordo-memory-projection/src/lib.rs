//! `ordo-memory-projection` â€” Crate 3 of the hierarchical memory
//! architecture. Given a query + a routing decision + retrieved
//! results, produce a DETERMINISTIC context window for the LLM.
//!
//! The projection pattern (DPM â€” Deterministic Projection Memory):
//!
//! ```text
//! (query, event_log_slice, tree_state, routing_decision,
//!  retrieved_results, pin_set)  â†’  context_window
//! ```
//!
//! Same inputs always produce byte-identical output. This is the
//! auditability guarantee and the replay guarantee.
//!
//! Blueprint invariants this crate enforces:
//!   - Identity assertions exceeding budget FAIL LOUDLY, not
//!     silently truncate (unless the caller opts in).
//!   - Token budget is respected exactly.
//!   - Retrieved items without provenance are DROPPED + emit a
//!     protocol violation.
//!   - Replay of a Classify-mode projection uses the CACHED
//!     classifier output, never recalls the LLM. Missing cache â†’
//!     `replay.degraded`.
//!   - Output hash is computed from deterministic inputs so replay
//!     can verify.

pub mod service;
pub mod types;

pub use service::{MemoryProjectionError, MemoryProjectionResult, MemoryProjectionService};
pub use types::{Budget, BuildInputs, RetrievedItem};
