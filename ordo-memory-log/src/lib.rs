//! `ordo-memory-log` â€” the append-only event log (Crate 1 of the
//! hierarchical memory architecture). Source of truth for the DPM
//! substrate: every decision the platform makes can be reconstructed
//! by replaying events up to the decision point.
//!
//! What this crate owns:
//!   - event ingestion (append-only, idempotent on payload_hash
//!     within a dedupe window)
//!   - storage (SQLite via `ordo-store`, hot + warm tiers in-line;
//!     cold tier lives in a separate archive DB attached on demand)
//!   - retrieval by id / range / parent
//!   - tier transitions + pin / unpin / soft-delete
//!   - bus event emission â€” every write broadcasts
//!     `ordo.memory.log.appended`
//!
//! What this crate does NOT own:
//!   - interpretation of payloads (that's projection)
//!   - routing decisions (that's the router)
//!   - embedding (that's the assistant / providers)
//!
//! Architectural rules honored (see docs/architecture-contract.md):
//!   - Rule 6: one SQLite, migrations in `ordo-store`;
//!     `workspace_id` from day one.
//!   - Rule 10: parking_lot::Mutex around the store handle; never
//!     held across an `await`.
//!   - Rule 11: wire types live in `ordo-protocol::memory`.

pub mod health;
pub mod service;
pub mod store;

pub use health::{MemoryLogHealthTask, DEFAULT_PROBE_INTERVAL_SECS};
pub use service::{AppendResult, MemoryLogError, MemoryLogResult, MemoryLogService, CANARY_TTL_MS};
pub use store::MemoryLogStore;
