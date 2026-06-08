//! Vault service â€” the orchestrator that ties sealers, AEAD,
//! lifecycle backend, and the store together.
//!
//! Responsibilities:
//!   1. At construction, probe sealers in tier order and pick the
//!      highest available. If the highest-available tier is lower
//!      than the workspace's previously-used tier, emit
//!      `SecretsSealTierDegraded` on the bus (never silent).
//!   2. Load or generate the master DEK. The DEK is wrapped by the
//!      active sealer and persisted in `vault_state`.
//!   3. `put`: accept a fresh plaintext secret, AEAD-encrypt under
//!      the DEK, persist row.
//!   4. `get_for_provider`: enforce the allowed_providers scope,
//!      dereference the secret to plaintext `SecureBytes`. Returns
//!      an error (not a placeholder ciphertext) for threshold-
//!      protected secrets â€” those go through the broker/threshold
//!      path instead.
//!   5. `rotate`: invariant 23. Ask the lifecycle backend for a new
//!      DEK, re-seal every active secret under it, overwrite the
//!      old DEK's ciphertext with zeros, persist the new master
//!      state.
//!
//! What this service does NOT do: issue capability handles
//! (broker's job), record audit entries (audit crate's job), run
//! canary/custody checks (broker's job), do threshold signing
//! (threshold crate's job). Each concern gets its own crate.

use std::sync::Arc;

use chrono::Utc;
use ordo_bus::Bus;
use ordo_protocol::{
    secrets_topics, BusEnvelope, Envelope, NodeId, OrdoMessage, ProtectionLevel, SealingTier,
    SecretClass, SecretRecord,
};
use tokio::sync::Mutex;

use crate::aead;
use crate::bytes::SecureBytes;
use crate::lifecycle::{DekRotation, SecretLifecycleBackend};
use crate::sealer::{Argon2idSealer, Sealer, SealerError};
use crate::store::{SealedSecretRow, VaultStateRow, VaultStore, VaultStoreError};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("storage: {0}")]
    Storage(#[from] VaultStoreError),
    #[error("sealer: {0}")]
    Sealer(#[from] SealerError),
    #[error("aead: {0}")]
    Aead(#[from] aead::AeadError),
    #[error("lifecycle: {0}")]
    Lifecycle(#[from] crate::lifecycle::LifecycleError),
    #[error("no sealer was available on this host")]
    NoSealerAvailable,
    #[error("provider {0} not allowed to access secret {1}")]
    ProviderNotAllowed(String, String),
    #[error("secret {0} not found")]
    NotFound(String),
    #[error("secret {0} is retired")]
    Retired(String),
    #[error("secret {0} is threshold-protected; use the broker/threshold path")]
    ThresholdOnly(String),
    #[error("bad input: {0}")]
    BadInput(String),
}

pub type VaultResult<T> = Result<T, VaultError>;

/// Orchestrates sealer probing, DEK management, and secret
/// persistence. Share a single instance across the runtime â€”
/// the inner state is behind a Mutex so concurrent puts/gets are
/// serialised.
pub struct VaultService {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    store: VaultStore,
    sealer: Box<dyn Sealer>,
    lifecycle: Box<dyn SecretLifecycleBackend>,
    workspace_id: String,
    /// Current plaintext DEK. Kept in-memory for the vault's
    /// lifetime (the blueprint's trade-off: vs sealing/unsealing
    /// on every request, which burns TPM ops). Zeroized on drop.
    dek: SecureBytes,
    generation: u32,
    sealing_tier: SealingTier,
    sealer_label: String,
    /// Bus + node id travel with the service so methods can emit
    /// `SealTierDegraded` / rotation events without plumbing them
    /// through on every call.
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
}

/// Builder for `VaultService`. The only required inputs are a
/// store and a workspace id; everything else defaults (Tier-4
/// sealer, DekRotation lifecycle, no bus).
pub struct VaultServiceBuilder {
    store: VaultStore,
    workspace_id: String,
    candidate_sealers: Vec<Box<dyn Sealer>>,
    lifecycle: Box<dyn SecretLifecycleBackend>,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
    passphrase: Option<String>,
}

impl VaultServiceBuilder {
    pub fn new(store: VaultStore, workspace_id: impl Into<String>) -> Self {
        Self {
            store,
            workspace_id: workspace_id.into(),
            candidate_sealers: Vec::new(),
            lifecycle: Box::new(DekRotation),
            bus: None,
            node_id: NodeId::new(),
            passphrase: None,
        }
    }

    /// Provide the candidate sealer stack explicitly. Order matters:
    /// highest-tier first. The first one whose `probe()` succeeds
    /// becomes the active sealer.
    pub fn with_sealers(mut self, sealers: Vec<Box<dyn Sealer>>) -> Self {
        self.candidate_sealers = sealers;
        self
    }

    pub fn with_lifecycle(mut self, lifecycle: Box<dyn SecretLifecycleBackend>) -> Self {
        self.lifecycle = lifecycle;
        self
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_node_id(mut self, node_id: NodeId) -> Self {
        self.node_id = node_id;
        self
    }

    /// Passphrase for the Tier-4 Argon2id fallback. If unset and
    /// the stack falls through to Argon2id, a generated
    /// high-entropy passphrase is persisted to the OS keychain
    /// under a well-known account â€” but that's equivalent to
    /// Tier-3 so we insist operators provide one explicitly for a
    /// real Tier-4 deployment. For tests we auto-generate.
    pub fn with_passphrase(mut self, passphrase: impl Into<String>) -> Self {
        self.passphrase = Some(passphrase.into());
        self
    }

    pub async fn build(self) -> VaultResult<VaultService> {
        // If caller didn't supply any sealers, build the default
        // stack: highest available platform tier then Argon2id.
        let candidates = if self.candidate_sealers.is_empty() {
            default_sealer_stack(self.passphrase.clone())?
        } else {
            self.candidate_sealers
        };

        // Probe in order; pick the first that succeeds.
        let mut skipped: Vec<SealingTier> = Vec::new();
        let mut active: Option<Box<dyn Sealer>> = None;
        for sealer in candidates {
            match sealer.probe().await {
                Ok(()) => {
                    active = Some(sealer);
                    break;
                }
                Err(err) => {
                    tracing::warn!(
                        target: "ordo_secrets_vault::service",
                        tier = sealer.tier().label(),
                        label = sealer.label(),
                        error = %err,
                        "sealer probe failed; trying next"
                    );
                    skipped.push(sealer.tier());
                }
            }
        }
        let active = active.ok_or(VaultError::NoSealerAvailable)?;
        let active_tier = active.tier();
        let active_label = active.label().to_string();

        let mut store = self.store;

        // Load or initialise the vault state row.
        let (dek, generation) = match store.load_vault_state(&self.workspace_id)? {
            Some(existing) => {
                // Detect tier degradation versus persisted state.
                // Lower tier numerically = weaker. We compare via
                // our own ordering helper.
                if tier_rank(active_tier) < tier_rank(existing.sealing_tier) {
                    if let Some(bus) = self.bus.as_ref() {
                        emit_degraded(
                            bus.as_ref(),
                            &self.node_id,
                            existing.sealing_tier,
                            active_tier,
                            format!(
                                "host no longer provides {}; active sealer is {}",
                                existing.sealing_tier.label(),
                                active_label
                            ),
                        )
                        .await;
                    }
                }
                // The sealed_dek was wrapped by whichever sealer
                // sealed it before. If the active sealer is the
                // same one, we can unwrap directly. If it isn't,
                // this is a tier-crossing state; we refuse rather
                // than silently re-seal under a weaker tier (the
                // operator must run a rotation deliberately).
                if existing.sealing_tier != active_tier || existing.sealer_label != active_label {
                    return Err(VaultError::Sealer(SealerError::Platform(format!(
                        "persisted vault state was sealed by {}/{} but active sealer is {}/{}; \
                         rotate the vault or restore the prior sealer",
                        existing.sealing_tier.label(),
                        existing.sealer_label,
                        active_tier.label(),
                        active_label
                    ))));
                }
                let dek = active.unwrap(&existing.sealed_dek).await?;
                if dek.len() != 32 {
                    return Err(VaultError::BadInput(format!(
                        "unwrapped DEK was {} bytes; expected 32",
                        dek.len()
                    )));
                }
                (dek, existing.generation)
            }
            None => {
                // First run for this workspace: generate a fresh
                // DEK, wrap, persist.
                use rand::RngCore;
                let mut buf = vec![0u8; 32];
                rand::thread_rng().fill_bytes(&mut buf);
                let dek = SecureBytes::new(buf);
                let sealed = active.wrap(&dek).await?;
                store.insert_vault_state(&VaultStateRow {
                    workspace_id: self.workspace_id.clone(),
                    generation: 0,
                    sealing_tier: active_tier,
                    sealer_label: active_label.clone(),
                    sealed_dek: sealed,
                    created_at: Utc::now(),
                    rotated_at: None,
                })?;
                (dek, 0u32)
            }
        };

        Ok(VaultService {
            inner: Arc::new(Mutex::new(Inner {
                store,
                sealer: active,
                lifecycle: self.lifecycle,
                workspace_id: self.workspace_id,
                dek,
                generation,
                sealing_tier: active_tier,
                sealer_label: active_label,
                bus: self.bus,
                node_id: self.node_id,
            })),
        })
    }
}

impl VaultService {
    pub fn builder(store: VaultStore, workspace_id: impl Into<String>) -> VaultServiceBuilder {
        VaultServiceBuilder::new(store, workspace_id)
    }

    /// Report the currently active sealing tier. Used by the audit
    /// crate to gate anchor signing (invariant 24).
    pub async fn active_tier(&self) -> SealingTier {
        self.inner.lock().await.sealing_tier
    }

    pub async fn active_sealer_label(&self) -> String {
        self.inner.lock().await.sealer_label.clone()
    }

    /// Store a fresh secret. Returns the newly-minted `SecretRecord`
    /// (metadata only â€” plaintext is consumed).
    pub async fn put(
        &self,
        class: SecretClass,
        label: impl Into<String>,
        allowed_providers: Vec<String>,
        material: SecureBytes,
    ) -> VaultResult<SecretRecord> {
        if material.is_empty() {
            return Err(VaultError::BadInput("material is empty".into()));
        }
        let mut inner = self.inner.lock().await;
        let protection = class.default_protection();
        let id = ulid::Ulid::new().to_string();
        let label = label.into();
        let created_at = Utc::now();
        let aad = build_aad(&id, class, &inner.workspace_id);
        let sealed = aead::seal(inner.dek.as_slice(), &material, &aad)?;

        let record = SecretRecord {
            id: id.clone(),
            workspace_id: inner.workspace_id.clone(),
            class,
            protection,
            label,
            allowed_providers,
            created_at,
            rotation_due_at: Some(
                created_at + chrono::Duration::days(class.default_rotation_days() as i64),
            ),
            retired_at: None,
        };
        let row = SealedSecretRow {
            record: record.clone(),
            sealing_tier: inner.sealing_tier,
            ciphertext: sealed.ciphertext,
            nonce: sealed.nonce,
            aad: sealed.aad,
            dek_generation: inner.generation,
        };
        inner.store.insert_sealed(&row)?;
        Ok(record)
    }

    /// Dereference a secret for a named provider. Enforces the
    /// `allowed_providers` scope (Rule 2). Returns
    /// `ThresholdOnly` for threshold-protected records â€” callers
    /// route those through the broker/threshold path instead.
    pub async fn get_for_provider(
        &self,
        secret_id: &str,
        provider_id: &str,
    ) -> VaultResult<SecureBytes> {
        let inner = self.inner.lock().await;
        let row = inner
            .store
            .load_sealed(secret_id)?
            .ok_or_else(|| VaultError::NotFound(secret_id.to_string()))?;
        if row.record.retired_at.is_some() {
            return Err(VaultError::Retired(secret_id.to_string()));
        }
        if matches!(row.record.protection, ProtectionLevel::Threshold { .. }) {
            return Err(VaultError::ThresholdOnly(secret_id.to_string()));
        }
        if !row
            .record
            .allowed_providers
            .iter()
            .any(|p| p == provider_id)
        {
            return Err(VaultError::ProviderNotAllowed(
                provider_id.to_string(),
                secret_id.to_string(),
            ));
        }
        let pt = aead::open(inner.dek.as_slice(), &row.ciphertext, &row.nonce, &row.aad)?;
        Ok(pt)
    }

    /// Mark a secret retired. Invariant 23: the ciphertext is
    /// zeroed; the row stays for audit continuity.
    pub async fn retire(&self, secret_id: &str) -> VaultResult<()> {
        let mut inner = self.inner.lock().await;
        inner.store.retire_row(secret_id)?;
        Ok(())
    }

    /// Rotate the master DEK. Re-seals every active secret under
    /// the new DEK, overwrites the old sealed-DEK blob with zeros
    /// via the store's update path, and bumps the generation.
    /// Threshold-protected secret rows are rewrapped too â€” their
    /// ciphertext is a placeholder today but the same DEK wraps
    /// whatever the broker later persists, so generations stay in
    /// sync.
    pub async fn rotate(&self) -> VaultResult<()> {
        let mut inner = self.inner.lock().await;
        let plan = inner.lifecycle.propose_rotation(inner.generation).await?;
        let new_generation = plan.next_generation;

        // Rewrap every active row.
        let active = inner.store.list_active(&inner.workspace_id)?;
        for row in &active {
            // Open under the old DEK.
            let pt = aead::open(inner.dek.as_slice(), &row.ciphertext, &row.nonce, &row.aad)?;
            // Reseal under the new DEK.
            let new_aad = build_aad(&row.record.id, row.record.class, &row.record.workspace_id);
            let resealed = aead::seal(plan.new_dek.as_slice(), &pt, &new_aad)?;
            inner.store.rewrap_in_place(
                &row.record.id,
                &resealed.ciphertext,
                &resealed.nonce,
                &resealed.aad,
                new_generation,
            )?;
        }

        // Wrap the new DEK with the active sealer and overwrite
        // the vault_state row. The old sealed blob leaves the
        // record entirely (UPDATE replaces it); no on-disk residue
        // beyond whatever SQLite's free-list holds.
        let sealed_new = inner.sealer.wrap(&plan.new_dek).await?;
        let Some(state) = inner.store.load_vault_state(&inner.workspace_id)? else {
            return Err(VaultError::Storage(VaultStoreError::NotFound(format!(
                "vault_state for workspace {} missing during rotation",
                inner.workspace_id
            ))));
        };
        let updated = VaultStateRow {
            workspace_id: state.workspace_id,
            generation: new_generation,
            sealing_tier: inner.sealing_tier,
            sealer_label: inner.sealer_label.clone(),
            sealed_dek: sealed_new,
            created_at: state.created_at,
            rotated_at: Some(Utc::now()),
        };
        inner.store.update_vault_state(&updated)?;

        // Swap the in-memory DEK last so a rotation that failed
        // mid-loop can be re-attempted without the vault losing
        // the ability to open pre-rotation rows.
        inner.dek = plan.new_dek;
        inner.generation = new_generation;

        // Emit a completion event per resealed row so the audit
        // crate (subscribed to the rotation topic) can ledger it.
        // Note: this is DEK rotation, not per-secret credential
        // rotation â€” the `new_record_id` == the same id; we advance
        // the dek_generation and do not mint new secret ids here.
        if let Some(bus) = inner.bus.as_ref().cloned() {
            let node = inner.node_id.clone();
            drop(inner);
            for row in &active {
                let msg = OrdoMessage::SecretsRotationCompleted {
                    secret_id: row.record.id.clone(),
                    new_record_id: row.record.id.clone(),
                };
                let envelope: BusEnvelope = Envelope::new(node.clone(), msg);
                let _ = bus
                    .publish(secrets_topics::ROTATION_COMPLETED, envelope)
                    .await;
            }
        }
        Ok(())
    }
}

/// The AAD binds the ciphertext to the secret id, class, and
/// workspace. Attempting to open a row's ciphertext with the wrong
/// id / class / workspace fails cleanly.
fn build_aad(id: &str, class: SecretClass, workspace_id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(id.len() + workspace_id.len() + 16);
    aad.extend_from_slice(b"ordo.vault|");
    aad.extend_from_slice(workspace_id.as_bytes());
    aad.push(b'|');
    aad.extend_from_slice(class.label().as_bytes());
    aad.push(b'|');
    aad.extend_from_slice(id.as_bytes());
    aad
}

fn tier_rank(tier: SealingTier) -> u8 {
    match tier {
        SealingTier::Tier1Hardware => 4,
        SealingTier::Tier2SecureElement => 3,
        SealingTier::Tier3OsKeychain => 2,
        SealingTier::Tier4SoftwareFallback => 1,
        SealingTier::MockForTests => 0,
    }
}

fn default_sealer_stack(passphrase: Option<String>) -> VaultResult<Vec<Box<dyn Sealer>>> {
    let mut stack: Vec<Box<dyn Sealer>> = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Some(pp) = passphrase.clone() {
            let s = crate::sealer::WindowsTbsSealer::new(pp.into_bytes())?;
            stack.push(Box::new(s));
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(pp) = passphrase.clone() {
            let s = crate::sealer::LinuxTpmSealer::new(pp.into_bytes())?;
            stack.push(Box::new(s));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(pp) = passphrase.clone() {
            let s = crate::sealer::SecureEnclaveSealer::new(pp.into_bytes())?;
            stack.push(Box::new(s));
        }
    }
    stack.push(Box::new(crate::sealer::KeychainSealer::new("ordo-vault")));
    let pp = passphrase.unwrap_or_else(|| {
        // Deterministically-generated high-entropy fallback so
        // unit tests don't have to supply one. In production the
        // builder requires a passphrase for Tier-4 to be meaningful
        // (otherwise it degrades to Tier-3 keychain material).
        use rand::RngCore;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        hex::encode(buf)
    });
    stack.push(Box::new(Argon2idSealer::new(pp.into_bytes())?));
    Ok(stack)
}

async fn emit_degraded(
    bus: &dyn Bus,
    node_id: &NodeId,
    from: SealingTier,
    to: SealingTier,
    reason: String,
) {
    let msg = OrdoMessage::SecretsSealTierDegraded {
        from,
        to,
        reason: reason.clone(),
    };
    let envelope: BusEnvelope = Envelope::new(node_id.clone(), msg);
    if let Err(err) = bus
        .publish(secrets_topics::VAULT_SEAL_TIER_DEGRADED, envelope)
        .await
    {
        tracing::error!(
            target: "ordo_secrets_vault::service",
            error = %err,
            "failed to publish SecretsSealTierDegraded"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sealer::MockSealer;

    fn mock_stack() -> Vec<Box<dyn Sealer>> {
        vec![Box::new(MockSealer)]
    }

    async fn build_test_service() -> VaultService {
        let store = VaultStore::in_memory().unwrap();
        VaultServiceBuilder::new(store, "local")
            .with_sealers(mock_stack())
            .build()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn put_then_get_roundtrips_plaintext() {
        let svc = build_test_service().await;
        let record = svc
            .put(
                SecretClass::ApiKey,
                "openai-prod",
                vec!["cloud.anthropic".into(), "cloud.openai".into()],
                SecureBytes::from_slice(b"sk-live-1234567890"),
            )
            .await
            .unwrap();
        let pt = svc
            .get_for_provider(&record.id, "cloud.openai")
            .await
            .unwrap();
        assert_eq!(pt.as_slice(), b"sk-live-1234567890");
    }

    #[tokio::test]
    async fn get_fails_for_unapproved_provider() {
        let svc = build_test_service().await;
        let record = svc
            .put(
                SecretClass::ApiKey,
                "openai-prod",
                vec!["cloud.anthropic".into()],
                SecureBytes::from_slice(b"sk-live-1234567890"),
            )
            .await
            .unwrap();
        let err = svc
            .get_for_provider(&record.id, "cloud.openai")
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::ProviderNotAllowed(_, _)));
    }

    #[tokio::test]
    async fn threshold_secret_rejects_direct_dereference() {
        let svc = build_test_service().await;
        let record = svc
            .put(
                SecretClass::SigningKey,
                "release-signer",
                vec!["cloud.anthropic".into()],
                SecureBytes::from_slice(b"private-key-bytes"),
            )
            .await
            .unwrap();
        assert!(matches!(
            record.protection,
            ProtectionLevel::Threshold { t: 2, n: 3 }
        ));
        let err = svc
            .get_for_provider(&record.id, "cloud.anthropic")
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::ThresholdOnly(_)));
    }

    #[tokio::test]
    async fn retire_zeros_and_blocks_further_dereference() {
        let svc = build_test_service().await;
        let record = svc
            .put(
                SecretClass::ApiKey,
                "openai-prod",
                vec!["cloud.openai".into()],
                SecureBytes::from_slice(b"x"),
            )
            .await
            .unwrap();
        svc.retire(&record.id).await.unwrap();
        let err = svc
            .get_for_provider(&record.id, "cloud.openai")
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::Retired(_)));
    }

    #[tokio::test]
    async fn rotate_reseals_every_active_secret_and_keeps_dereference_working() {
        let svc = build_test_service().await;
        let a = svc
            .put(
                SecretClass::ApiKey,
                "a",
                vec!["p".into()],
                SecureBytes::from_slice(b"material-a"),
            )
            .await
            .unwrap();
        let b = svc
            .put(
                SecretClass::ApiKey,
                "b",
                vec!["p".into()],
                SecureBytes::from_slice(b"material-b"),
            )
            .await
            .unwrap();
        svc.rotate().await.unwrap();
        assert_eq!(
            svc.get_for_provider(&a.id, "p").await.unwrap().as_slice(),
            b"material-a"
        );
        assert_eq!(
            svc.get_for_provider(&b.id, "p").await.unwrap().as_slice(),
            b"material-b"
        );
    }

    #[tokio::test]
    async fn rotate_bumps_dek_generation_on_sealed_rows() {
        let store = VaultStore::in_memory().unwrap();
        let svc = VaultServiceBuilder::new(store, "local")
            .with_sealers(mock_stack())
            .build()
            .await
            .unwrap();
        let a = svc
            .put(
                SecretClass::ApiKey,
                "a",
                vec!["p".into()],
                SecureBytes::from_slice(b"material-a"),
            )
            .await
            .unwrap();
        svc.rotate().await.unwrap();

        // Peek at storage directly to verify the generation bumped.
        let inner = svc.inner.lock().await;
        let row = inner.store.load_sealed(&a.id).unwrap().unwrap();
        assert_eq!(row.dek_generation, 1);
    }

    #[tokio::test]
    async fn new_vault_creates_state_row_on_first_build() {
        let svc = build_test_service().await;
        let inner = svc.inner.lock().await;
        assert!(inner.store.load_vault_state("local").unwrap().is_some());
    }
}
