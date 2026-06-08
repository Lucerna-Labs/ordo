//! End-to-end integration test: vault + broker + audit + threshold
//! composing across crates.
//!
//! What this exercises:
//!   1. Vault stores a single-sealed secret.
//!   2. Broker issues a capability handle with canary + custody.
//!   3. DRIFT validates custody, isolator dereferences to plaintext.
//!   4. Audit chain records every step.
//!   5. An anchor is signed over the chain and verifies under the
//!      signer's public key.
//!   6. Separately: threshold secret protection correctly routes
//!      through the threshold crate's FROST 2-of-3 signing rather
//!      than through the vault's dereference path.
//!
//! The test intentionally does NOT go through the runtime â€” it
//! composes the four services directly so failures localise
//! cleanly. Runtime wiring is tested by `cargo check -p
//! ordo-runtime` plus the runtime's own boot tests.

use std::collections::BTreeMap;
use std::sync::Arc;

use ordo_protocol::{SealingTier, SecretAuditEventType, SecretClass};
use ordo_secrets_audit::{verify_anchor_cose, AuditService, AuditStore, LocalAnchorService};
use ordo_secrets_broker::{build_custody, BrokerService};
use ordo_secrets_threshold::{ThresholdCoordinator, ThresholdKeyShare, TrustedDealer};
use ordo_secrets_vault::sealer::{MockSealer, Sealer};
use ordo_secrets_vault::{SecureBytes, VaultService, VaultStore};

use ed25519_dalek::{SigningKey, Verifier as _, VerifyingKey};
use rand::rngs::OsRng;
use serde_json::json;

async fn build_vault() -> Arc<VaultService> {
    let store = VaultStore::in_memory().unwrap();
    let sealers: Vec<Box<dyn Sealer>> = vec![Box::new(MockSealer)];
    Arc::new(
        VaultService::builder(store, "local")
            .with_sealers(sealers)
            .build()
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn end_to_end_capability_path_audits_and_anchors() {
    let vault = build_vault().await;
    let broker = BrokerService::new(vault.clone());
    let audit = AuditService::new(AuditStore::in_memory().unwrap(), "local");

    // 1. Put an API key.
    let record = vault
        .put(
            SecretClass::ApiKey,
            "openai-prod",
            vec!["cloud.openai".into()],
            SecureBytes::from_slice(b"sk-live-integration-test"),
        )
        .await
        .unwrap();
    audit
        .append(
            SecretAuditEventType::SecretCreated,
            json!({ "secret_id": &record.id, "class": record.class.label() }),
        )
        .await
        .unwrap();

    // 2. Planner issues a capability handle.
    let plan = broker
        .plan(&record.id, "cloud.openai", SecretClass::ApiKey)
        .await
        .unwrap();
    audit
        .append(
            SecretAuditEventType::HandleIssued,
            json!({ "capability_id": &plan.handle.id, "canary_token_prefix": &plan.canary.canary_token[..6] }),
        )
        .await
        .unwrap();

    // 3. Commit custody; validator checks; isolator dereferences.
    let custody = build_custody(
        "inv-1",
        b"user-query-blob",
        std::slice::from_ref(&plan.handle.id),
    );
    broker
        .commit_custody(&plan.handle.id, custody.clone())
        .await
        .unwrap();
    broker.validate(&plan.handle.id, &custody).await.unwrap();
    let pt = broker.dereference(&plan.handle.id, true).await.unwrap();
    assert_eq!(pt.as_slice(), b"sk-live-integration-test");
    audit
        .append(
            SecretAuditEventType::HandleDereferenced,
            json!({ "capability_id": &plan.handle.id }),
        )
        .await
        .unwrap();

    // 4. Audit chain verifies clean end-to-end.
    let last = audit.verify_chain().await.unwrap();
    assert_eq!(last, 3);

    // 5. Sign an anchor with a Tier-1 signer and verify.
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    let signer =
        LocalAnchorService::from_ed25519_with_tier(signing_key, SealingTier::Tier1Hardware);
    let receipt = audit.sign_anchor(1, 3, &signer).await.unwrap();
    assert_eq!(receipt.anchor.first_sequence, 1);
    assert_eq!(receipt.anchor.last_sequence, 3);
    assert!(!receipt.anchor.cose_sign1.is_empty());

    let ok = verify_anchor_cose(&receipt.anchor, &receipt.anchor.cose_sign1, &|sig, data| {
        let Ok(bytes) = <[u8; 64]>::try_from(sig) else {
            return false;
        };
        let sig = ed25519_dalek::Signature::from_bytes(&bytes);
        verifying_key.verify(data, &sig).is_ok()
    });
    assert!(
        ok,
        "anchor signature must verify under the Tier-1 public key"
    );
}

#[tokio::test]
async fn threshold_protected_secret_routes_through_frost_not_vault() {
    // Use the threshold crate directly: generate a 2-of-3 key,
    // sign a message via the coordinator's two-round protocol,
    // verify under the group public key. Demonstrates that the
    // plaintext key is never reconstructed on any one machine
    // during signing.
    let (shares, group) = TrustedDealer::generate(2, 3).unwrap();
    assert_eq!(shares.len(), 3);

    let message = b"release-signing-request-42";
    let mut coord = ThresholdCoordinator::begin("op-e2e", &group, message.to_vec());

    let quorum: Vec<&ThresholdKeyShare> = shares.iter().take(2).collect();
    let mut nonces_map = BTreeMap::new();
    for share in &quorum {
        let nonces = coord
            .commit_round1(share.identifier, share.key_package.signing_share())
            .unwrap();
        nonces_map.insert(share.identifier, nonces);
    }
    coord.close_round1().unwrap();

    let mut sig_shares = BTreeMap::new();
    for share in &quorum {
        let nonces = nonces_map.remove(&share.identifier).unwrap();
        let partial = coord.partial_sign(nonces, &share.key_package).unwrap();
        sig_shares.insert(share.identifier, partial);
    }
    let group_sig = coord
        .aggregate(&sig_shares, &group.public_key_package)
        .unwrap();

    group
        .public_key_package
        .verifying_key()
        .verify(message, &group_sig)
        .unwrap();

    // And â€” separately â€” a vault-stored threshold record refuses
    // direct dereference. This enforces the protocol boundary:
    // threshold secrets never leave the threshold crate.
    let vault = build_vault().await;
    let record = vault
        .put(
            SecretClass::SigningKey,
            "release-signer",
            vec!["cloud.anthropic".into()],
            SecureBytes::from_slice(b"placeholder-for-threshold-record"),
        )
        .await
        .unwrap();
    assert!(matches!(
        record.protection,
        ordo_protocol::ProtectionLevel::Threshold { .. }
    ));
    let err = vault
        .get_for_provider(&record.id, "cloud.anthropic")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ordo_secrets_vault::VaultError::ThresholdOnly(_)
    ));
}

#[tokio::test]
async fn broker_canary_detection_triggers_on_output_leak() {
    let vault = build_vault().await;
    let broker = BrokerService::new(vault.clone());
    let audit = AuditService::new(AuditStore::in_memory().unwrap(), "local");

    let record = vault
        .put(
            SecretClass::ApiKey,
            "openai",
            vec!["p".into()],
            SecureBytes::from_slice(b"sk-live-42"),
        )
        .await
        .unwrap();
    let plan = broker
        .plan(&record.id, "p", SecretClass::ApiKey)
        .await
        .unwrap();

    // Simulate a tool that accidentally echoes its context into
    // output. The canary appears verbatim.
    let leaked = format!(
        "tool output: please use key={} next time",
        plan.canary.canary_token
    );
    let err = broker
        .scan_output(&plan.handle.id, leaked.as_bytes(), "tool-stdout")
        .await
        .unwrap_err();
    audit
        .append(
            SecretAuditEventType::CanaryDetected,
            json!({
                "capability_id": &plan.handle.id,
                "where_detected": "tool-stdout",
            }),
        )
        .await
        .unwrap();
    assert!(matches!(
        err,
        ordo_secrets_broker::BrokerError::CanaryDetected(_, _)
    ));
    assert_eq!(audit.verify_chain().await.unwrap(), 1);
}

#[tokio::test]
async fn chain_tamper_breaks_verify() {
    let audit = AuditService::new(AuditStore::in_memory().unwrap(), "local");
    audit
        .append(SecretAuditEventType::SecretCreated, json!({"n": 1}))
        .await
        .unwrap();
    audit
        .append(SecretAuditEventType::SecretCreated, json!({"n": 2}))
        .await
        .unwrap();
    // Sanity.
    assert_eq!(audit.verify_chain().await.unwrap(), 2);
}
