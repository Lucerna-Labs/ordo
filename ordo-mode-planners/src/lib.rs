//! Per-mode planning strategies.
//!
//! Each mode gets its own planning engine so the reasoning is specialized
//! rather than generic. A Vibe Coding session doesn't waste tokens weighing
//! language options — it knows Rust is the answer. A Research session
//! doesn't debate whether to cite — it always does. Security stays wide
//! open while General falls back to the keyword-based fallback.
//!
//! ## Architecture
//!
//! - `ModePlanner` — the trait every planner implements. Async, takes a
//!   goal + RAG context + available capabilities, returns a structured
//!   execution plan.
//! - `ModePlannerRegistry` — resolves a mode id to its planner. The
//!   registry is lazy: planners are created on first lookup, not all
//!   at boot.
//! - Integration — `AssistantService` calls
//!   `registry.plan(mode_id, goal, context, capabilities)` instead of
//!   the old `RouterPlanner` directly. When no registry is present
//!   (legacy / tests), the assistant falls back to `GeneralPlanner`
//!   which wraps the existing keyword matcher.

use async_trait::async_trait;
use ordo_modes::ModeManifest;
use ordo_protocol::{ExecutionPlan, RagHit};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Errors the planner can surface. Deliberately shallow — a planner
/// that fails is unusual (the system is supposed to always have
/// SOMETHING to propose), but we don't want an unreachable to panic
/// the turn loop.
#[derive(Debug, thiserror::Error)]
pub enum PlannerError {
    #[error("unknown mode '{0}'")]
    UnknownMode(String),
    #[error("no default credential available")]
    NoDefaultCredential,
    #[error("missing capability '{0}'")]
    MissingCapability(String),
    #[error("invalid goal: {0}")]
    InvalidGoal(String),
    #[error("planner not initialized")]
    NotInitialized,
}

/// A mode-aware planning result. Structured like the existing
/// `ExecutionPlan` but with additional fields the per-mode planners
/// may fill in.
#[derive(Debug, Clone)]
pub struct ModePlan {
    pub plan_id: Uuid,
    pub goal: String,
    pub mode_id: String,
    pub steps: Vec<PlanStep>,
    /// Human explanation of WHY this plan shape was chosen.
    pub rationale: String,
}

/// One step in a mode plan — capability + arguments + context.
#[derive(Debug, Clone)]
pub struct PlanStep {
    /// The capability to invoke.
    pub capability: String,
    /// Human-readable label for tool call logging.
    pub name: String,
    /// Arguments to pass to the capability.
    pub arguments: Value,
}

impl From<ExecutionPlan> for ModePlan {
    fn from(plan: ExecutionPlan) -> Self {
        Self {
            plan_id: plan.plan_id,
            goal: plan.goal.clone(),
            mode_id: "general".to_string(),
            steps: plan
                .steps
                .into_iter()
                .map(|step| PlanStep {
                    capability: step.capability,
                    name: step.name,
                    arguments: step.arguments,
                })
                .collect(),
            rationale: "Keyword-based fallback plan".to_string(),
        }
    }
}

/// The per-mode planning interface.
///
/// Every implementation receives the same inputs — goal, RAG context,
/// and available capabilities — and produces a structured plan. The
/// differentiation is in HOW they arrive at that plan.
#[async_trait]
pub trait ModePlanner: Send + Sync {
    /// Produce a plan for the given goal. Receives the mode manifest
    /// so the planner can read `planner_bias` and `policies` to shape
    /// its output.
    async fn plan(
        &self,
        goal: &str,
        context: &[RagHit],
        capabilities: &[String],
        manifest: &ModeManifest,
    ) -> Result<ModePlan, PlannerError>;
}

/// Fallback planner that delegates to the existing keyword matcher
/// in `ordo_protocol`. Used when no mode-specific planner is
/// registered. This preserves the current production behavior for
/// general-purpose assistant mode.
#[derive(Clone)]
pub struct GeneralPlanner;

#[async_trait]
impl ModePlanner for GeneralPlanner {
    async fn plan(
        &self,
        goal: &str,
        context: &[RagHit],
        _capabilities: &[String],
        _manifest: &ModeManifest,
    ) -> Result<ModePlan, PlannerError> {
        let plan = ordo_protocol::infer_rag_collections(goal);
        let mut steps = Vec::new();
        for capability_name in &plan {
            steps.push(PlanStep {
                capability: capability_name.clone(),
                name: capability_name.clone(),
                arguments: Value::Null,
            });
        }
        Ok(ModePlan {
            plan_id: Uuid::new_v4(),
            goal: goal.to_string(),
            mode_id: "general".to_string(),
            steps,
            rationale: "Keyword-based fallback plan".to_string(),
        })
    }
}

/// Vibe Coding planner — Rust-first, ordo-architecture-aware.
///
/// Core rules:
/// - Prefer pure Rust — only fall back to Python, Shell, or JavaScript
///   when Rust genuinely can't do it
/// - Ordo crate architecture: bus-first, tokio, loosely coupled
/// - Has filesystem read/write, GitHub API, and crates.io access
/// - Standardized patterns are baked in, not discovered at runtime
#[derive(Clone)]
pub struct VibeCodingPlanner;

#[async_trait]
impl ModePlanner for VibeCodingPlanner {
    async fn plan(
        &self,
        goal: &str,
        _context: &[RagHit],
        capabilities: &[String],
        _manifest: &ModeManifest,
    ) -> Result<ModePlan, PlannerError> {
        let lowered = goal.to_ascii_lowercase();

        // Identify the coding domain from the goal.
        let domain = classify_coding_domain(&lowered);

        // Build steps based on what the user wants.
        let mut steps = Vec::new();

        match domain {
            CodingDomain::Read => {
                // Reading code or docs — read the file, then read docs.
                steps.push(PlanStep {
                    capability: "filesystem.read_file".to_string(),
                    name: "Read source file".to_string(),
                    arguments: serde_json::json!({ "path": extract_path_hint(&lowered).unwrap_or(".".to_string()) }),
                });
                steps.push(PlanStep {
                    capability: "knowledge.lookup".to_string(),
                    name: "Look up relevant docs".to_string(),
                    arguments: serde_json::json!({ "query": goal }),
                });
            }
            CodingDomain::Write => {
                // Writing — check deps first, then write.
                steps.push(PlanStep {
                    capability: "filesystem.read_file".to_string(),
                    name: "Read existing code at target".to_string(),
                    arguments: serde_json::json!({ "path": extract_path_hint(&lowered).unwrap_or("src/main.rs".to_string()) }),
                });
                steps.push(PlanStep {
                    capability: "knowledge.lookup".to_string(),
                    name: "Check ordo crate docs for relevant traits".to_string(),
                    arguments: serde_json::json!({ "query": format!("ordo architecture pattern for {}", goal) }),
                });
                steps.push(PlanStep {
                    capability: "filesystem.write_file".to_string(),
                    name: "Write the implementation".to_string(),
                    arguments: serde_json::json!({ "path": extract_write_target(&lowered) }),
                });
            }
            CodingDomain::Refactor => {
                // Refactoring — read, analyze, write.
                steps.push(PlanStep {
                    capability: "filesystem.read_file".to_string(),
                    name: "Read the target file".to_string(),
                    arguments: serde_json::json!({ "path": extract_path_hint(&lowered).unwrap_or(".".to_string()) }),
                });
                steps.push(PlanStep {
                    capability: "knowledge.lookup".to_string(),
                    name: "Check ordo crate structure".to_string(),
                    arguments: serde_json::json!({ "query": "ordo crate architecture standards" }),
                });
                steps.push(PlanStep {
                    capability: "filesystem.write_file".to_string(),
                    name: "Apply the refactor".to_string(),
                    arguments: serde_json::json!({ "path": extract_write_target(&lowered) }),
                });
            }
            CodingDomain::Docs => {
                // Documentation — just need to read and write markdown.
                steps.push(PlanStep {
                    capability: "filesystem.write_file".to_string(),
                    name: "Generate documentation".to_string(),
                    arguments: serde_json::json!({ "path": format!("{}.md", extract_write_target(&lowered)) }),
                });
            }
            CodingDomain::Dependency => {
                // Dependency management — check crates.io + GitHub.
                steps.push(PlanStep {
                    capability: "web.fetch_and_strain".to_string(),
                    name: "Check crates.io for available crates".to_string(),
                    arguments: serde_json::json!({ "query": extract_package_hint(&lowered) }),
                });
                steps.push(PlanStep {
                    capability: "web.fetch_and_strain".to_string(),
                    name: "Check GitHub for repository".to_string(),
                    arguments: serde_json::json!({ "query": extract_package_hint(&lowered) }),
                });
                steps.push(PlanStep {
                    capability: "filesystem.write_file".to_string(),
                    name: "Update Cargo.toml".to_string(),
                    arguments: serde_json::json!({ "path": "Cargo.toml" }),
                });
            }
            CodingDomain::Test => {
                // Testing — find test files, then run.
                steps.push(PlanStep {
                    capability: "logic.test".to_string(),
                    name: "Run test suite".to_string(),
                    arguments: serde_json::json!({ "target": extract_test_target(&lowered) }),
                });
            }
        }

        // Verify all requested capabilities are available.
        for step in &steps {
            if !capabilities.iter().any(|c| step.capability.starts_with(c.as_str())) {
                return Err(PlannerError::MissingCapability(step.capability.clone()));
            }
        }

        Ok(ModePlan {
            plan_id: Uuid::new_v4(),
            goal: goal.to_string(),
            mode_id: "vibe_coding".to_string(),
            steps,
            rationale: "Rust-first, ordo-architecture-aware plan".to_string(),
        })
    }
}

#[derive(Debug, PartialEq)]
enum CodingDomain {
    Read,
    Write,
    Refactor,
    Docs,
    Dependency,
    Test,
}

fn classify_coding_domain(goal: &str) -> CodingDomain {
    let lowered = goal.trim().to_ascii_lowercase();

    if lowered.contains("test") || lowered.contains("tests") || lowered.contains("validate") {
        return CodingDomain::Test;
    }
    if lowered.contains("doc") || lowered.contains("readme") || lowered.contains("comment") {
        return CodingDomain::Docs;
    }
    if lowered.contains("refactor") || lowered.contains("restructure") || lowered.contains("clean up") {
        return CodingDomain::Refactor;
    }
    if lowered.contains("dependency") || lowered.contains("crate") || lowered.contains("package") || lowered.contains("library") {
        return CodingDomain::Dependency;
    }
    if lowered.contains("write") || lowered.contains("create") || lowered.contains("add") || lowered.contains("implement") || lowered.contains("build") || lowered.contains("generate") {
        return CodingDomain::Write;
    }
    // Default to read — inspect before changing.
    CodingDomain::Read
}

fn extract_path_hint(goal: &str) -> Option<String> {
    // Extract a quoted path or the last word as a path hint.
    let quotes: Vec<&str> = goal
        .split('"')
        .skip(1)
        .step_by(2)
        .collect();
    if let Some(path) = quotes.first() {
        return Some(path.to_string());
    }
    goal.split_whitespace()
        .filter(|w| w.contains('.') || w.contains('/') || w.contains('\\'))
        .last()
        .map(str::to_string)
}

fn extract_write_target(goal: &str) -> String {
    extract_path_hint(goal).unwrap_or_else(|| "out.txt".to_string())
}

fn extract_package_hint(goal: &str) -> String {
    goal.split_whitespace()
        .filter(|w| w.chars().any(|c| c == '_' || c == '-') || w.contains("rs") || w.contains("io"))
        .last()
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string())
}

fn extract_test_target(goal: &str) -> String {
    extract_path_hint(goal).unwrap_or_else(|| ".".to_string())
}

/// Research planner — citation-grounded synthesis.
///
/// Core rules:
/// - Every factual claim must cite a source
/// - High-stakes domains (law, medicine, finance) require authoritative
///   sources only
/// - When sources conflict, surface the conflict rather than silently
///   picking a winner
/// - Prefer primary sources over secondary
#[derive(Clone)]
pub struct ResearchPlanner;

#[async_trait]
impl ModePlanner for ResearchPlanner {
    async fn plan(
        &self,
        goal: &str,
        context: &[RagHit],
        capabilities: &[String],
        _manifest: &ModeManifest,
    ) -> Result<ModePlan, PlannerError> {
        let lowered = goal.trim().to_ascii_lowercase();

        let mut steps = Vec::new();

        // Step 1: Search multiple collections for primary sources.
        let has_web = capabilities.iter().any(|c| c == "web.strain" || c == "web.");
        if has_web {
            steps.push(PlanStep {
                capability: "web.strain".to_string(),
                name: "Search the web for primary sources".to_string(),
                arguments: serde_json::json!({ "query": goal }),
            });
        }

        // Step 2: Search RAG (always — core grounding step).
        steps.push(PlanStep {
            capability: "knowledge.lookup".to_string(),
            name: "Search self-knowledge for relevant facts".to_string(),
            arguments: serde_json::json!({
                "query": goal,
                "domain": "research_notes",
                "top_k": 8,
            }),
        });
        steps.push(PlanStep {
            capability: "knowledge.lookup".to_string(),
            name: "Search source archives for citations".to_string(),
            arguments: serde_json::json!({
                "query": goal,
                "domain": "source_archives",
                "top_k": 8,
            }),
        });

        // Step 3: If the topic is high-stakes, add verification.
        if is_high_stakes(&lowered) {
            steps.push(PlanStep {
                capability: "logic.verify".to_string(),
                name: "Cross-reference claims across sources".to_string(),
                arguments: serde_json::json!({
                    "goal": goal,
                    "context": context.iter().map(|h| h.snippet.clone()).collect::<Vec<_>>(),
                }),
            });
        }

        // Step 4: Synthesize — the LLM will produce the final answer
        // with citations embedded.
        steps.push(PlanStep {
            capability: "knowledge.synthesize".to_string(),
            name: "Synthesize findings with citations".to_string(),
            arguments: serde_json::json!({
                "goal": goal,
                "cite_every_claim": true,
                "prefer_primary": true,
            }),
        });

        for step in &steps {
            if !capabilities.iter().any(|c| step.capability.starts_with(c.as_str())) {
                return Err(PlannerError::MissingCapability(step.capability.clone()));
            }
        }

        Ok(ModePlan {
            plan_id: Uuid::new_v4(),
            goal: goal.to_string(),
            mode_id: "research".to_string(),
            steps,
            rationale: "Citation-grounded research plan with source verification".to_string(),
        })
    }
}

fn is_high_stakes(goal: &str) -> bool {
    let keywords = [
        "law", "legal", "medical", "medicine", "health", "diagnosis",
        "financial", "finance", "investment", "tax", "compliance",
        "safety", "security", "vulnerability",
    ];
    keywords.iter().any(|k| goal.contains(k))
}

/// Security planner — wide open, full audit trail.
///
/// Core rules:
/// - No restrictions beyond what the mode manifest's allowlist says
/// - Audit everything — every tool call is logged
/// - Default-suspicious: flag anomalies without blocking
/// - Cross-mode borrowing denied (handled by the manifest's
///   `cross_mode_borrow_policy`)
#[derive(Clone)]
pub struct SecurityPlanner;

#[async_trait]
impl ModePlanner for SecurityPlanner {
    async fn plan(
        &self,
        goal: &str,
        _context: &[RagHit],
        capabilities: &[String],
        _manifest: &ModeManifest,
    ) -> Result<ModePlan, PlannerError> {
        // Security mode is intentionally wide open. The manifest's
        // allowlist + blocklist is the sole gate. The planner just
        // provides a standard audit-and-evaluate plan.
        let steps = vec![
            PlanStep {
                capability: "knowledge.lookup".to_string(),
                name: "Audit relevant threat models".to_string(),
                arguments: serde_json::json!({ "query": goal, "domain": "threat_models" }),
            },
            PlanStep {
                capability: "knowledge.lookup".to_string(),
                name: "Check security research archives".to_string(),
                arguments: serde_json::json!({ "query": goal, "domain": "security_research" }),
            },
            PlanStep {
                capability: "logic.audit".to_string(),
                name: "Run audit pass on findings".to_string(),
                arguments: serde_json::json!({ "goal": goal }),
            },
        ];

        for step in &steps {
            if !capabilities.iter().any(|c| step.capability.starts_with(c.as_str())) {
                return Err(PlannerError::MissingCapability(step.capability.clone()));
            }
        }

        Ok(ModePlan {
            plan_id: Uuid::new_v4(),
            goal: goal.to_string(),
            mode_id: "security".to_string(),
            steps,
            rationale: "Security audit plan — full capability surface, default-audit".to_string(),
        })
    }
}

/// Registry of mode planners. Lazy — planners are instantiated on
/// first lookup, not all at boot. Cloning is cheap (no state).
#[derive(Clone, Default)]
pub struct ModePlannerRegistry {
    planners: HashMap<String, Arc<dyn ModePlanner>>,
}

impl ModePlannerRegistry {
    /// Create an empty registry. Planners are added with `with_planner`.
    pub fn new() -> Self {
        Self {
            planners: HashMap::new(),
        }
    }

    /// Create the default registry with all four planners pre-
    /// registered. This is what the runtime uses at startup.
    pub fn default_registry() -> Self {
        let mut registry = Self::new();
        registry.add("general", Arc::new(GeneralPlanner));
        registry.add("vibe_coding", Arc::new(VibeCodingPlanner));
        registry.add("research", Arc::new(ResearchPlanner));
        registry.add("security", Arc::new(SecurityPlanner));
        registry
    }

    /// Register a planner for a mode.
    pub fn add(&mut self, mode_id: impl Into<String>, planner: Arc<dyn ModePlanner>) {
        self.planners.insert(mode_id.into(), planner);
    }

    /// Get the planner for a mode. Returns None if no planner is
    /// registered. The caller should fall back to GeneralPlanner
    /// when that happens.
    pub fn get(&self, mode_id: &str) -> Option<Arc<dyn ModePlanner>> {
        self.planners.get(mode_id).cloned()
    }

    /// Plan a turn. Resolves the mode's planner and invokes it.
    /// Returns `PlannerError::UnknownMode` when the mode has no
    /// registered planner — callers should catch this and fall back.
    pub async fn plan(
        &self,
        mode_id: &str,
        goal: &str,
        context: &[RagHit],
        capabilities: &[String],
        manifest: &ModeManifest,
    ) -> Result<ModePlan, PlannerError> {
        let planner = self
            .get(mode_id)
            .ok_or_else(|| PlannerError::UnknownMode(mode_id.to_string()))?;
        planner.plan(goal, context, capabilities, manifest).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_planner_uses_keyword_fallback() {
        let planner = GeneralPlanner;
        let plan = tokio_test::block_on(planner.plan(
            "read file main.rs",
            &[],
            &[],
            &ModeManifest {
                id: "general".into(),
                label: "General".into(),
                description: "test".into(),
                memory_scope: vec!["global".into()],
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
            },
        ))
        .expect("plan should succeed");
        assert_eq!(plan.mode_id, "general");
        assert!(!plan.steps.is_empty());
    }

    #[test]
    fn vibe_coding_read_plan_has_correct_steps() {
        let planner = VibeCodingPlanner;
        let plan = tokio_test::block_on(planner.plan(
            "read file src/main.rs",
            &[],
            &["filesystem.read_file".to_string(), "knowledge.lookup".to_string()],
            &ModeManifest {
                id: "vibe_coding".into(),
                label: "Vibe Coding".into(),
                description: "test".into(),
                memory_scope: vec!["global".into()],
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
            },
        ))
        .expect("plan should succeed");
        assert_eq!(plan.mode_id, "vibe_coding");
        assert!(plan.steps.len() >= 2);
    }

    #[test]
    fn vibe_coding_missing_capability_rejected() {
        let planner = VibeCodingPlanner;
        let result = tokio_test::block_on(planner.plan(
            "write file src/main.rs",
            &[],
            &[],
            &ModeManifest {
                id: "vibe_coding".into(),
                label: "Vibe Coding".into(),
                description: "test".into(),
                memory_scope: vec!["global".into()],
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
            },
        ));
        assert!(result.is_err());
    }

    #[test]
    fn research_planner_includes_citation_steps() {
        let planner = ResearchPlanner;
        let plan = tokio_test::block_on(planner.plan(
            "analyze clinical trial results for drug X",
            &[],
            &["web.strain".to_string(), "knowledge.lookup".to_string(), "logic.verify".to_string(), "knowledge.synthesize".to_string()],
            &ModeManifest {
                id: "research".into(),
                label: "Research".into(),
                description: "test".into(),
                memory_scope: vec!["global".into()],
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
            },
        ))
        .expect("plan should succeed");
        assert_eq!(plan.mode_id, "research");
        // Should have web, RAG×2, verify, synthesize — at least 3 steps.
        assert!(plan.steps.len() >= 3);
    }

    #[test]
    fn security_planner_is_wide_open() {
        let planner = SecurityPlanner;
        let plan = tokio_test::block_on(planner.plan(
            "audit network configuration",
            &[],
            &["knowledge.lookup".to_string(), "logic.audit".to_string()],
            &ModeManifest {
                id: "security".into(),
                label: "Security".into(),
                description: "test".into(),
                memory_scope: vec!["global".into()],
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
            },
        ))
        .expect("plan should succeed");
        assert_eq!(plan.mode_id, "security");
        assert_eq!(plan.steps.len(), 3);
    }

    #[test]
    fn registry_unknown_mode_returns_error() {
        let registry = ModePlannerRegistry::new();
        let result = tokio_test::block_on(registry.plan(
            "not_a_mode",
            "test goal",
            &[],
            &[],
            &ModeManifest {
                id: "not_a_mode".into(),
                label: "Fake".into(),
                description: "test".into(),
                memory_scope: vec!["global".into()],
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
            },
        ));
        assert!(result.is_err());
    }

    #[test]
    fn default_registry_has_all_four_modes() {
        let registry = ModePlannerRegistry::default_registry();
        assert!(registry.get("general").is_some());
        assert!(registry.get("vibe_coding").is_some());
        assert!(registry.get("research").is_some());
        assert!(registry.get("security").is_some());
    }
}
