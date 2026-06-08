//! ordo-secrets-audit â€” append-only hash-chained audit of every
//! secret operation, plus COSE_Sign1-wrapped transparency anchors
//! over contiguous slices of the chain.
//!
//! Responsibility boundary: this crate owns the *history-of-record*
//! for secret operations. It does not seal, broker, or threshold-
//! sign. It accepts audit events (emitted by the vault/broker/
//! threshold crates via the bus) and persists them in a tamper-
//! evident chain. Periodically it folds a slice of the chain into
//! an `AnchorStatement` and signs it via a
//! `TransparencyService` impl.
//!
//! Invariants:
//!
//! - **Genesis prev_hash is all zeros.** Every subsequent entry's
//!   `prev_hash` equals `blake3(canonical bytes of the previous
//!   entry)`. Breaking the chain is detectable; `verify_chain`
//!   walks it end-to-end.
//! - **Invariant 24** (blueprint): anchor signing requires a key
//!   sealed at Tier-1 or Tier-2. Enforced at the signer boundary
//!   via `SealingTier::can_sign_transparency_anchors`.
//! - **Anchors are idempotent** over the sequence range they cover.
//!   Signing the same range twice produces two anchors with the
//!   same chain_root; this is fine (a service may sign multiple
//!   times for different audiences).

use std::sync::Arc;

use async_trait::async_trait;
use blake3::Hasher;
use chrono::Utc;
use coset::{iana, CborSerializable, CoseSign1Builder, HeaderBuilder};
use ordo_bus::Bus;
use ordo_protocol::{
    secrets_topics, AnchorStatement, AuditEntry, BusEnvelope, Envelope, NodeId, OrdoMessage,
    SealingTier, SecretAuditEventType, TransparencyReceipt,
};
use serde_json::Value;
use tokio::sync::Mutex;

pub mod store;
pub mod transparency;

pub use store::{AuditStore, AuditStoreError};
pub use transparency::{LocalAnchorService, TransparencyError, TransparencyService};

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("store: {0}")]
    Store(#[from] AuditStoreError),
    #[error("chain broken at sequence {0}: prev_hash mismatch")]
    ChainBroken(u64),
    #[error("anchor signer rejected: {0}")]
    Signer(String),
    #[error("tier {0:?} is not allowed to sign anchors (invariant 24)")]
    TierNotAllowed(SealingTier),
    #[error("transparency: {0}")]
    Transparency(#[from] TransparencyError),
    #[error("bad input: {0}")]
    BadInput(String),
}

pub type AuditResult<T> = Result<T, AuditError>;

pub struct AuditService {
    store: Mutex<AuditStore>,
    workspace_id: String,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
}

impl AuditService {
    pub fn new(store: AuditStore, workspace_id: impl Into<String>) -> Self {
        Self {
            store: Mutex::new(store),
            workspace_id: workspace_id.into(),
            bus: None,
            node_id: NodeId::new(),
        }
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_node_id(mut self, node_id: NodeId) -> Self {
        self.node_id = node_id;
        self
    }

    pub async fn append(
        &self,
        event_type: SecretAuditEventType,
        payload: Value,
    ) -> AuditResult<AuditEntry> {
        let mut store = self.store.lock().await;
        let (sequence, prev_hash) = store.next_sequence_and_prev_hash(&self.workspace_id)?;
        let id = ulid::Ulid::new().to_string();
        let timestamp = Utc::now();
        let entry = AuditEntry {
            id,
            sequence,
            prev_hash,
            timestamp,
            event_type,
            payload,
        };
        let entry_hash = hash_entry(&entry);
        store.insert_entry(&self.workspace_id, &entry, &entry_hash)?;
        drop(store);

        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::SecretsAuditEntryAppended {
                    entry_id: entry.id.clone(),
                    sequence: entry.sequence,
                    event_type,
                },
            );
            let _ = bus.publish(secrets_topics::AUDIT_ENTRY_APPENDED, env).await;
        }
        Ok(entry)
    }

    pub async fn verify_chain(&self) -> AuditResult<u64> {
        let store = self.store.lock().await;
        let all = store.list_all(&self.workspace_id)?;
        let mut expected_prev = [0u8; 32];
        let mut last_sequence = 0u64;
        for (entry, stored_hash) in all {
            if entry.prev_hash != expected_prev {
                return Err(AuditError::ChainBroken(entry.sequence));
            }
            let computed = hash_entry(&entry);
            if computed != stored_hash {
                return Err(AuditError::ChainBroken(entry.sequence));
            }
            expected_prev = stored_hash;
            last_sequence = entry.sequence;
        }
        Ok(last_sequence)
    }

    pub async fn sign_anchor(
        &self,
        first_sequence: u64,
        last_sequence: u64,
        signer: &dyn TransparencyService,
    ) -> AuditResult<TransparencyReceipt> {
        if last_sequence < first_sequence {
            return Err(AuditError::BadInput(format!(
                "last_sequence {last_sequence} < first_sequence {first_sequence}"
            )));
        }
        let signer_tier = signer.signer_tier();
        if !signer_tier.can_sign_transparency_anchors() {
            return Err(AuditError::TierNotAllowed(signer_tier));
        }

        let store = self.store.lock().await;
        let slice = store.list_range(&self.workspace_id, first_sequence, last_sequence)?;
        if slice.is_empty() {
            return Err(AuditError::BadInput(format!(
                "no entries in [{first_sequence}, {last_sequence}]"
            )));
        }
        let chain_root = fold_range_root(&slice);
        drop(store);

        let draft = AnchorStatement {
            workspace_id: self.workspace_id.clone(),
            first_sequence,
            last_sequence,
            chain_root,
            signed_at: Utc::now(),
            cose_sign1: Vec::new(),
            signer_tier,
        };
        let receipt = signer.sign_anchor(draft).await?;

        let mut store = self.store.lock().await;
        store.insert_anchor(&self.workspace_id, &receipt)?;
        drop(store);

        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::SecretsAuditAnchorSigned(receipt.anchor.clone()),
            );
            let _ = bus.publish(secrets_topics::AUDIT_ANCHOR_SIGNED, env).await;
        }
        Ok(receipt)
    }
}

fn hash_entry(entry: &AuditEntry) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(entry.id.as_bytes());
    h.update(&entry.sequence.to_be_bytes());
    h.update(&entry.prev_hash);
    h.update(entry.timestamp.to_rfc3339().as_bytes());
    h.update(entry_type_label(entry.event_type).as_bytes());
    h.update(&canonical_json(&entry.payload));
    *h.finalize().as_bytes()
}

fn fold_range_root(slice: &[(AuditEntry, [u8; 32])]) -> [u8; 32] {
    let mut h = Hasher::new();
    for (_, hash) in slice {
        h.update(hash);
    }
    *h.finalize().as_bytes()
}

fn entry_type_label(t: SecretAuditEventType) -> &'static str {
    match t {
        SecretAuditEventType::SecretCreated => "secret_created",
        SecretAuditEventType::SecretRetired => "secret_retired",
        SecretAuditEventType::SecretRotated => "secret_rotated",
        SecretAuditEventType::HandleIssued => "handle_issued",
        SecretAuditEventType::HandleDereferenced => "handle_dereferenced",
        SecretAuditEventType::HandleExpired => "handle_expired",
        SecretAuditEventType::HandleRevoked => "handle_revoked",
        SecretAuditEventType::ThresholdSigningBegan => "threshold_signing_began",
        SecretAuditEventType::ThresholdSigningCompleted => "threshold_signing_completed",
        SecretAuditEventType::ThresholdShareRedistributed => "threshold_share_redistributed",
        SecretAuditEventType::CanaryDetected => "canary_detected",
        SecretAuditEventType::CustodyMismatchDetected => "custody_mismatch_detected",
        SecretAuditEventType::StructuralLimitExceeded => "structural_limit_exceeded",
        SecretAuditEventType::AnchorSigned => "anchor_signed",
        SecretAuditEventType::RotationDue => "rotation_due",
        SecretAuditEventType::SealTierDegraded => "seal_tier_degraded",
    }
}

fn canonical_json(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("serde_json::to_vec should not fail on an audit payload")
}

/// COSE_Sign1 helper. Signs the anchor's `chain_root` under the
/// provided Ed25519 signing callback; binds via AAD to the
/// workspace + sequence range.
pub fn sign_anchor_cose(
    anchor: &AnchorStatement,
    sign_fn: &(dyn Fn(&[u8]) -> Vec<u8> + Send + Sync),
) -> Vec<u8> {
    let protected = HeaderBuilder::new()
        .algorithm(iana::Algorithm::EdDSA)
        .build();
    let aad = anchor_aad(anchor);
    let sign1 = CoseSign1Builder::new()
        .protected(protected)
        .payload(anchor.chain_root.to_vec())
        .create_signature(&aad, |pt| sign_fn(pt))
        .build();
    sign1.to_vec().expect("cose_sign1 encode")
}

fn anchor_aad(anchor: &AnchorStatement) -> Vec<u8> {
    let mut aad = Vec::with_capacity(64);
    aad.extend_from_slice(b"ordo.audit.anchor|v1|");
    aad.extend_from_slice(anchor.workspace_id.as_bytes());
    aad.push(b'|');
    aad.extend_from_slice(&anchor.first_sequence.to_be_bytes());
    aad.push(b'|');
    aad.extend_from_slice(&anchor.last_sequence.to_be_bytes());
    aad
}

pub fn verify_anchor_cose(
    anchor: &AnchorStatement,
    cose_sign1: &[u8],
    verify_fn: &(dyn Fn(&[u8], &[u8]) -> bool + Send + Sync),
) -> bool {
    let Ok(parsed) = coset::CoseSign1::from_slice(cose_sign1) else {
        return false;
    };
    if parsed.payload.as_deref() != Some(anchor.chain_root.as_slice()) {
        return false;
    }
    let aad = anchor_aad(anchor);
    parsed
        .verify_signature(&aad, |sig, data| {
            if verify_fn(sig, data) {
                Ok::<(), ()>(())
            } else {
                Err(())
            }
        })
        .is_ok()
}

#[async_trait]
pub trait AnchorSigner: Send + Sync {
    fn tier(&self) -> SealingTier;
    async fn sign(&self, bytes: &[u8]) -> AuditResult<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{SigningKey, Verifier, VerifyingKey};
    use rand::rngs::OsRng;
    use serde_json::json;

    async fn append_event(svc: &AuditService, t: SecretAuditEventType, p: Value) -> AuditEntry {
        svc.append(t, p).await.unwrap()
    }

    #[tokio::test]
    async fn append_builds_hash_chain_starting_with_zero_prev() {
        let store = AuditStore::in_memory().unwrap();
        let svc = AuditService::new(store, "local");
        let e1 = append_event(
            &svc,
            SecretAuditEventType::SecretCreated,
            json!({ "secret_id": "a" }),
        )
        .await;
        assert_eq!(e1.sequence, 1);
        assert_eq!(e1.prev_hash, [0u8; 32]);
        let e2 = append_event(
            &svc,
            SecretAuditEventType::HandleIssued,
            json!({ "handle_id": "x" }),
        )
        .await;
        assert_eq!(e2.sequence, 2);
        assert_ne!(e2.prev_hash, [0u8; 32]);
        assert_eq!(svc.verify_chain().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn verify_chain_detects_tampering_via_store_hook() {
        let store = AuditStore::in_memory().unwrap();
        let svc = AuditService::new(store, "local");
        let _e1 = append_event(&svc, SecretAuditEventType::SecretCreated, json!({ "x": 1 })).await;
        let _e2 = append_event(&svc, SecretAuditEventType::SecretCreated, json!({ "x": 2 })).await;
        {
            let mut store = svc.store.lock().await;
            store
                .test_tamper_payload("local", 1, json!({ "x": 999 }))
                .unwrap();
        }
        let err = svc.verify_chain().await.unwrap_err();
        assert!(matches!(err, AuditError::ChainBroken(1)));
    }

    #[tokio::test]
    async fn sign_anchor_refuses_tier_3_or_below() {
        let store = AuditStore::in_memory().unwrap();
        let svc = AuditService::new(store, "local");
        let _e = append_event(&svc, SecretAuditEventType::SecretCreated, json!({"x": 1})).await;
        let signer = LocalAnchorService::new_weak();
        let err = svc.sign_anchor(1, 1, &signer).await.unwrap_err();
        assert!(matches!(err, AuditError::TierNotAllowed(_)));
    }

    #[tokio::test]
    async fn sign_anchor_with_tier1_produces_verifiable_cose_sign1() {
        let store = AuditStore::in_memory().unwrap();
        let svc = AuditService::new(store, "local");
        for i in 0..5u32 {
            let _ =
                append_event(&svc, SecretAuditEventType::SecretCreated, json!({ "i": i })).await;
        }
        let mut rng = OsRng;
        let signing_key = SigningKey::generate(&mut rng);
        let verifying_key: VerifyingKey = signing_key.verifying_key();
        let signer =
            LocalAnchorService::from_ed25519_with_tier(signing_key, SealingTier::Tier1Hardware);
        let receipt = svc.sign_anchor(1, 5, &signer).await.unwrap();
        assert_eq!(receipt.anchor.first_sequence, 1);
        assert_eq!(receipt.anchor.last_sequence, 5);
        assert!(!receipt.anchor.cose_sign1.is_empty());
        let ok = verify_anchor_cose(&receipt.anchor, &receipt.anchor.cose_sign1, &|sig, data| {
            let Ok(bytes) = <[u8; 64]>::try_from(sig) else {
                return false;
            };
            let signature = ed25519_dalek::Signature::from_bytes(&bytes);
            verifying_key.verify(data, &signature).is_ok()
        });
        assert!(
            ok,
            "signed anchor must verify under the signer's public key"
        );
    }

    #[tokio::test]
    async fn sign_anchor_with_empty_range_fails_cleanly() {
        let store = AuditStore::in_memory().unwrap();
        let svc = AuditService::new(store, "local");
        let signer = LocalAnchorService::from_ed25519_with_tier(
            SigningKey::generate(&mut OsRng),
            SealingTier::Tier1Hardware,
        );
        let err = svc.sign_anchor(10, 20, &signer).await.unwrap_err();
        assert!(matches!(err, AuditError::BadInput(_)));
    }

    #[tokio::test]
    async fn sign_anchor_reversed_range_rejected() {
        let store = AuditStore::in_memory().unwrap();
        let svc = AuditService::new(store, "local");
        let signer = LocalAnchorService::from_ed25519_with_tier(
            SigningKey::generate(&mut OsRng),
            SealingTier::Tier1Hardware,
        );
        let err = svc.sign_anchor(5, 1, &signer).await.unwrap_err();
        assert!(matches!(err, AuditError::BadInput(_)));
    }
}
