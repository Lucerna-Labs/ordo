//! `MemoryRouterService` â€” the orchestrator. Given a query, pick
//! providers; dispatch scatter-gather; return ranked results.

use std::sync::Arc;

use chrono::Utc;
use ordo_bus::{Bus, ProviderRegistry};
use ordo_protocol::{
    memory_topics, ClassifierNodeChoice, ClassifierOutput, Envelope, NodeId, OrdoMessage,
    ProtocolViolation, ProtocolViolationType, ProviderRegistration, RouteDecided, RouteMode,
    Severity, TreeChangeType, TreeNode,
};
use parking_lot::Mutex;

use crate::classifier::{Classifier, ClassifierError};
use crate::tree::{TreeStore, TreeStoreError};

/// Fast-mode confidence threshold used by Auto routing. Below this,
/// Auto falls through to Classify mode. The blueprint calls this
/// out as a "hypothesis, not measurement" that should be
/// recalibrated from feedback data.
pub const DEFAULT_FAST_THRESHOLD: f32 = 0.75;

/// Minimum per-node confidence the classifier must report for at
/// least ONE returned node, or the router rejects the classifier
/// output and falls back to broader fast-mode routing.
pub const CLASSIFIER_MIN_CONFIDENCE: f32 = 0.6;

#[derive(Debug, thiserror::Error)]
pub enum MemoryRouterError {
    #[error("no route: query matched no live tree nodes")]
    NoRoute,
    #[error("classifier hallucinated node `{0}` not in the live tree")]
    ClassifierHallucination(String),
    #[error("classifier confidence below threshold (best {0})")]
    ClassifierLowConfidence(f32),
    #[error("classifier: {0}")]
    Classifier(#[from] ClassifierError),
    #[error("tree store: {0}")]
    TreeStore(String),
    #[error("bus: {0}")]
    Bus(String),
}

impl From<TreeStoreError> for MemoryRouterError {
    fn from(err: TreeStoreError) -> Self {
        MemoryRouterError::TreeStore(err.to_string())
    }
}

pub type MemoryRouterResult<T> = Result<T, MemoryRouterError>;

#[derive(Debug, Clone)]
pub struct RouteOutcome {
    pub decision: RouteDecided,
    /// Providers selected for dispatch (copied from the decision
    /// for caller convenience). Empty = `NoRoute` was returned.
    pub providers: Vec<ProviderRegistration>,
}

#[derive(Clone)]
pub struct MemoryRouterService {
    tree: Arc<Mutex<TreeStore>>,
    registry: ProviderRegistry,
    classifier: Option<Arc<dyn Classifier>>,
    workspace_id: String,
    fast_threshold: f32,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
}

impl MemoryRouterService {
    pub fn new(
        tree: TreeStore,
        registry: ProviderRegistry,
        workspace_id: impl Into<String>,
    ) -> Self {
        Self {
            tree: Arc::new(Mutex::new(tree)),
            registry,
            classifier: None,
            workspace_id: workspace_id.into(),
            fast_threshold: DEFAULT_FAST_THRESHOLD,
            bus: None,
            node_id: NodeId::new(),
        }
    }

    pub fn with_classifier(mut self, classifier: Arc<dyn Classifier>) -> Self {
        self.classifier = Some(classifier);
        self
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_fast_threshold(mut self, threshold: f32) -> Self {
        self.fast_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    pub fn registry(&self) -> &ProviderRegistry {
        &self.registry
    }

    pub fn live_tree(&self) -> MemoryRouterResult<Vec<TreeNode>> {
        Ok(self.tree.lock().list_live(&self.workspace_id)?)
    }

    pub fn tree_at(&self, timestamp_ms: i64) -> MemoryRouterResult<Vec<TreeNode>> {
        Ok(self
            .tree
            .lock()
            .list_at_timestamp(&self.workspace_id, timestamp_ms)?)
    }

    /// Upsert a tree node. Emits `ordo.memory.tree.change` BEFORE
    /// the mutation takes effect so subscribers (and the log)
    /// record the intent; if the mutation fails, the log still
    /// shows the attempt â€” the blueprint treats tree intent as
    /// audit-worthy.
    pub async fn upsert_node(&self, node: TreeNode) -> MemoryRouterResult<()> {
        let before = self.tree.lock().get(&self.workspace_id, &node.path)?;
        self.broadcast_tree_change(
            node.path.clone(),
            TreeChangeType::Upsert,
            before.clone(),
            Some(node.clone()),
        )
        .await;
        self.tree.lock().upsert(&self.workspace_id, &node)?;
        Ok(())
    }

    pub async fn tombstone_node(&self, path: &str) -> MemoryRouterResult<()> {
        let now = Utc::now().timestamp_millis();
        let before = self.tree.lock().get(&self.workspace_id, path)?;
        self.broadcast_tree_change(
            path.to_string(),
            TreeChangeType::Tombstone,
            before.clone(),
            None,
        )
        .await;
        self.tree.lock().tombstone(&self.workspace_id, path, now)?;
        Ok(())
    }

    /// Fast route. Deterministic. Picks top-K live nodes by combining
    /// lexical and domain-hint signals. Returns the signal's best score
    /// as `confidence`; downstream can gate on that.
    pub async fn route_fast(
        &self,
        query_id: String,
        query: &str,
        domain_hint: Option<&str>,
        max_providers: u32,
    ) -> MemoryRouterResult<RouteOutcome> {
        let tree = self.tree.lock().list_live(&self.workspace_id)?;
        let scored = rank_fast(query, domain_hint, &tree);
        if scored.is_empty() {
            return Err(MemoryRouterError::NoRoute);
        }
        let top_k = scored
            .into_iter()
            .take(max_providers.max(1) as usize)
            .collect::<Vec<_>>();
        let best = top_k.first().map(|(_, s)| *s).unwrap_or(0.0);
        let selected: Vec<TreeNode> = top_k.into_iter().map(|(n, _)| n).collect();
        self.finalize(query_id, RouteMode::Fast, selected, best, None)
            .await
    }

    /// Classify route. Calls the injected classifier, validates the
    /// returned paths against the live tree, rejects hallucinated
    /// paths as protocol violations, caches output on the decision.
    pub async fn route_classify(
        &self,
        query_id: String,
        query: &str,
    ) -> MemoryRouterResult<RouteOutcome> {
        let classifier = self.classifier.clone().ok_or_else(|| {
            MemoryRouterError::Classifier(ClassifierError::Transport(
                "no classifier configured".into(),
            ))
        })?;
        let tree = self.tree.lock().list_live(&self.workspace_id)?;
        let output = classifier.classify(query, &tree).await?;

        // Validate + reject hallucinations.
        let live_paths: std::collections::HashSet<String> =
            tree.iter().map(|n| n.path.clone()).collect();
        let mut validated: Vec<ClassifierNodeChoice> = Vec::new();
        for choice in &output.nodes {
            if !live_paths.contains(&choice.path) {
                self.emit_violation(
                    ProtocolViolationType::ClassifierHallucination,
                    Some(query_id.clone()),
                    format!("classifier proposed unknown path `{}`", choice.path),
                    Severity::Error,
                )
                .await;
                return Err(MemoryRouterError::ClassifierHallucination(
                    choice.path.clone(),
                ));
            }
            validated.push(choice.clone());
        }

        // Minimum-confidence gate. If no node has self-reported
        // confidence â‰¥ `CLASSIFIER_MIN_CONFIDENCE`, log the
        // low-confidence event and fall back to broader fast-mode
        // routing.
        let best_classifier_conf = validated
            .iter()
            .map(|c| c.confidence)
            .fold(0.0_f32, f32::max);
        if best_classifier_conf < CLASSIFIER_MIN_CONFIDENCE {
            if let Some(bus) = &self.bus {
                let env = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::MemoryRouteLowConfidence {
                        query_id: query_id.clone(),
                        best_classifier_confidence: best_classifier_conf,
                    },
                );
                let _ = bus.publish(memory_topics::ROUTE_LOW_CONFIDENCE, env).await;
            }
            // Fallback: top-3 by fast-route score.
            let scored = rank_fast(query, None, &tree);
            let fallback: Vec<TreeNode> = scored.into_iter().take(3).map(|(n, _)| n).collect();
            if fallback.is_empty() {
                return Err(MemoryRouterError::NoRoute);
            }
            return self
                .finalize(query_id, RouteMode::Fast, fallback, 0.0, None)
                .await;
        }

        // Happy path: dispatch to providers for validated paths.
        let selected: Vec<TreeNode> = validated
            .iter()
            .filter_map(|c| tree.iter().find(|n| n.path == c.path).cloned())
            .collect();
        let cache = ClassifierOutput {
            model: output.model.clone(),
            nodes: validated,
        };
        self.finalize(
            query_id,
            RouteMode::Classify,
            selected,
            best_classifier_conf,
            Some(cache),
        )
        .await
    }

    /// Auto mode: try fast first. If confidence exceeds threshold
    /// or no classifier is configured, return the fast result.
    /// Otherwise fall through to classify.
    pub async fn route_auto(
        &self,
        query_id: String,
        query: &str,
        domain_hint: Option<&str>,
        max_providers: u32,
    ) -> MemoryRouterResult<RouteOutcome> {
        let fast = self
            .route_fast(query_id.clone(), query, domain_hint, max_providers)
            .await?;
        if fast.decision.confidence >= self.fast_threshold || self.classifier.is_none() {
            return Ok(fast);
        }
        self.route_classify(query_id, query).await
    }

    /// Build `RouteDecided`, pick providers for the selected nodes
    /// via the registry, broadcast on the bus, return outcome.
    async fn finalize(
        &self,
        query_id: String,
        mode_used: RouteMode,
        selected: Vec<TreeNode>,
        confidence: f32,
        classifier_output_cache: Option<ClassifierOutput>,
    ) -> MemoryRouterResult<RouteOutcome> {
        let mut providers: Vec<ProviderRegistration> = Vec::new();
        let mut provider_ids: Vec<String> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        for node in &selected {
            let entries = self.registry.for_path(&node.path);
            for entry in entries {
                if !seen_ids.insert(entry.provider_id.clone()) {
                    continue;
                }
                let registration = entry_to_registration(&entry);
                provider_ids.push(registration.provider_id.clone());
                providers.push(registration);
            }
        }

        let decision = RouteDecided {
            query_id,
            mode_used,
            nodes_selected: selected.iter().map(|n| n.path.clone()).collect(),
            providers_dispatched: provider_ids,
            confidence,
            classifier_output_cache,
        };

        if let Some(bus) = &self.bus {
            let env = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::MemoryRouteDecided(decision.clone()),
            );
            let _ = bus.publish(memory_topics::ROUTE_DECIDED, env).await;
        }

        Ok(RouteOutcome {
            decision,
            providers,
        })
    }

    async fn broadcast_tree_change(
        &self,
        path: String,
        change_type: TreeChangeType,
        before: Option<TreeNode>,
        after: Option<TreeNode>,
    ) {
        if let Some(bus) = &self.bus {
            let env = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::MemoryTreeChange {
                    path,
                    change_type,
                    before,
                    after,
                },
            );
            let _ = bus.publish(memory_topics::TREE_CHANGE, env).await;
        }
    }

    async fn emit_violation(
        &self,
        violation_type: ProtocolViolationType,
        offending_id: Option<String>,
        details: String,
        severity: Severity,
    ) {
        if let Some(bus) = &self.bus {
            let v = ProtocolViolation {
                violation_type,
                offending_id,
                details,
                severity,
            };
            let env = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::MemoryProtocolViolation(v),
            );
            let _ = bus.publish(memory_topics::PROTOCOL_VIOLATION, env).await;
        }
    }
}

/// Score every live node against the query using lexical overlap +
/// an optional domain-hint boost. Returns the nodes sorted by score
/// descending, ties broken by path asc for determinism.
fn rank_fast(query: &str, domain_hint: Option<&str>, tree: &[TreeNode]) -> Vec<(TreeNode, f32)> {
    let q_tokens = tokenize(query);
    if q_tokens.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(TreeNode, f32)> = tree
        .iter()
        .filter(|n| !n.tombstoned)
        .map(|node| {
            let desc_tokens = tokenize(&node.description);
            let mut overlap = 0usize;
            for t in &q_tokens {
                if desc_tokens.iter().any(|d| d == t) {
                    overlap += 1;
                }
            }
            let mut score = if q_tokens.is_empty() {
                0.0
            } else {
                overlap as f32 / q_tokens.len() as f32
            };
            // Domain-hint boost: contains-match on path.
            if let Some(hint) = domain_hint {
                if node.path.contains(hint) {
                    score = (score + 0.25).min(1.0);
                }
            }
            (node.clone(), score)
        })
        .filter(|(_, score)| *score > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.path.cmp(&b.0.path))
    });
    scored
}

fn tokenize(s: &str) -> Vec<String> {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty() && t.len() > 2)
        .map(str::to_string)
        .collect()
}

fn entry_to_registration(entry: &ordo_bus::ProviderRegistryEntry) -> ProviderRegistration {
    use ordo_protocol::{CostHint, RetrievalSemantics};
    // Best-effort field decode from the JSON metadata; missing fields
    // use safe defaults.
    let semantics = entry
        .metadata
        .get("retrieval_semantics")
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "lexical" => Some(RetrievalSemantics::Lexical),
            "dense" => Some(RetrievalSemantics::Dense),
            "hybrid" => Some(RetrievalSemantics::Hybrid),
            "exact" => Some(RetrievalSemantics::Exact),
            _ => None,
        })
        .unwrap_or(RetrievalSemantics::Hybrid);
    let cost_hint = entry
        .metadata
        .get("cost_hint")
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "cheap" => Some(CostHint::Cheap),
            "moderate" => Some(CostHint::Moderate),
            "expensive" => Some(CostHint::Expensive),
            _ => None,
        })
        .unwrap_or(CostHint::Moderate);
    let provenance_guarantee = entry
        .metadata
        .get("provenance_guarantee")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    ProviderRegistration {
        provider_id: entry.provider_id.clone(),
        serves_paths: entry.serves_paths.clone(),
        retrieval_semantics: semantics,
        cost_hint,
        provenance_guarantee,
        heartbeat_interval_ms: entry.heartbeat_interval.as_millis() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_bus::ProviderRegistryEntry;
    use serde_json::json;
    use std::time::Duration;

    fn make_tree_node(path: &str, desc: &str) -> TreeNode {
        TreeNode {
            path: path.into(),
            parent_path: None,
            description: desc.into(),
            retrieval_hint: None,
            created_at_ms: 0,
            updated_at_ms: 0,
            tombstoned: false,
        }
    }

    async fn svc_with_tree() -> MemoryRouterService {
        let mut tree = TreeStore::in_memory().unwrap();
        tree.upsert(
            "local",
            &make_tree_node("lucerna/voice", "lucerna brand voice examples"),
        )
        .unwrap();
        tree.upsert(
            "local",
            &make_tree_node("lucerna/brand", "lucerna brand guidelines"),
        )
        .unwrap();
        tree.upsert(
            "local",
            &make_tree_node("labs/codebase", "codebase internals reference"),
        )
        .unwrap();
        let registry = ProviderRegistry::new();
        registry.register(ProviderRegistryEntry::new(
            "voice-rag",
            vec!["lucerna/voice".into()],
            json!({"retrieval_semantics": "hybrid", "cost_hint": "cheap", "provenance_guarantee": true}),
            Duration::from_secs(60),
        ));
        MemoryRouterService::new(tree, registry, "local")
    }

    #[tokio::test]
    async fn fast_route_picks_nodes_by_lexical_overlap() {
        let svc = svc_with_tree().await;
        let outcome = svc
            .route_fast("q1".into(), "brand voice guidelines", None, 2)
            .await
            .unwrap();
        assert!(
            outcome
                .decision
                .nodes_selected
                .iter()
                .any(|p| p.contains("lucerna")),
            "expected a lucerna node; got {:?}",
            outcome.decision.nodes_selected
        );
        assert_eq!(outcome.decision.mode_used, RouteMode::Fast);
    }

    #[tokio::test]
    async fn fast_route_no_match_returns_no_route() {
        let svc = svc_with_tree().await;
        let err = svc
            .route_fast("q1".into(), "xyz quantum unrelated", None, 2)
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryRouterError::NoRoute));
    }

    #[tokio::test]
    async fn classify_route_rejects_hallucinated_paths() {
        use crate::classifier::ScriptedClassifier;
        let mut svc = svc_with_tree().await;
        svc = svc.with_classifier(Arc::new(ScriptedClassifier::new(
            "mock",
            vec![("not/a/real/path", 0.9)],
        )));
        let err = svc
            .route_classify("q1".into(), "anything")
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryRouterError::ClassifierHallucination(_)));
    }

    #[tokio::test]
    async fn classify_route_low_confidence_falls_back() {
        use crate::classifier::ScriptedClassifier;
        let mut svc = svc_with_tree().await;
        svc = svc.with_classifier(Arc::new(ScriptedClassifier::new(
            "mock",
            // All below 0.6 threshold â€” trigger fallback.
            vec![("lucerna/voice", 0.3), ("lucerna/brand", 0.2)],
        )));
        let outcome = svc
            .route_classify("q1".into(), "brand voice")
            .await
            .unwrap();
        // Fallback path uses fast mode.
        assert_eq!(outcome.decision.mode_used, RouteMode::Fast);
    }

    #[tokio::test]
    async fn classify_caches_output_on_decision_event() {
        use crate::classifier::ScriptedClassifier;
        let mut svc = svc_with_tree().await;
        svc = svc.with_classifier(Arc::new(ScriptedClassifier::new(
            "mock-v1",
            vec![("lucerna/voice", 0.85)],
        )));
        let outcome = svc.route_classify("q1".into(), "voice").await.unwrap();
        let cache = outcome
            .decision
            .classifier_output_cache
            .expect("classify should cache output");
        assert_eq!(cache.model, "mock-v1");
        assert_eq!(cache.nodes.len(), 1);
        assert_eq!(cache.nodes[0].path, "lucerna/voice");
    }

    #[tokio::test]
    async fn auto_uses_fast_when_confident_enough() {
        use crate::classifier::ScriptedClassifier;
        let mut svc = svc_with_tree().await;
        svc = svc.with_classifier(Arc::new(ScriptedClassifier::new(
            "mock",
            vec![("lucerna/voice", 0.95)],
        )));
        let outcome = svc
            .route_auto(
                "q1".into(),
                "lucerna brand voice examples reference",
                None,
                2,
            )
            .await
            .unwrap();
        // Fast mode gets confidence 1.0 on a fully-overlapping query.
        assert_eq!(outcome.decision.mode_used, RouteMode::Fast);
    }
}
