//! ordo-modes — mode-scoped workspaces for the assistant.
//!
//! ## What a mode is
//!
//! A mode is a bounded operating environment for the assistant.
//! When a session is created against mode X, every subsequent turn
//! in that session sees only:
//!
//!   - Memory facts whose `scope` matches the mode's `memory_scope`
//!     list (always includes `"global"` plus `"mode:<id>"`)
//!   - RAG collections in the mode's `rag_domains` list
//!   - Tools whose lane matches one of the mode's `allowed_tool_lanes`
//!     and isn't in `blocked_tool_capabilities`
//!   - The mode's planner bias and persona appended to the bootstrap
//!     prompt
//!
//! Cross-mode access exists but is **explicit, scoped, classifier-
//! gated, and logged** — not silent merging. See `cross_mode.rs`
//! (later step).
//!
//! ## What a mode is NOT
//!
//! - Not a persona toggle. Persona text is one thing the manifest
//!   declares, but the load-bearing change is the **scope** —
//!   memory partition, RAG partition, tool allowlist, policy set.
//! - Not mid-conversation switchable. Mode is **fixed at session
//!   creation**. Switching modes in the UXI = "open a new chat in
//!   mode X." This is the architectural answer to the spec's
//!   containment promise; mid-conversation switching would leave
//!   prior-mode memory and turns sitting in the conversation
//!   history of a now-supposedly-different mode.
//! - Not hardcoded into the assistant prompt. The assistant code
//!   path is one and the same; the mode parameterizes its inputs.
//!
//! ## What's in this crate
//!
//! - [`ModeManifest`] — the typed config (this file's import)
//! - [`ModeRegistry`] — loads compiled-in defaults + on-disk
//!   overrides at `<runtime>/user-files/modes/*.json`, hands out
//!   manifests by id
//! - [`defaults`] — the 6 default modes shipped compiled in
//!
//! Future steps add: cross-mode borrow gate, mode telemetry events,
//! RAG domain scoping in the retrieval layer, mode advanced-view
//! exposure on the control API.

pub mod audit;
pub mod defaults;
pub mod manifest;
pub mod registry;

pub use audit::{audit_skill_routing, RoutingAnomaly, RoutingAudit, SkillRoutingHealth};
pub use manifest::{slugify_mode_id, ModeManifest, ModeManifestError, SkillDecision};
pub use registry::{ModeMutationError, ModeRegistry, ModeRegistryError, RegistryStats};

/// Canonical id for the General Assistant mode. The runtime defaults
/// new sessions to this when no explicit mode is requested.
pub const DEFAULT_MODE_ID: &str = "general";
