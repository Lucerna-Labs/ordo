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
    #[error("max_skill_risk '{value}' is not one of low/medium/high")]
    InvalidSkillRisk { value: String },
    #[error("default_skill_admission '{value}' is not one of permissive/restrictive")]
    InvalidSkillAdmission { value: String },
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

    // ── Skill routing (hybrid: skills self-declare; modes can veto/broaden).
    // See docs/skill-routing.md. These gate which markdown SKILL.md playbooks
    // are SURFACED in this mode — discovery only, never execution authority
    // (tool calls remain gated by `allowed_tool_lanes`).
    /// Skill tags this mode admits even when a skill did not self-declare this
    /// mode. Lets an operator broaden a mode by tag (e.g. `["rust"]`). Empty =
    /// no tag-based broadening.
    #[serde(default)]
    pub allowed_skill_tags: Vec<String>,

    /// Skill tags this mode vetoes. A skill carrying any of these tags is never
    /// surfaced here, regardless of its self-declaration. Safety backstop.
    #[serde(default)]
    pub blocked_skill_tags: Vec<String>,

    /// Skill ids this mode vetoes outright.
    #[serde(default)]
    pub blocked_skills: Vec<String>,

    /// Risk ceiling: skills above this level are vetoed. One of
    /// `"low"`/`"medium"`/`"high"`. `None` = no ceiling. Set `"low"` on
    /// high-isolation modes to keep risky community skills out by default.
    #[serde(default)]
    pub max_skill_risk: Option<String>,

    /// How to treat a skill that declares NO modes and matches no allowed tag.
    /// `"permissive"` (default) surfaces it; `"restrictive"` hides it. Isolation
    /// modes should set `"restrictive"`. `None` = permissive.
    #[serde(default)]
    pub default_skill_admission: Option<String>,

    /// Protected (built-in core) mode. Protected modes ship compiled-in and the
    /// mode-lifecycle surface refuses to DELETE them — so an operator can't
    /// casually remove `general`, `diagnostic`, or another core mode. Editing a
    /// protected mode's config is still allowed; only deletion is guarded.
    /// User-created modes default to `false` and are freely deletable.
    /// See `docs/mode-lifecycle.md`.
    #[serde(default)]
    pub protected: bool,
}

/// The outcome of routing a skill to a mode, with the reason — produced by
/// [`ModeManifest::skill_verdict`] and consumed by the routing audit
/// (`crate::audit`). `allows_skill` is just `skill_verdict(...).admitted()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillDecision {
    /// Admitted: the skill self-declared this mode in `available_to_modes`.
    AdmittedDeclared,
    /// Admitted: the mode opted into one of the skill's tags.
    AdmittedTag,
    /// Admitted: undeclared skill, mode's default admission is permissive.
    AdmittedDefault,
    /// Vetoed: the skill id is in the mode's `blocked_skills`.
    VetoedById,
    /// Vetoed: the skill carries a tag in the mode's `blocked_skill_tags`.
    VetoedByTag(String),
    /// Vetoed: the skill's risk exceeds the mode's `max_skill_risk` ceiling.
    VetoedByRisk,
    /// Rejected: the skill self-declared OTHER modes (not this one) and no tag
    /// broadened it here. Not an anomaly — the skill author scoped it out.
    RejectedNotDeclared,
    /// Rejected: undeclared skill, mode's default admission is restrictive.
    RejectedRestrictive,
}

impl SkillDecision {
    /// Was the skill surfaced in the mode?
    pub fn admitted(&self) -> bool {
        matches!(
            self,
            SkillDecision::AdmittedDeclared
                | SkillDecision::AdmittedTag
                | SkillDecision::AdmittedDefault
        )
    }

    /// Was the skill actively vetoed (as opposed to merely not admitted)?
    pub fn is_veto(&self) -> bool {
        matches!(
            self,
            SkillDecision::VetoedById | SkillDecision::VetoedByTag(_) | SkillDecision::VetoedByRisk
        )
    }

    /// A short human-readable reason, for audit reports.
    pub fn reason(&self) -> String {
        match self {
            SkillDecision::AdmittedDeclared => "admitted: self-declared this mode".into(),
            SkillDecision::AdmittedTag => "admitted: mode allows a skill tag".into(),
            SkillDecision::AdmittedDefault => "admitted: permissive default".into(),
            SkillDecision::VetoedById => "vetoed: skill id is blocked".into(),
            SkillDecision::VetoedByTag(tag) => format!("vetoed: blocked tag '{tag}'"),
            SkillDecision::VetoedByRisk => "vetoed: risk above mode ceiling".into(),
            SkillDecision::RejectedNotDeclared => {
                "rejected: skill declared other modes, not this one".into()
            }
            SkillDecision::RejectedRestrictive => {
                "rejected: undeclared + restrictive default".into()
            }
        }
    }
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

        if let Some(risk) = &self.max_skill_risk {
            if ordo_skills::RiskLevel::parse(risk).is_none() {
                return Err(ModeManifestError::InvalidSkillRisk {
                    value: risk.clone(),
                });
            }
        }
        if let Some(admission) = &self.default_skill_admission {
            match admission.as_str() {
                "permissive" | "restrictive" => {}
                other => {
                    return Err(ModeManifestError::InvalidSkillAdmission {
                        value: other.to_string(),
                    });
                }
            }
        }

        // Dedupe vec fields — operators sometimes copy lines and
        // it's harmless but ugly. Stable order preserved.
        dedupe_preserve_order(&mut self.memory_scope);
        dedupe_preserve_order(&mut self.rag_domains);
        dedupe_preserve_order(&mut self.allowed_tool_lanes);
        dedupe_preserve_order(&mut self.blocked_tool_capabilities);
        dedupe_preserve_order(&mut self.policies);
        dedupe_preserve_order(&mut self.allowed_skill_tags);
        dedupe_preserve_order(&mut self.blocked_skill_tags);
        dedupe_preserve_order(&mut self.blocked_skills);
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

    /// Hybrid skill routing: should this mode SURFACE the given skill?
    ///
    /// Precedence (see `docs/skill-routing.md`): **veto > self-declaration >
    /// tag-allow > per-mode default**. Discovery only — a surfaced skill grants
    /// no capability; tool calls remain gated by [`allows_capability`].
    pub fn allows_skill(&self, skill: &ordo_skills::SkillManifest) -> bool {
        self.skill_verdict(skill).admitted()
    }

    /// Like [`allows_skill`] but returns WHY. The routing audit uses this to
    /// explain orphaned skills and declared-but-vetoed contradictions. Encodes
    /// the precedence: veto > self-declaration > tag-allow > per-mode default.
    pub fn skill_verdict(&self, skill: &ordo_skills::SkillManifest) -> SkillDecision {
        // ── veto first; safety always wins ──
        if self.blocked_skills.iter().any(|id| id == &skill.id) {
            return SkillDecision::VetoedById;
        }
        for tag in &skill.tags {
            if self.blocked_skill_tags.iter().any(|b| b == tag) {
                return SkillDecision::VetoedByTag(tag.clone());
            }
        }
        if let Some(ceiling) = self
            .max_skill_risk
            .as_deref()
            .and_then(ordo_skills::RiskLevel::parse)
        {
            if skill.risk_level.rank() > ceiling.rank() {
                return SkillDecision::VetoedByRisk;
            }
        }

        // ── admission ──
        let tag_allowed = skill
            .tags
            .iter()
            .any(|t| self.allowed_skill_tags.iter().any(|a| a == t));
        if !skill.modes.is_empty() {
            // Skill self-declared its modes. Admit if this mode is named, or if
            // the operator broadened by allowing one of the skill's tags.
            if skill.modes.iter().any(|m| m == &self.id) {
                return SkillDecision::AdmittedDeclared;
            }
            if tag_allowed {
                return SkillDecision::AdmittedTag;
            }
            return SkillDecision::RejectedNotDeclared;
        }
        if tag_allowed {
            return SkillDecision::AdmittedTag;
        }
        // Undeclared + no tag match → per-mode default.
        if matches!(self.default_skill_admission.as_deref(), Some("restrictive")) {
            SkillDecision::RejectedRestrictive
        } else {
            SkillDecision::AdmittedDefault
        }
    }

    /// Build a fresh, UNPROTECTED user mode from a display name, with safe
    /// General-like defaults — the target of "Create mode" (see
    /// `docs/mode-lifecycle.md`). The id is slugified from the name
    /// (`[a-z0-9_]`); the operator tunes lanes / skills / persona afterward.
    pub fn new_user_mode(name: &str) -> Result<Self, ModeManifestError> {
        let label = name.trim();
        if label.is_empty() {
            return Err(ModeManifestError::MissingLabel);
        }
        let id = slugify_mode_id(label);
        if id.is_empty() {
            return Err(ModeManifestError::InvalidId {
                id: name.to_string(),
            });
        }
        let mut manifest = ModeManifest {
            id: id.clone(),
            label: label.to_string(),
            description: format!("{label} workspace."),
            memory_scope: vec!["global".to_string(), format!("mode:{id}")],
            rag_domains: vec![format!("research_{id}")],
            allowed_tool_lanes: vec![
                "filesystem.read_".to_string(),
                "knowledge.".to_string(),
                "memory.list_".to_string(),
                "memory.remember_".to_string(),
                "web.".to_string(),
                "code.".to_string(),
                "workspace.".to_string(),
            ],
            blocked_tool_capabilities: Vec::new(),
            policies: Vec::new(),
            planner_bias: Vec::new(),
            persona: Vec::new(),
            default_timeout_secs: None,
            default_strictness: None,
            default_credential: None,
            cross_mode_borrow_policy: None,
            cross_mode_consult_policy: None,
            allowed_skill_tags: Vec::new(),
            blocked_skill_tags: Vec::new(),
            blocked_skills: Vec::new(),
            max_skill_risk: None,
            default_skill_admission: None,
            protected: false,
        };
        manifest.normalize_and_validate()?;
        Ok(manifest)
    }
}

/// Derive a valid mode id (`[a-z0-9_]`, no leading/trailing/doubled `_`) from a
/// free-text display name. Runs of non-alphanumerics collapse to a single `_`.
pub fn slugify_mode_id(name: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            pending_sep = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_sep = true;
        }
    }
    out
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
            allowed_skill_tags: vec![],
            blocked_skill_tags: vec![],
            blocked_skills: vec![],
            max_skill_risk: None,
            default_skill_admission: None,
            protected: false,
        }
    }

    fn skill(
        id: &str,
        modes: &[&str],
        tags: &[&str],
        risk: ordo_skills::RiskLevel,
    ) -> ordo_skills::SkillManifest {
        ordo_skills::SkillManifest {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            modes: modes.iter().map(|s| s.to_string()).collect(),
            risk_level: risk,
            requires_tools: false,
            lane_label: "Installed Skills".into(),
            path: None,
        }
    }

    #[test]
    fn skill_self_declared_for_this_mode_is_allowed() {
        let m = minimal_manifest(); // id = "test"
        assert!(m.allows_skill(&skill("s", &["test"], &[], ordo_skills::RiskLevel::Medium)));
        assert!(!m.allows_skill(&skill("s", &["other"], &[], ordo_skills::RiskLevel::Medium)));
    }

    #[test]
    fn undeclared_skill_follows_mode_default() {
        let mut m = minimal_manifest();
        // permissive by default
        assert!(m.allows_skill(&skill("s", &[], &[], ordo_skills::RiskLevel::Medium)));
        // restrictive hides undeclared skills
        m.default_skill_admission = Some("restrictive".into());
        assert!(!m.allows_skill(&skill("s", &[], &[], ordo_skills::RiskLevel::Medium)));
    }

    #[test]
    fn tag_allow_broadens_admission() {
        let mut m = minimal_manifest();
        m.default_skill_admission = Some("restrictive".into());
        m.allowed_skill_tags = vec!["rust".into()];
        // undeclared but tagged rust → admitted despite restrictive default
        assert!(m.allows_skill(&skill("s", &[], &["rust"], ordo_skills::RiskLevel::Medium)));
        // a skill declared for another mode is broadened in here via its tag
        assert!(m.allows_skill(&skill(
            "s",
            &["other"],
            &["rust"],
            ordo_skills::RiskLevel::Medium
        )));
    }

    #[test]
    fn veto_overrides_everything() {
        let mut m = minimal_manifest();
        // blocked by id even though self-declared for this mode
        m.blocked_skills = vec!["danger".into()];
        assert!(!m.allows_skill(&skill(
            "danger",
            &["test"],
            &[],
            ordo_skills::RiskLevel::Low
        )));
        // blocked by tag even though self-declared + allowed by tag
        let mut m2 = minimal_manifest();
        m2.blocked_skill_tags = vec!["exploit".into()];
        m2.allowed_skill_tags = vec!["exploit".into()];
        assert!(!m2.allows_skill(&skill(
            "s",
            &["test"],
            &["exploit"],
            ordo_skills::RiskLevel::Low
        )));
    }

    #[test]
    fn risk_ceiling_vetoes_above_level() {
        let mut m = minimal_manifest();
        m.max_skill_risk = Some("low".into());
        // high-risk skill blocked even when self-declared for this mode
        assert!(!m.allows_skill(&skill("s", &["test"], &[], ordo_skills::RiskLevel::High)));
        // low-risk skill still allowed
        assert!(m.allows_skill(&skill("s", &["test"], &[], ordo_skills::RiskLevel::Low)));
    }

    #[test]
    fn invalid_skill_risk_and_admission_rejected() {
        let mut m = minimal_manifest();
        m.max_skill_risk = Some("extreme".into());
        assert!(matches!(
            m.normalize_and_validate(),
            Err(ModeManifestError::InvalidSkillRisk { .. })
        ));
        let mut m2 = minimal_manifest();
        m2.default_skill_admission = Some("loose".into());
        assert!(matches!(
            m2.normalize_and_validate(),
            Err(ModeManifestError::InvalidSkillAdmission { .. })
        ));
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
