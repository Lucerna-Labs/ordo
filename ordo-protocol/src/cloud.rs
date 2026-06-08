//! Cloud provider credential types and topics.
//!
//! Two structs:
//!
//! - [`CloudCredentialView`] — the public, secret-omitted view
//!   broadcast on list responses and change events. Anyone
//!   subscribing to cloud topics sees the credential exists,
//!   what auth style it uses, and its operator-set config — but
//!   never the secret.
//! - [`CloudCredentialFull`] — write-side; carries the secret in
//!   plaintext for upsert requests. Lives on the in-process bus
//!   only (no IPC, no serialization across trust boundaries).
//!   The existing `ordo-cloud` vault encrypts on persist.
//!
//! The matching [`cloud_topics`] module declares the request /
//! response / event topic strings. Naming follows the existing
//! `ordo.<area>.<thing>.<verb>` convention used by
//! [`crate::memory_topics`], [`crate::secrets_topics`], and
//! [`crate::mcp_topics`].
//!
//! Nothing in this file publishes or subscribes. The bridge in
//! `ordo-cloud` (Cycle 3 of the Cloud-tab work) is the
//! publisher; the studio Cloud/Provider tab is the consumer.
//! Until those land, the topics defined here are dormant.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Public view of a configured cloud credential. The `secret`
/// field of `ordo-cloud::CloudCredential` is intentionally
/// **not** present — anything that ends up on a list response or
/// change event uses this view so subscribers never see plaintext
/// secrets.
///
/// The `extras` map carries operator-set per-provider config
/// (`model`, `context_window`, `temperature`, ...). The bridge
/// is responsible for redacting any secret-looking keys to
/// `"***"` before constructing a view — same policy as the
/// existing `ordo-cloud::CloudCredential::redacted` helper.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudCredentialView {
    /// Service key — `"openai"`, `"anthropic"`, `"ollama"`, etc.
    /// Unique within the store; used as the identity for
    /// upsert / remove / set-default.
    pub service: String,
    /// Operator-chosen display name.
    pub label: String,
    /// Authentication scheme: `"bearer"`, `"basic"`,
    /// `"api_key_header"`, `"api_key_query"`, `"anthropic"`.
    pub auth_style: String,
    /// Custom base URL when the provider isn't at the default
    /// endpoint (Ollama on localhost, an OpenAI-compatible
    /// proxy, etc.).
    pub base_url: Option<String>,
    /// Operator-set per-provider config. Secret-looking keys
    /// arrive already redacted to `"***"`.
    pub extras: HashMap<String, String>,
    /// ISO-8601 timestamps from the underlying store.
    pub created_at: String,
    pub updated_at: String,
}

/// Write-side credential. Carries the secret in plaintext for
/// upsert requests. **In-process only** — never crosses a
/// process or network boundary; the existing `ordo-cloud` vault
/// encrypts the secret before it touches disk.
///
/// Timestamps are not on this struct because the store computes
/// them on insert / update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudCredentialFull {
    pub service: String,
    pub label: String,
    pub auth_style: String,
    /// Plaintext credential. Encrypted by the vault on persist;
    /// never echoed back in a [`CloudCredentialView`].
    pub secret: String,
    pub base_url: Option<String>,
    pub extras: HashMap<String, String>,
}

/// Bus topics for cloud-credential operations. Request /
/// response and request / event pairs follow the same pattern
/// as the memory and secrets CRUD topics.
pub mod cloud_topics {
    pub const CREDENTIALS_LIST_REQUEST: &str = "ordo.cloud.credentials.list.request";
    pub const CREDENTIALS_LIST_RESPONSE: &str = "ordo.cloud.credentials.list.response";
    pub const CREDENTIAL_UPSERT_REQUEST: &str = "ordo.cloud.credential.upsert.request";
    pub const CREDENTIAL_UPSERTED: &str = "ordo.cloud.credential.upserted";
    pub const CREDENTIAL_REMOVE_REQUEST: &str = "ordo.cloud.credential.remove.request";
    pub const CREDENTIAL_REMOVED: &str = "ordo.cloud.credential.removed";
    pub const CREDENTIAL_TEST_REQUEST: &str = "ordo.cloud.credential.test.request";
    pub const CREDENTIAL_TEST_RESULT: &str = "ordo.cloud.credential.test.result";
    pub const DEFAULT_SET_REQUEST: &str = "ordo.cloud.default.set.request";
    pub const DEFAULT_CHANGED: &str = "ordo.cloud.default.changed";
}
