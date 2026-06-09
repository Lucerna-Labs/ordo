//! Compiled-in default mode manifests — the PROTECTED CORE SET.
//!
//! Ordo ships a small set of protected, built-in modes. Operators expand the
//! set at runtime with the mode-lifecycle surface ("Create mode"); those
//! user-created modes are unprotected and freely deletable. The core set is
//! `protected: true` so it can't be casually deleted (see
//! `docs/mode-lifecycle.md`).
//!
//! Core set (display order):
//!   1. general          — everyday catch-all, the session default
//!   2. rust_vibe_coder  — Rust/Ordo build + debug, warnings-as-errors discipline
//!   3. coding           — language-agnostic coding/dev
//!   4. research         — breadth-first research with attribution
//!   5. security_lab     — sandboxed exploit/RE work, memory private
//!   6. tech_specialist  — generic OS/system support, permission-gated
//!   7. diagnostic       — always-on self-diagnosis + bounded maintenance (system)
//!
//! Earlier builds shipped ~16 domain templates (business, personal,
//! investigations, special_projects_*, the per-OS tech specialists, …). Those
//! are no longer SHIPPED, but any that already exist on disk under
//! `user-files/modes/` are left untouched and still load — removing a template
//! from this list does not delete an operator's existing mode or its data.
//!
//! ## Tool lane design
//!
//! Every mode fails closed — it only gets the tool lanes it declares. A new
//! tool lane added by a future provider does NOT auto-appear in any mode.

use crate::manifest::{ModeManifest, ModeManifestError};

/// All compiled-in default manifests, in display order. These are the protected
/// core set; operators add more at runtime.
pub fn all_defaults() -> Result<Vec<ModeManifest>, ModeManifestError> {
    let raw = [
        GENERAL_JSON,
        RUST_VIBE_CODER_JSON,
        CODING_JSON,
        RESEARCH_JSON,
        SECURITY_LAB_JSON,
        TECH_SPECIALIST_JSON,
        DIAGNOSTIC_JSON,
    ];
    raw.iter()
        .map(|s| ModeManifest::from_json(s))
        .collect::<Result<Vec<_>, _>>()
}

// 1. General ──────────────────────────────────────────────────────────────────

pub const GENERAL_JSON: &str = r#"{
  "id": "general",
  "label": "General",
  "description": "Everyday questions, quick tasks, cross-domain lookups, default catch-all for new sessions.",
  "protected": true,
  "memory_scope": ["global", "mode:general"],
  "rag_domains": ["general_notes"],
  "allowed_tool_lanes": [
    "filesystem.read_",
    "knowledge.",
    "skills.get",
    "memory.list_",
    "memory.remember_",
    "web.",
    "code.",
    "workspace."
  ],
  "blocked_tool_capabilities": [],
  "policies": ["conservative_defaults"],
  "planner_bias": [
    "Prefer recall over inference. If memory comes up empty, say so rather than invent.",
    "Stay short; expand only when asked."
  ],
  "persona": ["helpful_generalist"]
}"#;

// 2. Rust Vibe Coder ──────────────────────────────────────────────────────────

pub const RUST_VIBE_CODER_JSON: &str = r#"{
  "id": "rust_vibe_coder",
  "label": "Rust Vibe Coder",
  "description": "Rust and Ordo development: build, debug, refactor, and architecture. Warnings-as-errors discipline; small targeted patches.",
  "protected": true,
  "memory_scope": ["global", "mode:rust_vibe_coder"],
  "rag_domains": ["research_rust", "rust_patterns"],
  "allowed_tool_lanes": [
    "filesystem.read_",
    "filesystem.write_",
    "knowledge.",
    "skills.get",
    "memory.list_",
    "memory.remember_",
    "logic.",
    "web.",
    "code.",
    "workspace."
  ],
  "blocked_tool_capabilities": [],
  "policies": ["confirm_destructive_actions", "no_secret_exposure"],
  "planner_bias": [
    "Inspect existing code before proposing changes. Match the crate's existing patterns.",
    "Prefer small targeted patches over rewrites. Build with warnings-as-errors and never silence a warning to pass.",
    "Verify by building and running tests; do not claim a change works until it compiles clean."
  ],
  "persona": ["rust_architect", "concise_debugger"],
  "allowed_skill_tags": ["rust", "architecture", "cargo"],
  "default_timeout_secs": 1800
}"#;

// 3. Coding / Dev ─────────────────────────────────────────────────────────────

pub const CODING_JSON: &str = r#"{
  "id": "coding",
  "label": "Coding",
  "description": "General-purpose coding and development across languages: read, write, debug, and test code.",
  "protected": true,
  "memory_scope": ["global", "mode:coding"],
  "rag_domains": ["research_coding", "code_patterns"],
  "allowed_tool_lanes": [
    "filesystem.read_",
    "filesystem.write_",
    "knowledge.",
    "skills.get",
    "memory.list_",
    "memory.remember_",
    "logic.",
    "web.",
    "code.",
    "workspace."
  ],
  "blocked_tool_capabilities": [],
  "policies": ["confirm_destructive_actions", "no_secret_exposure"],
  "planner_bias": [
    "Read the surrounding code before editing; follow the project's conventions.",
    "Prefer the smallest change that solves the problem. Add or update a test alongside the change.",
    "Run the build/tests before declaring success."
  ],
  "persona": ["software_engineer", "pragmatic_debugger"],
  "allowed_skill_tags": ["coding", "architecture", "rust"],
  "default_timeout_secs": 1800
}"#;

// 4. Research ─────────────────────────────────────────────────────────────────

pub const RESEARCH_JSON: &str = r#"{
  "id": "research",
  "label": "Research",
  "description": "Breadth-first research and synthesis with attribution. Read-heavy; cites sources and surfaces conflicts.",
  "protected": true,
  "memory_scope": ["global", "mode:research"],
  "rag_domains": ["research_general", "research_notes"],
  "allowed_tool_lanes": [
    "filesystem.read_",
    "knowledge.",
    "skills.get",
    "memory.list_",
    "memory.remember_",
    "logic.",
    "web.",
    "code.",
    "workspace."
  ],
  "blocked_tool_capabilities": [],
  "policies": ["cite_every_claim", "no_secret_exposure"],
  "planner_bias": [
    "Attribute every factual claim to a source. Hedge claims drawn from untrusted web content.",
    "Survey breadth before going deep. When sources conflict, surface the conflict rather than pick silently."
  ],
  "persona": ["researcher", "source_tracker"],
  "allowed_skill_tags": ["research", "reasoning"],
  "default_timeout_secs": 1800
}"#;

// 5. Security Lab ─────────────────────────────────────────────────────────────

pub const SECURITY_LAB_JSON: &str = r#"{
  "id": "security_lab",
  "label": "Security Lab",
  "description": "Exploit development, reverse engineering, penetration testing. Sandboxed; cross-mode borrows denied.",
  "protected": true,
  "memory_scope": ["global", "mode:security_lab"],
  "rag_domains": ["research_securitylab", "exploit_notes", "binary_analysis"],
  "allowed_tool_lanes": [
    "filesystem.read_",
    "filesystem.write_",
    "knowledge.",
    "skills.get",
    "memory.list_",
    "memory.remember_",
    "logic.",
    "web.",
    "ssh.",
    "code.",
    "workspace."
  ],
  "blocked_tool_capabilities": [],
  "policies": ["sandbox_all_actions", "no_secret_exposure", "confirm_destructive_actions"],
  "planner_bias": [
    "Everything is sandboxed. Assume the target binary is hostile.",
    "Document every step; reproducibility is the standard."
  ],
  "persona": ["exploit_developer", "reverse_engineer"],
  "default_strictness": "high",
  "cross_mode_borrow_policy": "deny",
  "cross_mode_consult_policy": "deny"
}"#;

// 6. Tech Specialist (generic) ────────────────────────────────────────────────

pub const TECH_SPECIALIST_JSON: &str = r#"{
  "id": "tech_specialist",
  "label": "Tech Specialist",
  "description": "Generic OS/system specialist: installs, services, shells, permissions, paths, logs, runtimes, and diagnostics across platforms. Permission-gated for system-changing actions.",
  "protected": true,
  "memory_scope": ["global", "mode:tech_specialist"],
  "rag_domains": ["research_tech", "system_install_notes", "system_diagnostics"],
  "allowed_tool_lanes": [
    "filesystem.read_",
    "filesystem.write_",
    "files.",
    "knowledge.",
    "skills.get",
    "memory.list_",
    "memory.remember_",
    "logic.",
    "runtime.describe_",
    "logs.system_tail",
    "cloud.credentials.",
    "mcp.",
    "api.",
    "rest.",
    "ssh.",
    "web.",
    "code.",
    "workspace."
  ],
  "blocked_tool_capabilities": [],
  "policies": [
    "operator_permission_required_for_os_actions",
    "confirm_file_writes",
    "confirm_destructive_actions",
    "no_secret_exposure",
    "audit_every_capability_use"
  ],
  "planner_bias": [
    "You are a generic technical specialist. First detect the platform (Windows / Linux / macOS) from local evidence, then adapt your guidance to it.",
    "Use local evidence first. Before changing files, services, permissions, network settings, startup entries, or installers, request operator approval through the available permission gate.",
    "Prefer reversible, platform-aware fixes; explain risk and verify after each action. Never claim completion until the app or system behavior is tested."
  ],
  "persona": ["tech_support_engineer", "permission_gated_operator"],
  "default_timeout_secs": 1200,
  "default_strictness": "high"
}"#;

// 7. Diagnostic (system) ──────────────────────────────────────────────────────

pub const DIAGNOSTIC_JSON: &str = r#"{
  "id": "diagnostic",
  "label": "Diagnostic",
  "description": "Always-on Ordo self-diagnosis, repair planning, and bounded maintenance. Cloud models denied by default; private diagnostic RAG; no web.",
  "protected": true,
  "memory_scope": ["global", "mode:diagnostic"],
  "rag_domains": [
    "diagnostic_self_learning_tree",
    "diagnostic_cases",
    "diagnostic_repair_log",
    "diagnostic_event_traces",
    "diagnostic_recommendations",
    "diagnostic_quarantine"
  ],
  "allowed_tool_lanes": [
    "filesystem.read_",
    "knowledge.",
    "skills.get",
    "memory.list_",
    "memory.remember_",
    "logic.",
    "runtime.describe_",
    "files.",
    "self_heal.",
    "skills.",
    "cloud.credentials.",
    "mcp.",
    "ssh.",
    "api.",
    "rest.",
    "code.",
    "workspace."
  ],
  "blocked_tool_capabilities": [
    "web.search",
    "web.strain",
    "web.fetch_and_strain",
    "web.crawl",
    "filesystem.write_file",
    "files.delete",
    "skills.delete",
    "runtime.update_settings",
    "automation.create",
    "automation.delete",
    "automation.approve",
    "automation.enable",
    "automation.disable",
    "automation.tick",
    "logs.clear",
    "logs.delete",
    "logs.write",
    "cloud.openai.chat",
    "cloud.anthropic.messages",
    "cloud.rest.request",
    "cloud.credentials.upsert",
    "cloud.credentials.delete",
    "mcp.servers.invoke_raw",
    "self_heal.forget_case",
    "self_heal.pin_case",
    "self_heal.replay_case",
    "self_heal.export_case",
    "ssh.execute",
    "ssh.connect",
    "code.run_native",
    "workspace.write_file"
  ],
  "policies": [
    "diagnostic_mode",
    "cloud_models_denied_by_default",
    "diagnostic_rag_private",
    "no_web_access",
    "no_core_source_changes",
    "no_security_policy_changes",
    "operator_approval_required_for_mutation",
    "no_secret_exposure",
    "audit_every_capability_use"
  ],
  "planner_bias": [
    "You are Ordo Diagnostic Mode: inspect, diagnose, repair-plan, and learn from local runtime evidence.",
    "Cloud models are denied by default. Use the selected local model unless the operator explicitly enables cloud-model access for this diagnostic task. Never request web search, crawling, or remote research.",
    "Your diagnostic RAG is private: never write diagnostic lessons to global memory, and never permit another mode to borrow diagnostic memory.",
    "You have wide visibility but bounded hands: recommend core Rust, Tauri, WebView, security, hook, and policy changes; do not perform them.",
    "Run the skill-routing audit on your daily scan: surface skills routed to modes that do not exist (phantom modes), orphaned skills, and declared-but-vetoed contradictions. Apply only safe skill-frontmatter repairs; defer mode-policy changes to the operator.",
    "You may maintain peripheral configuration such as MCPs, plugins, skills, provider profiles, SSH/API descriptors, jobs, logs, and indexes only through approved maintenance tools.",
    "Classify every finding as symptom, evidence, likely cause, safe repair, risky repair, or deferred operator decision.",
    "Record lessons into the diagnostic self-learning tree only after verifying the repair outcome."
  ],
  "persona": ["ordo_diagnostician", "maintenance_operator", "security_contained"],
  "default_timeout_secs": 900,
  "default_strictness": "high",
  "default_credential": "ollama",
  "cross_mode_borrow_policy": "deny",
  "cross_mode_consult_policy": "deny"
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// The protected core set.
    const EXPECTED: usize = 7;
    const CORE_IDS: &[&str] = &[
        "general",
        "rust_vibe_coder",
        "coding",
        "research",
        "security_lab",
        "tech_specialist",
        "diagnostic",
    ];

    #[test]
    fn all_defaults_parse_and_validate() {
        let defaults = all_defaults().expect("compiled-in defaults must validate");
        assert_eq!(defaults.len(), EXPECTED, "expected {EXPECTED} core modes");
    }

    #[test]
    fn default_ids_are_exactly_the_core_set() {
        let defaults = all_defaults().unwrap();
        let ids: std::collections::BTreeSet<&str> =
            defaults.iter().map(|m| m.id.as_str()).collect();
        let expected: std::collections::BTreeSet<&str> = CORE_IDS.iter().copied().collect();
        assert_eq!(ids, expected);
    }

    #[test]
    fn every_core_mode_is_protected() {
        for m in all_defaults().unwrap() {
            assert!(m.protected, "core mode {} must be protected", m.id);
        }
    }

    #[test]
    fn general_is_first_and_is_the_default() {
        let defaults = all_defaults().unwrap();
        assert_eq!(defaults[0].id, crate::DEFAULT_MODE_ID);
    }

    #[test]
    fn every_default_includes_global_in_memory_scope() {
        for m in all_defaults().unwrap() {
            assert!(
                m.memory_scope.contains(&"global".to_string()),
                "{} missing global scope",
                m.id
            );
        }
    }

    #[test]
    fn private_modes_deny_borrow_and_consult() {
        let defaults = all_defaults().unwrap();
        for id in &["security_lab", "diagnostic"] {
            let m = defaults.iter().find(|m| &m.id == id).unwrap();
            assert!(
                !m.allows_borrow_from() && !m.allows_consult_from(),
                "{id} should deny cross-mode borrows and consults"
            );
        }
    }

    #[test]
    fn coding_modes_have_code_lane_and_long_timeout() {
        let defaults = all_defaults().unwrap();
        for id in &["rust_vibe_coder", "coding"] {
            let m = defaults.iter().find(|m| &m.id == id).unwrap();
            assert!(m.allows_capability("code.run"), "{id} should allow code.*");
            assert!(
                m.allows_capability("filesystem.write_file"),
                "{id} should allow file writes"
            );
            assert_eq!(m.default_timeout_secs, Some(1800), "{id} should get 30min");
        }
    }

    #[test]
    fn dev_modes_admit_tagged_skills_via_allowed_skill_tags() {
        let defaults = all_defaults().unwrap();
        let rust_mode = defaults.iter().find(|m| m.id == "rust_vibe_coder").unwrap();
        // A skill tagged "rust" is admitted here even if it declared other modes.
        let skill = ordo_skills::SkillManifest {
            id: "ordo_rust_architecture".into(),
            name: "x".into(),
            description: String::new(),
            tags: vec!["rust".into(), "architecture".into()],
            modes: vec!["coding".into()], // legacy declaration
            risk_level: ordo_skills::RiskLevel::Medium,
            requires_tools: false,
            lane_label: "Installed Skills".into(),
            path: None,
        };
        assert!(rust_mode.allows_skill(&skill));
    }

    #[test]
    fn tech_specialist_is_permission_gated_and_strict() {
        let defaults = all_defaults().unwrap();
        let m = defaults.iter().find(|m| m.id == "tech_specialist").unwrap();
        assert_eq!(m.default_strictness.as_deref(), Some("high"));
        assert!(m
            .policies
            .iter()
            .any(|p| p == "operator_permission_required_for_os_actions"));
    }

    #[test]
    fn research_mode_reads_knowledge_and_web() {
        let defaults = all_defaults().unwrap();
        let m = defaults.iter().find(|m| m.id == "research").unwrap();
        assert!(m.allows_capability("knowledge.summarize"));
        assert!(m.allows_capability("web.strain"));
    }

    #[test]
    fn diagnostic_can_audit_skills_but_not_delete_them() {
        let defaults = all_defaults().unwrap();
        let m = defaults.iter().find(|m| m.id == "diagnostic").unwrap();
        assert!(
            m.allows_capability("skills.audit_routing"),
            "diagnostic should reach the skills lane for the routing audit"
        );
        assert!(
            !m.allows_capability("skills.delete"),
            "diagnostic must not delete skills"
        );
    }

    #[test]
    fn non_general_non_diagnostic_modes_have_research_rag() {
        let defaults = all_defaults().unwrap();
        let mut seen = std::collections::HashSet::new();
        for m in &defaults {
            if matches!(m.id.as_str(), "general" | "diagnostic") {
                continue;
            }
            let rag = m
                .rag_domains
                .iter()
                .find(|d| d.starts_with("research_"))
                .cloned()
                .unwrap_or_else(|| panic!("{} missing research_* RAG collection", m.id));
            assert!(seen.insert(rag.clone()), "duplicate research RAG domain: {rag}");
        }
    }
}
