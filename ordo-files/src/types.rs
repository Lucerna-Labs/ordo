//! Local types for the files service. The wire-shared `FileEntry`
//! lives in `ordo-protocol`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Input for `FilesService::upload`. Bytes are kept out of this
/// struct so the caller can stream them without double-buffering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewUpload {
    pub original_name: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub app_id: Option<Uuid>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilesQuery {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub app_id: Option<Uuid>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error("file '{0}' not found")]
    NotFound(Uuid),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("storage root is outside the runtime's user_files/: {0}")]
    StorageEscape(String),
    #[error("local storage error: {0}")]
    Storage(String),
    #[error("disk i/o: {0}")]
    Io(String),
}

pub type FilesResult<T> = Result<T, FilesError>;
