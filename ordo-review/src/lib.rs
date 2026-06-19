//! Human-in-the-loop review surface for Ordo.
//!
//! When a capability produces content Ordo wants to ship â€” a
//! runtime change, model configuration, or generated plan, the
//! operator should be in the loop before it becomes real. This crate
//! is that loop:
//!
//! - `ReviewService` â€” in-memory orchestrator: SQLite-backed queue,
//!   per-id oneshot waiters, broadcast events for the WebSocket.
//! - `ReviewProvider` â€” `capabilities`-bus facade so agents call
//!   `review.request_approval` like any other tool.
//! - `ReviewEvent` â€” the serde-serialisable stream the studio
//!   subscribes to via `/ws/review`.
//!
//! Persistence lives in `data/ordo.db` under the `review_requests`
//! table, so a pending request survives a runtime restart; the
//! operator picks up where they left off.

pub mod service;
pub mod store;
pub mod types;

pub use service::ReviewService;
pub use store::ReviewStore;
pub use types::{
    NewReviewRequest, ReviewDecisionKind, ReviewError, ReviewEvent, ReviewRequest, ReviewResult,
    ReviewState,
};
