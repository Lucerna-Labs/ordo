//! `ModeManifest` — typed config for one mode.
//!
//! Designed to be flat and Vec-of-strings heavy: every downstream
//! consumer (FactStore for memory_scope, RAG router for rag_domains,
//! ToolGateway for allowed_tool_lanes, security layer for policies)
//! reads a list of opaque strings it interprets in its own domain.
//! That keeps the manifest type stable across feature additions.
//!
//! ## Schema rules
//!
//! - `id`, `label`, `description` are required and non-empty.
//! - `memory_scope` is required and must include at least `"global"`.
//!   If not present, the loader inserts `"global"` and warns.
//! - All vec fields default to empty when absent. An empty
//!   `allowed_tool_lanes` means **no tools** are exposed (locked-down
//!   mode), not "all tools" — fail-closed.
//! - Optional overrides (`default_timeout_secs`, `default_strictness`,
//!   `default_credential`) fall through to the global setting when
//!   None.
//!
//! ## Trust boundary
//!
//! The manifest is operator-authored config. It isn't a security
//! statement — the security layer still has the final say on every
//! tool call. A malicious or buggy manifest can cripple a mode (no
//! tools, no memory) but it cannot escalate privilege. That's
//! deliberate: modes are about scoping the assistant's view of the
//! runtime, not about granting capabilities the runtime didn't
//! already have.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModeManifestError {
    #[error("manifest id must not be empty")]
    MissingId,
    #[error("manifest label must not be empty")]
    MissingLabel,
    #[error("manifest description must not be empty")]
    MissingDescription,
    #[error("manifest memory_scope must include 'global'")]
    MemoryScopeMissingGlobal,
    #[error("manifest id '{id}' contains invalid characters; use [a-z0-9_]")]
    InvalidId { id: String },
    #[error("default_strictness '{value}' is not one of off/low/medium/high")]
    InvalidStrictness { value: String },
    #[error("cross_mode_borrow_policy '{value}' is not one of allow/deny")]
    InvalidBorrowPolicy { value: String },
    #[error("cross_mode_consult_policy '{value}' is not one of allow/deny")]
    InvalidConsultPolicy { value: String },
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// A mode manifest. Operator-authored, runtime-loaded, validated at
/// load time.
///
/// Fields use snake_case JSON to match the rest of the codebase's
/// on-disk format conventions (niche modules, plugin manifests).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModeManifest {
    /// Stable id — used as the routing key in TurnRequest.metadata
    /// and as the suffix in `mode:<id>` memory scopes. Lowercase
    /// alphanumeric + underscores.
    pub id: String,
    /// Human-friendly name, shown in the UXI mode switcher.
    pub label: String,
    /// One-sentence description, shown in the mode tooltip.
    pub description: String,

    /// Memory scopes this mode can read. Always includes `"global"`
    /// (loader enforces). Typically also `"mode:<id>"`. Cross-mode
    /// borrowing CAN add transient scopes for one response, but
    /// not by default.
    #[serde(default)]
    pub memory_scope: Vec<String>,

    /// RAG collections this mode is allowed to search.
    #[serde(default)]
    pub rag_domains: Vec<String>,

    /// Tool lane prefixes the assistant can autonomously call when
    /// this mode is active. Empty = no tools (fail-closed). Match
    /// is by `starts_with` against the capability name, same
    /// semantics as the existing `DEFAULT_ALLOWED_LANES`.
    #[serde(default)]
    pub allowed_tool_lanes: Vec<String>,

    /// Specific capability names always blocked, even when their
    /// lane is allowed. Use sparingly — mostly for narrow
    /// "knowledge.* is allowed but knowledge.ingest_url is not"
    /// situations.
    #[serde(default)]
    pub blocked_tool_capabilities: Vec<String>,

    /// Policy ids the security layer interprets. The mode-side
    /// declares; the security layer enforces. Manifest-only fields
    /// the security layer doesn't recognize are ignored (forward
    /// compat for new policy types).
    #[serde(default)]
    pub policies: Vec<String>,

    /// Lines appended to the bootstrap system prompt as planner
    /// guidance. Free text — operators write what they want the
    /// model to bias toward in this mode.
    #[serde(default)]
    pub planner_bias: Vec<String>,

    /// Persona descriptors appended to the bootstrap system prompt.
    /// Free text — short role-or-tone tags.
    #[serde(default)]
    pub persona: Vec<String>,

    /// Per-mode timeout override. None = inherit global preset.
    /// In seconds. The Vibe Coding and Research defaults set this
    /// to 1800 (30 min) so deep loops don't time out at the
    /// global 5-min default.
    #[serde(default)]
    pub default_timeout_secs: Option<u64>,

    /// Per-mode untrusted-content strictness override. One of
    /// "off" / "low" / "medium" / "high". None = inherit global.
    #[serde(default)]
    pub default_strictness: Option<String>,

    /// Per-mode default credential service id. None = inherit
    /// operator's default credential.
    #[serde(default)]
    pub default_credential: Option<String>,

    /// Policy controlling whether OTHER modes can borrow raw memory/RAG from
    /// this one via a future `assistant.borrow_from_mode` flow. Values:
    ///
    ///   - `"allow"` (default) — borrows from this mode are
    ///     auto-approved. Audited regardless.
    ///   - `"deny"` — borrows from this mode are rejected at the
    ///     gate. Use for modes whose memory is sensitive enough
    ///     that the operator wants explicit mode-switching to
    ///     access it (Security mode's audit notes; future
    ///     compliance modes).
    ///
    /// The TARGET mode's policy is what's checked, not the active
    /// mode's. The decision is "may THIS mode be read FROM?" not
    /// "may THIS mode read?" — readers are universally permitted;
    /// readees decide if they want to be read.
    ///
    /// `None` is treated as `"allow"`.
    #[serde(default)]
    pub cross_mode_borrow_policy: Option<String>,

    /// Policy controlling whether OTHER modes may consult this mode
    /// through `assistant.consult_mode_agent`. This is separate from
    /// raw memory/RAG borrowing: consultation starts a bounded target-mode
    /// agent and returns only that agent's answer.
    ///
    /// `None` is treated as `"allow"`.
    #[serde(default)]
    pub cross_mode_consult_policy: Option<String>,
}

impl ModeManifest {
    /// Parse a manifest from a JSON string and validate it.
    pub fn from_json(input: &str) -> Result<Self, ModeManifestError> {
        let mut parsed: Self = serde_json::from_str(input)?;
        parsed.normalize_and_validate()?;
        Ok(parsed)
    }

    /// Serialize to pretty JSON — used when materializing the
    /// compiled-in defaults to disk on first run.
    pub fn to_pretty_json(&self) -> Result<String, ModeManifestError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Validate after deserialization OR direct construction.
    /// Public so the registry can call it on compiled-in defaults
    /// at startup — a default that fails validation is a build bug,
    /// not a runtime bug, and we want to know immediately.
    pub fn normalize_and_validate(&mut self) -> Result<(), ModeManifestError> {
        if self.id.trim().is_empty() {
            return Err(ModeManifestError::MissingId);
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(ModeManifestError::InvalidId {
                id: self.id.clone(),
            });
        }
        if self.label.trim().is_empty() {
            return Err(ModeManifestError::MissingLabel);
        }
        if self.description.trim().is_empty() {
            return Err(ModeManifestError::MissingDescription);
        }

        // Enforce memory_scope contains "global". An operator who
        // wants a hyper-locked mode can still write `["global"]`
        // — but never an empty list, because that would mean the
        // mode has no memory at all and the planner has nothing
        // to recall.
        if !self.memory_scope.iter().any(|s| s == "global") {
            return Err(ModeManifestError::MemoryScopeMissingGlobal);
        }

        if let Some(strict) = &self.default_strictness {
            match strict.as_str() {
                "off" | "low" | "medium" | "high" => {}
                other => {
                    return Err(ModeManifestError::InvalidStrictness {
                        value: other.to_string(),
                    });
                }
            }
        }

        validate_policy(self.cross_mode_borrow_policy.as_deref(), |value| {
            ModeManifestError::InvalidBorrowPolicy { value }
        })?;
        validate_policy(self.cross_mode_consult_policy.as_deref(), |value| {
            ModeManifestError::InvalidConsultPolicy { value }
        })?;

        // Dedupe vec fields — operators sometimes copy lines and
        // it's harmless but ugly. Stable order preserved.
        dedupe_preserve_order(&mut self.memory_scope);
        dedupe_preserve_order(&mut self.rag_domains);
        dedupe_preserve_order(&mut self.allowed_tool_lanes);
        dedupe_preserve_order(&mut self.blocked_tool_capabilities);
        dedupe_preserve_order(&mut self.policies);
        // planner_bias and persona are free-text — duplicates there
        // could be intentional (operator says it twice for emphasis).
        // Don't dedupe.

        Ok(())
    }

    /// Convenience — is borrowing FROM this mode permitted?
    /// `cross_mode_borrow_policy` defaults to `"allow"` when None.
    pub fn allows_borrow_from(&self) -> bool {
        !matches!(self.cross_mode_borrow_policy.as_deref(), Some("deny"))
    }

    /// Convenience — may another mode consult this mode's agent?
    /// `cross_mode_consult_policy` defaults to `"allow"` when None.
    pub fn allows_consult_from(&self) -> bool {
        !matches!(self.cross_mode_consult_policy.as_deref(), Some("deny"))
    }

    /// Convenience — does this mode allow the given capability?
    /// Lane match + blocklist check. Used by ToolGateway.
    pub fn allows_capability(&self, capability: &str) -> bool {
        if self
            .blocked_tool_capabilities
            .iter()
            .any(|c| c == capability)
        {
            return false;
        }
        self.allowed_tool_lanes
            .iter()
            .any(|prefix| capability.starts_with(prefix.as_str()))
    }
}

fn validate_policy(
    policy: Option<&str>,
    error: fn(String) -> ModeManifestError,
) -> Result<(), ModeManifestError> {
    match policy {
        Some("allow" | "deny") | None => Ok(()),
        Some(other) => Err(error(other.to_string())),
    }
}

fn dedupe_preserve_order(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|item| seen.insert(item.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_manifest() -> ModeManifest {
        ModeManifest {
            id: "test".into(),
            label: "Test".into(),
            description: "Test mode".into(),
            memory_scope: vec!["global".into(), "mode:test".into()],
            rag_domains: vec![],
            allowed_tool_lanes: vec![],
            blocked_tool_capabilities: vec![],
            policies: vec![],
            planner_bias: vec![],
            persona: vec![],
            default_timeout_secs: None,
            default_strictness: None,
            default_credential: None,
            cross_mode_borrow_policy: None,
            cross_mode_consult_policy: None,
        }
    }

    #[test]
    fn valid_manifest_passes() {
        let mut m = minimal_manifest();
        m.normalize_and_validate().expect("valid");
    }

    #[test]
    fn empty_id_rejected() {
        let mut m = minimal_manifest();
        m.id = "".into();
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::MissingId)
        ));
    }

    #[test]
    fn id_with_uppercase_rejected() {
        let mut m = minimal_manifest();
        m.id = "Vibe_Coding".into();
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::InvalidId { .. })
        ));
    }

    #[test]
    fn id_with_hyphen_rejected() {
        let mut m = minimal_manifest();
        m.id = "vibe-coding".into();
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::InvalidId { .. })
        ));
    }

    #[test]
    fn memory_scope_without_global_rejected() {
        let mut m = minimal_manifest();
        m.memory_scope = vec!["mode:test".into()];
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::MemoryScopeMissingGlobal)
        ));
    }

    #[test]
    fn empty_memory_scope_rejected() {
        let mut m = minimal_manifest();
        m.memory_scope = vec![];
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::MemoryScopeMissingGlobal)
        ));
    }

    #[test]
    fn invalid_strictness_rejected() {
        let mut m = minimal_manifest();
        m.default_strictness = Some("paranoid".into());
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::InvalidStrictness { .. })
        ));
    }

    #[test]
    fn dedupes_preserving_order() {
        let mut m = minimal_manifest();
        m.allowed_tool_lanes = vec![
            "filesystem.read_".into(),
            "knowledge.".into(),
            "filesystem.read_".into(), // dup
        ];
        m.normalize_and_validate().unwrap();
        assert_eq!(
            m.allowed_tool_lanes,
            vec!["filesystem.read_".to_string(), "knowledge.".into()]
        );
    }

    #[test]
    fn unknown_field_rejected() {
        // deny_unknown_fields means a typo'd field surfaces as a
        // load error, not silent acceptance.
        let bad = r#"{
            "id": "x",
            "label": "x",
            "description": "x",
            "memory_scope": ["global"],
            "memry_scop": ["typo here"]
        }"#;
        assert!(ModeManifest::from_json(bad).is_err());
    }

    #[test]
    fn allows_capability_lane_match() {
        let mut m = minimal_manifest();
        m.allowed_tool_lanes = vec!["filesystem.read_".into(), "web.".into()];
        m.normalize_and_validate().unwrap();
        assert!(m.allows_capability("filesystem.read_file"));
        assert!(m.allows_capability("web.fetch_and_strain"));
        assert!(!m.allows_capability("filesystem.write_file"));
        assert!(!m.allows_capability("api.dispatch_webhook"));
    }

    #[test]
    fn allows_capability_blocklist_overrides_lane() {
        let mut m = minimal_manifest();
        m.allowed_tool_lanes = vec!["knowledge.".into()];
        m.blocked_tool_capabilities = vec!["knowledge.ingest_url".into()];
        m.normalize_and_validate().unwrap();
        assert!(m.allows_capability("knowledge.answer_question"));
        assert!(!m.allows_capability("knowledge.ingest_url"));
    }

    #[test]
    fn empty_lane_list_means_no_tools() {
        // Fail-closed by design: no `allowed_tool_lanes` = locked
        // down. Operator opts INTO tool exposure per mode.
        let mut m = minimal_manifest();
        m.allowed_tool_lanes = vec![];
        m.normalize_and_validate().unwrap();
        assert!(!m.allows_capability("filesystem.read_file"));
        assert!(!m.allows_capability("web.strain"));
    }

    #[test]
    fn borrow_policy_defaults_to_allow_when_none() {
        let m = minimal_manifest();
        assert!(m.allows_borrow_from(), "None policy must allow borrow");
    }

    #[test]
    fn borrow_policy_allow_explicit() {
        let mut m = minimal_manifest();
        m.cross_mode_borrow_policy = Some("allow".into());
        m.normalize_and_validate().unwrap();
        assert!(m.allows_borrow_from());
    }

    #[test]
    fn borrow_policy_deny_blocks() {
        let mut m = minimal_manifest();
        m.cross_mode_borrow_policy = Some("deny".into());
        m.normalize_and_validate().unwrap();
        assert!(!m.allows_borrow_from());
    }

    #[test]
    fn invalid_borrow_policy_rejected_at_validation() {
        let mut m = minimal_manifest();
        m.cross_mode_borrow_policy = Some("ask".into()); // future, not yet supported
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::InvalidBorrowPolicy { .. })
        ));
    }

    #[test]
    fn consult_policy_defaults_to_allow_when_none() {
        let m = minimal_manifest();
        assert!(m.allows_consult_from(), "None policy must allow consult");
    }

    #[test]
    fn consult_policy_deny_blocks_consult_but_not_borrow() {
        let mut m = minimal_manifest();
        m.cross_mode_consult_policy = Some("deny".into());
        m.normalize_and_validate().unwrap();
        assert!(!m.allows_consult_from());
        assert!(m.allows_borrow_from());
    }

    #[test]
    fn invalid_consult_policy_rejected_at_validation() {
        let mut m = minimal_manifest();
        m.cross_mode_consult_policy = Some("ask".into());
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::InvalidConsultPolicy { .. })
        ));
    }

    #[test]
    fn round_trip_through_json() {
        let mut original = minimal_manifest();
        original.allowed_tool_lanes = vec!["web.".into(), "knowledge.".into()];
        original.policies = vec!["confirm_file_writes".into()];
        original.default_timeout_secs = Some(1800);
        original.normalize_and_validate().unwrap();

        let serialized = original.to_pretty_json().unwrap();
        let restored = ModeManifest::from_json(&serialized).unwrap();
        assert_eq!(original, restored);
    }
}
