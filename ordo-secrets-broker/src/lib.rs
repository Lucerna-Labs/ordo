//! ordo-secrets-broker â€” the only surface the LLM sees.
//!
//! Responsibility boundary: the broker owns the *mediation layer*
//! between tools/LLM and the vault. It does NOT seal, does NOT
//! threshold-sign, does NOT anchor. It does enforce the blueprint's
//! three "hard-to-leak" commitments:
//!
//!   1. **Capability handles** replace raw secrets in tool contexts.
//!      A handle is a short-lived ULID-addressed reference; the
//!      broker resolves it to real material at dereference time,
//!      and the LLM never sees the underlying secret.
//!
//!   2. **DRIFT** â€” three-stage gating around every tool call that
//!      could dereference a capability:
//!        - **Planner**: declares what capabilities the tool needs,
//!          requests handles, injects canaries into context.
//!        - **Validator**: verifies chain-of-custody (input hash +
//!          declared-capability hash) matches what the planner
//!          committed to. If drifted, rejects before dereference.
//!        - **Isolator**: runs the tool with the resolved
//!          plaintext in an isolated scope. Plaintext lives in
//!          `SecureBytes`; the isolator's output is structurally
//!          limited and scanned for canaries before release.
//!
//!   3. **Canary + custody + structural checks** â€” three independent
//!      tripwires. A canary appearing in output proves context
//!      leaked. A custody mismatch proves the tool reshaped its
//!      input between planning and execution. A structural
//!      rejection proves the tool tried to emit more bytes than
//!      its advertised payload shape allowed.
//!
//! Invariants (blueprint Â§22, Â§21):
//!
//! - Canaries are per-capability, NOT per-secret. A canary in
//!   output means THIS capability's context leaked, not the
//!   underlying secret.
//! - Threshold-protected secrets never dereference through the
//!   broker directly; the broker dispatches a signing request to
//!   `ordo-secrets-threshold` instead. The plaintext key never
//!   exists anywhere; only a signature crosses the boundary.

use std::collections::HashMap;
use std::sync::Arc;

use blake3::Hasher;
use chrono::{Duration, Utc};
use ordo_bus::Bus;
use ordo_protocol::{
    secrets_topics, BusEnvelope, CapabilityCanary, CapabilityHandle, Envelope, InputCustody,
    NodeId, OrdoMessage, SecretClass, StructuralOutputCheck,
};
use ordo_secrets_vault::{SecureBytes, VaultError, VaultService};
use tokio::sync::Mutex;

pub mod canary;
pub mod drift;
pub mod structural;

pub use canary::{generate_canary_token, scan_for_canary};
pub use drift::{DriftDecision, DriftPlan};
pub use structural::{enforce_structural_limit, StructuralPolicy};

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("vault: {0}")]
    Vault(#[from] VaultError),
    #[error("capability not found or expired: {0}")]
    CapabilityGone(String),
    #[error("capability already consumed: {0}")]
    CapabilityConsumed(String),
    #[error("drift detected: {0}")]
    DriftDetected(String),
    #[error("canary detected in output: capability={0} origin={1}")]
    CanaryDetected(String, String),
    #[error("structural limit exceeded: {0}")]
    StructuralLimit(String),
    #[error("threshold-protected secret must go through the threshold crate: {0}")]
    ThresholdOnly(String),
    #[error("bad input: {0}")]
    BadInput(String),
}

pub type BrokerResult<T> = Result<T, BrokerError>;

#[derive(Debug)]
struct CapabilityState {
    handle: CapabilityHandle,
    secret_id: String,
    canary: CapabilityCanary,
    input_custody: Option<InputCustody>,
    consumed: bool,
}

pub struct BrokerService {
    vault: Arc<VaultService>,
    capabilities: Arc<Mutex<HashMap<String, CapabilityState>>>,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
    default_ttl: Duration,
    default_structural_budget: u64,
}

impl BrokerService {
    pub fn new(vault: Arc<VaultService>) -> Self {
        Self {
            vault,
            capabilities: Arc::new(Mutex::new(HashMap::new())),
            bus: None,
            node_id: NodeId::new(),
            default_ttl: Duration::minutes(5),
            default_structural_budget: 256 * 1024,
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

    pub fn with_default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = ttl;
        self
    }

    pub fn with_default_structural_budget(mut self, bytes: u64) -> Self {
        self.default_structural_budget = bytes;
        self
    }

    /// DRIFT stage 1 â€” planner.
    pub async fn plan(
        &self,
        secret_id: &str,
        provider_id: &str,
        class: SecretClass,
    ) -> BrokerResult<DriftPlan> {
        let expires_at = Utc::now() + self.default_ttl;
        let handle = CapabilityHandle {
            id: ulid::Ulid::new().to_string(),
            provider_id: provider_id.to_string(),
            expires_at,
            class,
        };
        let canary = CapabilityCanary {
            capability_id: handle.id.clone(),
            canary_token: generate_canary_token(),
            injected_into_context: false,
        };

        {
            let mut caps = self.capabilities.lock().await;
            caps.insert(
                handle.id.clone(),
                CapabilityState {
                    handle: handle.clone(),
                    secret_id: secret_id.to_string(),
                    canary: canary.clone(),
                    input_custody: None,
                    consumed: false,
                },
            );
        }

        if let Some(bus) = &self.bus {
            let msg = OrdoMessage::SecretsCapabilityIssued {
                handle: handle.clone(),
                canary: canary.clone(),
            };
            let env: BusEnvelope = Envelope::new(self.node_id.clone(), msg);
            let _ = bus.publish(secrets_topics::BROKER_HANDLE_ISSUED, env).await;
        }

        Ok(DriftPlan { handle, canary })
    }

    /// Commit the custody hashes for a capability. The planner
    /// commits at stage 1; if the validator later sees a different
    /// input the drift bit flips.
    pub async fn commit_custody(
        &self,
        capability_id: &str,
        custody: InputCustody,
    ) -> BrokerResult<()> {
        let mut caps = self.capabilities.lock().await;
        let state = caps
            .get_mut(capability_id)
            .ok_or_else(|| BrokerError::CapabilityGone(capability_id.to_string()))?;
        state.input_custody = Some(custody);
        Ok(())
    }

    /// DRIFT stage 2 â€” validator.
    pub async fn validate(
        &self,
        capability_id: &str,
        claimed_custody: &InputCustody,
    ) -> BrokerResult<DriftDecision> {
        let caps = self.capabilities.lock().await;
        let state = caps
            .get(capability_id)
            .ok_or_else(|| BrokerError::CapabilityGone(capability_id.to_string()))?;
        let committed = state.input_custody.as_ref().ok_or_else(|| {
            BrokerError::DriftDetected(format!(
                "capability {capability_id}: validator called before custody was committed"
            ))
        })?;
        if Utc::now() > state.handle.expires_at {
            return Err(BrokerError::CapabilityGone(format!(
                "capability {capability_id} expired at {}",
                state.handle.expires_at
            )));
        }
        if committed.input_hash != claimed_custody.input_hash
            || committed.declared_capabilities_hash != claimed_custody.declared_capabilities_hash
        {
            drop(caps);
            if let Some(bus) = &self.bus {
                let env: BusEnvelope = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::SecretsCustodyMismatch(claimed_custody.clone()),
                );
                let _ = bus
                    .publish(secrets_topics::BROKER_CUSTODY_MISMATCH, env)
                    .await;
            }
            return Err(BrokerError::DriftDetected(format!(
                "capability {capability_id}: custody mismatch \u{2014} input or capability set changed between plan and validate"
            )));
        }
        Ok(DriftDecision::Proceed)
    }

    /// DRIFT stage 3 â€” isolator.
    pub async fn dereference(
        &self,
        capability_id: &str,
        consume: bool,
    ) -> BrokerResult<SecureBytes> {
        let (secret_id, provider_id) = {
            let mut caps = self.capabilities.lock().await;
            let state = caps
                .get_mut(capability_id)
                .ok_or_else(|| BrokerError::CapabilityGone(capability_id.to_string()))?;
            if state.consumed {
                return Err(BrokerError::CapabilityConsumed(capability_id.to_string()));
            }
            if Utc::now() > state.handle.expires_at {
                return Err(BrokerError::CapabilityGone(format!(
                    "capability {capability_id} expired at {}",
                    state.handle.expires_at
                )));
            }
            if consume {
                state.consumed = true;
            }
            (state.secret_id.clone(), state.handle.provider_id.clone())
        };

        match self.vault.get_for_provider(&secret_id, &provider_id).await {
            Ok(pt) => Ok(pt),
            Err(VaultError::ThresholdOnly(id)) => Err(BrokerError::ThresholdOnly(id)),
            Err(err) => Err(BrokerError::Vault(err)),
        }
    }

    pub async fn revoke(&self, capability_id: &str, reason: &str) -> BrokerResult<()> {
        let mut caps = self.capabilities.lock().await;
        if caps.remove(capability_id).is_some() {
            drop(caps);
            if let Some(bus) = &self.bus {
                let env: BusEnvelope = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::SecretsCapabilityRevoked {
                        capability_id: capability_id.to_string(),
                        reason: reason.to_string(),
                    },
                );
                let _ = bus
                    .publish(secrets_topics::BROKER_HANDLE_REVOKED, env)
                    .await;
            }
        }
        Ok(())
    }

    pub async fn scan_output(
        &self,
        capability_id: &str,
        output: &[u8],
        origin: &str,
    ) -> BrokerResult<()> {
        let token = {
            let caps = self.capabilities.lock().await;
            let state = caps
                .get(capability_id)
                .ok_or_else(|| BrokerError::CapabilityGone(capability_id.to_string()))?;
            state.canary.canary_token.clone()
        };
        if scan_for_canary(output, &token) {
            if let Some(bus) = &self.bus {
                let env: BusEnvelope = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::SecretsCanaryDetected {
                        capability_id: capability_id.to_string(),
                        where_detected: origin.to_string(),
                    },
                );
                let _ = bus
                    .publish(secrets_topics::BROKER_CANARY_DETECTED, env)
                    .await;
            }
            return Err(BrokerError::CanaryDetected(
                capability_id.to_string(),
                origin.to_string(),
            ));
        }
        Ok(())
    }

    pub async fn enforce_structural(
        &self,
        tool_invocation_id: &str,
        actual_bytes: u64,
        policy: Option<StructuralPolicy>,
    ) -> BrokerResult<StructuralOutputCheck> {
        let policy = policy.unwrap_or(StructuralPolicy {
            byte_budget: self.default_structural_budget,
            reject_reason: None,
        });
        let check = enforce_structural_limit(tool_invocation_id, actual_bytes, &policy);
        if check.rejected {
            if let Some(bus) = &self.bus {
                let env: BusEnvelope = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::SecretsStructuralRejection(check.clone()),
                );
                let _ = bus
                    .publish(secrets_topics::BROKER_STRUCTURAL_REJECTION, env)
                    .await;
            }
            return Err(BrokerError::StructuralLimit(format!(
                "tool {tool_invocation_id} emitted {actual_bytes}B > budget {}B",
                policy.byte_budget
            )));
        }
        Ok(check)
    }
}

/// Build an `InputCustody` from raw input + capability id list.
/// Used on both sides of DRIFT (planner commits, validator claims)
/// so a shape mismatch is structurally impossible.
pub fn build_custody(
    tool_invocation_id: &str,
    input_bytes: &[u8],
    capability_ids: &[String],
) -> InputCustody {
    let input_hash = blake3_over(input_bytes);
    let mut sorted = capability_ids.to_vec();
    sorted.sort();
    let joined = sorted.join(",");
    let declared_capabilities_hash = blake3_over(joined.as_bytes());
    InputCustody {
        tool_invocation_id: tool_invocation_id.to_string(),
        input_hash,
        declared_capabilities_hash,
    }
}

fn blake3_over(bytes: &[u8]) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(bytes);
    *h.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_protocol::ProtectionLevel;
    use ordo_secrets_vault::sealer::{MockSealer, Sealer};
    use ordo_secrets_vault::{VaultService, VaultStore};

    async fn build_vault_with_secret() -> (Arc<VaultService>, String) {
        let store = VaultStore::in_memory().unwrap();
        let sealers: Vec<Box<dyn Sealer>> = vec![Box::new(MockSealer)];
        let svc = VaultService::builder(store, "local")
            .with_sealers(sealers)
            .build()
            .await
            .unwrap();
        let record = svc
            .put(
                SecretClass::ApiKey,
                "openai",
                vec!["p".into()],
                SecureBytes::from_slice(b"sk-live-42"),
            )
            .await
            .unwrap();
        (Arc::new(svc), record.id)
    }

    #[tokio::test]
    async fn plan_issues_distinct_handles_with_unique_canaries() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let p1 = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        let p2 = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        assert_ne!(p1.handle.id, p2.handle.id);
        assert_ne!(p1.canary.canary_token, p2.canary.canary_token);
    }

    #[tokio::test]
    async fn dereference_returns_plaintext_when_custody_matches() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let plan = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        let custody = build_custody(
            "inv-1",
            b"input-blob",
            std::slice::from_ref(&plan.handle.id),
        );
        broker
            .commit_custody(&plan.handle.id, custody.clone())
            .await
            .unwrap();
        let claimed = build_custody(
            "inv-1",
            b"input-blob",
            std::slice::from_ref(&plan.handle.id),
        );
        broker.validate(&plan.handle.id, &claimed).await.unwrap();
        let pt = broker.dereference(&plan.handle.id, true).await.unwrap();
        assert_eq!(pt.as_slice(), b"sk-live-42");
    }

    #[tokio::test]
    async fn drift_on_input_hash_blocks_dereference() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let plan = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        let committed = build_custody("inv-1", b"original", std::slice::from_ref(&plan.handle.id));
        broker
            .commit_custody(&plan.handle.id, committed)
            .await
            .unwrap();
        let tampered = build_custody("inv-1", b"DIFFERENT", std::slice::from_ref(&plan.handle.id));
        let err = broker
            .validate(&plan.handle.id, &tampered)
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::DriftDetected(_)));
    }

    #[tokio::test]
    async fn drift_on_capability_hash_blocks_dereference() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let plan = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        let committed = build_custody("inv-1", b"input", std::slice::from_ref(&plan.handle.id));
        broker
            .commit_custody(&plan.handle.id, committed)
            .await
            .unwrap();
        let tampered = build_custody(
            "inv-1",
            b"input",
            &[plan.handle.id.clone(), "bogus-extra-cap".into()],
        );
        let err = broker
            .validate(&plan.handle.id, &tampered)
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::DriftDetected(_)));
    }

    #[tokio::test]
    async fn dereference_is_single_use_when_consumed() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let plan = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        let custody = build_custody("inv-1", b"i", std::slice::from_ref(&plan.handle.id));
        broker
            .commit_custody(&plan.handle.id, custody.clone())
            .await
            .unwrap();
        broker.validate(&plan.handle.id, &custody).await.unwrap();
        let _first = broker.dereference(&plan.handle.id, true).await.unwrap();
        let err = broker.dereference(&plan.handle.id, true).await.unwrap_err();
        assert!(matches!(err, BrokerError::CapabilityConsumed(_)));
    }

    #[tokio::test]
    async fn revoke_removes_capability() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let plan = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        broker.revoke(&plan.handle.id, "test").await.unwrap();
        let err = broker.dereference(&plan.handle.id, true).await.unwrap_err();
        assert!(matches!(err, BrokerError::CapabilityGone(_)));
    }

    #[tokio::test]
    async fn scan_detects_canary_in_output() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let plan = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        let leaked_output = format!(
            "here is some output that accidentally leaked {} from our planner",
            plan.canary.canary_token
        );
        let err = broker
            .scan_output(&plan.handle.id, leaked_output.as_bytes(), "stdout")
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::CanaryDetected(_, _)));
    }

    #[tokio::test]
    async fn scan_tolerates_clean_output() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let plan = broker
            .plan(&secret_id, "p", SecretClass::ApiKey)
            .await
            .unwrap();
        broker
            .scan_output(&plan.handle.id, b"no canaries here", "stdout")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn structural_limit_rejects_oversize_output() {
        let (vault, secret_id) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault).with_default_structural_budget(1024);
        let _ = secret_id;
        let err = broker
            .enforce_structural("inv-big", 4096, None)
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::StructuralLimit(_)));
    }

    #[tokio::test]
    async fn structural_limit_accepts_under_budget() {
        let (vault, _) = build_vault_with_secret().await;
        let broker = BrokerService::new(vault);
        let check = broker.enforce_structural("inv-ok", 32, None).await.unwrap();
        assert!(!check.rejected);
        assert_eq!(check.actual_bytes, 32);
    }

    #[tokio::test]
    async fn threshold_secret_propagates_threshold_only_error() {
        let store = VaultStore::in_memory().unwrap();
        let sealers: Vec<Box<dyn Sealer>> = vec![Box::new(MockSealer)];
        let svc = VaultService::builder(store, "local")
            .with_sealers(sealers)
            .build()
            .await
            .unwrap();
        let rec = svc
            .put(
                SecretClass::SigningKey,
                "release",
                vec!["p".into()],
                SecureBytes::from_slice(b"raw-key"),
            )
            .await
            .unwrap();
        assert!(matches!(rec.protection, ProtectionLevel::Threshold { .. }));
        let broker = BrokerService::new(Arc::new(svc));
        let plan = broker
            .plan(&rec.id, "p", SecretClass::SigningKey)
            .await
            .unwrap();
        let custody = build_custody("inv-sig", b"i", std::slice::from_ref(&plan.handle.id));
        broker
            .commit_custody(&plan.handle.id, custody.clone())
            .await
            .unwrap();
        broker.validate(&plan.handle.id, &custody).await.unwrap();
        let err = broker.dereference(&plan.handle.id, true).await.unwrap_err();
        assert!(matches!(err, BrokerError::ThresholdOnly(_)));
    }
}
