//! ordo-secrets-vault â€” the sealed-material layer.
//!
//! Responsibility boundary: this crate owns *sealing* and
//! *unsealing*. It does NOT issue capability handles, run DRIFT
//! checks, write audit entries, or talk to the LLM. Those are the
//! broker / audit / threshold crates' jobs.
//!
//! Blueprint commitments this crate enforces:
//!
//! - Master key is wrapped by the highest-available `Sealer` tier
//!   at construction. Fallback to a lower tier emits
//!   `SecretsSealTierDegraded` on the bus (never silent).
//! - Plaintext never exists outside `SecureBytes` (zeroize-on-drop,
//!   no `Clone`, opaque `Debug`).
//! - Rotation destroys the old ciphertext (invariant 23): the
//!   sealed row's ciphertext is overwritten with zeros, the row
//!   marked `retired_at`, and a fresh row inserted.
//! - The `SecretLifecycleBackend` trait abstracts rotation policy
//!   so future puncturable-encryption backends drop in without
//!   touching the service layer.
//!
//! What this crate deliberately does NOT do:
//!
//! - No FROST / threshold logic. Threshold-protected secrets'
//!   material is stored as a placeholder ciphertext; dereference
//!   attempts return `VaultError::ThresholdOnly` so the broker can
//!   route to `ordo-secrets-threshold` for signing.
//! - No direct LLM-facing surface. Callers are the broker (issuing
//!   capability handles) and the audit crate (recording events).

pub mod aead;
pub mod bytes;
pub mod lifecycle;
pub mod sealer;
pub mod service;
pub mod store;

pub use bytes::SecureBytes;
pub use lifecycle::{DekRotation, LifecycleError, LifecycleResult, SecretLifecycleBackend};
pub use sealer::{
    select_highest_available, Argon2idSealer, KeychainSealer, MockSealer, Sealer, SealerError,
    SealerResult,
};
pub use service::{VaultError, VaultResult, VaultService, VaultServiceBuilder};
pub use store::{SealedSecretRow, VaultStateRow, VaultStore, VaultStoreError};

#[cfg(target_os = "linux")]
pub use sealer::LinuxTpmSealer;
#[cfg(target_os = "macos")]
pub use sealer::SecureEnclaveSealer;
#[cfg(target_os = "windows")]
pub use sealer::WindowsTbsSealer;
