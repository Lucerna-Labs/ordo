//! End-to-end test: the three memory crates wired together.
//!
//! Simulates a single turn:
//!   1. Operator speaks â†’ append `user.message` to the log
//!   2. Router picks nodes for the query (Fast mode â€” no classifier)
//!   3. Projection assembles the context window with the routing
//!      decision + some retrieved items
//!   4. Verify: projection output hash is deterministic across two
//!      calls with identical inputs.
//!
//! This is the **acceptance test** for the memory architecture
//! coming alive. If this fails, the three crates aren't talking.

use std::sync::Arc;

use chrono::Utc;
use futures::StreamExt;
use ordo_bus::{Bus, InProcessBus, ProviderRegistry, ProviderRegistryEntry};
use ordo_memory_log::{MemoryLogService, MemoryLogStore};
use ordo_memory_projection::{Budget, BuildInputs, MemoryProjectionService, RetrievedItem};
use ordo_memory_router::{MemoryRouterService, TreeStore};
use ordo_protocol::{
    memory_topics, MemoryEvent, MemoryEventType, OrdoMessage, RetentionTier, RetrievalSemantics,
    TreeNode,
};
use serde_json::json;
use std::time::Duration;

fn build_tree_node(path: &str, desc: &str) -> TreeNode {
    TreeNode {
        path: path.into(),
        parent_path: None,
        description: desc.into(),
        retrieval_hint: Some(RetrievalSemantics::Hybrid),
        created_at_ms: Utc::now().timestamp_millis(),
        updated_at_ms: Utc::now().timestamp_millis(),
        tombstoned: false,
    }
}

fn make_user_event(payload: serde_json::Value) -> MemoryEvent {
    let hash = MemoryLogService::compute_payload_hash(&payload);
    MemoryEvent {
        id: MemoryLogService::new_event_id(),
        timestamp_ms: Utc::now().timestamp_millis(),
        event_type: MemoryEventType::UserMessage,
        actor: "operator".into(),
        domain: Some("lucerna".into()),
        category: None,
        parent_id: None,
        turn_id: None,
        payload,
        payload_hash: hash,
        tier: RetentionTier::Hot,
        pinned: false,
        soft_deleted: false,
        soft_deleted_at: None,
        soft_deleted_reason: None,
    }
}

#[tokio::test]
async fn three_memory_crates_cooperate_on_a_single_turn() {
    let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());

    // --- Log ---------------------------------------------------
    let log_store = MemoryLogStore::in_memory().expect("log store");
    let log = MemoryLogService::new(log_store, "local").with_bus(bus.clone());

    // --- Router ------------------------------------------------
    let mut tree = TreeStore::in_memory().expect("tree store");
    tree.upsert(
        "local",
        &build_tree_node("lucerna/voice", "lucerna brand voice examples"),
    )
    .expect("tree upsert");
    tree.upsert(
        "local",
        &build_tree_node("lucerna/brand", "lucerna brand guidelines"),
    )
    .expect("tree upsert");
    let registry = ProviderRegistry::new();
    registry.register(ProviderRegistryEntry::new(
        "voice-rag",
        vec!["lucerna/voice".into()],
        json!({"retrieval_semantics": "hybrid", "cost_hint": "cheap", "provenance_guarantee": true}),
        Duration::from_secs(60),
    ));
    let router = MemoryRouterService::new(tree, registry, "local").with_bus(bus.clone());

    // --- Projection --------------------------------------------
    let projection = MemoryProjectionService::new().with_bus(bus.clone());

    // --- Turn 1: append the user message ----------------------
    // Subscribe to the `appended` topic BEFORE we append so we can
    // assert the event actually reached the bus.
    let mut appended_sub = bus
        .subscribe(memory_topics::LOG_APPENDED)
        .await
        .expect("sub");
    let user_event = make_user_event(json!({"text": "help me refine the warped reality voice"}));
    let append_result = log
        .append(user_event.clone())
        .await
        .expect("append user msg");
    assert!(!append_result.deduplicated);

    // Confirm the appended notification hit the bus.
    let appended_env = tokio::time::timeout(Duration::from_secs(1), appended_sub.next())
        .await
        .expect("got appended event")
        .expect("envelope present");
    match appended_env.payload {
        OrdoMessage::MemoryLogAppended { event_id, .. } => {
            assert_eq!(event_id, append_result.event.id);
        }
        other => panic!("unexpected envelope: {other:?}"),
    }

    // --- Turn 2: route the query ------------------------------
    let outcome = router
        .route_fast(
            "qid-1".into(),
            "lucerna brand voice reference",
            Some("lucerna"),
            2,
        )
        .await
        .expect("route");
    assert_eq!(outcome.decision.mode_used, ordo_protocol::RouteMode::Fast);
    assert!(
        outcome
            .decision
            .nodes_selected
            .iter()
            .any(|p| p == "lucerna/voice"),
        "should have picked the voice node"
    );

    // --- Turn 3: build the projection --------------------------
    let retrieved = vec![RetrievedItem {
        provider_id: "voice-rag".into(),
        score: 0.87,
        text: "Keep copy terse. Avoid exclamation points. Cite hidden patterns.".into(),
        provenance: json!({"source": "rag", "chunk_id": "voice-1"}),
    }];
    let inputs_a = BuildInputs {
        query: "help me refine the warped reality voice".into(),
        routing_decision: outcome.decision.clone(),
        tree_state: router.live_tree().expect("live tree"),
        pinned_events: vec![],
        recent_events: vec![append_result.event.clone()],
        retrieved: retrieved.clone(),
        budget: Budget { max_tokens: 2000 },
        allow_identity_truncation: false,
        replay_timestamp_ms: None,
    };
    let projection_a = projection.build(inputs_a).await.expect("build a");

    // Same inputs again â€” context_window must match byte-for-byte
    // (DPM determinism guarantee).
    let inputs_b = BuildInputs {
        query: "help me refine the warped reality voice".into(),
        routing_decision: outcome.decision.clone(),
        tree_state: router.live_tree().expect("live tree"),
        pinned_events: vec![],
        recent_events: vec![append_result.event.clone()],
        retrieved: retrieved.clone(),
        budget: Budget { max_tokens: 2000 },
        allow_identity_truncation: false,
        replay_timestamp_ms: None,
    };
    let projection_b = projection.build(inputs_b).await.expect("build b");

    assert_eq!(
        projection_a.context_window, projection_b.context_window,
        "projections must be deterministic for same inputs"
    );
    assert!(
        projection_a.context_window.contains("voice-rag"),
        "retrieved context should be in the window"
    );
    assert!(
        projection_a.context_window.contains("warped reality"),
        "the query itself should be in the window"
    );
}

#[tokio::test]
async fn soft_deleted_events_dont_reach_the_projection_window() {
    let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
    let log =
        MemoryLogService::new(MemoryLogStore::in_memory().unwrap(), "local").with_bus(bus.clone());
    // Append two events, soft-delete one.
    let kept = log
        .append(make_user_event(json!({"text": "keep this"})))
        .await
        .unwrap();
    let delete = log
        .append(make_user_event(json!({"text": "forget this"})))
        .await
        .unwrap();
    log.soft_delete(&delete.event.id, "operator-request")
        .unwrap();

    let recent = log.query_by_range(0, i64::MAX, &[], None).unwrap().events;
    // Only `keep this` survives the query.
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].id, kept.event.id);
    assert!(!recent.iter().any(|e| e.payload["text"] == "forget this"));
}
