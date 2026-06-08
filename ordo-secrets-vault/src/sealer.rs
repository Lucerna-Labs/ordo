//! Master-key sealing.
//!
//! A `Sealer` owns the platform-specific material needed to wrap
//! and unwrap the vault's master DEK. Four tiers are defined;
//! each host picks the highest tier that actually works.
//!
//! The blueprint's non-negotiables:
//!
//! - Tier-1 (TPM/TBS): available on Windows 11 machines with TPM
//!   2.0 (the vast majority). Linux hosts with TPM use tss-esapi.
//! - Tier-2 (Secure Enclave): macOS + iOS only.
//! - Tier-3 (OS keychain): Windows Credential Manager / macOS
//!   Keychain / Linux SecretService. Reachable from user-space
//!   but encrypted at rest; invariant 24 forbids its use as the
//!   transparency-anchor signer.
//! - Tier-4 (Argon2id): pure software. Last resort. Ships on
//!   every platform because it has no OS dependency.
//!
//! The vault probes tiers in order at startup and picks the first
//! that succeeds on the host. When degrading to a lower tier
//! (e.g. after a TPM failure), the vault emits
//! `SealTierDegraded` on the bus â€” silent drops are how
//! "somebody moved us to software crypto without telling anyone"
//! happens.

use async_trait::async_trait;
use ordo_protocol::SealingTier;

use crate::bytes::SecureBytes;

#[derive(Debug, thiserror::Error)]
pub enum SealerError {
    #[error("sealer unavailable on this platform: {0}")]
    Unavailable(String),
    #[error("sealer crypto: {0}")]
    Crypto(String),
    #[error("sealer platform: {0}")]
    Platform(String),
}

pub type SealerResult<T> = Result<T, SealerError>;

/// A `Sealer` wraps/unwraps the vault's master DEK. Implementations
/// must be idempotent: `unwrap(wrap(k)) == k` and no side effects
/// on repeated calls.
#[async_trait]
pub trait Sealer: Send + Sync {
    fn tier(&self) -> SealingTier;

    /// Human-readable label for logs. Distinct per tier and per
    /// impl (e.g. "argon2id-default", "windows-tbs-bound-pcr0").
    fn label(&self) -> &str;

    /// Wrap `plaintext_key` (always 32 bytes, the master DEK)
    /// into a platform-sealed blob that only this sealer can
    /// unwrap on this host. The blob is persisted in the vault's
    /// on-disk state; all ciphertext in `sealed_secrets` is
    /// downstream of this wrap.
    async fn wrap(&self, plaintext_key: &SecureBytes) -> SealerResult<Vec<u8>>;

    async fn unwrap(&self, sealed: &[u8]) -> SealerResult<SecureBytes>;

    /// Cheap availability probe. Called at vault startup to pick
    /// the highest tier that actually works. Implementations
    /// return `Ok(())` on success, `Err(Unavailable)` otherwise.
    async fn probe(&self) -> SealerResult<()>;
}

pub mod argon2id;
pub mod keychain;
pub mod mock;

#[cfg(target_os = "windows")]
pub mod windows_tbs;

#[cfg(target_os = "linux")]
pub mod linux_tpm;

#[cfg(target_os = "macos")]
pub mod sep;

pub use argon2id::Argon2idSealer;
pub use keychain::KeychainSealer;
pub use mock::MockSealer;

#[cfg(target_os = "linux")]
pub use linux_tpm::LinuxTpmSealer;
#[cfg(target_os = "macos")]
pub use sep::SecureEnclaveSealer;
#[cfg(target_os = "windows")]
pub use windows_tbs::WindowsTbsSealer;

/// Probe sealers in descending tier order, return the first
/// available. Callers typically pass the full stack; this helper
/// picks the winner and reports what was skipped (for the
/// `SealTierDegraded` logic).
///
/// Invariant: at least one sealer MUST succeed. Argon2id works on
/// every platform with a passphrase, so including it at the tail
/// guarantees this.
pub async fn select_highest_available(
    sealers: Vec<Box<dyn Sealer>>,
) -> SealerResult<(Box<dyn Sealer>, Vec<SealingTier>)> {
    let mut skipped = Vec::new();
    for sealer in sealers {
        match sealer.probe().await {
            Ok(()) => return Ok((sealer, skipped)),
            Err(err) => {
                tracing::debug!(
                    target: "ordo_secrets_vault::sealer",
                    tier = sealer.tier().label(),
                    error = %err,
                    "sealer probe failed; trying next"
                );
                skipped.push(sealer.tier());
            }
        }
    }
    Err(SealerError::Unavailable(
        "no sealer was available; include Tier-4 Argon2id as a fallback".into(),
    ))
}
