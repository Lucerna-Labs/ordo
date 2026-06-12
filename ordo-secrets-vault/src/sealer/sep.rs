//! Tier-2 macOS Secure Enclave sealer.
//!
//! On Apple platforms the Secure Enclave Processor (SEP) holds
//! ECC P-256 keys that never leave the chip; signing happens on-
//! chip. We can't directly use SEP as an AEAD key source, but we
//! can use it to sign/authenticate a vault-generated DEK so that
//! unwrap proves the same SEP is present.
//!
//! The composition in this sealer:
//!   1. On `wrap`, generate a random 32-byte DEK (ephemeral).
//!   2. Ask SEP to sign a vault identity payload; the signature
//!      + Argon2id of the passphrase become the KEK material.
//!   3. Wrap the DEK under that KEK.
//!
//! On a different machine (no matching SEP key) or without the
//! passphrase, unwrap fails. This is genuine Tier-2 because the
//! SEP key is non-exportable, per-device, and every unwrap
//! requires an on-chip signing operation.

// Compiled only on macOS via the `#[cfg(target_os = "macos")] pub mod` gate in
// `sealer.rs` (matching the `windows_tbs` pattern — no redundant inner `cfg`).
//
// Scaffold module: wrap/unwrap/probe deliberately return `Unavailable` until the
// security-framework SEP wiring lands (see `sep_sign`). The wrapped-blob layout
// constants and key-derivation helpers below are the stable shape of that pending
// implementation, so they read as dead code today. macOS is the only target that
// compiles this file, and CI runs with `-D warnings`; allow until SEP goes live.
#![allow(dead_code)]

use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;
use ordo_protocol::SealingTier;
use zeroize::Zeroize;

use crate::bytes::SecureBytes;
use crate::sealer::{Sealer, SealerError, SealerResult};

const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const DEK_LEN: usize = 32;
const TAG_LEN: usize = 32;
const SIG_SLOT_LEN: usize = 64; // P-256 signature
const WRAPPED_LEN: usize = 1 + SALT_LEN + SIG_SLOT_LEN + DEK_LEN + TAG_LEN;

pub struct SecureEnclaveSealer {
    passphrase: zeroize::Zeroizing<Vec<u8>>,
    label: String,
    key_tag: String,
}

impl SecureEnclaveSealer {
    pub fn new(passphrase: impl Into<Vec<u8>>) -> SealerResult<Self> {
        let pass: Vec<u8> = passphrase.into();
        if pass.is_empty() {
            return Err(SealerError::Platform(
                "SEP sealer requires a non-empty passphrase".into(),
            ));
        }
        Ok(Self {
            passphrase: zeroize::Zeroizing::new(pass),
            label: "macos-secure-enclave".to_string(),
            key_tag: "io.ordo.vault.sep".to_string(),
        })
    }

    fn derive_kek(&self, salt: &[u8; SALT_LEN], sig: &[u8]) -> SealerResult<[u8; DEK_LEN]> {
        let params = Params::new(64 * 1024, 3, 1, Some(DEK_LEN))
            .map_err(|err| SealerError::Crypto(format!("argon2 params: {err}")))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        // Mix passphrase + SEP signature. The SEP signature is the
        // "hardware binding"; the passphrase prevents a stolen
        // machine+user-session from unwrapping without an
        // additional knowledge factor.
        let mut combined = Vec::with_capacity(self.passphrase.len() + sig.len());
        combined.extend_from_slice(self.passphrase.as_slice());
        combined.extend_from_slice(sig);
        let mut kek = [0u8; DEK_LEN];
        argon
            .hash_password_into(&combined, salt, &mut kek)
            .map_err(|err| SealerError::Crypto(format!("argon2 hash: {err}")))?;
        combined.zeroize();
        Ok(kek)
    }

    /// Ensure the SEP key identified by `key_tag` exists; create
    /// it (non-exportable, kSecAttrTokenIDSecureEnclave) if not.
    /// Returns a 64-byte ECDSA P-256 signature over the provided
    /// challenge.
    ///
    /// Implementation note: the real Security.framework call
    /// sequence is `SecKeyCreateSignature` on a key whose
    /// attribute dictionary includes `kSecAttrTokenIDSecureEnclave`.
    /// This file scaffolds the signature flow; the
    /// security-framework crate's bindings are used directly.
    /// On hosts where SEP isn't available (macOS VMs without it),
    /// `probe` fails and this sealer is skipped.
    fn sep_sign(challenge: &[u8], key_tag: &str) -> SealerResult<Vec<u8>> {
        // SEP integration via security-framework is platform-
        // specific enough that we gate the full key-creation logic
        // behind a feature flag in a follow-up; the probe + sign
        // shape is stable, so the architectural slot is live.
        // Implementations should fill this in by calling
        // SecKeyGeneratePair / SecKeyCreateSignature.
        let _ = (challenge, key_tag);
        Err(SealerError::Unavailable(
            "SEP signing: requires security-framework wiring; not yet live on this build".into(),
        ))
    }
}

#[async_trait]
impl Sealer for SecureEnclaveSealer {
    fn tier(&self) -> SealingTier {
        SealingTier::Tier2SecureElement
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn wrap(&self, _plaintext_key: &SecureBytes) -> SealerResult<Vec<u8>> {
        // Compiles and runs on macOS; returns Unavailable when
        // SEP signing isn't wired. Wiring is one file; the
        // architecture doesn't move.
        Err(SealerError::Unavailable(
            "SEP wrap: wiring pending; probe will report unavailable".into(),
        ))
    }

    async fn unwrap(&self, _sealed: &[u8]) -> SealerResult<SecureBytes> {
        Err(SealerError::Unavailable(
            "SEP unwrap: wiring pending".into(),
        ))
    }

    async fn probe(&self) -> SealerResult<()> {
        // Hard-fail probe on this build â€” the vault falls through
        // to Tier-3 Keychain, which is a real and correct tier
        // for this platform until SEP is wired in.
        Err(SealerError::Unavailable(
            "SEP sealer wiring pending; Tier-3 Keychain available".into(),
        ))
    }
}
