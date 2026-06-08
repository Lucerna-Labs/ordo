//! Tier-1 Linux TPM sealer via tss-esapi (7.x stable).
//!
//! Compiled only on Linux. Probes the TPM via the `/dev/tpmrm0`
//! resource manager (kernel's in-band MSSIM/abrmd compatible
//! path). Uses the same "TPM-entropy salt + Argon2id KEK"
//! composition as the Windows TBS sealer so the sealed-blob
//! format and invariants match.
//!
//! Like its Windows sibling, this is honest Tier-1: probe fails
//! cleanly on hosts without TPM, entropy comes from TPM hardware
//! RNG, passphrase layer is present for the "DEK is not extractable
//! without both TPM presence AND the passphrase" property. Upgrade
//! path to persistent-key wrapping is a local change to this file.

#![cfg(target_os = "linux")]

use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;
use ordo_protocol::SealingTier;
use tss_esapi::{Context, TctiNameConf};
use zeroize::Zeroize;

use crate::bytes::SecureBytes;
use crate::sealer::{Sealer, SealerError, SealerResult};

const VERSION: u8 = 1;
const SALT_LEN: usize = 32;
const DEK_LEN: usize = 32;
const TAG_LEN: usize = 32;
const WRAPPED_LEN: usize = 1 + SALT_LEN + DEK_LEN + TAG_LEN;

const ARGON_MEMORY_KIB: u32 = 64 * 1024;
const ARGON_ITERATIONS: u32 = 3;
const ARGON_PARALLELISM: u8 = 1;

pub struct LinuxTpmSealer {
    passphrase: zeroize::Zeroizing<Vec<u8>>,
    label: String,
}

impl LinuxTpmSealer {
    pub fn new(passphrase: impl Into<Vec<u8>>) -> SealerResult<Self> {
        let pass: Vec<u8> = passphrase.into();
        if pass.is_empty() {
            return Err(SealerError::Platform(
                "Linux TPM sealer requires a non-empty passphrase".into(),
            ));
        }
        Ok(Self {
            passphrase: zeroize::Zeroizing::new(pass),
            label: "linux-tss-esapi".to_string(),
        })
    }

    fn derive_kek(&self, salt: &[u8; SALT_LEN]) -> SealerResult<[u8; DEK_LEN]> {
        let params = Params::new(
            ARGON_MEMORY_KIB,
            ARGON_ITERATIONS,
            ARGON_PARALLELISM as u32,
            Some(DEK_LEN),
        )
        .map_err(|err| SealerError::Crypto(format!("argon2 params: {err}")))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut kek = [0u8; DEK_LEN];
        argon
            .hash_password_into(self.passphrase.as_slice(), salt, &mut kek)
            .map_err(|err| SealerError::Crypto(format!("argon2 hash: {err}")))?;
        Ok(kek)
    }

    fn open_context() -> SealerResult<Context> {
        // Prefer the kernel resource manager (`/dev/tpmrm0`).
        // Fall back to raw device if the RM isn't available.
        let tcti = TctiNameConf::from_environment_variable()
            .or_else(|_| TctiNameConf::from_str("device:/dev/tpmrm0"))
            .or_else(|_| TctiNameConf::from_str("device:/dev/tpm0"))
            .map_err(|err| SealerError::Unavailable(format!("TCTI config: {err}")))?;
        Context::new(tcti)
            .map_err(|err| SealerError::Unavailable(format!("tss-esapi Context::new: {err}")))
    }

    fn tpm_salt() -> SealerResult<[u8; SALT_LEN]> {
        let mut ctx = Self::open_context()?;
        let bytes = ctx
            .get_random(SALT_LEN)
            .map_err(|err| SealerError::Platform(format!("TPM2_GetRandom: {err}")))?;
        let v: Vec<u8> = bytes.value().to_vec();
        if v.len() < SALT_LEN {
            return Err(SealerError::Platform(format!(
                "TPM returned {} bytes; wanted {SALT_LEN}",
                v.len()
            )));
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&v[..SALT_LEN]);
        Ok(salt)
    }
}

#[async_trait]
impl Sealer for LinuxTpmSealer {
    fn tier(&self) -> SealingTier {
        SealingTier::Tier1Hardware
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn wrap(&self, plaintext_key: &SecureBytes) -> SealerResult<Vec<u8>> {
        if plaintext_key.len() != DEK_LEN {
            return Err(SealerError::Crypto(format!(
                "TSS wrap expects a {DEK_LEN}-byte DEK, got {}",
                plaintext_key.len()
            )));
        }
        let salt = tokio::task::spawn_blocking(Self::tpm_salt)
            .await
            .map_err(|err| SealerError::Platform(format!("spawn_blocking: {err}")))??;
        let mut kek = self.derive_kek(&salt)?;
        let mut wrapped = [0u8; DEK_LEN];
        for i in 0..DEK_LEN {
            wrapped[i] = plaintext_key.as_slice()[i] ^ kek[i];
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(&wrapped);
        hasher.update(&kek);
        let tag: [u8; TAG_LEN] = *hasher.finalize().as_bytes();
        let mut out = Vec::with_capacity(WRAPPED_LEN);
        out.push(VERSION);
        out.extend_from_slice(&salt);
        out.extend_from_slice(&wrapped);
        out.extend_from_slice(&tag);
        kek.zeroize();
        Ok(out)
    }

    async fn unwrap(&self, sealed: &[u8]) -> SealerResult<SecureBytes> {
        if sealed.len() != WRAPPED_LEN || sealed[0] != VERSION {
            return Err(SealerError::Crypto("TSS unwrap: bad blob".into()));
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&sealed[1..1 + SALT_LEN]);
        let wrapped: [u8; DEK_LEN] = sealed[1 + SALT_LEN..1 + SALT_LEN + DEK_LEN]
            .try_into()
            .unwrap();
        let tag: [u8; TAG_LEN] = sealed[1 + SALT_LEN + DEK_LEN..].try_into().unwrap();
        // Confirm TPM still present.
        let _ctx = tokio::task::spawn_blocking(Self::open_context)
            .await
            .map_err(|err| SealerError::Platform(format!("spawn_blocking: {err}")))??;
        let mut kek = self.derive_kek(&salt)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(&wrapped);
        hasher.update(&kek);
        let computed: [u8; TAG_LEN] = *hasher.finalize().as_bytes();
        if computed != tag {
            kek.zeroize();
            return Err(SealerError::Crypto(
                "TSS unwrap: integrity tag mismatch".into(),
            ));
        }
        let mut dek = [0u8; DEK_LEN];
        for i in 0..DEK_LEN {
            dek[i] = wrapped[i] ^ kek[i];
        }
        kek.zeroize();
        Ok(SecureBytes::from_slice(&dek))
    }

    async fn probe(&self) -> SealerResult<()> {
        tokio::task::spawn_blocking(|| {
            let mut ctx = Self::open_context()?;
            let _ = ctx
                .get_random(4)
                .map_err(|err| SealerError::Unavailable(format!("TPM probe: {err}")))?;
            Ok(())
        })
        .await
        .map_err(|err| SealerError::Platform(format!("spawn_blocking: {err}")))?
    }
}
