//! Secrets architecture wire types (v2 blueprint â€” complete, not
//! phased).
//!
//! Load-bearing commitments (enforced across `ordo-secrets-vault`,
//! `ordo-secrets-broker`, `ordo-secrets-audit`, and
//! `ordo-secrets-threshold`):
//!
//! 1. The LLM never sees secrets. Only `CapabilityHandle`.
//! 2. Secrets are scoped to providers by registration.
//! 3. Every dereference is a hash-chained audit event.
//! 4. Master key is sealed to the best available hardware; the
//!    four sealing tiers are enumerated here and implemented in
//!    the vault.
//!
//! Protocol invariants 21â€“24 (from the blueprint) live as doc
//! comments on the relevant types below â€” future refactors read
//! them first.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::Ulid;

// -----------------------------------------------------------------------------
// Secret classes + sealing tiers
// -----------------------------------------------------------------------------

/// Taxonomy of secret content. Drives:
///   - default `ProtectionLevel` (SshClusterAdmin + SigningKey â†’
///     Threshold; everything else â†’ SingleSealed)
///   - default rotation policy (days_until_rotation per class)
///   - which providers can even request a handle to this class
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretClass {
    /// Standard API key for a remote service (OpenAI, Anthropic,
    /// Stripe, GitHub, etc.). Single-sealed; rotation via
    /// per-provider AutoRotator where supported.
    ApiKey,
    /// OAuth refresh token. Single-sealed. Always auto-rotatable.
    OAuthRefreshToken,
    /// Ordinary SSH key for a single host. Single-sealed; rotation
    /// requires user action unless a CA is registered.
    SshKey,
    /// SSH key with administrative access to a cluster / fleet.
    /// **Threshold-protected by default.** Loss of one device
    /// holding a share does not compromise the key.
    SshClusterAdmin,
    /// Long-lived signing key (release signing, anchor signing,
    /// code signing). **Threshold-protected by default.**
    SigningKey,
    /// DB connection credential (password, connection URL with
    /// embedded password, TLS client cert material).
    DatabaseCredential,
    /// Anything else. The vault still seals it and the broker
    /// still gates access; class defaults pick up "generic"
    /// policies.
    Generic,
}

impl SecretClass {
    pub fn default_protection(self) -> ProtectionLevel {
        match self {
            Self::SshClusterAdmin | Self::SigningKey => ProtectionLevel::Threshold { t: 2, n: 3 },
            _ => ProtectionLevel::SingleSealed,
        }
    }

    /// Blueprint rotation defaults. Override per-secret via
    /// `RotationPolicy`.
    pub fn default_rotation_days(self) -> u32 {
        match self {
            Self::OAuthRefreshToken => 30,
            Self::ApiKey | Self::DatabaseCredential => 90,
            Self::SshKey => 180,
            Self::SshClusterAdmin | Self::SigningKey => 365,
            Self::Generic => 180,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::OAuthRefreshToken => "oauth_refresh_token",
            Self::SshKey => "ssh_key",
            Self::SshClusterAdmin => "ssh_cluster_admin",
            Self::SigningKey => "signing_key",
            Self::DatabaseCredential => "database_credential",
            Self::Generic => "generic",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Some(match label {
            "api_key" => Self::ApiKey,
            "oauth_refresh_token" => Self::OAuthRefreshToken,
            "ssh_key" => Self::SshKey,
            "ssh_cluster_admin" => Self::SshClusterAdmin,
            "signing_key" => Self::SigningKey,
            "database_credential" => Self::DatabaseCredential,
            "generic" => Self::Generic,
            _ => return None,
        })
    }
}

/// Sealing tier for the master key (and, transitively, everything
/// derived from it).
///
/// Ordering: Tier1 (hardware root) > Tier2 (secure element) >
/// Tier3 (OS keychain) > Tier4 (software fallback). A vault always
/// uses the highest tier that works on the host. Invariant 24:
/// transparency anchors can only be signed by keys sealed at Tier1
/// or Tier2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SealingTier {
    /// Hardware root of trust: TPM 2.0 (Windows TBS / Linux tss-
    /// esapi). Seal operations bound to PCR state.
    Tier1Hardware,
    /// Secure element: Apple Secure Enclave (SEP). Keys never
    /// leave the hardware; signing happens on-chip.
    Tier2SecureElement,
    /// OS-managed credential store (Windows Credential Manager,
    /// macOS Keychain, SecretService on Linux). Encrypted at rest
    /// but reachable by any process running as the user.
    Tier3OsKeychain,
    /// Pure software: Argon2id KDF from a user passphrase + salt.
    /// Fallback when no hardware is available. NOT suitable for
    /// transparency anchor signing.
    Tier4SoftwareFallback,
    /// Explicit mock for CI. Never appears in production.
    /// Distinguishable in logs from any real sealer.
    MockForTests,
}

impl SealingTier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tier1Hardware => "tier1_hardware",
            Self::Tier2SecureElement => "tier2_secure_element",
            Self::Tier3OsKeychain => "tier3_os_keychain",
            Self::Tier4SoftwareFallback => "tier4_software_fallback",
            Self::MockForTests => "mock_for_tests",
        }
    }

    /// Invariant 24: anchor signing requires a key sealed at
    /// Tier-1 or Tier-2. Tier-3 signed anchors have no external
    /// meaning (anyone on the box can produce them).
    pub fn can_sign_transparency_anchors(self) -> bool {
        matches!(self, Self::Tier1Hardware | Self::Tier2SecureElement)
    }
}

/// Protection level â€” single-sealed secrets live in the vault
/// and are unsealed by the vault's master key; threshold-protected
/// secrets require quorum signing (FROST) and never exist
/// fully-reconstructed in any one location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ProtectionLevel {
    SingleSealed,
    /// t-of-n threshold. For (2, 3): laptop + phone/YubiKey +
    /// paper backup, any two combine to sign.
    Threshold {
        t: u32,
        n: u32,
    },
}

// -----------------------------------------------------------------------------
// Secret records + capability handles
// -----------------------------------------------------------------------------

/// Metadata about a secret. The *material* itself lives encrypted
/// in `sealed_secrets.ciphertext`; this struct is what callers
/// pass around.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecretRecord {
    pub id: Ulid,
    pub workspace_id: String,
    pub class: SecretClass,
    pub protection: ProtectionLevel,
    /// Human-readable label, e.g. "openai-prod" or
    /// "lucerna-ssh-admin". Shown in UIs; NOT a secret.
    pub label: String,
    /// Providers allowed to request handles. Empty = none can.
    /// Rule 2 (scoping) is enforced here.
    pub allowed_providers: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub rotation_due_at: Option<DateTime<Utc>>,
    /// Set when the secret is retired (rotated out). A retired
    /// record's ciphertext MUST be destroyed; the row stays for
    /// audit continuity.
    #[serde(default)]
    pub retired_at: Option<DateTime<Utc>>,
}

/// Opaque handle the LLM / tools can see. Does NOT reveal the
/// underlying secret id. Expires after a bounded window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityHandle {
    /// Fresh ULID per issuance. Dereferenced once (or per
    /// configured reuse policy) before expiry.
    pub id: Ulid,
    pub provider_id: String,
    pub expires_at: DateTime<Utc>,
    /// Scope hint: which SecretClass this handle resolves to. The
    /// vault still rejects if the provider isn't in the secret's
    /// allowed_providers list.
    pub class: SecretClass,
}

// -----------------------------------------------------------------------------
// Threshold (ordo-secrets-threshold)
// -----------------------------------------------------------------------------

/// Announced when a share is first placed on a device. Consumed
/// by the registry so `secret.require_threshold_dereference` can
/// find the holders and kick off a signing round.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThresholdShareAnnouncement {
    pub share_id: Ulid,
    pub secret_id: Ulid,
    /// SHA-256 of the holder device's stable identifier. Does not
    /// reveal the identifier itself (which might be a passkey id
    /// or a hardware token serial).
    pub holder_device_fingerprint: [u8; 32],
    pub share_index: u32,
    pub total_shares: u32,
}

/// First round of FROST signing: participants publish nonce
/// commitments. Blueprint invariant: nonces are single-use,
/// enforced at the threshold crate boundary â€” callers cannot
/// accidentally reuse.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NonceCommitment {
    pub share_index: u32,
    pub hiding: [u8; 32],
    pub binding: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThresholdSigningRequest {
    pub operation_id: Ulid,
    pub secret_id: Ulid,
    pub nonce_commitments: Vec<NonceCommitment>,
    pub message_hash: [u8; 32],
}

// -----------------------------------------------------------------------------
// DRIFT: canary + custody (ordo-secrets-broker)
// -----------------------------------------------------------------------------

/// Per-capability canary. Generated at capability issuance,
/// injected into the tool's prompt context as a trap. Invariant
/// 22: the canary is tied to the *capability*, not the underlying
/// secret â€” a canary appearing in output means that specific
/// capability's context leaked, not that the secret is compromised.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityCanary {
    pub capability_id: Ulid,
    /// High-entropy token. Recommended: 24 bytes base32 = ~39
    /// chars, low collision risk, greppable in logs.
    pub canary_token: String,
    /// True once the planner has confirmed the canary is in the
    /// tool's context. Flips during issuance, before the tool
    /// runs.
    pub injected_into_context: bool,
}

/// Chain-of-custody over tool inputs. Verified by the validator
/// against the tool's claimed input.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputCustody {
    pub tool_invocation_id: Ulid,
    /// SHA-256 of the user intent + declared capability envelope.
    pub input_hash: [u8; 32],
    /// SHA-256 of the capability id list (sorted) at issuance.
    /// A tool that claims to respond to a different capability
    /// set than it was issued fails this check.
    pub declared_capabilities_hash: [u8; 32],
}

/// Outcome of the structural-limit check. `rejected_at` is set
/// only when the output exceeded the configured cap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuralOutputCheck {
    pub tool_invocation_id: Ulid,
    pub byte_budget: u64,
    pub actual_bytes: u64,
    pub rejected: bool,
    pub reason: Option<String>,
}

// -----------------------------------------------------------------------------
// Rotation (blueprint Â§"Automated rotation â€” IN")
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationReason {
    ScheduledPolicy,
    ComplianceRequirement,
    SuspectedCompromise,
    UserRequested,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RotationDue {
    pub secret_id: Ulid,
    pub class: SecretClass,
    pub reason: RotationReason,
    /// True when a registered `AutoRotator` can handle this class;
    /// false means the user must act.
    pub auto_rotator_available: bool,
}

/// Per-secret rotation policy. Defaults come from
/// `SecretClass::default_rotation_days`; operators can override
/// via `secret.update_rotation_policy`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RotationPolicy {
    pub secret_id: Ulid,
    pub days_until_rotation: u32,
    /// When in violation of the policy, immediately emit
    /// `RotationDue{reason:SuspectedCompromise}`.
    pub compromise_check: bool,
}

// -----------------------------------------------------------------------------
// Audit chain + transparency (ordo-secrets-audit)
// -----------------------------------------------------------------------------

/// One entry in the append-only audit hash chain. Every secret
/// operation that mutates state or dereferences material emits
/// one of these.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEntry {
    pub id: Ulid,
    pub sequence: u64,
    /// Blake3 of the prior entry's full serialized form. For the
    /// first entry in a workspace, this is all zeros (genesis).
    pub prev_hash: [u8; 32],
    pub timestamp: DateTime<Utc>,
    pub event_type: SecretAuditEventType,
    /// Opaque event-specific payload.
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretAuditEventType {
    SecretCreated,
    SecretRetired,
    SecretRotated,
    HandleIssued,
    HandleDereferenced,
    HandleExpired,
    HandleRevoked,
    ThresholdSigningBegan,
    ThresholdSigningCompleted,
    ThresholdShareRedistributed,
    CanaryDetected,
    CustodyMismatchDetected,
    StructuralLimitExceeded,
    AnchorSigned,
    RotationDue,
    SealTierDegraded,
}

/// Signed anchor over a contiguous slice of the audit chain.
/// COSE-shaped (SCITT receipts are signed in COSE format) so
/// plugging a real transparency service swaps only the
/// `TransparencyService` impl, not the signed-statement shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnchorStatement {
    pub workspace_id: String,
    /// Sequence range anchored, inclusive both ends.
    pub first_sequence: u64,
    pub last_sequence: u64,
    /// Blake3 of the anchored slice (prev_hash of first entry
    /// through the last entry's hash, folded).
    pub chain_root: [u8; 32],
    pub signed_at: DateTime<Utc>,
    /// COSE_Sign1-wrapped signature. Empty on unsigned drafts;
    /// populated by the signer before the anchor leaves the
    /// process.
    pub cose_sign1: Vec<u8>,
    /// Tier of the key that signed. Invariant 24 check at the
    /// caller.
    pub signer_tier: SealingTier,
}

/// Receipt returned by a transparency service. For the local
/// impl, this echoes the anchor. For an external service, this
/// contains the service's inclusion proof + service signature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransparencyReceipt {
    pub anchor: AnchorStatement,
    /// Service-specific additional material (inclusion proof,
    /// etc.). Opaque to the auditor crate.
    #[serde(default)]
    pub service_attestation: Option<Vec<u8>>,
    pub service_id: String,
}

// -----------------------------------------------------------------------------
// Bus topics
// -----------------------------------------------------------------------------

pub mod secrets_topics {
    // Vault
    pub const VAULT_STORE_REQUEST: &str = "ordo.secrets.vault.store.request";
    pub const VAULT_STORE_RESPONSE: &str = "ordo.secrets.vault.store.response";
    pub const VAULT_DEREFERENCE_REQUEST: &str = "ordo.secrets.vault.dereference.request";
    pub const VAULT_DEREFERENCE_RESPONSE: &str = "ordo.secrets.vault.dereference.response";
    pub const VAULT_SEAL_TIER_DEGRADED: &str = "ordo.secrets.vault.seal_tier_degraded";

    // Broker
    pub const BROKER_HANDLE_ISSUED: &str = "ordo.secrets.broker.handle_issued";
    pub const BROKER_HANDLE_REVOKED: &str = "ordo.secrets.broker.handle_revoked";
    pub const BROKER_CANARY_DETECTED: &str = "ordo.secrets.broker.canary_detected";
    pub const BROKER_CUSTODY_MISMATCH: &str = "ordo.secrets.broker.custody_mismatch";
    pub const BROKER_STRUCTURAL_REJECTION: &str = "ordo.secrets.broker.structural_rejection";

    // Threshold
    pub const THRESHOLD_DKG_REQUEST: &str = "ordo.secrets.threshold.dkg.request";
    pub const THRESHOLD_SIGNING_REQUEST: &str = "ordo.secrets.threshold.signing.request";
    pub const THRESHOLD_SHARE_ANNOUNCEMENT: &str = "ordo.secrets.threshold.share_announcement";
    pub const THRESHOLD_SIGNING_COMPLETED: &str = "ordo.secrets.threshold.signing_completed";

    // Audit
    pub const AUDIT_ENTRY_APPENDED: &str = "ordo.secrets.audit.entry_appended";
    pub const AUDIT_ANCHOR_SIGNED: &str = "ordo.secrets.audit.anchor_signed";
    pub const AUDIT_VERIFICATION_REQUEST: &str = "ordo.secrets.audit.verification.request";
    pub const AUDIT_VERIFICATION_RESPONSE: &str = "ordo.secrets.audit.verification.response";

    // Rotation
    pub const ROTATION_DUE: &str = "ordo.secrets.rotation.due";
    pub const ROTATION_COMPLETED: &str = "ordo.secrets.rotation.completed";
}

// -----------------------------------------------------------------------------
// Protocol invariants (21â€“24 from the blueprint)
// -----------------------------------------------------------------------------
//
// 21. Threshold-protected secrets require quorum to dereference.
//     Enforced in `ordo-secrets-broker::dereference_threshold`.
//
// 22. Canary tokens are per-capability, not per-secret. A canary
//     in output proves that capability's context leaked, not the
//     underlying secret. Enforced by `CapabilityCanary::capability_id`.
//
// 23. Rotation that destroys the old secret material is the only
//     valid rotation. Enforced in `ordo-secrets-vault::rotate`:
//     old ciphertext overwritten with zeros then the row marked
//     `retired_at`.
//
// 24. Transparency anchors are signed by keys sealed at Tier 1 or
//     Tier 2 only. Enforced in `ordo-secrets-audit::sign_anchor`
//     via `SealingTier::can_sign_transparency_anchors()`.
