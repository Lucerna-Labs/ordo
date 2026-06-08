//! Connections â€” operator-facing layer over the secrets vault +
//! per-type test handlers.
//!
//! The operator opens the studio's Connections tab, clicks a tile
//! ("OpenAI", "SSH", "generic API", ...), fills in a form whose shape
//! comes from this crate's `ConnectionType` registry, and saves.
//! Behind the scenes:
//!
//!   1. The non-secret config (handle, site URL, etc.) lands in the
//!      `connections` SQLite row as `fields_json`.
//!   2. The secret material (app password, API key, SSH password,
//!      private-key PEM) lands in `ordo-secrets-vault`, which uses
//!      the same OS-keychain-backed sealing the rest of the
//!      platform relies on. The connection row holds only the
//!      vault row's `id` â€” the secret bytes never sit alongside
//!      the metadata.
//!   3. The platform automatically runs `test_connection` against
//!      the live destination's API. The result lands in `status` +
//!      `status_detail` so the studio can show a green check or
//!      the actual error.
//!
//! Why this is a separate crate from `ordo-secrets-vault`:
//!   - The vault is a generic sealed-storage primitive.
//!   - Connections are the operator concept on top â€” friendly
//!     name, type-specific fields, test handler.
//!   - A future revision could persist connections to a
//!     synced backing store, leave the vault local-only.

pub mod service;
pub mod store;
pub mod testers;
pub mod types;

pub use service::{ConnectionService, ConnectionServiceError};
pub use store::{ConnectionRow, ConnectionStatus, ConnectionStore, ConnectionStoreError};
pub use types::{
    catalog, ConnectionType, ConnectionTypeId, FieldSchema, FieldType, TestReport, TestStatus,
};
