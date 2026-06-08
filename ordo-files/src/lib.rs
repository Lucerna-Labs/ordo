//! Ordo files primitive (Phase 1.4).
//!
//! Metadata-in-SQLite + bytes-on-disk hybrid. The service owns:
//!   - a filesystem root (`user_files/`) where bytes live under
//!     `<workspace_id>/<file_id>/<safe_name>`;
//!   - a SQLite `files` table with the metadata the platform queries
//!     frequently (size, content_type, sha256, timestamps).
//!
//! Follows the architecture contract: Rule 2 (CapabilityProvider is
//! the extension point), Rule 6 (workspace_id from day one), Rule 7
//! (builder pattern for service construction), Rule 11 (wire types in
//! ordo-protocol with CHANGELOG entry).

pub mod provider;
pub mod service;
pub mod store;
pub mod types;

pub use provider::FilesProvider;
pub use service::{FilesService, DEFAULT_WORKSPACE_ID};
pub use store::FilesStore;
pub use types::{FilesError, FilesQuery, FilesResult, NewUpload};
