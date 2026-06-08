//! Tests for the push-4 knowledge seeder + parallel_lookup meta-tool.
//!
//! We don't spin up a real bus here â€” instead we populate the
//! knowledge store directly (bypassing the capability inventory hop)
//! and verify the pieces that matter:
//!   - re-seeding is idempotent via `upsert_by_source`
//!   - domain blurbs land under the right domain
//!   - `recall` scoped by domain finds the seeded content
//!
//! A bus-backed end-to-end test is cheap to add later but would
//! duplicate what `ordo-runtime`'s integration tests already cover.

use std::sync::Arc;

use ordo_assistant::{AssistantStore, KnowledgeKind, KnowledgeStore, NewKnowledge};
use ordo_models::{EmbeddingClient, HashingEmbedder};
use parking_lot::Mutex;

fn knowledge_store() -> KnowledgeStore {
    let store = AssistantStore::in_memory().expect("store");
    let store = Arc::new(Mutex::new(store));
    let embedder: Arc<dyn EmbeddingClient> = Arc::new(HashingEmbedder::new(96));
    KnowledgeStore::new(store, embedder)
}

#[tokio::test]
async fn upsert_by_source_is_idempotent() {
    let knowledge = knowledge_store();
    let source = "auto:capability:creative.capture_brief".to_string();

    let first = knowledge
        .upsert_by_source(NewKnowledge {
            kind: KnowledgeKind::Skill,
            domain: Some("creative".into()),
            title: "creative.capture_brief".into(),
            body: "initial body".into(),
            source: source.clone(),
            confidence: 0.6,
        })
        .await
        .expect("first upsert");

    let second = knowledge
        .upsert_by_source(NewKnowledge {
            kind: KnowledgeKind::Skill,
            domain: Some("creative".into()),
            title: "creative.capture_brief".into(),
            body: "updated body".into(),
            source: source.clone(),
            confidence: 0.8,
        })
        .await
        .expect("second upsert");

    // Same id â€” it was updated in place, not duplicated.
    assert_eq!(first.id, second.id);
    assert_eq!(second.body, "updated body");

    // And there's exactly one row.
    let all = knowledge.list(None, None).expect("list");
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn domain_scoped_recall_only_returns_matching_domain() {
    let knowledge = knowledge_store();
    for (domain, title, body) in [
        ("creative", "brief intake", "how to capture briefs"),
        ("workflow", "approval flow", "how to route approvals"),
    ] {
        knowledge
            .upsert_by_source(NewKnowledge {
                kind: KnowledgeKind::Note,
                domain: Some(domain.into()),
                title: title.into(),
                body: body.into(),
                source: format!("auto:domain:{domain}"),
                confidence: 0.7,
            })
            .await
            .expect("upsert");
    }

    let creative_hits = knowledge
        .recall("brief", 5, None, Some("creative"))
        .await
        .expect("recall");
    assert!(
        creative_hits
            .iter()
            .all(|h| h.entry.domain.as_deref() == Some("creative")),
        "domain scoping should exclude other domains"
    );

    let workflow_hits = knowledge
        .recall("approval", 5, None, Some("workflow"))
        .await
        .expect("recall");
    assert!(
        workflow_hits
            .iter()
            .any(|h| h.entry.title == "approval flow"),
        "workflow lookup should find its own entry"
    );
}
