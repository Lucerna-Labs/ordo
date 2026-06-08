//! Skill-routing audit.
//!
//! Pure logic over the discovered skills (`ordo_skills::SkillManifest`) and all
//! loaded mode manifests: for every (skill, mode) pair it computes
//! [`ModeManifest::skill_verdict`] and classifies routing health. This is the
//! reusable heart of the diagnostic mode's daily routing check (see
//! `docs/skill-routing.md` — "Diagnostics: daily routing audit"). It is
//! READ-ONLY — it observes routing, it never changes a skill or a mode.

use serde::{Deserialize, Serialize};

use ordo_skills::SkillManifest;

use crate::manifest::ModeManifest;

/// A routing anomaly for one skill. Whether each is auto-repairable is a
/// property of the *fix surface* (skill-side frontmatter is repairable;
/// mode-side policy is operator-deferred), documented in `docs/skill-routing.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RoutingAnomaly {
    /// Admitted by no mode at all — a dead skill. (skill-side repairable: give
    /// it `available_to_modes` or relax; or operator broadens a mode)
    Orphaned,
    /// The skill self-declares this mode, but the mode vetoes it — a
    /// contradiction the skill author cannot have intended. (mode-side: deferred)
    DeclaredButVetoed { mode: String, reason: String },
    /// `available_to_modes` names a mode id that does not exist (typo).
    /// (skill-side repairable: fix or drop the bad entry)
    PhantomMode { declared: String },
    /// No `available_to_modes` and no tags — the skill relies entirely on each
    /// mode's default admission. Informational, not an error.
    Undeclared,
}

/// One skill's routing health across all modes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRoutingHealth {
    pub skill_id: String,
    /// `available_to_modes` as declared in the skill's frontmatter.
    pub declared_modes: Vec<String>,
    /// Mode ids that actually surface this skill (`allows_skill == true`).
    pub admitting_modes: Vec<String>,
    /// Anomalies found; empty means healthy.
    pub anomalies: Vec<RoutingAnomaly>,
}

impl SkillRoutingHealth {
    pub fn is_healthy(&self) -> bool {
        self.anomalies.is_empty()
    }

    /// Anomalies a skill-side frontmatter repair can address without touching
    /// any mode policy (phantom-mode typos; orphans needing a declaration).
    pub fn has_skill_side_repair(&self) -> bool {
        self.anomalies.iter().any(|a| {
            matches!(
                a,
                RoutingAnomaly::PhantomMode { .. } | RoutingAnomaly::Orphaned
            )
        })
    }
}

/// The full audit over all skills + modes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingAudit {
    pub skills: Vec<SkillRoutingHealth>,
}

impl RoutingAudit {
    pub fn anomaly_count(&self) -> usize {
        self.skills.iter().map(|s| s.anomalies.len()).sum()
    }

    /// Skills admitted by no mode.
    pub fn orphaned(&self) -> Vec<&str> {
        self.skills
            .iter()
            .filter(|s| s.anomalies.iter().any(|a| matches!(a, RoutingAnomaly::Orphaned)))
            .map(|s| s.skill_id.as_str())
            .collect()
    }

    /// Skills with at least one anomaly (excluding the purely-informational
    /// `Undeclared`).
    pub fn unhealthy(&self) -> Vec<&SkillRoutingHealth> {
        self.skills
            .iter()
            .filter(|s| {
                s.anomalies
                    .iter()
                    .any(|a| !matches!(a, RoutingAnomaly::Undeclared))
            })
            .collect()
    }
}

/// Audit how every skill routes across every mode. Read-only.
pub fn audit_skill_routing(modes: &[ModeManifest], skills: &[SkillManifest]) -> RoutingAudit {
    let known: std::collections::BTreeSet<&str> = modes.iter().map(|m| m.id.as_str()).collect();

    let mut out = Vec::with_capacity(skills.len());
    for skill in skills {
        let mut admitting = Vec::new();
        let mut anomalies = Vec::new();

        // Phantom modes: a declared mode that does not exist.
        for declared in &skill.modes {
            if !known.contains(declared.as_str()) {
                anomalies.push(RoutingAnomaly::PhantomMode {
                    declared: declared.clone(),
                });
            }
        }

        // Per-mode verdicts.
        for mode in modes {
            let verdict = mode.skill_verdict(skill);
            if verdict.admitted() {
                admitting.push(mode.id.clone());
            } else if verdict.is_veto() && skill.modes.iter().any(|m| m == &mode.id) {
                // The skill asked for this mode, but the mode vetoes it.
                anomalies.push(RoutingAnomaly::DeclaredButVetoed {
                    mode: mode.id.clone(),
                    reason: verdict.reason(),
                });
            }
        }

        if admitting.is_empty() {
            anomalies.push(RoutingAnomaly::Orphaned);
        }
        if skill.modes.is_empty() && skill.tags.is_empty() {
            anomalies.push(RoutingAnomaly::Undeclared);
        }

        out.push(SkillRoutingHealth {
            skill_id: skill.id.clone(),
            declared_modes: skill.modes.clone(),
            admitting_modes: admitting,
            anomalies,
        });
    }

    RoutingAudit { skills: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_skills::RiskLevel;

    fn mode(id: &str) -> ModeManifest {
        // Build via JSON so defaults apply; permissive (no skill veto/allow).
        ModeManifest::from_json(&format!(
            r#"{{"id":"{id}","label":"{id}","description":"d","memory_scope":["global"]}}"#
        ))
        .expect("valid mode")
    }

    fn skill(id: &str, modes: &[&str], tags: &[&str], risk: RiskLevel) -> SkillManifest {
        SkillManifest {
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
    fn declared_skill_routes_cleanly() {
        let modes = vec![mode("coding"), mode("research")];
        let skills = vec![skill("s", &["coding"], &[], RiskLevel::Medium)];
        let audit = audit_skill_routing(&modes, &skills);
        let h = &audit.skills[0];
        assert_eq!(h.admitting_modes, vec!["coding"]); // research permissive → also admits? no: declared coding only
        assert!(h.is_healthy(), "anomalies: {:?}", h.anomalies);
    }

    #[test]
    fn permissive_modes_admit_undeclared_but_flag_undeclared() {
        let modes = vec![mode("coding"), mode("research")];
        let skills = vec![skill("s", &[], &[], RiskLevel::Medium)];
        let audit = audit_skill_routing(&modes, &skills);
        let h = &audit.skills[0];
        // permissive default → admitted everywhere, but flagged Undeclared (info)
        assert_eq!(h.admitting_modes.len(), 2);
        assert!(h.anomalies.iter().any(|a| matches!(a, RoutingAnomaly::Undeclared)));
        assert!(audit.orphaned().is_empty());
    }

    #[test]
    fn phantom_mode_is_flagged() {
        let modes = vec![mode("coding")];
        let skills = vec![skill("s", &["codng"], &[], RiskLevel::Medium)]; // typo
        let audit = audit_skill_routing(&modes, &skills);
        let h = &audit.skills[0];
        assert!(h
            .anomalies
            .iter()
            .any(|a| matches!(a, RoutingAnomaly::PhantomMode { declared } if declared == "codng")));
        // typo'd → admitted by no real mode → also orphaned
        assert!(audit.orphaned().contains(&"s"));
    }

    #[test]
    fn declared_but_vetoed_is_a_contradiction() {
        let mut coding = mode("coding");
        coding.max_skill_risk = Some("low".into());
        let modes = vec![coding];
        // skill declares coding but is high-risk → coding vetoes by risk
        let skills = vec![skill("s", &["coding"], &[], RiskLevel::High)];
        let audit = audit_skill_routing(&modes, &skills);
        let h = &audit.skills[0];
        assert!(h.admitting_modes.is_empty());
        assert!(h.anomalies.iter().any(|a| matches!(
            a,
            RoutingAnomaly::DeclaredButVetoed { mode, .. } if mode == "coding"
        )));
        assert!(audit.orphaned().contains(&"s"));
    }

    #[test]
    fn restrictive_mode_orphans_an_undeclared_skill() {
        let mut locked = mode("secure");
        locked.default_skill_admission = Some("restrictive".into());
        let modes = vec![locked];
        let skills = vec![skill("s", &[], &[], RiskLevel::Low)];
        let audit = audit_skill_routing(&modes, &skills);
        assert!(audit.orphaned().contains(&"s"));
    }
}
