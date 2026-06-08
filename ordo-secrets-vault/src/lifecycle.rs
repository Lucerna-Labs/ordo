//! Secret-lifecycle backend.
//!
//! The vault talks to a backend trait so the *policy* (how rotation
//! works, how punctures happen) is orthogonal to the *mechanism*
//! (sealing, AEAD, persistence). Today we ship `DekRotation` — the
//! classic "rotate the DEK, re-seal every active secret under the
//! new DEK, zero the old ciphertext" implementation.
//!
//! Tomorrow we can drop in puncturable encryption (a backend that
//! invalidates specific past capability handles without rotating
//! everything) as a new impl of this trait without touching the
//! service layer. That is the blueprint's explicit architectural
//! slot — the reason this trait exists on the first commit even
//! though only one implementation ships today.
//!
//! Invariant 23 lives here: after rotation, the old DEK's ciphertext
//! MUST be overwritten with zeros and the old row marked retired.

use async_trait::async_trait;

use crate::bytes::SecureBytes;

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("lifecycle backend rejected operation: {0}")]
    Rejected(String),
    #[error("lifecycle backend internal: {0}")]
    Internal(String),
}

pub type LifecycleResult<T> = Result<T, LifecycleError>;

/// Plan for a DEK rotation: the new DEK and the rotation mode.
/// Callers hand this to the vault store's rotate method.
pub struct RotationPlan {
    pub new_dek: SecureBytes,
    /// Incremented generation number. The vault store records this
    /// so an operator auditing the `sealed_secrets.dek_generation`
    /// column can spot rows that missed a rotation pass.
    pub next_generation: u32,
}

/// Abstracts *what* a rotation does. `DekRotation` re-seals every
/// active row under the new DEK; a future `PuncturableRotation`
/// could invalidate a specific handle without touching anything else.
#[async_trait]
pub trait SecretLifecycleBackend: Send + Sync {
    /// Human-readable label for logs / audit entries.
    fn label(&self) -> &str;

    /// Produce a fresh DEK. Default impl uses `OsRng`. Override if
    /// the backend wants a deterministic / committed DEK (e.g.
    /// bound to a PCR quote).
    async fn propose_rotation(&self, current_generation: u32) -> LifecycleResult<RotationPlan>;
}

/// Default implementation: rotate the DEK, the service layer
/// re-seals every live secret.
pub struct DekRotation;

#[async_trait]
impl SecretLifecycleBackend for DekRotation {
    fn label(&self) -> &str {
        "dek-rotation"
    }

    async fn propose_rotation(&self, current_generation: u32) -> LifecycleResult<RotationPlan> {
        use rand::RngCore;
        let mut buf = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        Ok(RotationPlan {
            new_dek: SecureBytes::new(buf),
            next_generation: current_generation.saturating_add(1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dek_rotation_produces_32_byte_key_and_bumps_generation() {
        let backend = DekRotation;
        let plan = backend.propose_rotation(7).await.unwrap();
        assert_eq!(plan.new_dek.len(), 32);
        assert_eq!(plan.next_generation, 8);
    }

    #[tokio::test]
    async fn dek_rotation_is_not_deterministic() {
        let backend = DekRotation;
        let a = backend.propose_rotation(0).await.unwrap();
        let b = backend.propose_rotation(0).await.unwrap();
        assert_ne!(a.new_dek.as_slice(), b.new_dek.as_slice());
    }
}
