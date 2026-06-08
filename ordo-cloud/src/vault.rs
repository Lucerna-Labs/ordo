//! Credential vault â€” the boundary between the SQLite metadata row and the
//! raw secret bytes.
//!
//! The store never writes raw secrets into SQLite when a vault is active.
//! Instead the `secret` column holds a small sentinel (`"keyring:v1"`) that
//! tells the loader to ask the vault for the real value. Rows that already
//! contain a plaintext value keep working â€” that is the migration path for
//! older installs.

use std::sync::{Arc, Mutex};

use tracing::warn;

/// Sentinel stored in the SQLite `secret` column when the real secret lives
/// in the vault.
pub const KEYRING_SENTINEL: &str = "keyring:v1";

/// Keyring service name used for all cloud credentials.
const KEYRING_SERVICE: &str = "ordo";

/// Returns `true` when the stored SQLite value points at the vault rather
/// than containing the secret inline.
pub fn is_vault_sentinel(value: &str) -> bool {
    value == KEYRING_SENTINEL
}

/// Build the keyring username for a given cloud service.
fn keyring_username(service: &str) -> String {
    format!("cloud:{service}")
}

/// Abstraction over the secret backing store. The production implementation
/// is the OS keychain via [`KeyringVault`]; tests use [`MemoryVault`]; and
/// headless environments fall back to [`NullVault`] (plaintext in SQLite).
pub trait CredentialVault: Send + Sync {
    /// Name of the vault implementation, reported to the operator.
    fn name(&self) -> &'static str;

    /// Store `secret` for `service`. Must overwrite any prior value.
    fn set(&self, service: &str, secret: &str) -> Result<(), VaultError>;

    /// Fetch the secret for `service`, if any.
    fn get(&self, service: &str) -> Result<Option<String>, VaultError>;

    /// Remove the secret for `service`. A missing entry is not an error.
    fn delete(&self, service: &str) -> Result<(), VaultError>;
}

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault backend error: {0}")]
    Backend(String),
}

/// In-memory vault â€” used in tests and as a fallback when the OS keyring is
/// unavailable (e.g. a headless Linux container with no D-Bus session).
#[derive(Default)]
pub struct MemoryVault {
    inner: Mutex<std::collections::HashMap<String, String>>,
}

impl MemoryVault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }
}

impl CredentialVault for MemoryVault {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn set(&self, service: &str, secret: &str) -> Result<(), VaultError> {
        self.inner
            .lock()
            .map_err(|err| VaultError::Backend(err.to_string()))?
            .insert(service.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, service: &str) -> Result<Option<String>, VaultError> {
        Ok(self
            .inner
            .lock()
            .map_err(|err| VaultError::Backend(err.to_string()))?
            .get(service)
            .cloned())
    }

    fn delete(&self, service: &str) -> Result<(), VaultError> {
        self.inner
            .lock()
            .map_err(|err| VaultError::Backend(err.to_string()))?
            .remove(service);
        Ok(())
    }
}

/// Null vault â€” keeps secrets in SQLite exactly like the old behavior.
/// Used when the OS keyring is not available and the operator has opted
/// into the plaintext path with `ORDO_CLOUD_ALLOW_PLAINTEXT=1`.
pub struct NullVault;

impl CredentialVault for NullVault {
    fn name(&self) -> &'static str {
        "plaintext"
    }

    fn set(&self, _service: &str, _secret: &str) -> Result<(), VaultError> {
        // Intentionally a no-op: the store writes the secret directly to
        // SQLite when the vault is null.
        Ok(())
    }

    fn get(&self, _service: &str) -> Result<Option<String>, VaultError> {
        Ok(None)
    }

    fn delete(&self, _service: &str) -> Result<(), VaultError> {
        Ok(())
    }
}

/// OS keychain-backed vault. Uses platform-native storage:
/// - macOS keychain
/// - Windows Credential Manager
/// - Linux Secret Service (kwallet/gnome-keyring)
pub struct KeyringVault;

impl KeyringVault {
    pub fn new() -> Self {
        Self
    }

    /// Try to initialize a keyring-backed vault. Returns `Some(KeyringVault)`
    /// when the OS keychain appears usable, otherwise `None` so callers can
    /// fall back.
    pub fn try_new() -> Option<Self> {
        // A cheap probe: try to open an entry (no I/O to the secret store
        // happens until get/set). We do a real get on a sentinel entry to
        // confirm the backend is reachable.
        let vault = Self::new();
        match vault.get("__probe__") {
            Ok(_) => Some(vault),
            Err(err) => {
                warn!(
                    target: "ordo_cloud::vault",
                    backend = "keyring",
                    error = %err,
                    "OS keychain unavailable; falling back to plaintext vault"
                );
                None
            }
        }
    }
}

impl Default for KeyringVault {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialVault for KeyringVault {
    fn name(&self) -> &'static str {
        "keyring"
    }

    fn set(&self, service: &str, secret: &str) -> Result<(), VaultError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &keyring_username(service))
            .map_err(|err| VaultError::Backend(err.to_string()))?;
        entry
            .set_password(secret)
            .map_err(|err| VaultError::Backend(err.to_string()))
    }

    fn get(&self, service: &str) -> Result<Option<String>, VaultError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &keyring_username(service))
            .map_err(|err| VaultError::Backend(err.to_string()))?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(VaultError::Backend(err.to_string())),
        }
    }

    fn delete(&self, service: &str) -> Result<(), VaultError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &keyring_username(service))
            .map_err(|err| VaultError::Backend(err.to_string()))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(VaultError::Backend(err.to_string())),
        }
    }
}

/// Build the default vault:
/// - Prefer the OS keychain.
/// - Fall back to an in-memory vault when `ORDO_CLOUD_VAULT=memory` is set
///   (used by tests and headless environments).
/// - Fall back to the plaintext (null) vault when
///   `ORDO_CLOUD_ALLOW_PLAINTEXT=1` is set.
pub fn default_vault() -> Arc<dyn CredentialVault> {
    match std::env::var("ORDO_CLOUD_VAULT").ok().as_deref() {
        Some("memory") => return MemoryVault::shared(),
        Some("plaintext") => return Arc::new(NullVault),
        Some("keyring") | None => {}
        Some(other) => {
            warn!(
                target: "ordo_cloud::vault",
                requested = other,
                "unknown ORDO_CLOUD_VAULT value; using default selection"
            );
        }
    }

    if let Some(vault) = KeyringVault::try_new() {
        return Arc::new(vault);
    }
    if matches!(
        std::env::var("ORDO_CLOUD_ALLOW_PLAINTEXT").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    ) {
        warn!(
            target: "ordo_cloud::vault",
            "OS keychain unavailable and ORDO_CLOUD_ALLOW_PLAINTEXT=1 is set; \
             storing cloud secrets in the local SQLite file"
        );
        return Arc::new(NullVault);
    }
    warn!(
        target: "ordo_cloud::vault",
        "OS keychain unavailable; using an in-memory vault for this process. \
         Set ORDO_CLOUD_ALLOW_PLAINTEXT=1 to persist secrets in SQLite \
         (not recommended) or ORDO_CLOUD_VAULT=memory to silence this warning."
    );
    MemoryVault::shared()
}
