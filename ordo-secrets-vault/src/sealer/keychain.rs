//! Tier-3 OS-keychain sealer.
//!
//! The master DEK is stored in the OS keychain under a vault-
//! specific service name; wrap/unwrap just move bytes in and out.
//! The "sealed blob" on disk is a short pointer that the keyring
//! lookup resolves against.
//!
//! Platforms:
//!   - Windows: Credential Manager
//!   - macOS:   Keychain Services
//!   - Linux:   SecretService (GNOME Keyring, KWallet, etc.)
//!
//! Invariant 24: this tier CANNOT sign transparency anchors â€” any
//! process running as the user can reach the keyring, so the
//! signing key has no attestation to external verifiers.

use async_trait::async_trait;
use keyring::Entry;
use ordo_protocol::SealingTier;
use rand::RngCore;

use crate::bytes::SecureBytes;
use crate::sealer::{Sealer, SealerError, SealerResult};

/// On-disk blob layout (self-describing):
///
/// ```text
/// offset   len   field
/// 0        1     version (= 1)
/// 1        8     nonce (u64 LE) â€” not cryptographic; just lets us
///                have multiple sealed blobs per keyring entry
///                during rotation
/// 9        N     UTF-8 of the keyring account name
/// ```
///
/// The account name encodes the vault's identity. The secret
/// material under that name is the actual 32-byte DEK (hex-
/// encoded because `keyring` stores strings, not bytes).
const VERSION: u8 = 1;
const DEK_LEN: usize = 32;
const DEFAULT_SERVICE: &str = "ordo-vault";

pub struct KeychainSealer {
    service: String,
    label: String,
}

impl Default for KeychainSealer {
    fn default() -> Self {
        Self::new(DEFAULT_SERVICE)
    }
}

impl KeychainSealer {
    pub fn new(service: impl Into<String>) -> Self {
        let service = service.into();
        let label = format!("keychain-{service}");
        Self { service, label }
    }

    fn entry(&self, account: &str) -> SealerResult<Entry> {
        Entry::new(&self.service, account)
            .map_err(|err| SealerError::Platform(format!("keyring entry: {err}")))
    }

    /// Generate a fresh keyring account name. Bound to a random
    /// 8-byte nonce so concurrent vault setups don't collide.
    fn fresh_account() -> (String, u64) {
        let nonce = rand::thread_rng().next_u64();
        (format!("vault-dek-{nonce:016x}"), nonce)
    }
}

#[async_trait]
impl Sealer for KeychainSealer {
    fn tier(&self) -> SealingTier {
        SealingTier::Tier3OsKeychain
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn wrap(&self, plaintext_key: &SecureBytes) -> SealerResult<Vec<u8>> {
        if plaintext_key.len() != DEK_LEN {
            return Err(SealerError::Crypto(format!(
                "keychain wrap expects a {DEK_LEN}-byte DEK, got {}",
                plaintext_key.len()
            )));
        }
        let (account, nonce) = Self::fresh_account();
        let hex_encoded = hex::encode(plaintext_key.as_slice());
        let entry = self.entry(&account)?;
        entry
            .set_password(&hex_encoded)
            .map_err(|err| SealerError::Platform(format!("keyring set_password: {err}")))?;

        let mut out = Vec::with_capacity(1 + 8 + account.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce.to_le_bytes());
        out.extend_from_slice(account.as_bytes());
        Ok(out)
    }

    async fn unwrap(&self, sealed: &[u8]) -> SealerResult<SecureBytes> {
        if sealed.len() < 9 {
            return Err(SealerError::Crypto(
                "keychain unwrap: blob too short".into(),
            ));
        }
        if sealed[0] != VERSION {
            return Err(SealerError::Crypto(format!(
                "keychain unwrap: unknown version {}",
                sealed[0]
            )));
        }
        let account = std::str::from_utf8(&sealed[9..])
            .map_err(|err| {
                SealerError::Crypto(format!("keychain unwrap: bad account utf8: {err}"))
            })?
            .to_string();
        let entry = self.entry(&account)?;
        let hex_encoded = entry
            .get_password()
            .map_err(|err| SealerError::Platform(format!("keyring get_password: {err}")))?;
        let bytes = hex::decode(&hex_encoded)
            .map_err(|err| SealerError::Crypto(format!("keychain unwrap: hex decode: {err}")))?;
        if bytes.len() != DEK_LEN {
            return Err(SealerError::Crypto(format!(
                "keychain unwrap: DEK length mismatch ({} != {DEK_LEN})",
                bytes.len()
            )));
        }
        Ok(SecureBytes::new(bytes))
    }

    async fn probe(&self) -> SealerResult<()> {
        // Round-trip a throwaway entry to confirm the OS keyring
        // is reachable and the process has permission to use it.
        let probe_account = format!("vault-probe-{}", rand::thread_rng().next_u64());
        let entry = self.entry(&probe_account)?;
        // Best-effort: write something small, read it, delete.
        entry
            .set_password("probe")
            .map_err(|err| SealerError::Unavailable(format!("keyring write: {err}")))?;
        let read = entry
            .get_password()
            .map_err(|err| SealerError::Unavailable(format!("keyring read: {err}")))?;
        if read != "probe" {
            let _ = entry.delete_credential();
            return Err(SealerError::Unavailable(
                "keyring round-trip returned unexpected value".into(),
            ));
        }
        let _ = entry.delete_credential();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE â€” these tests exercise the real OS keychain. Running
    // them pollutes the keychain with test entries; each test
    // cleans up after itself. On CI without a desktop session
    // (headless Linux without SecretService), these will fail the
    // probe and skip; that's expected.

    #[tokio::test]
    async fn tier_is_tier3_and_cannot_sign_anchors() {
        let sealer = KeychainSealer::new("ordo-test-tier-only");
        assert_eq!(sealer.tier(), SealingTier::Tier3OsKeychain);
        assert!(!sealer.tier().can_sign_transparency_anchors());
    }

    #[tokio::test]
    async fn wrap_rejects_wrong_dek_length() {
        let sealer = KeychainSealer::new("ordo-test-wrap-reject");
        let short = SecureBytes::from_slice(&[0u8; 16]);
        let err = sealer.wrap(&short).await.unwrap_err();
        assert!(matches!(err, SealerError::Crypto(_)));
    }
}
