//! Transparency service trait and the built-in local anchor.
//!
//! The trait abstracts "produce a signed receipt for this anchor
//! statement". Today we ship the `LocalAnchorService` which signs
//! with a caller-provided Ed25519 key; tomorrow someone can plug
//! in a Sigstore/Rekor adapter or a CT-style witness network
//! adapter without touching the audit chain itself.
//!
//! Invariant 24 is enforced ONE layer up â€” `AuditService::sign_anchor`
//! calls `signer.signer_tier()` and rejects if the tier can't
//! sign anchors. This module's `LocalAnchorService` publishes its
//! tier honestly so the guard applies.

use std::sync::Arc;

use async_trait::async_trait;
use ed25519_dalek::{Signer, SigningKey};
use ordo_protocol::{AnchorStatement, SealingTier, TransparencyReceipt};
use parking_lot::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum TransparencyError {
    #[error("signing failed: {0}")]
    Signing(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
}

pub type TransparencyResult<T> = Result<T, TransparencyError>;

#[async_trait]
pub trait TransparencyService: Send + Sync {
    fn service_id(&self) -> &str;

    /// The tier of the key that will sign. The audit service
    /// gates anchor signing on this value per invariant 24.
    fn signer_tier(&self) -> SealingTier;

    /// Sign the anchor. Implementations fill in `cose_sign1` in
    /// the returned receipt's anchor; may also include a
    /// `service_attestation` blob (e.g. Rekor inclusion proof).
    async fn sign_anchor(&self, draft: AnchorStatement) -> TransparencyResult<TransparencyReceipt>;
}

/// The built-in local signer. Holds an Ed25519 signing key in
/// memory (typically derived from a Tier-1 sealer at service
/// startup). `new_weak` produces a Tier-3 signer for tests â€” the
/// audit service will refuse to use it for real anchors.
pub struct LocalAnchorService {
    signing_key: Arc<Mutex<SigningKey>>,
    tier: SealingTier,
    service_id: String,
}

impl LocalAnchorService {
    /// Build with a concrete Ed25519 `SigningKey`. Caller chooses
    /// the tier honestly; the audit service won't sign anchors
    /// with a tier that can't.
    pub fn from_ed25519_with_tier(key: SigningKey, tier: SealingTier) -> Self {
        Self {
            signing_key: Arc::new(Mutex::new(key)),
            tier,
            service_id: "ordo.local-anchor".to_string(),
        }
    }

    /// Deliberately-weak fallback used only in tests to confirm
    /// the tier gate rejects Tier-3 signers.
    pub fn new_weak() -> Self {
        use rand::rngs::OsRng;
        Self {
            signing_key: Arc::new(Mutex::new(SigningKey::generate(&mut OsRng))),
            tier: SealingTier::Tier3OsKeychain,
            service_id: "ordo.local-anchor-weak".to_string(),
        }
    }
}

#[async_trait]
impl TransparencyService for LocalAnchorService {
    fn service_id(&self) -> &str {
        &self.service_id
    }

    fn signer_tier(&self) -> SealingTier {
        self.tier
    }

    async fn sign_anchor(
        &self,
        mut draft: AnchorStatement,
    ) -> TransparencyResult<TransparencyReceipt> {
        // Sign via the sign_anchor_cose helper in the crate root.
        // We hand it a closure that calls our in-memory signing
        // key. In a Tier-1 deployment the closure would invoke
        // the TPM; the trait shape is the same.
        let key = self.signing_key.clone();
        let sign_fn: Box<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync> = Box::new(move |pt| {
            let k = key.lock();
            k.sign(pt).to_bytes().to_vec()
        });
        let cose = crate::sign_anchor_cose(&draft, sign_fn.as_ref());
        draft.cose_sign1 = cose;
        Ok(TransparencyReceipt {
            anchor: draft,
            service_attestation: None,
            service_id: self.service_id.clone(),
        })
    }
}
