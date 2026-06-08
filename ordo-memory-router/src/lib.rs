//! `ordo-memory-router` â€” Crate 2 of the hierarchical memory
//! architecture (blueprint v2).
//!
//! Given a query, decide which providers to invoke. Owns:
//!   - the memory tree (mutable at runtime; tombstone-based
//!     soft-delete so replay can reconstruct past structure)
//!   - provider registry wiring (delegates to `ordo_bus::ProviderRegistry`)
//!   - routing decisions (fast deterministic mode + classify mode
//!     with injectable LLM classifier)
//!
//! Does NOT own retrieval execution or context assembly â€” providers
//! execute; projection assembles.
//!
//! Architecture contract compliance:
//!   - Rule 1 (pub/sub bus): scatter-gather uses `BusCorrelator`
//!     layered helpers, not a new `Bus` trait method.
//!   - Rule 2 (CapabilityProvider): exposed by the runtime as
//!     `MemoryRouterProvider` (thin wrapper).
//!   - Rule 11 (protocol): wire types in `ordo-protocol::memory`.

pub mod classifier;
pub mod service;
pub mod tree;

pub use classifier::{Classifier, ClassifyOutput, ScriptedClassifier};
pub use service::{MemoryRouterError, MemoryRouterResult, MemoryRouterService, RouteOutcome};
pub use tree::{TreeStore, TreeStoreError};
