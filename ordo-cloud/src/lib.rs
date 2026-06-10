//! `ordo-cloud` â€” real outbound HTTP to named cloud providers.
//!
//! This crate is the local-first-but-not-local-only boundary. Every
//! capability is opt-in: a service only becomes usable once a credential
//! has been configured. Credentials live in the shared local SQLite
//! database next to runtime settings and memory. No credential ever
//! leaves the machine except as an outbound `Authorization` header to the
//! explicitly configured service.
//!
//! Supported auth styles:
//! - `bearer` â€” `Authorization: Bearer <secret>`
//! - `basic` â€” `Authorization: Basic <base64(user:pass)>` (set `secret`
//!   to `user:pass`)
//! - `api_key_header` â€” custom header (extras: `header_name`)
//! - `api_key_query` â€” query parameter (extras: `param_name`)
//! - `anthropic` â€” `x-api-key` + `anthropic-version` headers
//!
//! Configured services today:
//! - `openai` â€” chat completions + embeddings
//! - `anthropic` â€” messages API
//! - `gemini` â€” Google Generative Language (optional stub)
//! - arbitrary named REST services via the generic request helper

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::Utc;
use ordo_store::{OrdoDatabase, StorageTask, StorageTaskError};
use reqwest::{
    header::{HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Client, RequestBuilder, Response, Url,
};

pub use reqwest::Method;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use thiserror::Error;

pub mod anthropic;
pub mod bus_bridge;
pub mod openai;
pub mod vault;
pub mod voice;

pub use bus_bridge::run_bus_bridge;

pub use vault::{CredentialVault, KeyringVault, MemoryVault, NullVault, VaultError};

// =============================================================================
// Public test + discovery helpers
//
// One source of truth for "what URL do I hit to verify this
// provider is reachable" and "what models does this provider
// expose". Used by:
//   - `bus_bridge::perform_test` (the cycle-3 bus path)
//   - `cloud.credentials.test` MCP tool (the HTTP control-API path)
//   - `cloud.credentials.models` MCP tool (model discovery)
//   - The React `CloudCredentialsView` test + discover buttons
// =============================================================================

/// Per-service test path. **Relative** (no leading slash) by
/// convention — `CloudHttp::send_request` joins with the
/// credential's `base_url` which is expected to already include
/// the API version segment (`/v1`, `/v1beta`, …) the same way
/// `openai::chat` and `anthropic::messages` expect it.
///
/// Default: `GET <base>/models`. This is the de-facto standard
/// for every modern LLM provider — OpenAI, Anthropic, Gemini
/// (with `/v1beta` in base), Ollama (OpenAI-compat at `/v1`),
/// LM Studio, OpenRouter, LiteLLM proxies, Groq, Together,
/// Mistral, ollama-cloud, etc. The few exceptions opt out
/// explicitly below. Custom service names operators invent
/// (e.g. "ollama-cloud", "my-vllm-proxy") just work without
/// requiring a code change.
pub fn test_path_for(service: &str) -> (&'static str, Method) {
    match service.to_ascii_lowercase().as_str() {
        // AWS Bedrock requires SigV4 + regional routing — both
        // test + discover are explicitly unsupported. The
        // `test_credential` / `list_models` functions check the
        // service name separately to return a meaningful error,
        // but the HEAD probe below also keeps us safe.
        "bedrock" | "aws_bedrock" => ("", Method::HEAD),
        // Ollama Cloud API speaks the OpenAI-compatible /v1 surface
        // (its base_url ends in /v1), so it uses the default `models`
        // probe like every other OpenAI-shape provider — discovery and
        // chat then share one base (/v1/models and /v1/chat/completions).
        _ => ("models", Method::GET),
    }
}

/// Run a connectivity test against the provider's API. Returns
/// `Ok(())` on HTTP 2xx, `Err(message)` otherwise.
///
/// Bedrock requires AWS SigV4 + regional routing; not in scope.
/// Tested services excluded explicitly so we surface a useful
/// error rather than masquerading 401s.
pub async fn test_credential(http: &CloudHttp, credential: &CloudCredential) -> Result<(), String> {
    if credential.service.eq_ignore_ascii_case("bedrock")
        || credential.service.eq_ignore_ascii_case("aws_bedrock")
    {
        return Err("test not supported for AWS Bedrock (SigV4 not implemented)".into());
    }
    let (path, method) = test_path_for(&credential.service);
    let response = http
        .send_request(credential, method, path, None, &[])
        .await
        .map_err(|err| format!("network: {err}"))?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!(
            "HTTP {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("?")
        ))
    }
}

/// Discover available models exposed by the provider. Used by
/// the React Cloud view's "Discover models" button to populate a
/// dropdown instead of asking the operator to type the model
/// name (which is how the typo-causing-404 bug landed in the
/// first place).
///
/// Response-shape handling:
///   - **OpenAI / OpenAI-compatible / Anthropic / Ollama**:
///     `{ "data": [{ "id": "...", ... }, ...] }`
///   - **Gemini**: `{ "models": [{ "name": "models/...", ... }, ...] }`
///     (the `models/` prefix is stripped for parity with what
///     operators type in the model field).
///   - **Ollama native (local)**: `/api/tags` returns
///     `{ "models": [{ "name": "...", "model": "..." }, ...] }`.
///     (Ollama Cloud API uses the OpenAI-compatible `/v1/models`
///     surface above — the `data[].id` branch — not native `/api/tags`.)
///
/// Returns an empty list if the provider responds but with no
/// models; returns Err for network or auth failures.
pub async fn list_models(
    http: &CloudHttp,
    credential: &CloudCredential,
) -> Result<Vec<String>, String> {
    if credential.service.eq_ignore_ascii_case("bedrock")
        || credential.service.eq_ignore_ascii_case("aws_bedrock")
    {
        return Err("model discovery not supported for AWS Bedrock (SigV4 not implemented)".into());
    }
    let (path, _) = test_path_for(&credential.service);
    if path.is_empty() {
        return Err(format!(
            "model discovery not supported for service '{}'",
            credential.service
        ));
    }
    let payload: Value = http
        .send_json(credential, Method::GET, path, None, &[])
        .await
        .map_err(|err| format!("network: {err}"))?;
    if let Some(arr) = payload.get("data").and_then(|v| v.as_array()) {
        let mut models: Vec<String> = arr
            .iter()
            .filter_map(|v| v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string()))
            .collect();
        models.sort();
        return Ok(models);
    }
    if let Some(arr) = payload.get("models").and_then(|v| v.as_array()) {
        let mut models: Vec<String> = arr
            .iter()
            .filter_map(|v| {
                v.get("name")
                    .or_else(|| v.get("model"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.strip_prefix("models/").unwrap_or(s).to_string())
            })
            .collect();
        models.sort();
        return Ok(models);
    }
    Err(format!(
        "unrecognized models response shape from service '{}'",
        credential.service
    ))
}

use std::sync::Arc;

#[derive(Debug, Error)]
pub enum CloudError {
    #[error("credential for service '{0}' is not configured")]
    NotConfigured(String),
    #[error("credential for service '{service}' has invalid auth style '{auth_style}'")]
    InvalidAuthStyle { service: String, auth_style: String },
    #[error("cloud request to '{service}' failed: {message}")]
    Request { service: String, message: String },
    #[error("cloud response from '{service}' returned status {status}: {body}")]
    BadStatus {
        service: String,
        status: u16,
        body: String,
    },
    #[error("cloud response from '{service}' could not be parsed: {message}")]
    Parse { service: String, message: String },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("local storage error: {0}")]
    Storage(String),
    #[error("credential vault error: {0}")]
    Vault(String),
}

impl From<VaultError> for CloudError {
    fn from(value: VaultError) -> Self {
        CloudError::Vault(value.to_string())
    }
}

pub type CloudResult<T> = Result<T, CloudError>;

impl From<StorageTaskError> for CloudError {
    fn from(value: StorageTaskError) -> Self {
        CloudError::Storage(value.to_string())
    }
}

/// A saved cloud credential. Stored per-service in the local SQLite
/// database.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudCredential {
    pub service: String,
    pub label: String,
    pub auth_style: String,
    pub secret: String,
    pub base_url: Option<String>,
    #[serde(default)]
    pub extras: HashMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
}

impl CloudCredential {
    pub fn enabled(&self) -> bool {
        self.extras
            .get("enabled")
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "false" | "0" | "off" | "paused" | "disabled"
                )
            })
            .unwrap_or(true)
    }

    /// Pass-through view of a credential with the `secret` field
    /// removed. `extras` is preserved verbatim for operator-set config
    /// (model name, context window, temperature, etc.) — except for
    /// keys whose name looks secret-y, which are redacted as a safety
    /// net against accidental misuse of the extras bag for real
    /// credentials. The dedicated `secret` field is the one and only
    /// place an api key / token belongs.
    pub fn redacted(&self) -> Value {
        let mut extras = Map::new();
        for (key, value) in &self.extras {
            if looks_like_secret_key(key) {
                extras.insert(key.clone(), Value::String("***".into()));
            } else {
                extras.insert(key.clone(), Value::String(value.clone()));
            }
        }
        json!({
            "service": self.service,
            "label": self.label,
            "auth_style": self.auth_style,
            "base_url": self.base_url,
            "has_secret": !self.secret.is_empty(),
            "enabled": self.enabled(),
            "extras": extras,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        })
    }

    /// Typed, secret-omitted view for protocol events. Same
    /// redaction policy as [`Self::redacted`] (the `Value`
    /// JSON variant) but returns the strongly-typed
    /// `CloudCredentialView` carried on bus envelopes.
    pub fn view(&self) -> ordo_protocol::CloudCredentialView {
        let mut extras = HashMap::with_capacity(self.extras.len());
        for (key, value) in &self.extras {
            if looks_like_secret_key(key) {
                extras.insert(key.clone(), "***".to_string());
            } else {
                extras.insert(key.clone(), value.clone());
            }
        }
        ordo_protocol::CloudCredentialView {
            service: self.service.clone(),
            label: self.label.clone(),
            auth_style: self.auth_style.clone(),
            base_url: self.base_url.clone(),
            extras,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

// True for extras keys that read like they hold a secret value, even
// though the canonical home for secrets is the dedicated `secret`
// field. Defensive — operators shouldn't put credentials in extras,
// but if they do, this stops it leaking back through read paths.
fn looks_like_secret_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    matches!(lowered.as_str(), "secret" | "password" | "passphrase")
        || lowered.contains("api_key")
        || lowered.contains("apikey")
        || lowered.contains("access_key")
        || lowered.contains("auth_token")
        || lowered.contains("bearer_token")
        || lowered.ends_with("_token")
        || lowered.ends_with("_secret")
}

/// Fields supplied when creating or updating a credential. Unspecified
/// fields leave the existing stored value alone.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CloudCredentialUpdate {
    pub service: String,
    pub label: Option<String>,
    pub auth_style: Option<String>,
    pub secret: Option<String>,
    pub base_url: Option<String>,
    #[serde(default)]
    pub extras: Option<HashMap<String, String>>,
}

/// Synchronous credential store backed by `OrdoDatabase`.
pub struct CloudCredentialStore {
    db: OrdoDatabase,
    vault: Arc<dyn CredentialVault>,
}

impl CloudCredentialStore {
    pub fn open(path: impl AsRef<Path>) -> CloudResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| CloudError::Storage(err.to_string()))?;
        Ok(Self {
            db,
            vault: vault::default_vault(),
        })
    }

    pub fn in_memory() -> CloudResult<Self> {
        let db = OrdoDatabase::in_memory().map_err(|err| CloudError::Storage(err.to_string()))?;
        Ok(Self {
            db,
            vault: MemoryVault::shared(),
        })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self {
            db,
            vault: vault::default_vault(),
        }
    }

    pub fn with_vault(mut self, vault: Arc<dyn CredentialVault>) -> Self {
        self.vault = vault;
        self
    }

    pub fn vault_name(&self) -> &'static str {
        self.vault.name()
    }

    pub fn list(&self) -> CloudResult<Vec<CloudCredential>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT service, label, auth_style, secret, base_url, extras_json, \
                 created_at, updated_at FROM cloud_credentials ORDER BY service ASC",
            )
            .map_err(|err| CloudError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map([], row_to_credential)
            .map_err(|err| CloudError::Storage(err.to_string()))?;
        let mut credentials = Vec::new();
        for row in rows {
            let mut credential = row.map_err(|err| CloudError::Storage(err.to_string()))?;
            self.hydrate_secret(&mut credential)?;
            credentials.push(credential);
        }
        Ok(credentials)
    }

    pub fn get(&self, service: &str) -> CloudResult<Option<CloudCredential>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT service, label, auth_style, secret, base_url, extras_json, \
                 created_at, updated_at FROM cloud_credentials WHERE service = ?1",
            )
            .map_err(|err| CloudError::Storage(err.to_string()))?;
        let mut rows = stmt
            .query_map(params![service], row_to_credential)
            .map_err(|err| CloudError::Storage(err.to_string()))?;
        match rows.next() {
            Some(row) => {
                let mut credential = row.map_err(|err| CloudError::Storage(err.to_string()))?;
                self.hydrate_secret(&mut credential)?;
                Ok(Some(credential))
            }
            None => Ok(None),
        }
    }

    /// If the SQLite secret column holds the vault sentinel, replace it with
    /// the real secret from the vault. Rows that pre-date the vault (plain
    /// strings) are returned as-is so existing installs keep working.
    fn hydrate_secret(&self, credential: &mut CloudCredential) -> CloudResult<()> {
        if vault::is_vault_sentinel(&credential.secret) {
            match self.vault.get(&credential.service)? {
                Some(value) => {
                    credential.secret = value;
                }
                None => {
                    credential.secret = String::new();
                }
            }
        }
        Ok(())
    }

    pub fn upsert(&mut self, update: CloudCredentialUpdate) -> CloudResult<CloudCredential> {
        let service = update.service.trim().to_string();
        if service.is_empty() {
            return Err(CloudError::InvalidArgument(
                "service name cannot be empty".into(),
            ));
        }
        let existing = self.get(&service)?;
        let now = Utc::now().to_rfc3339();
        let credential = if let Some(mut current) = existing {
            if let Some(label) = update.label {
                current.label = label;
            }
            if let Some(auth_style) = update.auth_style {
                current.auth_style = auth_style;
            }
            if let Some(secret) = update.secret {
                current.secret = secret;
            }
            if let Some(base_url) = update.base_url {
                current.base_url = if base_url.is_empty() {
                    None
                } else {
                    Some(base_url)
                };
            }
            if let Some(extras) = update.extras {
                current.extras = extras;
            }
            current.updated_at = now.clone();
            current
        } else {
            CloudCredential {
                service: service.clone(),
                label: update.label.unwrap_or_else(|| service.clone()),
                auth_style: update.auth_style.unwrap_or_else(|| "bearer".into()),
                secret: update.secret.unwrap_or_default(),
                base_url: update.base_url.filter(|value| !value.is_empty()),
                extras: update.extras.unwrap_or_default(),
                created_at: now.clone(),
                updated_at: now.clone(),
            }
        };
        let extras_json = serde_json::to_string(&credential.extras)
            .map_err(|err| CloudError::Storage(err.to_string()))?;

        // Decide where the secret actually lives. A real vault stores the
        // secret out-of-band and only leaves a sentinel in SQLite. The null
        // vault keeps the historical plaintext-in-SQLite behavior.
        let db_secret = if credential.secret.is_empty() {
            String::new()
        } else if self.vault.name() == "plaintext" {
            credential.secret.clone()
        } else {
            self.vault.set(&credential.service, &credential.secret)?;
            vault::KEYRING_SENTINEL.to_string()
        };

        let conn = self.db.conn_mut();
        conn.execute(
            "INSERT INTO cloud_credentials \
             (service, label, auth_style, secret, base_url, extras_json, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(service) DO UPDATE SET \
               label = excluded.label, \
               auth_style = excluded.auth_style, \
               secret = excluded.secret, \
               base_url = excluded.base_url, \
               extras_json = excluded.extras_json, \
               updated_at = excluded.updated_at",
            params![
                credential.service,
                credential.label,
                credential.auth_style,
                db_secret,
                credential.base_url,
                extras_json,
                credential.created_at,
                credential.updated_at,
            ],
        )
        .map_err(|err| CloudError::Storage(err.to_string()))?;
        Ok(credential)
    }

    pub fn delete(&mut self, service: &str) -> CloudResult<DeleteOutcome> {
        let conn = self.db.conn_mut();
        // ATOMIC: a single SQLite transaction covers both the
        // credential row removal AND the conditional default
        // clear. There is no window during which the database
        // could reflect "credential gone, default still points at
        // it" — by the time `commit` returns Ok, both writes are
        // visible together (or neither is).
        //
        // `IMMEDIATE` (vs the default DEFERRED) acquires the write
        // lock at BEGIN, not at the first write. Without this, if
        // another connection (e.g. the memory-log writer) commits
        // between our SELECT and our DELETE, SQLite returns
        // `SQLITE_BUSY_SNAPSHOT` — which busy_timeout does NOT
        // retry on. IMMEDIATE makes us wait for the write lock up
        // front, which busy_timeout *does* honor.
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|err| CloudError::Storage(err.to_string()))?;
        // Snapshot the current default *inside* the transaction
        // so a concurrent writer can't change it between our read
        // and our write.
        let prev_default: Option<String> = tx
            .query_row(
                "SELECT setting_value FROM runtime_settings WHERE setting_key = ?1",
                params![CLOUD_DEFAULT_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| CloudError::Storage(err.to_string()))?;
        let was_default = prev_default.as_deref() == Some(service);

        let removed = tx
            .execute(
                "DELETE FROM cloud_credentials WHERE service = ?1",
                params![service],
            )
            .map_err(|err| CloudError::Storage(err.to_string()))?;

        // Clear the default key only when we actually deleted the
        // credential that was the default. Avoids clearing the
        // default if the user deletes a *different* provider.
        let default_cleared = if removed > 0 && was_default {
            tx.execute(
                "DELETE FROM runtime_settings WHERE setting_key = ?1",
                params![CLOUD_DEFAULT_KEY],
            )
            .map_err(|err| CloudError::Storage(err.to_string()))?;
            true
        } else {
            false
        };

        tx.commit()
            .map_err(|err| CloudError::Storage(err.to_string()))?;

        // Vault cleanup runs after commit — best-effort and
        // intentionally outside the transaction. Never fail
        // deletion because the vault couldn't confirm a sentinel
        // removal.
        if removed > 0 {
            if let Err(err) = self.vault.delete(service) {
                tracing::warn!(
                    target: "ordo_cloud",
                    service = %service,
                    backend = self.vault.name(),
                    error = %err,
                    "failed to remove secret from vault"
                );
            }
        }

        Ok(DeleteOutcome {
            removed: removed > 0,
            default_cleared,
        })
    }

    /// Read the current default provider service name, if set.
    ///
    /// Backed by the existing `runtime_settings` key-value table
    /// under the key `cloud.default_provider`. Returns `None` when
    /// nothing is configured (initial state, or after the last
    /// credential is deleted).
    pub fn get_default(&self) -> CloudResult<Option<String>> {
        let result = self
            .db
            .conn()
            .query_row(
                "SELECT setting_value FROM runtime_settings WHERE setting_key = ?1",
                params![CLOUD_DEFAULT_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| CloudError::Storage(err.to_string()))?;
        Ok(result)
    }

    /// Set or clear the default provider. `None` clears.
    ///
    /// Caller is responsible for ensuring the service name refers
    /// to an existing credential when setting; the store does not
    /// enforce that constraint to keep the operation single-row
    /// fast. The bridge enforces it before calling here.
    pub fn set_default(&self, service: Option<&str>) -> CloudResult<()> {
        let conn = self.db.conn();
        match service {
            Some(name) => {
                conn.execute(
                    "INSERT INTO runtime_settings (setting_key, setting_value, updated_at) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(setting_key) DO UPDATE SET \
                        setting_value = excluded.setting_value, \
                        updated_at = excluded.updated_at",
                    params![CLOUD_DEFAULT_KEY, name, Utc::now().to_rfc3339()],
                )
                .map_err(|err| CloudError::Storage(err.to_string()))?;
            }
            None => {
                conn.execute(
                    "DELETE FROM runtime_settings WHERE setting_key = ?1",
                    params![CLOUD_DEFAULT_KEY],
                )
                .map_err(|err| CloudError::Storage(err.to_string()))?;
            }
        }
        Ok(())
    }
}

/// Outcome of a `CloudCredentialStore::delete` call. Carries
/// whether the credential row was actually removed (callers may
/// pass non-existent service names) and whether the default
/// pointer was atomically cleared as part of the same
/// transaction (true only when the deleted credential *was* the
/// default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteOutcome {
    pub removed: bool,
    pub default_cleared: bool,
}

/// `runtime_settings` key under which the default cloud provider
/// service name is stored. Lives here (not in `ordo-store`)
/// because it's a cloud-domain concern; the key-value table is
/// just the cheapest reversible storage.
const CLOUD_DEFAULT_KEY: &str = "cloud.default_provider";

fn row_to_credential(row: &rusqlite::Row<'_>) -> rusqlite::Result<CloudCredential> {
    let extras_json: String = row.get(5)?;
    let extras: HashMap<String, String> = serde_json::from_str(&extras_json).unwrap_or_default();
    Ok(CloudCredential {
        service: row.get(0)?,
        label: row.get(1)?,
        auth_style: row.get(2)?,
        secret: row.get(3)?,
        base_url: row.get(4)?,
        extras,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

/// Bus identity for publish-on-mutation events. The runtime
/// supplies both fields once at boot via
/// [`CloudCredentialTask::with_bus`]. Cloning is cheap (Arc + a
/// small NodeId wrapper) — the task itself is `Clone`, so each
/// clone shares the same publisher.
#[derive(Clone)]
struct BusPublisher {
    bus: std::sync::Arc<dyn ordo_bus::Bus>,
    node_id: ordo_protocol::NodeId,
}

/// Tokio-friendly handle around the synchronous credential store.
///
/// When configured with [`Self::with_bus`], every successful
/// mutation (upsert, delete, set_default) publishes the matching
/// event on the bus *after* the underlying SQLite transaction
/// commits. This way bus subscribers can trust that any event
/// they receive corresponds to a durable state change — the
/// only way to see a `CloudCredentialUpserted` envelope is for
/// the row to be on disk.
#[derive(Clone)]
pub struct CloudCredentialTask {
    inner: StorageTask<CloudCredentialStore>,
    publisher: Option<BusPublisher>,
}

impl CloudCredentialTask {
    pub fn start(store: CloudCredentialStore) -> Self {
        Self {
            inner: StorageTask::start("ordo-cloud-credentials", store),
            publisher: None,
        }
    }

    /// Attach a bus publisher. Once set, every successful mutation
    /// publishes the matching protocol event on the bus *after*
    /// the storage transaction commits.
    ///
    /// Builder-style on purpose — callers chain with future
    /// configuration without proliferating constructors.
    pub fn with_bus(
        mut self,
        bus: std::sync::Arc<dyn ordo_bus::Bus>,
        node_id: ordo_protocol::NodeId,
    ) -> Self {
        self.publisher = Some(BusPublisher { bus, node_id });
        self
    }

    pub async fn list(&self) -> CloudResult<Vec<CloudCredential>> {
        let result = self
            .inner
            .call(|store| store.list().map_err(|err| err.to_string()))
            .await?;
        Ok(result)
    }

    pub async fn get(&self, service: String) -> CloudResult<Option<CloudCredential>> {
        let result = self
            .inner
            .call(move |store| store.get(&service).map_err(|err| err.to_string()))
            .await?;
        Ok(result)
    }

    pub async fn upsert(&self, update: CloudCredentialUpdate) -> CloudResult<CloudCredential> {
        let result = self
            .inner
            .call(move |store| store.upsert(update).map_err(|err| err.to_string()))
            .await?;
        // PUBLISH-AFTER-COMMIT: `inner.call` returns Ok only when
        // the closure returned Ok, which means the SQLite write
        // committed. Publishing here is safe — subscribers can
        // trust the row is on disk by the time they see the
        // event.
        if let Some(publisher) = &self.publisher {
            publish_event(
                publisher,
                ordo_protocol::cloud_topics::CREDENTIAL_UPSERTED,
                ordo_protocol::OrdoMessage::CloudCredentialUpserted(result.view()),
            )
            .await;
        }
        Ok(result)
    }

    pub async fn delete(&self, service: String) -> CloudResult<bool> {
        let service_for_event = service.clone();
        let outcome = self
            .inner
            .call(move |store| store.delete(&service).map_err(|err| err.to_string()))
            .await?;
        // PUBLISH-AFTER-COMMIT for both the removal event and the
        // auto-clear-default event when applicable. The store's
        // single transaction guaranteed atomicity; here we fan
        // those two state changes out as two separate envelopes.
        if let Some(publisher) = &self.publisher {
            if outcome.removed {
                publish_event(
                    publisher,
                    ordo_protocol::cloud_topics::CREDENTIAL_REMOVED,
                    ordo_protocol::OrdoMessage::CloudCredentialRemoved {
                        service: service_for_event,
                    },
                )
                .await;
            }
            if outcome.default_cleared {
                publish_event(
                    publisher,
                    ordo_protocol::cloud_topics::DEFAULT_CHANGED,
                    ordo_protocol::OrdoMessage::CloudCredentialDefaultChanged { service: None },
                )
                .await;
            }
        }
        // Existing callers (`ordo-mcp-host::cloud_credentials_delete`)
        // expect a bool. The `DeleteOutcome.default_cleared` flag
        // is observable only through the bus event above.
        Ok(outcome.removed)
    }

    /// Read the current default provider service name. `None` if
    /// no default is set.
    pub async fn get_default(&self) -> CloudResult<Option<String>> {
        let result = self
            .inner
            .call(|store| store.get_default().map_err(|err| err.to_string()))
            .await?;
        Ok(result)
    }

    /// Set or clear the default provider. Publishes
    /// `CloudCredentialDefaultChanged` on success when a bus is
    /// configured. `None` clears.
    pub async fn set_default(&self, service: Option<String>) -> CloudResult<()> {
        let service_for_event = service.clone();
        self.inner
            .call(move |store| {
                store
                    .set_default(service.as_deref())
                    .map_err(|err| err.to_string())
            })
            .await?;
        if let Some(publisher) = &self.publisher {
            publish_event(
                publisher,
                ordo_protocol::cloud_topics::DEFAULT_CHANGED,
                ordo_protocol::OrdoMessage::CloudCredentialDefaultChanged {
                    service: service_for_event,
                },
            )
            .await;
        }
        Ok(())
    }
}

/// Publish a single event. Errors are logged but never bubbled —
/// the storage mutation already succeeded; failing to broadcast
/// shouldn't make the operation report failure.
async fn publish_event(publisher: &BusPublisher, topic: &str, payload: ordo_protocol::OrdoMessage) {
    let envelope = ordo_protocol::Envelope::new(publisher.node_id.clone(), payload);
    if let Err(err) = publisher.bus.publish(topic, envelope).await {
        tracing::warn!(
            target: "ordo_cloud",
            topic,
            error = %err,
            "failed to publish cloud-credential event"
        );
    }
}

/// Thin wrapper around `reqwest::Client` that knows how to apply a
/// `CloudCredential` to an outgoing request.
#[derive(Clone)]
pub struct CloudHttp {
    client: Client,
}

/// Default per-request timeout for cloud HTTP calls. 300 s (five
/// minutes) is generous enough for local reasoning models (qwen3,
/// deepseek-r1, …) to think + emit a long answer on consumer
/// hardware, while still bounding hung connections so the runtime
/// doesn't lock up forever. Operators can override per credential
/// via `extras.timeout_secs`.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 300;

/// Pull a per-credential timeout out of `extras.timeout_secs` if set,
/// falling back to the default. Invalid / non-numeric values fall back
/// silently — operators shouldn't have to fix typos to keep the
/// runtime working.
///
/// Public so the assistant service (and any other layer wrapping
/// cloud calls in tokio::time::timeout) can use the same bound and
/// honor the same per-credential override.
pub fn timeout_for(credential: &CloudCredential) -> Duration {
    let secs = credential
        .extras
        .get("timeout_secs")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

impl CloudHttp {
    pub fn new() -> Self {
        Self {
            // No client-level timeout — per-request timeouts (set in
            // build_request below) are the authoritative bound. A
            // client-level timeout of 60 s used to short-circuit local
            // reasoning models mid-thought.
            client: Client::builder()
                .user_agent("ordo/0.1")
                .build()
                .expect("build reqwest client"),
        }
    }

    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// Build an authenticated request against a service. `url` may be a
    /// full URL or a path: when it starts with `http` it is used as-is,
    /// otherwise it is joined against the credential's `base_url`.
    pub fn build_request(
        &self,
        credential: &CloudCredential,
        method: Method,
        url: &str,
    ) -> CloudResult<RequestBuilder> {
        let mut full_url = resolve_url(credential, url)?;
        if credential.auth_style == "api_key_query" {
            let param_name = credential
                .extras
                .get("param_name")
                .cloned()
                .unwrap_or_else(|| "api_key".to_string());
            let secret = credential_secret_or_env(credential)?;
            full_url.query_pairs_mut().append_pair(&param_name, &secret);
        }
        let builder = self
            .client
            .request(method, full_url)
            .timeout(timeout_for(credential));
        apply_auth(builder, credential)
    }

    pub async fn send_json(
        &self,
        credential: &CloudCredential,
        method: Method,
        url: &str,
        body: Option<&Value>,
        extra_headers: &[(String, String)],
    ) -> CloudResult<Value> {
        let response = self
            .send_request(credential, method, url, body, extra_headers)
            .await?;
        handle_json_response(credential, response).await
    }

    /// Lower-level variant of `send_json` that returns the raw
    /// `reqwest::Response`. Used by streaming endpoints (SSE) which
    /// need to iterate the body as a byte stream rather than slurp
    /// the whole thing into JSON.
    pub async fn send_request(
        &self,
        credential: &CloudCredential,
        method: Method,
        url: &str,
        body: Option<&Value>,
        extra_headers: &[(String, String)],
    ) -> CloudResult<reqwest::Response> {
        let mut builder = self.build_request(credential, method, url)?;
        for (name, value) in extra_headers {
            builder = builder.header(name, value);
        }
        if let Some(payload) = body {
            builder = builder.json(payload);
        }
        // Track elapsed time so timeout errors surface "after Xs"
        // instead of just reqwest's bare "error sending request" —
        // the operator needs to know whether they hit the bound or
        // the connection dropped immediately.
        let started = std::time::Instant::now();
        let timeout = timeout_for(credential);
        builder.send().await.map_err(|err| {
            let elapsed = started.elapsed();
            let near_timeout =
                err.is_timeout() || elapsed >= timeout.saturating_sub(Duration::from_secs(2));
            let suffix = if near_timeout {
                format!(
                    " (after {:.1}s; timeout is {}s — bump extras.timeout_secs on the credential to allow longer)",
                    elapsed.as_secs_f64(),
                    timeout.as_secs()
                )
            } else {
                format!(" (after {:.1}s)", elapsed.as_secs_f64())
            };
            CloudError::Request {
                service: credential.service.clone(),
                message: format!("{err}{suffix}"),
            }
        })
    }
}

impl Default for CloudHttp {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_url(credential: &CloudCredential, url: &str) -> CloudResult<Url> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Url::parse(url).map_err(|err| CloudError::InvalidArgument(err.to_string()));
    }
    let base = credential.base_url.as_deref().ok_or_else(|| {
        CloudError::InvalidArgument(format!(
            "service '{}' has no base_url and a relative path '{url}' was requested",
            credential.service
        ))
    })?;
    let joined = if base.ends_with('/') || url.starts_with('/') {
        if base.ends_with('/') && url.starts_with('/') {
            format!("{}{}", base.trim_end_matches('/'), url)
        } else {
            format!("{base}{url}")
        }
    } else {
        format!("{base}/{url}")
    };
    Url::parse(&joined).map_err(|err| CloudError::InvalidArgument(err.to_string()))
}

fn apply_auth(
    builder: RequestBuilder,
    credential: &CloudCredential,
) -> CloudResult<RequestBuilder> {
    let mut builder = builder.header(CONTENT_TYPE, "application/json");
    match credential.auth_style.as_str() {
        "bearer" => {
            let secret = credential_secret_or_env(credential)?;
            let value = HeaderValue::from_str(&format!("Bearer {secret}"))
                .map_err(|err| CloudError::InvalidArgument(err.to_string()))?;
            builder = builder.header(AUTHORIZATION, value);
        }
        "basic" => {
            let secret = credential_secret_or_env(credential)?;
            let encoded = base64_encode(secret.as_bytes());
            let value = HeaderValue::from_str(&format!("Basic {encoded}"))
                .map_err(|err| CloudError::InvalidArgument(err.to_string()))?;
            builder = builder.header(AUTHORIZATION, value);
        }
        "api_key_header" => {
            let header_name = credential
                .extras
                .get("header_name")
                .cloned()
                .unwrap_or_else(|| "x-api-key".to_string());
            let name = HeaderName::from_bytes(header_name.as_bytes())
                .map_err(|err| CloudError::InvalidArgument(err.to_string()))?;
            let secret = credential_secret_or_env(credential)?;
            let value = HeaderValue::from_str(&secret)
                .map_err(|err| CloudError::InvalidArgument(err.to_string()))?;
            builder = builder.header(name, value);
        }
        "api_key_query" => {
            // Query-parameter auth is already applied in build_request()
            // before the request was created. No header work needed here.
        }
        "anthropic" => {
            let api_version = credential
                .extras
                .get("anthropic_version")
                .cloned()
                .unwrap_or_else(|| "2023-06-01".to_string());
            let secret = credential_secret_or_env(credential)?;
            let key_value = HeaderValue::from_str(&secret)
                .map_err(|err| CloudError::InvalidArgument(err.to_string()))?;
            let version_value = HeaderValue::from_str(&api_version)
                .map_err(|err| CloudError::InvalidArgument(err.to_string()))?;
            builder = builder
                .header(HeaderName::from_static("x-api-key"), key_value)
                .header(HeaderName::from_static("anthropic-version"), version_value);
        }
        other => {
            return Err(CloudError::InvalidAuthStyle {
                service: credential.service.clone(),
                auth_style: other.to_string(),
            });
        }
    }
    Ok(builder)
}

fn credential_secret_or_env(credential: &CloudCredential) -> CloudResult<String> {
    if credential
        .extras
        .get("auth_source")
        .is_some_and(|source| source == "environment")
    {
        let env_var = credential
            .extras
            .get("env_var")
            .map(String::as_str)
            .unwrap_or_else(|| default_env_var_for(credential));
        let value = env::var(env_var)
            .ok()
            .or_else(|| ordo_local_env_value(env_var))
            .ok_or_else(|| {
                CloudError::InvalidArgument(format!(
                    "service '{}' expects environment variable {env_var} to be set",
                    credential.service
                ))
            })?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(CloudError::InvalidArgument(format!(
                "service '{}' found empty environment variable {env_var}",
                credential.service
            )));
        }
        return Ok(trimmed.to_string());
    }
    let trimmed = credential.secret.trim();
    if trimmed.is_empty() {
        return Err(CloudError::InvalidArgument(format!(
            "service '{}' has no stored secret",
            credential.service
        )));
    }
    Ok(trimmed.to_string())
}

fn ordo_local_env_value(env_var: &str) -> Option<String> {
    let path = ordo_local_env_path()?;
    let raw = fs::read_to_string(path).ok()?;
    let values = serde_json::from_str::<HashMap<String, String>>(&raw).ok()?;
    values.get(env_var).cloned()
}

fn ordo_local_env_path() -> Option<PathBuf> {
    if cfg!(windows) {
        if let Some(appdata) = env::var_os("APPDATA") {
            return Some(
                PathBuf::from(appdata)
                    .join("Ordo")
                    .join("env")
                    .join("api-keys.json"),
            );
        }
        return env::var_os("USERPROFILE").map(|home| {
            PathBuf::from(home)
                .join("AppData")
                .join("Roaming")
                .join("Ordo")
                .join("env")
                .join("api-keys.json")
        });
    }
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Some(
            PathBuf::from(config_home)
                .join("ordo")
                .join("env")
                .join("api-keys.json"),
        );
    }
    env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("ordo")
            .join("env")
            .join("api-keys.json")
    })
}

fn default_env_var_for(credential: &CloudCredential) -> &'static str {
    let service = credential.service.to_ascii_lowercase();
    match credential.auth_style.as_str() {
        "anthropic" => "ANTHROPIC_API_KEY",
        "api_key_query" if service.contains("google") || service.contains("gemini") => {
            "GOOGLE_API_KEY"
        }
        "api_key_header" if service.contains("azure") => "AZURE_OPENAI_API_KEY",
        _ if service.contains("openrouter") => "OPENROUTER_API_KEY",
        _ if service.contains("groq") => "GROQ_API_KEY",
        _ if service.contains("moonshot") => "MOONSHOT_API_KEY",
        _ if service.contains("qwen") || service.contains("dashscope") => "DASHSCOPE_API_KEY",
        _ => "OPENAI_API_KEY",
    }
}

async fn handle_json_response(
    credential: &CloudCredential,
    response: Response,
) -> CloudResult<Value> {
    let status = response.status();
    let text = response.text().await.map_err(|err| CloudError::Request {
        service: credential.service.clone(),
        message: err.to_string(),
    })?;
    if !status.is_success() {
        return Err(CloudError::BadStatus {
            service: credential.service.clone(),
            status: status.as_u16(),
            body: text,
        });
    }
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str::<Value>(&text).map_err(|err| CloudError::Parse {
        service: credential.service.clone(),
        message: err.to_string(),
    })
}

/// Collect optional header map overrides from a generic JSON "headers"
/// field.
pub fn headers_from_value(value: Option<&Value>) -> Vec<(String, String)> {
    let Some(object) = value.and_then(|value| value.as_object()) else {
        return Vec::new();
    };
    object
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
        .collect()
}

/// Minimal base64 encoder. Avoids pulling in another crate just for
/// basic-auth headers. RFC 4648 alphabet, no padding omitted.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Convenience: convert common boolean-ish JSON values to `Option<f64>`.
pub fn optional_f64(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| value.as_f64())
}

/// Convenience: extract a required string.
pub fn require_string(arguments: &Value, key: &str) -> CloudResult<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| CloudError::InvalidArgument(format!("missing required field '{key}'")))
}

/// Convenience: extract an optional string.
pub fn optional_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod store_tests {
    use super::*;

    #[test]
    fn upsert_and_list_cycle() {
        let mut store = CloudCredentialStore::in_memory().expect("store");
        let credential = store
            .upsert(CloudCredentialUpdate {
                service: "openai".into(),
                label: Some("OpenAI (personal)".into()),
                auth_style: Some("bearer".into()),
                secret: Some("sk-test".into()),
                base_url: Some("https://api.openai.com/v1".into()),
                extras: None,
            })
            .expect("upsert");
        assert_eq!(credential.service, "openai");
        let list = store.list().expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].secret, "sk-test");
        assert_eq!(list[0].auth_style, "bearer");
    }

    #[test]
    fn vault_holds_secret_while_sqlite_only_holds_sentinel() {
        let db = ordo_store::OrdoDatabase::in_memory().expect("db");
        let vault = MemoryVault::shared();
        let mut store = CloudCredentialStore::from_database(db).with_vault(vault.clone());

        store
            .upsert(CloudCredentialUpdate {
                service: "openai".into(),
                secret: Some("sk-secret-never-in-db".into()),
                ..Default::default()
            })
            .expect("upsert");

        // Vault has the real secret.
        assert_eq!(
            vault.get("openai").expect("vault get"),
            Some("sk-secret-never-in-db".to_string())
        );

        // SQLite secret column holds the sentinel, not the raw secret.
        {
            let db_secret: String = store
                .db
                .conn()
                .query_row(
                    "SELECT secret FROM cloud_credentials WHERE service = ?1",
                    rusqlite::params!["openai"],
                    |row| row.get(0),
                )
                .expect("direct sql read");
            assert_eq!(db_secret, vault::KEYRING_SENTINEL);
            assert!(!db_secret.contains("sk-secret"));
        }

        // Reading back rehydrates through the vault.
        let hydrated = store.get("openai").expect("get").expect("present");
        assert_eq!(hydrated.secret, "sk-secret-never-in-db");

        // Delete clears both sides.
        assert!(store.delete("openai").expect("delete").removed);
        assert_eq!(vault.get("openai").expect("vault get after delete"), None);
    }

    #[test]
    fn plaintext_vault_keeps_legacy_behavior() {
        let db = ordo_store::OrdoDatabase::in_memory().expect("db");
        let mut store = CloudCredentialStore::from_database(db).with_vault(Arc::new(NullVault));
        store
            .upsert(CloudCredentialUpdate {
                service: "legacy".into(),
                secret: Some("kept-in-sqlite".into()),
                ..Default::default()
            })
            .expect("upsert");

        let db_secret: String = store
            .db
            .conn()
            .query_row(
                "SELECT secret FROM cloud_credentials WHERE service = ?1",
                rusqlite::params!["legacy"],
                |row| row.get(0),
            )
            .expect("direct sql read");
        assert_eq!(db_secret, "kept-in-sqlite");
    }

    #[test]
    fn upsert_partial_preserves_existing_fields() {
        let mut store = CloudCredentialStore::in_memory().expect("store");
        store
            .upsert(CloudCredentialUpdate {
                service: "anthropic".into(),
                label: Some("Anthropic prod".into()),
                auth_style: Some("anthropic".into()),
                secret: Some("sk-ant".into()),
                base_url: Some("https://api.anthropic.com/v1".into()),
                extras: None,
            })
            .expect("upsert");
        let rotated = store
            .upsert(CloudCredentialUpdate {
                service: "anthropic".into(),
                secret: Some("sk-ant-new".into()),
                ..Default::default()
            })
            .expect("rotate");
        assert_eq!(rotated.secret, "sk-ant-new");
        assert_eq!(rotated.label, "Anthropic prod");
        assert_eq!(
            rotated.base_url.as_deref(),
            Some("https://api.anthropic.com/v1")
        );
    }

    #[test]
    fn delete_returns_true_when_removed() {
        let mut store = CloudCredentialStore::in_memory().expect("store");
        store
            .upsert(CloudCredentialUpdate {
                service: "alpha".into(),
                secret: Some("zzz".into()),
                ..Default::default()
            })
            .expect("upsert");
        assert!(store.delete("alpha").expect("delete").removed);
        assert!(!store.delete("alpha").expect("delete-again").removed);
    }

    #[test]
    fn default_round_trip_no_setting_returns_none() {
        let store = CloudCredentialStore::in_memory().expect("store");
        assert_eq!(store.get_default().expect("get_default"), None);
    }

    #[test]
    fn default_set_and_read_back() {
        let store = CloudCredentialStore::in_memory().expect("store");
        store.set_default(Some("openai")).expect("set_default");
        assert_eq!(
            store.get_default().expect("get_default"),
            Some("openai".to_string())
        );
    }

    #[test]
    fn default_set_replaces_existing_value() {
        let store = CloudCredentialStore::in_memory().expect("store");
        store.set_default(Some("openai")).expect("set first");
        store.set_default(Some("anthropic")).expect("set second");
        assert_eq!(
            store.get_default().expect("get_default"),
            Some("anthropic".to_string())
        );
    }

    #[test]
    fn default_set_none_clears() {
        let store = CloudCredentialStore::in_memory().expect("store");
        store.set_default(Some("openai")).expect("set");
        store.set_default(None).expect("clear");
        assert_eq!(store.get_default().expect("get_default"), None);
    }

    #[test]
    fn delete_clears_default_when_service_was_default() {
        // Most important test for the cycle: atomic
        // delete-and-clear behavior is the load-bearing
        // invariant. After this returns Ok, the database must
        // never reflect "credential gone, default still pointing
        // at it" — the transaction in `delete` guarantees that.
        let mut store = CloudCredentialStore::in_memory().expect("store");
        store
            .upsert(CloudCredentialUpdate {
                service: "openai".into(),
                secret: Some("sk-x".into()),
                ..Default::default()
            })
            .expect("upsert");
        store.set_default(Some("openai")).expect("set_default");
        let outcome = store.delete("openai").expect("delete");
        assert!(outcome.removed);
        assert!(
            outcome.default_cleared,
            "deleting the default service must clear the default key"
        );
        assert_eq!(
            store.get_default().expect("get_default"),
            None,
            "default key must be gone after the delete commit"
        );
    }

    #[test]
    fn delete_leaves_default_when_service_was_not_default() {
        // Inverse invariant: deleting a non-default provider
        // must NOT clear the default.
        let mut store = CloudCredentialStore::in_memory().expect("store");
        store
            .upsert(CloudCredentialUpdate {
                service: "openai".into(),
                secret: Some("sk-x".into()),
                ..Default::default()
            })
            .expect("upsert openai");
        store
            .upsert(CloudCredentialUpdate {
                service: "anthropic".into(),
                secret: Some("sk-y".into()),
                ..Default::default()
            })
            .expect("upsert anthropic");
        store.set_default(Some("openai")).expect("set_default");
        let outcome = store.delete("anthropic").expect("delete anthropic");
        assert!(outcome.removed);
        assert!(
            !outcome.default_cleared,
            "deleting a non-default service must not clear the default"
        );
        assert_eq!(
            store.get_default().expect("get_default"),
            Some("openai".to_string()),
            "default must still point at the un-deleted provider"
        );
    }

    #[test]
    fn delete_nonexistent_does_not_clear_default() {
        // Calling delete on a service that doesn't exist while a
        // default is set must not touch the default — `removed`
        // is false, transaction commits with no changes.
        let store = CloudCredentialStore::in_memory().expect("store");
        store.set_default(Some("openai")).expect("set_default");
        let mut store = store; // shadow as mut for delete signature
        let outcome = store.delete("ghost").expect("delete nonexistent");
        assert!(!outcome.removed);
        assert!(!outcome.default_cleared);
        assert_eq!(
            store.get_default().expect("get_default"),
            Some("openai".to_string())
        );
    }

    #[test]
    fn redacted_hides_secret_but_keeps_operator_config() {
        let credential = CloudCredential {
            service: "svc".into(),
            label: "Ollama (local)".into(),
            auth_style: "bearer".into(),
            secret: "hunter2".into(),
            base_url: Some("http://localhost:11434/v1".into()),
            extras: [
                ("name".to_string(), "Ollama (local)".to_string()),
                ("model".to_string(), "qwen3.6:35b".to_string()),
                ("context_window".to_string(), "32768".to_string()),
                ("temperature".to_string(), "0.2".to_string()),
                ("supports_images".to_string(), "false".to_string()),
                // A misuse: someone stuffed a real key in extras. The
                // allowlist still catches it.
                ("api_key".to_string(), "sk-leaked".to_string()),
            ]
            .into_iter()
            .collect(),
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        let redacted = credential.redacted();
        let serialized = redacted.to_string();
        assert_eq!(
            redacted.get("has_secret").and_then(|value| value.as_bool()),
            Some(true)
        );
        assert!(!serialized.contains("hunter2"), "primary secret leaked");
        assert!(
            !serialized.contains("sk-leaked"),
            "extras-shaped secret leaked"
        );
        // Operator config — model name, context window, etc. — must
        // round-trip so the studio's edit view shows real values
        // instead of `***`.
        let extras = redacted
            .get("extras")
            .and_then(|v| v.as_object())
            .expect("extras");
        assert_eq!(
            extras.get("model").and_then(|v| v.as_str()),
            Some("qwen3.6:35b")
        );
        assert_eq!(
            extras.get("context_window").and_then(|v| v.as_str()),
            Some("32768")
        );
        assert_eq!(
            extras.get("name").and_then(|v| v.as_str()),
            Some("Ollama (local)")
        );
        assert_eq!(extras.get("api_key").and_then(|v| v.as_str()), Some("***"));
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn bearer_credential(service: &str, base_url: &str, secret: &str) -> CloudCredential {
        CloudCredential {
            service: service.into(),
            label: service.into(),
            auth_style: "bearer".into(),
            secret: secret.into(),
            base_url: Some(base_url.into()),
            extras: Default::default(),
            created_at: "now".into(),
            updated_at: "now".into(),
        }
    }

    #[tokio::test]
    async fn cloud_http_sends_bearer_auth_and_parses_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/echo"))
            .and(header("authorization", "Bearer sk-test"))
            .and(body_json(json!({ "ping": "pong" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let http = CloudHttp::new();
        let credential = bearer_credential("echo", &server.uri(), "sk-test");
        let body = json!({ "ping": "pong" });
        let result = http
            .send_json(&credential, Method::POST, "/echo", Some(&body), &[])
            .await
            .expect("ok response");
        assert_eq!(
            result.get("ok").and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn cloud_http_surfaces_non_success_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/boom"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
            .mount(&server)
            .await;

        let http = CloudHttp::new();
        let credential = bearer_credential("boom", &server.uri(), "sk");
        let error = http
            .send_json(&credential, Method::GET, "/boom", None, &[])
            .await
            .expect_err("expected bad status");
        match error {
            CloudError::BadStatus { status, body, .. } => {
                assert_eq!(status, 503);
                assert_eq!(body, "upstream down");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn cloud_http_appends_api_key_query_param() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ping"))
            .and(query_param("api_key", "zzz"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let credential = CloudCredential {
            service: "kq".into(),
            label: "kq".into(),
            auth_style: "api_key_query".into(),
            secret: "zzz".into(),
            base_url: Some(server.uri()),
            extras: Default::default(),
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        let http = CloudHttp::new();
        let result = http
            .send_json(&credential, Method::GET, "/ping", None, &[])
            .await
            .expect("ok response");
        assert_eq!(
            result.get("ok").and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn ollama_cloud_api_uses_openai_models_for_discovery() {
        // Ollama Cloud API speaks the OpenAI-compatible /v1 surface, so
        // discovery hits GET {base}/models and parses the {data:[{id}]}
        // shape — NOT native /api/tags (which only exists for local
        // Ollama). base_url for this service ends in /v1.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer ollama-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    { "id": "gpt-oss:120b" },
                    { "id": "minimax-m2.5" },
                    { "id": "glm-4.7" }
                ]
            })))
            .mount(&server)
            .await;

        let credential = bearer_credential("ollama-cloud-api", &server.uri(), "ollama-key");
        let http = CloudHttp::new();
        let models = list_models(&http, &credential)
            .await
            .expect("ollama cloud models");

        assert_eq!(
            models,
            vec![
                "glm-4.7".to_string(),
                "gpt-oss:120b".to_string(),
                "minimax-m2.5".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn cloud_http_sets_custom_api_key_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("x-custom-key", "ck-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let mut extras = HashMap::new();
        extras.insert("header_name".into(), "x-custom-key".into());
        let credential = CloudCredential {
            service: "api".into(),
            label: "api".into(),
            auth_style: "api_key_header".into(),
            secret: "ck-1".into(),
            base_url: Some(server.uri()),
            extras,
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        let http = CloudHttp::new();
        let result = http
            .send_json(&credential, Method::GET, "/me", None, &[])
            .await
            .expect("ok response");
        assert_eq!(
            result.get("ok").and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn openai_chat_extracts_assistant_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer sk-openai"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [
                    { "message": { "role": "assistant", "content": "hello from the mock" } }
                ]
            })))
            .mount(&server)
            .await;
        let credential = bearer_credential("openai", &server.uri(), "sk-openai");
        let http = CloudHttp::new();
        let args = json!({
            "model": "gpt-4o-mini",
            "messages": [ { "role": "user", "content": "hi" } ],
        });
        let result = openai::chat(&http, &credential, &args).await.expect("chat");
        assert_eq!(
            result
                .get("assistant_message")
                .and_then(|value| value.as_str()),
            Some("hello from the mock")
        );
    }

    #[tokio::test]
    async fn openai_embed_counts_vectors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .and(header("authorization", "Bearer sk-e"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "embedding": [0.1, 0.2, 0.3] },
                    { "embedding": [0.4, 0.5, 0.6] }
                ]
            })))
            .mount(&server)
            .await;
        let credential = bearer_credential("openai", &server.uri(), "sk-e");
        let http = CloudHttp::new();
        let args = json!({ "input": ["hello", "world"] });
        let result = openai::embed(&http, &credential, &args)
            .await
            .expect("embed");
        assert_eq!(
            result.get("vector_count").and_then(|value| value.as_u64()),
            Some(2)
        );
    }

    #[tokio::test]
    async fn anthropic_messages_extracts_assistant_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .and(header("x-api-key", "sk-ant"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [ { "type": "text", "text": "reply body" } ],
                "stop_reason": "end_turn"
            })))
            .mount(&server)
            .await;
        let credential = CloudCredential {
            service: "anthropic".into(),
            label: "anthropic".into(),
            auth_style: "anthropic".into(),
            secret: "sk-ant".into(),
            base_url: Some(server.uri()),
            extras: Default::default(),
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        let http = CloudHttp::new();
        let args = json!({
            "messages": [ { "role": "user", "content": "hi" } ],
        });
        let result = anthropic::messages(&http, &credential, &args)
            .await
            .expect("messages");
        assert_eq!(
            result
                .get("assistant_text")
                .and_then(|value| value.as_str()),
            Some("reply body")
        );
        assert_eq!(
            result.get("stop_reason").and_then(|value| value.as_str()),
            Some("end_turn")
        );
    }

    #[tokio::test]
    async fn anthropic_messages_can_read_key_from_environment() {
        let env_var = "ORDO_TEST_ANTHROPIC_API_KEY";
        unsafe {
            env::set_var(env_var, "sk-ant-env");
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .and(header("x-api-key", "sk-ant-env"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [ { "type": "text", "text": "reply from env" } ],
                "stop_reason": "end_turn"
            })))
            .mount(&server)
            .await;
        let credential = CloudCredential {
            service: "anthropic-env".into(),
            label: "Anthropic Local Env".into(),
            auth_style: "anthropic".into(),
            secret: String::new(),
            base_url: Some(server.uri()),
            extras: HashMap::from([
                ("auth_source".to_string(), "environment".to_string()),
                ("env_var".to_string(), env_var.to_string()),
            ]),
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        let http = CloudHttp::new();
        let args = json!({
            "messages": [ { "role": "user", "content": "hi" } ],
        });
        let result = anthropic::messages(&http, &credential, &args)
            .await
            .expect("messages");
        unsafe {
            env::remove_var(env_var);
        }
        assert_eq!(
            result
                .get("assistant_text")
                .and_then(|value| value.as_str()),
            Some("reply from env")
        );
    }

    #[tokio::test]
    async fn openai_chat_can_read_bearer_key_from_environment() {
        let env_var = "ORDO_TEST_OPENAI_API_KEY";
        unsafe {
            env::set_var(env_var, "sk-openai-env");
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer sk-openai-env"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [
                    {
                        "message": { "content": "reply from openai env" },
                        "finish_reason": "stop"
                    }
                ]
            })))
            .mount(&server)
            .await;
        let credential = CloudCredential {
            service: "openai".into(),
            label: "OpenAI API".into(),
            auth_style: "bearer".into(),
            secret: String::new(),
            base_url: Some(server.uri()),
            extras: HashMap::from([
                ("auth_source".to_string(), "environment".to_string()),
                ("env_var".to_string(), env_var.to_string()),
            ]),
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        let http = CloudHttp::new();
        let args = json!({
            "messages": [ { "role": "user", "content": "hi" } ],
        });
        let result = openai::chat(&http, &credential, &args).await.expect("chat");
        unsafe {
            env::remove_var(env_var);
        }
        assert_eq!(
            result
                .get("assistant_message")
                .and_then(|value| value.as_str()),
            Some("reply from openai env")
        );
    }
}
