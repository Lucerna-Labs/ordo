//! Tier-4 software-fallback sealer.
//!
//! Wraps the master DEK by deriving a KEK from a user passphrase
//! plus per-vault salt via Argon2id, then XOR-wrapping the DEK under
//! the KEK. "XOR-wrap" means key XOR KEK â€” but a proper KDF over
//! a high-entropy salt gives this the security of the KDF itself.
//! We use a 32-byte KEK to match the DEK length; sealed output is
//! `salt || xor(DEK, KEK) || tag` where `tag` is blake3(KEK || DEK
//! ciphertext) for integrity.
//!
//! Parameters (memory, iterations, parallelism) follow the
//! `argon2` crate's defaults plus a bump: m=65536 KiB, t=3, p=1.
//! This is slow enough to discourage offline brute force but
//! still completes in well under a second on any reasonable CPU.

use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;
use ordo_protocol::SealingTier;
use rand::RngCore;
use zeroize::Zeroize;

use crate::bytes::SecureBytes;
use crate::sealer::{Sealer, SealerError, SealerResult};

/// Serialized sealed blob layout (little-endian, self-describing
/// so a future parameter change is detectable on unwrap):
///
/// ```text
/// offset   len   field
/// 0        1     version          (= 1)
/// 1        4     argon2 memory_kib (u32 LE)
/// 5        4     argon2 iterations (u32 LE)
/// 9        1     argon2 parallelism (u8)
/// 10       16    salt
/// 26       32    xor-wrapped DEK
/// 58       32    integrity tag (blake3 of DEK ciphertext || KEK)
/// total    90 bytes
/// ```
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const DEK_LEN: usize = 32;
const TAG_LEN: usize = 32;
const WRAPPED_LEN: usize = 1 + 4 + 4 + 1 + SALT_LEN + DEK_LEN + TAG_LEN;

const DEFAULT_MEMORY_KIB: u32 = 64 * 1024;
const DEFAULT_ITERATIONS: u32 = 3;
const DEFAULT_PARALLELISM: u8 = 1;

#[derive(Debug)]
pub struct Argon2idSealer {
    passphrase: zeroize::Zeroizing<Vec<u8>>,
    label: String,
}

impl Argon2idSealer {
    /// Build a sealer from a user-supplied passphrase. The
    /// passphrase is zeroized on drop. Empty passphrase is
    /// rejected â€” a 0-entropy passphrase is worse than no sealer
    /// at all because it pretends to protect.
    pub fn new(passphrase: impl Into<Vec<u8>>) -> SealerResult<Self> {
        let pass: Vec<u8> = passphrase.into();
        if pass.is_empty() {
            return Err(SealerError::Platform(
                "argon2id sealer requires a non-empty passphrase".into(),
            ));
        }
        Ok(Self {
            passphrase: zeroize::Zeroizing::new(pass),
            label: "argon2id-default".to_string(),
        })
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    fn derive_kek(
        &self,
        salt: &[u8; SALT_LEN],
        memory_kib: u32,
        iterations: u32,
        parallelism: u8,
    ) -> SealerResult<[u8; DEK_LEN]> {
        let params = Params::new(memory_kib, iterations, parallelism as u32, Some(DEK_LEN))
            .map_err(|err| SealerError::Crypto(format!("argon2 params: {err}")))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut kek = [0u8; DEK_LEN];
        argon
            .hash_password_into(self.passphrase.as_slice(), salt, &mut kek)
            .map_err(|err| SealerError::Crypto(format!("argon2 hash: {err}")))?;
        Ok(kek)
    }
}

#[async_trait]
impl Sealer for Argon2idSealer {
    fn tier(&self) -> SealingTier {
        SealingTier::Tier4SoftwareFallback
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn wrap(&self, plaintext_key: &SecureBytes) -> SealerResult<Vec<u8>> {
        if plaintext_key.len() != DEK_LEN {
            return Err(SealerError::Crypto(format!(
                "argon2id wrap expects a {DEK_LEN}-byte DEK, got {}",
                plaintext_key.len()
            )));
        }
        let mut salt = [0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);

        let mut kek = self.derive_kek(
            &salt,
            DEFAULT_MEMORY_KIB,
            DEFAULT_ITERATIONS,
            DEFAULT_PARALLELISM,
        )?;
        let mut wrapped_dek = [0u8; DEK_LEN];
        for i in 0..DEK_LEN {
            wrapped_dek[i] = plaintext_key.as_slice()[i] ^ kek[i];
        }

        // Integrity tag binds wrapped DEK + KEK. Verifying on
        // unwrap confirms the passphrase was correct before we
        // treat the XOR output as a DEK.
        let mut hasher = blake3::Hasher::new();
        hasher.update(&wrapped_dek);
        hasher.update(&kek);
        let tag: [u8; TAG_LEN] = *hasher.finalize().as_bytes();

        let mut out = Vec::with_capacity(WRAPPED_LEN);
        out.push(VERSION);
        out.extend_from_slice(&DEFAULT_MEMORY_KIB.to_le_bytes());
        out.extend_from_slice(&DEFAULT_ITERATIONS.to_le_bytes());
        out.push(DEFAULT_PARALLELISM);
        out.extend_from_slice(&salt);
        out.extend_from_slice(&wrapped_dek);
        out.extend_from_slice(&tag);

        kek.zeroize();
        Ok(out)
    }

    async fn unwrap(&self, sealed: &[u8]) -> SealerResult<SecureBytes> {
        if sealed.len() != WRAPPED_LEN {
            return Err(SealerError::Crypto(format!(
                "argon2id unwrap: expected {WRAPPED_LEN} bytes, got {}",
                sealed.len()
            )));
        }
        let version = sealed[0];
        if version != VERSION {
            return Err(SealerError::Crypto(format!(
                "argon2id unwrap: unknown version {version}"
            )));
        }
        let memory_kib = u32::from_le_bytes(sealed[1..5].try_into().unwrap());
        let iterations = u32::from_le_bytes(sealed[5..9].try_into().unwrap());
        let parallelism = sealed[9];
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&sealed[10..10 + SALT_LEN]);
        let wrapped_start = 10 + SALT_LEN;
        let wrapped_dek: [u8; DEK_LEN] = sealed[wrapped_start..wrapped_start + DEK_LEN]
            .try_into()
            .unwrap();
        let tag_start = wrapped_start + DEK_LEN;
        let tag: [u8; TAG_LEN] = sealed[tag_start..tag_start + TAG_LEN].try_into().unwrap();

        let mut kek = self.derive_kek(&salt, memory_kib, iterations, parallelism)?;

        // Verify tag before trusting the unwrapped DEK.
        let mut hasher = blake3::Hasher::new();
        hasher.update(&wrapped_dek);
        hasher.update(&kek);
        let computed_tag: [u8; TAG_LEN] = *hasher.finalize().as_bytes();
        if computed_tag != tag {
            kek.zeroize();
            return Err(SealerError::Crypto(
                "argon2id unwrap: integrity tag mismatch (passphrase wrong?)".into(),
            ));
        }

        let mut dek = [0u8; DEK_LEN];
        for i in 0..DEK_LEN {
            dek[i] = wrapped_dek[i] ^ kek[i];
        }
        kek.zeroize();
        Ok(SecureBytes::from_slice(&dek))
    }

    async fn probe(&self) -> SealerResult<()> {
        // Argon2id works everywhere with a passphrase. The
        // passphrase non-empty check happens in `new`.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wrap_unwrap_round_trip() {
        let sealer = Argon2idSealer::new(b"correct horse battery staple".to_vec()).unwrap();
        let dek = SecureBytes::from_slice(&[0x42u8; 32]);
        let sealed = sealer.wrap(&dek).await.unwrap();
        assert_eq!(sealed.len(), WRAPPED_LEN);
        let unwrapped = sealer.unwrap(&sealed).await.unwrap();
        assert_eq!(unwrapped.as_slice(), &[0x42u8; 32]);
    }

    #[tokio::test]
    async fn wrong_passphrase_fails_integrity() {
        let good = Argon2idSealer::new(b"correct".to_vec()).unwrap();
        let bad = Argon2idSealer::new(b"wrong".to_vec()).unwrap();
        let dek = SecureBytes::from_slice(&[1u8; 32]);
        let sealed = good.wrap(&dek).await.unwrap();
        let err = bad.unwrap(&sealed).await.unwrap_err();
        assert!(matches!(err, SealerError::Crypto(_)));
    }

    #[tokio::test]
    async fn empty_passphrase_rejected() {
        let err = Argon2idSealer::new(Vec::<u8>::new()).unwrap_err();
        assert!(matches!(err, SealerError::Platform(_)));
    }

    #[tokio::test]
    async fn wrap_rejects_non_32_byte_dek() {
        let sealer = Argon2idSealer::new(b"p".to_vec()).unwrap();
        let err = sealer
            .wrap(&SecureBytes::from_slice(&[0u8; 16]))
            .await
            .unwrap_err();
        assert!(matches!(err, SealerError::Crypto(_)));
    }

    #[tokio::test]
    async fn wrap_produces_different_output_each_time() {
        let sealer = Argon2idSealer::new(b"pw".to_vec()).unwrap();
        let dek = SecureBytes::from_slice(&[7u8; 32]);
        let a = sealer.wrap(&dek).await.unwrap();
        let b = sealer.wrap(&dek).await.unwrap();
        assert_ne!(a, b, "salt is fresh each call so output must differ");
    }

    #[tokio::test]
    async fn probe_always_succeeds_for_valid_passphrase() {
        let sealer = Argon2idSealer::new(b"pw".to_vec()).unwrap();
        sealer.probe().await.unwrap();
    }

    #[tokio::test]
    async fn tier_is_tier4() {
        let sealer = Argon2idSealer::new(b"pw".to_vec()).unwrap();
        assert_eq!(sealer.tier(), SealingTier::Tier4SoftwareFallback);
        assert!(!sealer.tier().can_sign_transparency_anchors());
    }
}
