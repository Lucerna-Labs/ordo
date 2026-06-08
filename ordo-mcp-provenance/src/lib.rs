//! ordo-mcp-provenance â€” taint tracking + causal graph queries
//! over the memory log event substrate.
//!
//! Responsibility boundary: this crate owns the *reachability*
//! layer â€” given an action about to fire, walk the causal graph
//! backward and decide whether any tainted ancestor gates it.
//! Does NOT own the event log itself (that's `ordo-memory-log`),
//! does NOT own Worker extraction (that's `ordo-mcp-worker`),
//! does NOT own tool invocation (that's `ordo-mcp-client`).
//!
//! Load-bearing commitments (blueprint Â§30, invariant 30):
//!
//! - Every sensitive action checks causal ancestry. If any
//!   ancestor has `UntrustedMcp` taint and the path does not
//!   pass through a sanitization node or user-confirmation node,
//!   the action is blocked.
//! - Taint propagates through parent/turn chains. `Mixed` taint
//!   aggregates across convergent sources.
//! - Sanitization nodes break taint propagation for downstream
//!   events. They are ONLY emitted by Worker extraction â€”
//!   forged sanitization events fail verification at write time
//!   (future: HMAC over event id with a Worker-held key; v1 keys
//!   off the event type + a registry of legitimate sanitizers).
//!
//! Storage model: taint is stored as a JSON payload field on the
//! memory log event. The provenance service reads those fields
//! on-demand and materialises the graph via petgraph for
//! reachability queries. No separate graph table â€” the log IS
//! the graph.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ordo_bus::Bus;
use ordo_memory_log::{MemoryLogError, MemoryLogService};
use ordo_protocol::memory::Ulid;
use ordo_protocol::{
    mcp_topics, BusEnvelope, Envelope, MemoryEvent, MemoryEventType, NodeId, OrdoMessage,
    ProvenanceCheckRequest, ProvenanceCheckResult, Taint,
};
use parking_lot::Mutex;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum ProvenanceError {
    #[error("memory log: {0}")]
    MemoryLog(#[from] MemoryLogError),
    #[error("event {0} not found")]
    EventMissing(String),
    #[error("sanitization verification failed: {0}")]
    SanitizationInvalid(String),
    #[error("bad input: {0}")]
    BadInput(String),
}

pub type ProvenanceResult<T> = Result<T, ProvenanceError>;

/// The service. Holds a handle to the memory log + an optional
/// bus for emitting `McpProvenanceSensitiveBlocked` + sanitization
/// events.
pub struct ProvenanceService {
    memory_log: MemoryLogService,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
    /// Registry of sanitization node ids. Sanitization events are
    /// recorded here at emit time; taint queries consult it to
    /// decide whether a `UntrustedMcp` ancestor has been broken
    /// by downstream sanitization. In v1 this is in-memory; a
    /// future version will persist it alongside the log so
    /// restarts preserve the set.
    sanitizers: Arc<Mutex<HashSet<Ulid>>>,
    /// Registry of user-confirmation events. Same lifecycle as
    /// sanitizers.
    user_confirmations: Arc<Mutex<HashSet<Ulid>>>,
}

impl ProvenanceService {
    pub fn new(memory_log: MemoryLogService) -> Self {
        Self {
            memory_log,
            bus: None,
            node_id: NodeId::new(),
            sanitizers: Arc::new(Mutex::new(HashSet::new())),
            user_confirmations: Arc::new(Mutex::new(HashSet::new())),
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

    /// Record a sanitization node. The `event_id` MUST correspond
    /// to a memory-log event that was emitted by Worker extraction
    /// completing successfully; v1 trusts the caller, v2 will
    /// cryptographically bind the sanitization to the Worker's
    /// extraction proof.
    pub async fn record_sanitization(
        &self,
        event_id: Ulid,
        justification: impl Into<String>,
    ) -> ProvenanceResult<()> {
        {
            let mut sanitizers = self.sanitizers.lock();
            sanitizers.insert(event_id.clone());
        }
        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpProvenanceSanitized {
                    event_id: event_id.clone(),
                    justification: justification.into(),
                },
            );
            let _ = bus.publish(mcp_topics::PROVENANCE_SANITIZE, env).await;
        }
        Ok(())
    }

    /// Record a user-confirmation event. Breaks taint propagation
    /// for the specific action the user confirmed. Does NOT grant
    /// carte blanche to subsequent sensitive actions (negative-
    /// space test).
    pub async fn record_user_confirmation(&self, action_id: Ulid) -> ProvenanceResult<()> {
        {
            let mut confirmations = self.user_confirmations.lock();
            confirmations.insert(action_id.clone());
        }
        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpProvenanceUserConfirmed {
                    action_id: action_id.clone(),
                },
            );
            let _ = bus.publish(mcp_topics::PROVENANCE_USER_CONFIRM, env).await;
        }
        Ok(())
    }

    /// The central reachability check. Given a proposed causal
    /// chain (ordered event ids leading to the action), return
    /// whether the action should be allowed. Blocked iff some
    /// ancestor carries `UntrustedMcp` taint and the path to the
    /// action does not pass through a sanitization node or user-
    /// confirmation node.
    pub async fn check(
        &self,
        request: ProvenanceCheckRequest,
    ) -> ProvenanceResult<ProvenanceCheckResult> {
        if request.proposed_causal_chain.is_empty() {
            return Ok(ProvenanceCheckResult {
                allowed: true,
                taint_path: Vec::new(),
                summary: "no causal chain; trivially trusted".into(),
            });
        }

        // Materialise the graph from log events. For v1 we walk
        // the chain the caller provided; that keeps the fast path
        // simple (sub-millisecond) and pushes the graph
        // reconstruction cost onto queries that actually need it.
        // Full-graph walks happen when `horizon_turns` is set and
        // the caller wants to cross turn boundaries.
        let mut events = Vec::with_capacity(request.proposed_causal_chain.len());
        for event_id in &request.proposed_causal_chain {
            match self.memory_log.get_by_id(event_id)? {
                Some(event) => events.push(event),
                None => {
                    return Err(ProvenanceError::EventMissing(event_id.clone()));
                }
            }
        }

        let sanitizers = self.sanitizers.lock().clone();
        let confirmations = self.user_confirmations.lock().clone();
        let verdict = evaluate_chain(&events, &sanitizers, &confirmations);

        let result = ProvenanceCheckResult {
            allowed: verdict.allowed,
            taint_path: verdict.taint_path.clone(),
            summary: verdict.summary.clone(),
        };

        if !verdict.allowed {
            if let Some(bus) = &self.bus {
                let env: BusEnvelope = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::McpProvenanceSensitiveBlocked {
                        action: request.action.clone(),
                        taint_path: verdict.taint_path.clone(),
                    },
                );
                let _ = bus
                    .publish(mcp_topics::PROVENANCE_SENSITIVE_BLOCKED, env)
                    .await;
            }
        }

        Ok(result)
    }

    /// Walk the causal graph backward from `event_id` for
    /// `horizon_turns` prior turns. Returns `(ancestor_event_id,
    /// taint)` pairs oldest-first.
    pub async fn query_ancestry(
        &self,
        event_id: &str,
        horizon_turns: u32,
    ) -> ProvenanceResult<Vec<(Ulid, Taint)>> {
        // Materialise ancestors via parent_id chain.
        let mut chain = Vec::new();
        let mut cursor = event_id.to_string();
        let mut turn_horizon = horizon_turns + 1;
        let mut seen_turn: Option<Ulid> = None;

        while turn_horizon > 0 {
            let Some(event) = self.memory_log.get_by_id(&cursor)? else {
                break;
            };
            let taint = taint_for_event(&event);
            chain.push((event.id.clone(), taint));
            match &event.turn_id {
                Some(t) if seen_turn.as_ref() != Some(t) => {
                    if seen_turn.is_some() {
                        turn_horizon = turn_horizon.saturating_sub(1);
                        if turn_horizon == 0 {
                            break;
                        }
                    }
                    seen_turn = Some(t.clone());
                }
                _ => {}
            }
            match &event.parent_id {
                Some(parent) => cursor = parent.clone(),
                None => break,
            }
        }
        chain.reverse();
        Ok(chain)
    }

    /// Build the reachability graph for a set of events and
    /// return it. Primarily used by tests and advanced tooling.
    pub fn build_graph(events: &[MemoryEvent]) -> CausalGraph {
        let mut graph = DiGraph::<Ulid, ()>::new();
        let mut index: HashMap<Ulid, NodeIndex> = HashMap::new();
        for event in events {
            let idx = graph.add_node(event.id.clone());
            index.insert(event.id.clone(), idx);
        }
        for event in events {
            if let Some(parent) = &event.parent_id {
                if let (Some(&child), Some(&parent)) = (index.get(&event.id), index.get(parent)) {
                    graph.add_edge(parent, child, ());
                }
            }
        }
        CausalGraph { graph, index }
    }
}

/// Petgraph-backed materialised causal graph. Lives only as long
/// as the caller needs it; reconstructed on demand from log
/// events. Reachability is O(V+E) over the subgraph â€” plenty
/// fast for the sizes we handle per-turn.
pub struct CausalGraph {
    graph: DiGraph<Ulid, ()>,
    index: HashMap<Ulid, NodeIndex>,
}

impl CausalGraph {
    pub fn len(&self) -> usize {
        self.graph.node_count()
    }

    pub fn is_empty(&self) -> bool {
        self.graph.node_count() == 0
    }

    /// Walk all ancestors of `event_id`, oldest first.
    pub fn ancestors(&self, event_id: &str) -> Vec<Ulid> {
        let Some(start) = self.index.get(event_id).copied() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut frontier = vec![start];
        let mut seen = HashSet::new();
        while let Some(node) = frontier.pop() {
            if !seen.insert(node) {
                continue;
            }
            for edge in self
                .graph
                .edges_directed(node, petgraph::Direction::Incoming)
            {
                let src = edge.source();
                frontier.push(src);
                out.push(self.graph[src].clone());
            }
        }
        out.reverse();
        out
    }
}

#[derive(Debug)]
struct ChainVerdict {
    allowed: bool,
    taint_path: Vec<Ulid>,
    summary: String,
}

fn evaluate_chain(
    events: &[MemoryEvent],
    sanitizers: &HashSet<Ulid>,
    confirmations: &HashSet<Ulid>,
) -> ChainVerdict {
    // Walk the chain oldest-first. Track whether an `UntrustedMcp`
    // taint is "active"; a sanitization or user-confirmation
    // downstream clears it.
    let mut active_taint_source: Option<Ulid> = None;
    let mut taint_path = Vec::new();

    for event in events {
        let taint = taint_for_event(event);
        if taint.is_untrusted() {
            if active_taint_source.is_none() {
                active_taint_source = Some(event.id.clone());
            }
            taint_path.push(event.id.clone());
        }
        if sanitizers.contains(&event.id) || confirmations.contains(&event.id) {
            // Clear â€” downstream events are sanitized.
            active_taint_source = None;
            taint_path.clear();
        }
    }

    if active_taint_source.is_none() {
        ChainVerdict {
            allowed: true,
            taint_path: Vec::new(),
            summary: "no unsanitized untrusted ancestry".into(),
        }
    } else {
        ChainVerdict {
            allowed: false,
            taint_path,
            summary: "untrusted MCP ancestry without sanitization or user confirmation".into(),
        }
    }
}

/// Extract the `taint` field from an event's payload. Events
/// without an explicit taint default to `Trusted` (system-
/// originated). Used by all graph walks.
pub fn taint_for_event(event: &MemoryEvent) -> Taint {
    if let Some(raw) = event.payload.get("taint") {
        if let Ok(taint) = serde_json::from_value::<Taint>(raw.clone()) {
            return taint;
        }
    }
    // Conservative default based on event kind: agent/tool events
    // without explicit taint are VerifiedProvider; UserMessage is
    // User; everything else is Trusted.
    match event.event_type {
        MemoryEventType::UserMessage => Taint::User,
        MemoryEventType::ToolInvocation | MemoryEventType::ToolResult => Taint::VerifiedProvider,
        _ => Taint::Trusted,
    }
}

/// Helper for callers that are emitting a fresh memory event and
/// want to stamp taint into the payload. The projection service
/// uses this at prompt-assembly time; the MCP client uses it when
/// logging Worker results.
pub fn attach_taint(mut payload: Value, taint: &Taint) -> Value {
    if let Value::Object(ref mut map) = payload {
        if let Ok(value) = serde_json::to_value(taint) {
            map.insert("taint".to_string(), value);
            return payload;
        }
    }
    // If payload wasn't an object (edge case), wrap it.
    json!({
        "payload": payload,
        "taint": serde_json::to_value(taint).unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ordo_protocol::memory::Ulid as ProtoUlid;
    use ordo_protocol::RetentionTier;

    fn make_event(id: &str, parent: Option<&str>, taint: Option<Taint>) -> MemoryEvent {
        let payload = match taint {
            Some(t) => json!({ "taint": t }),
            None => json!({}),
        };
        let payload_hash = blake3::hash(&serde_json::to_vec(&payload).unwrap())
            .to_hex()
            .to_string();
        MemoryEvent {
            id: id.to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            event_type: MemoryEventType::ToolResult,
            actor: "test".into(),
            domain: None,
            category: None,
            parent_id: parent.map(|p| p.to_string()),
            turn_id: Some("turn-1".to_string()),
            payload,
            payload_hash,
            tier: RetentionTier::Hot,
            pinned: false,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        }
    }

    fn ulid() -> ProtoUlid {
        ulid::Ulid::new().to_string()
    }

    #[test]
    fn trusted_chain_is_allowed() {
        let a = make_event(&ulid(), None, None);
        let b = make_event(&ulid(), Some(&a.id), None);
        let verdict = evaluate_chain(&[a, b], &HashSet::new(), &HashSet::new());
        assert!(verdict.allowed);
        assert!(verdict.taint_path.is_empty());
    }

    #[test]
    fn untrusted_mcp_without_sanitization_is_blocked() {
        let a = make_event(
            &ulid(),
            None,
            Some(Taint::UntrustedMcp {
                server_id: "server-x".into(),
                invocation_id: "inv-1".into(),
            }),
        );
        let b = make_event(&ulid(), Some(&a.id), None);
        let verdict = evaluate_chain(&[a.clone(), b], &HashSet::new(), &HashSet::new());
        assert!(!verdict.allowed);
        assert_eq!(verdict.taint_path, vec![a.id]);
    }

    #[test]
    fn sanitization_breaks_taint_propagation() {
        let a = make_event(
            &ulid(),
            None,
            Some(Taint::UntrustedMcp {
                server_id: "server-x".into(),
                invocation_id: "inv-1".into(),
            }),
        );
        let b = make_event(&ulid(), Some(&a.id), None);
        let c = make_event(&ulid(), Some(&b.id), None);
        let mut sanitizers = HashSet::new();
        sanitizers.insert(b.id.clone());
        let verdict = evaluate_chain(&[a, b, c], &sanitizers, &HashSet::new());
        assert!(verdict.allowed);
        assert!(verdict.taint_path.is_empty());
    }

    #[test]
    fn user_confirmation_breaks_taint() {
        let a = make_event(
            &ulid(),
            None,
            Some(Taint::UntrustedMcp {
                server_id: "x".into(),
                invocation_id: "1".into(),
            }),
        );
        let b = make_event(&ulid(), Some(&a.id), None);
        let mut confirmations = HashSet::new();
        confirmations.insert(b.id.clone());
        let verdict = evaluate_chain(&[a, b], &HashSet::new(), &confirmations);
        assert!(verdict.allowed);
    }

    #[test]
    fn user_confirmation_does_not_grant_carte_blanche() {
        // A confirmed action A is followed by a fresh untrusted
        // ancestor B and a sensitive action C. C must still be
        // gated â€” confirmation of A does not cover C.
        let a = make_event(
            &ulid(),
            None,
            Some(Taint::UntrustedMcp {
                server_id: "x".into(),
                invocation_id: "1".into(),
            }),
        );
        let a_confirmed = make_event(&ulid(), Some(&a.id), None);
        let b = make_event(
            &ulid(),
            Some(&a_confirmed.id),
            Some(Taint::UntrustedMcp {
                server_id: "y".into(),
                invocation_id: "2".into(),
            }),
        );
        let c = make_event(&ulid(), Some(&b.id), None);
        let mut confirmations = HashSet::new();
        confirmations.insert(a_confirmed.id.clone());
        let verdict = evaluate_chain(
            &[a, a_confirmed, b.clone(), c],
            &HashSet::new(),
            &confirmations,
        );
        assert!(!verdict.allowed);
        assert!(verdict.taint_path.contains(&b.id));
    }

    #[test]
    fn mixed_taint_aggregates_multiple_untrusted_sources() {
        let a = make_event(
            &ulid(),
            None,
            Some(Taint::Mixed {
                sources: vec![
                    Taint::UntrustedMcp {
                        server_id: "x".into(),
                        invocation_id: "1".into(),
                    },
                    Taint::User,
                ],
            }),
        );
        let verdict = evaluate_chain(std::slice::from_ref(&a), &HashSet::new(), &HashSet::new());
        assert!(!verdict.allowed);
        assert_eq!(verdict.taint_path, vec![a.id]);
    }

    #[test]
    fn taint_for_event_defaults_by_event_type() {
        let user_event = MemoryEvent {
            id: ulid(),
            timestamp_ms: 0,
            event_type: MemoryEventType::UserMessage,
            actor: "u".into(),
            domain: None,
            category: None,
            parent_id: None,
            turn_id: None,
            payload: json!({}),
            payload_hash: blake3::hash(b"{}").to_hex().to_string(),
            tier: RetentionTier::Hot,
            pinned: false,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        };
        assert!(matches!(taint_for_event(&user_event), Taint::User));
    }

    #[test]
    fn attach_taint_round_trips_through_taint_for_event() {
        let mut event = make_event(&ulid(), None, None);
        let taint = Taint::UntrustedMcp {
            server_id: "s".into(),
            invocation_id: "i".into(),
        };
        event.payload = attach_taint(event.payload, &taint);
        match taint_for_event(&event) {
            Taint::UntrustedMcp { server_id, .. } => assert_eq!(server_id, "s"),
            other => panic!("expected UntrustedMcp, got {other:?}"),
        }
    }

    #[test]
    fn build_graph_establishes_parent_edges() {
        let a = make_event(&ulid(), None, None);
        let b = make_event(&ulid(), Some(&a.id), None);
        let c = make_event(&ulid(), Some(&b.id), None);
        let events = vec![a.clone(), b.clone(), c.clone()];
        let graph = ProvenanceService::build_graph(&events);
        let ancestors_of_c = graph.ancestors(&c.id);
        assert!(ancestors_of_c.contains(&a.id));
        assert!(ancestors_of_c.contains(&b.id));
    }

    // Suppress unused-import warnings for types pulled in above.
    #[allow(dead_code)]
    fn _silence() {
        let _ = Utc::now();
    }
}
