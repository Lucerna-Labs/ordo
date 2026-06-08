//! Push 5 â€” end-to-end test for the `review: true` turn flag.
//!
//! Verifies the three interesting paths:
//!   1. Approve â†’ the original draft is persisted verbatim and the
//!      turn records a `ReviewOutcome { state: "approved" }`.
//!   2. Edit   â†’ the operator's edited text replaces the draft and
//!      the outcome says `edited_and_approved`.
//!   3. Deny   â†’ the persisted response is a denial note; the outcome
//!      captures the operator's note.

use std::sync::Arc;
use std::time::Duration;

use ordo_assistant::{AssistantService, AssistantStore, TurnRequest};
use ordo_cloud::{CloudCredentialStore, CloudCredentialTask, CloudCredentialUpdate};
use ordo_models::{EmbeddingClient, HashingEmbedder};
use ordo_review::{ReviewDecisionKind, ReviewService, ReviewStore};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn hasher() -> Arc<dyn EmbeddingClient> {
    Arc::new(HashingEmbedder::new(96))
}

async fn credentials_for(base_url: &str) -> CloudCredentialTask {
    let store = CloudCredentialStore::in_memory().expect("store");
    let task = CloudCredentialTask::start(store);
    task.upsert(CloudCredentialUpdate {
        service: "openai".into(),
        auth_style: Some("bearer".into()),
        secret: Some("sk-test".into()),
        base_url: Some(format!("{}/", base_url)),
        ..Default::default()
    })
    .await
    .expect("upsert credential");
    task
}

async fn make_service(draft: &str) -> (AssistantService, ReviewService, MockServer) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock-gpt",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": draft },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;
    let credentials = credentials_for(&server.uri()).await;
    let review = ReviewService::new(ReviewStore::in_memory().expect("review store"));
    let store = AssistantStore::in_memory().expect("assistant store");
    let service = AssistantService::new(store, hasher(), credentials).with_review(review.clone());
    (service, review, server)
}

fn review_turn_request(message: &str) -> TurnRequest {
    TurnRequest {
        session_id: None,
        user_message: message.into(),
        credential: None,
        use_rag: false,
        use_memory: false,
        use_tools: false,
        review: true,
        review_wait_secs: 10,
        stream: false,
        history_window: 0,
        fact_top_k: 0,
        rag_top_k: 0,
        metadata: Default::default(),
        attachments: Default::default(),
        subagent_depth: 0,
        mode: None,
        ..Default::default()
    }
}

/// Spawn the turn in the background and drain the pending queue until
/// the submitted request appears â€” then apply `decision`. This mirrors
/// what the studio's review UI does.
async fn resolve_next_pending(review: ReviewService, decision: ReviewDecisionKind) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pending) = review.pending().expect("pending").first().cloned() {
            review.decide(pending.id, decision).expect("decide");
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("review request never appeared on the queue");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn review_approve_persists_original_draft() {
    let (service, review, _server) = make_service("Original draft.").await;
    let service_clone = service.clone();

    let approver = tokio::spawn(async move {
        resolve_next_pending(review, ReviewDecisionKind::Approve { note: None }).await;
    });
    let result = service_clone
        .turn(review_turn_request("draft something"))
        .await
        .expect("turn");
    approver.await.expect("approver");

    assert_eq!(result.turn.assistant_response, "Original draft.");
    let outcome = result.review_outcome.expect("outcome");
    assert_eq!(outcome.state, "approved");
    assert_eq!(outcome.delivered_content, "Original draft.");
}

#[tokio::test]
async fn review_edit_replaces_draft_with_operator_text() {
    let (service, review, _server) = make_service("Rough draft.").await;
    let service_clone = service.clone();

    let editor = tokio::spawn(async move {
        resolve_next_pending(
            review,
            ReviewDecisionKind::Edit {
                content: "Polished final.".into(),
                note: Some("tightened".into()),
            },
        )
        .await;
    });
    let result = service_clone
        .turn(review_turn_request("draft something"))
        .await
        .expect("turn");
    editor.await.expect("editor");

    assert_eq!(result.turn.assistant_response, "Polished final.");
    let outcome = result.review_outcome.expect("outcome");
    assert_eq!(outcome.state, "edited_and_approved");
    assert_eq!(outcome.note.as_deref(), Some("tightened"));
}

#[tokio::test]
async fn review_deny_persists_denial_note() {
    let (service, review, _server) = make_service("Sketchy draft.").await;
    let service_clone = service.clone();

    let denier = tokio::spawn(async move {
        resolve_next_pending(
            review,
            ReviewDecisionKind::Deny {
                note: Some("off-brand".into()),
            },
        )
        .await;
    });
    let result = service_clone
        .turn(review_turn_request("draft something"))
        .await
        .expect("turn");
    denier.await.expect("denier");

    assert!(
        result.turn.assistant_response.starts_with("[draft denied"),
        "denied response should start with the marker: {}",
        result.turn.assistant_response
    );
    assert!(result.turn.assistant_response.contains("off-brand"));
    let outcome = result.review_outcome.expect("outcome");
    assert_eq!(outcome.state, "denied");
}

#[tokio::test]
async fn review_without_service_fails_cleanly() {
    // No `.with_review(...)` on the assistant.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock-gpt",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "doesn't matter" },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;
    let credentials = credentials_for(&server.uri()).await;
    let store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(store, hasher(), credentials);

    let err = service
        .turn(review_turn_request("draft something"))
        .await
        .expect_err("should fail without a review service");
    assert!(err.to_string().contains("review service"));
}
