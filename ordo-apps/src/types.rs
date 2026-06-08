//! Local types for the apps service.
//!
//! Wire-shared types (`App`, `AppStatus`, `AppEvent`, `AppEventKind`)
//! live in `ordo-protocol` per Rule 11 â€” this file carries only the
//! crate-local request/error shapes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Input for creating a new app. `slug` is optional â€” when omitted the
/// service derives one from `name`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewApp {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

/// Partial-update payload. Any `Some` field is applied; `None` leaves
/// the existing value untouched. Use dedicated endpoints for status
/// transitions (publish/archive) so the provider can route them
/// through review.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Sparse metadata patch: keys with `Some(Value)` get set, keys
    /// with `Some(Value::Null)` get removed, missing keys are
    /// untouched.
    #[serde(default)]
    pub metadata_patch: BTreeMap<String, Value>,
    /// Actor label recorded on generated events. Defaults to
    /// `"operator"` at the service layer when unset.
    #[serde(default)]
    pub actor: Option<String>,
}

/// Query filter for `list_apps`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppsQuery {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub status: Option<ordo_protocol::AppStatus>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Identity used to address an app across API surfaces. Accepting
/// either a UUID or a `(workspace, slug)` pair keeps the MCP bridge
/// tool surface friendly â€” agents usually have a slug, not a UUID.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AppRef {
    Id(Uuid),
    Slug { workspace_id: String, slug: String },
}

#[derive(Debug, thiserror::Error)]
pub enum AppsError {
    #[error("app '{0}' not found")]
    NotFound(String),
    #[error("slug '{slug}' already taken in workspace '{workspace}'")]
    SlugConflict { workspace: String, slug: String },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("invalid status transition: {from} \u{2192} {to}")]
    InvalidTransition {
        from: &'static str,
        to: &'static str,
    },
    #[error("local storage error: {0}")]
    Storage(String),
}

pub type AppsResult<T> = Result<T, AppsError>;
