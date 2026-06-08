//! MCP security architecture wire types.
//!
//! Load-bearing commitments (enforced across `ordo-mcp-worker`,
//! `ordo-mcp-sandbox`, `ordo-mcp-client`, `ordo-mcp-registry`,
//! `ordo-mcp-provenance`):
//!
//! 1. The Planner LLM never processes raw MCP tool responses â€”
//!    Workers extract structured data in a quarantined context.
//! 2. Every external MCP server runs in a WASM sandbox with
//!    default-deny host functions.
//! 3. Every tool invocation carries an ordinal privilege tier.
//! 4. Every installed MCP server has a signed lockfile; drift
//!    blocks execution.
//! 5. Causal provenance tracks taint for every event; sensitive
//!    actions check ancestry.
//!
//! Protocol invariants 25â€“34 live as doc comments at the bottom
//! of this file.
//!
//! Rule 11: this crate owns wire-shared shapes. Changes go through
//! the CHANGELOG. Nothing outside this module defines these types.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::memory::Ulid;
use crate::secrets::SecretClass;

// -----------------------------------------------------------------------------
// Server identity + trust
// -----------------------------------------------------------------------------

/// Ordinal trust state for an installed MCP server. Graduates
/// automatically on clean invocation history; demotes on anomaly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerTrustState {
    /// Freshly installed. Every call logged at full fidelity.
    Untrusted,
    /// Recent anomaly or drift observed. Tighter monitoring.
    Observed,
    /// Lockfile clean; invocation history clean; background monitoring.
    Validated,
    /// Long clean history. Exception-based alerting only.
    Trusted,
    /// Active anomaly or user-flagged. Blocked from invocation.
    Quarantined,
}

impl ServerTrustState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::Observed => "observed",
            Self::Validated => "validated",
            Self::Trusted => "trusted",
            Self::Quarantined => "quarantined",
        }
    }

    /// Ordinal â€” higher = more trusted. `Quarantined` is treated as
    /// "lower than Untrusted" for gating (cannot invoke at all).
    pub fn rank(self) -> i8 {
        match self {
            Self::Quarantined => -1,
            Self::Untrusted => 0,
            Self::Observed => 1,
            Self::Validated => 2,
            Self::Trusted => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerIdentity {
    pub name: String,
    pub version: String,
    pub publisher: String,
    /// Sigstore certificate bytes. When absent / zero-length the
    /// install path MUST refuse the server (invariant 28).
    #[serde(default)]
    pub sigstore_cert: Vec<u8>,
    pub identity_hash: [u8; 32],
}

// -----------------------------------------------------------------------------
// Capability declaration (server â†’ runtime at install time)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CapabilityDeclaration {
    /// Named host functions the server claims to need (see
    /// ordo-mcp-sandbox for the legal list).
    pub host_functions: Vec<String>,
    /// Outbound HTTP allowlist. Empty = no outbound access.
    pub domains: Vec<String>,
    /// Filesystem paths the server may read/write. Empty = no fs.
    pub filesystem_paths: Vec<String>,
    /// Bus topics the server may emit / listen on. Scoped; empty =
    /// none.
    pub bus_topics: Vec<String>,
    /// Secret classes the server may request handles to.
    pub secret_classes: Vec<SecretClass>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceLimits {
    pub fuel_per_invocation: u64,
    pub memory_bytes: u64,
    pub max_response_size_bytes: u64,
    pub max_nesting_depth: u32,
    pub rate_limit_per_minute: u32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        // Blueprint suggested defaults; tighter by design so
        // misbehavior surfaces early.
        Self {
            fuel_per_invocation: 100_000_000,
            memory_bytes: 128 * 1024 * 1024,
            max_response_size_bytes: 10 * 1024 * 1024,
            max_nesting_depth: 32,
            rate_limit_per_minute: 60,
        }
    }
}

// -----------------------------------------------------------------------------
// Tool schema + risk
// -----------------------------------------------------------------------------

/// A tool's declared surface. `input_schema` + `output_schema` are
/// stored as opaque JSON Schema values (Rule 9: descriptive, not
/// runtime-enforced typed decoding). The Worker uses
/// `output_schema` at extraction time to validate shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default)]
    pub output_schema: Value,
    pub risk_level: ToolRiskLevel,
}

/// Ordinal risk per tool. Drives minimum-trust gating at
/// invocation time: `HighRisk` tools require server at `Trusted`
/// or above; `Sensitive` requires `Validated` or above; etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskLevel {
    /// Read-only queries. No state mutation, no outbound effects.
    ReadOnly,
    /// Mutates state the user owns (local files, app data).
    Mutating,
    /// Touches credentials, user data across services, outbound
    /// calls on the user's behalf.
    Sensitive,
    /// Can cause persistent or irreversible effects; requires
    /// `Trusted` server state.
    HighRisk,
}

impl ToolRiskLevel {
    /// Minimum server trust state required to invoke a tool at
    /// this risk level. Gating happens in `ordo-mcp-client`.
    pub fn min_trust(self) -> ServerTrustState {
        match self {
            Self::ReadOnly => ServerTrustState::Untrusted,
            Self::Mutating => ServerTrustState::Observed,
            Self::Sensitive => ServerTrustState::Validated,
            Self::HighRisk => ServerTrustState::Trusted,
        }
    }
}

// -----------------------------------------------------------------------------
// Taint + privilege tier
// -----------------------------------------------------------------------------

/// Taint classification per event in the log. Propagates through
/// causal chains (see `ordo-mcp-provenance`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Taint {
    /// System-originated. Default for runtime-emitted events.
    Trusted,
    /// User message or user-initiated action.
    User,
    /// Event emitted by a first-party verified provider.
    VerifiedProvider,
    /// Came out of an external MCP server. Tracked so sensitive
    /// actions downstream can be gated.
    UntrustedMcp {
        server_id: String,
        invocation_id: String,
    },
    /// Came from a web fetch that flowed through `ordo-strainer`.
    /// Pairs with the boundary tag the assistant sees in its prompt.
    /// Once a conversation has ingested untrusted-web content, all
    /// subsequent turns inherit the taint until the operator clears
    /// it explicitly — defends against slow-injection ("plant a fact
    /// now, exploit five turns later") attacks the strainer can't
    /// catch on the content layer.
    UntrustedWeb {
        source_url: String,
        fetched_at: String,
    },
    /// Aggregation when multiple tainted sources converge.
    Mixed { sources: Vec<Taint> },
}

impl Taint {
    /// True when this taint should gate sensitive actions.
    pub fn is_untrusted(&self) -> bool {
        match self {
            Taint::Trusted | Taint::User | Taint::VerifiedProvider => false,
            Taint::UntrustedMcp { .. } => true,
            Taint::UntrustedWeb { .. } => true,
            Taint::Mixed { sources } => sources.iter().any(Taint::is_untrusted),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Taint::Trusted => "trusted",
            Taint::User => "user",
            Taint::VerifiedProvider => "verified_provider",
            Taint::UntrustedMcp { .. } => "untrusted_mcp",
            Taint::UntrustedWeb { .. } => "untrusted_web",
            Taint::Mixed { .. } => "mixed",
        }
    }
}

/// Privilege tier for content entering the LLM. Ordinal;
/// projection assembles prompts with explicit tier tags so
/// Planner models see authority-framed content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivilegeTier {
    /// Tier 1 â€” system prompt, architectural invariants.
    System,
    /// Tier 2 â€” pinned identity assertions.
    Identity,
    /// Tier 3 â€” user messages, active workflow checkpoints.
    User,
    /// Tier 4 â€” verified provider tool output (first-party crates).
    VerifiedProvider,
    /// Tier 5 â€” untrusted MCP server tool output, Worker-extracted.
    UntrustedMcp,
}

impl PrivilegeTier {
    pub fn ordinal(self) -> u8 {
        match self {
            Self::System => 1,
            Self::Identity => 2,
            Self::User => 3,
            Self::VerifiedProvider => 4,
            Self::UntrustedMcp => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Identity => "Identity",
            Self::User => "User",
            Self::VerifiedProvider => "VerifiedProvider",
            Self::UntrustedMcp => "UntrustedMcp",
        }
    }

    /// Opening tag used by projection when assembling prompts.
    pub fn open_tag(self) -> String {
        format!("[[Privilege {}: {}]]", self.ordinal(), self.label())
    }

    /// Closing tag. Symmetric with `open_tag` â€” same ordinal.
    pub fn close_tag(self) -> String {
        format!("[[/Privilege {}]]", self.ordinal())
    }
}

// -----------------------------------------------------------------------------
// Capability attenuation (AAT slot on top of secrets CapabilityHandle)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AttenuationConstraints {
    /// Per-argument constraints. Empty map = no constraints.
    #[serde(default)]
    pub argument_constraints: HashMap<String, ArgumentConstraint>,
    /// When present, the capability may only drive tool calls whose
    /// name is in the list.
    #[serde(default)]
    pub tool_allowlist: Option<Vec<String>>,
    /// Max total invocations this capability may drive.
    #[serde(default)]
    pub max_invocations: Option<u32>,
    /// Wall-clock deadline (ms since unix epoch). Past deadline =
    /// capability inert even if not explicitly revoked.
    #[serde(default)]
    pub deadline_ms: Option<i64>,
}

impl AttenuationConstraints {
    /// Monotonic attenuation check: `other` must be at least as
    /// restrictive as `self`. Invariant 33 â€” widening is rejected.
    pub fn is_narrower_or_equal(&self, other: &AttenuationConstraints) -> bool {
        // tool_allowlist narrowing: None â†’ Some OK; Some(A) â†’ Some(B) OK only if B âŠ† A
        match (&self.tool_allowlist, &other.tool_allowlist) {
            (None, _) => {}
            (Some(_), None) => return false,
            (Some(a), Some(b)) => {
                for item in b {
                    if !a.contains(item) {
                        return false;
                    }
                }
            }
        }
        // max_invocations narrowing: None â†’ Some OK; Some(n) â†’ Some(m) OK only if m â‰¤ n
        match (self.max_invocations, other.max_invocations) {
            (None, _) => {}
            (Some(_), None) => return false,
            (Some(n), Some(m)) => {
                if m > n {
                    return false;
                }
            }
        }
        // deadline narrowing: None â†’ Some OK; Some(d1) â†’ Some(d2) OK only if d2 â‰¤ d1
        match (self.deadline_ms, other.deadline_ms) {
            (None, _) => {}
            (Some(_), None) => return false,
            (Some(d1), Some(d2)) => {
                if d2 > d1 {
                    return false;
                }
            }
        }
        // Argument constraints: every key in self must remain, and
        // its value must not loosen. We approximate "no loosening"
        // by requiring identical constraint shapes â€” a real narrower
        // check is constraint-type-specific.
        for (k, v) in &self.argument_constraints {
            match other.argument_constraints.get(k) {
                None => return false,
                Some(o) => {
                    if v != o {
                        return false;
                    }
                }
            }
        }
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArgumentConstraint {
    Exact {
        value: Value,
    },
    Enum {
        values: Vec<Value>,
    },
    Range {
        min: Option<Value>,
        max: Option<Value>,
    },
    Pattern {
        regex: String,
    },
    Custom {
        label: String,
    },
}

// -----------------------------------------------------------------------------
// Authentication (DPoP)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DpopProof {
    pub jwt: String,
    pub nonce: [u8; 32],
    pub session_key_fingerprint: [u8; 32],
}

// -----------------------------------------------------------------------------
// Attestation (community-reputation slot â€” `LocalAttestationOnly`
// is the default impl; plugs in external sources when ecosystem
// networks stabilize)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attestation {
    pub attester_pubkey: Vec<u8>,
    pub server_identity_hash: [u8; 32],
    pub behavior_trace_hash: [u8; 32],
    pub trust_claim: TrustClaim,
    pub signature: Vec<u8>,
    pub attested_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrustClaim {
    BehaviorMatches { lockfile_hash: [u8; 32] },
    AnomalyObserved { anomaly_type: String },
    RecommendBlock { reason: String },
}

// -----------------------------------------------------------------------------
// Lockfile
// -----------------------------------------------------------------------------

/// A signed snapshot of a server's installed surface. Stored
/// alongside the sandbox installation; on every invocation the
/// client verifies the server's current shape against this
/// lockfile before executing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerLockfile {
    pub server_id: Ulid,
    pub server_identity: ServerIdentity,
    pub installed_at_ms: i64,
    #[serde(default)]
    pub sigstore_certificate: Vec<u8>,
    /// blake3 over the server's sorted tool surface (name+schema+risk).
    pub tool_catalog_hash: [u8; 32],
    pub declared_capabilities: CapabilityDeclaration,
    pub host_function_allowlist: Vec<String>,
    pub domain_allowlist: Vec<String>,
    pub filesystem_paths_allowlist: Vec<String>,
    pub bus_topics_allowlist: Vec<String>,
    pub resource_limits: ResourceLimits,
    pub signed_at_ms: i64,
    /// COSE_Sign1 or Ed25519 detached signature of the lockfile
    /// (minus this field) produced by the runtime's signing key.
    pub runtime_signature: Vec<u8>,
}

// -----------------------------------------------------------------------------
// Extraction (Worker â†” Client)
// -----------------------------------------------------------------------------

/// Worker extraction result. `sanitization_node_id` is the event
/// id the Worker emitted recording the validation; provenance
/// uses it to break taint propagation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpExtractionResult {
    pub extracted_data: Value,
    pub sanitization_node_id: Ulid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpExtractionError {
    SchemaViolation { details: String },
    InstructionDensityExceeded { matches: u32 },
    SchemaChangeAttempt { details: String },
    StructuralAnomaly { details: String },
    WorkerFailure { details: String },
}

// -----------------------------------------------------------------------------
// Sandbox reporting
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ResourceUsage {
    pub fuel_consumed: u64,
    pub memory_peak_bytes: u64,
    pub host_calls: u32,
    pub wall_clock_ms: u64,
}

/// A host-function call the sandbox mediated. Emitted on the bus
/// so audit / provenance can observe every policy decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostCallRecord {
    pub server_id: Ulid,
    pub invocation_id: Ulid,
    pub function: String,
    pub arguments_summary: String,
    pub outcome: HostCallOutcome,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostCallOutcome {
    Allowed,
    DeniedEgress { domain: String },
    DeniedFilesystem { path: String },
    DeniedTopic { topic: String },
    DeniedCapability { capability: String },
    ResourceLimit { limit: String },
    Error { details: String },
}

// -----------------------------------------------------------------------------
// Provenance
// -----------------------------------------------------------------------------

/// Reachability check payload. The caller submits a proposed
/// action id plus the causal chain (event ids in order) leading
/// to it; the service returns whether any tainted ancestor sits
/// in the path that hasn't been sanitized.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProvenanceCheckRequest {
    pub action: String,
    pub proposed_causal_chain: Vec<Ulid>,
    /// How many prior turns to walk. Default 2 (current + 2 prior).
    #[serde(default)]
    pub horizon_turns: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProvenanceCheckResult {
    pub allowed: bool,
    /// If blocked, the ancestor path that carries the taint â€”
    /// oldest first. Empty when `allowed == true`.
    pub taint_path: Vec<Ulid>,
    pub summary: String,
}

// -----------------------------------------------------------------------------
// Topics â€” MCP bus routing constants (single source of truth)
// -----------------------------------------------------------------------------

pub mod mcp_topics {
    // Worker
    pub const WORKER_EXTRACT: &str = "ordo.mcp.worker.extract";
    pub const WORKER_EXTRACT_RESULT: &str = "ordo.mcp.worker.extract.result";
    pub const WORKER_STATUS: &str = "ordo.mcp.worker.status";

    // Sandbox
    pub const SANDBOX_INSTALL: &str = "ordo.mcp.sandbox.install";
    pub const SANDBOX_INSTALLED: &str = "ordo.mcp.sandbox.installed";
    pub const SANDBOX_UNINSTALL: &str = "ordo.mcp.sandbox.uninstall";
    pub const SANDBOX_INVOKE: &str = "ordo.mcp.sandbox.invoke";
    pub const SANDBOX_INVOKE_RESULT: &str = "ordo.mcp.sandbox.invoke.result";
    pub const SANDBOX_STATUS: &str = "ordo.mcp.sandbox.status";
    pub const SANDBOX_HOST_CALL: &str = "ordo.mcp.sandbox.host_call";
    pub const SANDBOX_VIOLATION: &str = "ordo.mcp.sandbox.violation";

    // Client
    pub const CLIENT_INVOKE: &str = "ordo.mcp.client.invoke";
    pub const CLIENT_INVOKE_RESULT: &str = "ordo.mcp.client.invoke.result";
    pub const CLIENT_DISCOVER: &str = "ordo.mcp.client.discover";
    pub const CLIENT_DISCOVER_RESULT: &str = "ordo.mcp.client.discover.result";
    pub const CLIENT_AUTH_DEGRADED: &str = "ordo.mcp.client.auth.degraded";

    // Registry
    pub const REGISTRY_STATE: &str = "ordo.mcp.registry.state";
    pub const REGISTRY_DRIFT_DETECTED: &str = "ordo.mcp.registry.drift_detected";
    pub const REGISTRY_TRUST_CHANGED: &str = "ordo.mcp.registry.trust_changed";
    pub const REGISTRY_QUARANTINE: &str = "ordo.mcp.registry.quarantine";
    pub const REGISTRY_RE_AUTHORIZED: &str = "ordo.mcp.registry.re_authorized";

    // Provenance
    pub const PROVENANCE_CHECK: &str = "ordo.mcp.provenance.check";
    pub const PROVENANCE_CHECK_RESULT: &str = "ordo.mcp.provenance.check.result";
    pub const PROVENANCE_SANITIZE: &str = "ordo.mcp.provenance.sanitize";
    pub const PROVENANCE_USER_CONFIRM: &str = "ordo.mcp.provenance.user_confirm";
    pub const PROVENANCE_SENSITIVE_BLOCKED: &str = "ordo.mcp.provenance.sensitive.blocked";
}

// -----------------------------------------------------------------------------
// Protocol invariants (25â€“34 from the blueprint)
// -----------------------------------------------------------------------------
//
// 25. The Planner LLM never processes raw MCP tool responses.
//     Enforced in `ordo-mcp-client` â€” every `mcp.invoke` path
//     routes raw responses through `ordo-mcp-worker` before
//     returning to the Planner.
//
// 26. Every external MCP server runs in a WASM sandbox.
//     Enforced in `ordo-mcp-sandbox::install` â€” non-WASM rejected.
//
// 27. Every MCP invocation carries a privilege tier tag.
//     Enforced by projection assembly (memory-projection) +
//     `ordo-mcp-client` invocation pipeline.
//
// 28. Every installed MCP server has a signed lockfile.
//     Enforced in `ordo-mcp-registry::install_lockfile` â€”
//     unsigned / unverifiable installations rejected.
//
// 29. Drift from a signed lockfile blocks execution.
//     Enforced in `ordo-mcp-registry::detect_drift` â€” any
//     catalog-add or capability-widen blocks until re-authorized.
//
// 30. Sensitive actions with tainted causal ancestry require
//     sanitization or user confirmation. Enforced in
//     `ordo-mcp-provenance::check`.
//
// 31. Workers are zeroized on disposal. Enforced in
//     `ordo-mcp-worker::WorkerPool::dispose` â€” context + KV cache
//     cleared.
//
// 32. Egress from MCP servers is default-deny, declared-allow.
//     Enforced in `ordo-mcp-sandbox::host_http_fetch`.
//
// 33. Capability attenuation is monotonic. Enforced in
//     `AttenuationConstraints::is_narrower_or_equal`.
//
// 34. DPoP proofs are single-use. Enforced by nonce registry
//     in `ordo-mcp-client::DpopNonceLedger`.
