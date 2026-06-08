//! Ordo apps primitive.
//!
//! An **app** is a persisted, lifecycle-managed artifact â€” the base44
//! analog â€” that bundles a UI extension, plugin config, RAG corpus,
//! and assistant sessions under one addressable thing. Apps move
//! through `draft â†’ published â†’ archived` and every mutation appends
//! to an event log (Phase 1.1 ships the log; Phase 1.2 layers rewind
//! on top).
//!
//! See `docs/architecture-contract.md` â€” this crate follows Rule 2
//! (capability provider is the extension point), Rule 6 (single
//! SQLite, workspace_id from day one), Rule 7 (builder pattern),
//! and Rule 11 (wire types live in `ordo-protocol`).

pub mod provider;
pub mod service;
pub mod store;
pub mod types;

pub use provider::AppsProvider;
pub use service::{AppsService, DEFAULT_WORKSPACE_ID};
pub use store::{AppsStore, Mutation};
pub use types::{AppRef, AppUpdate, AppsError, AppsQuery, AppsResult, NewApp};
