//! Push 6 â€” token-level streaming test.
//!
//! Wires wiremock to respond with a synthetic SSE body (a few `data:
//! {...}\n\n` frames ending with `data: [DONE]\n\n`) and checks that
//! the assistant (a) assembles the tokens into `assistant_response`
//! and (b) emits a `TokenDelta` event per chunk so the studio can
//! render live typing.

use std::sync::Arc;
use std::time::Duration;

use ordo_assistant::{AssistantService, AssistantStore, TurnEvent, TurnRequest};
use ordo_cloud::{CloudCredentialStore, CloudCredentialTask, CloudCredentialUpdate};
use ordo_models::{EmbeddingClient, HashingEmbedder};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn hasher() -> Arc<dyn EmbeddingClient> {
    Arc::new(HashingEmbedder::new(96))
}

fn sse_body(chunks: &[&str]) -> String {
    let mut buf = String::new();
    for chunk in chunks {
        let frame = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": { "content": *chunk },
                "finish_reason": serde_json::Value::Null
            }]
        });
        buf.push_str(&format!("data: {}\n\n", frame));
    }
    // final frame with finish_reason + DONE terminator
    let stop = serde_json::json!({
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }]
    });
    buf.push_str(&format!("data: {}\n\n", stop));
    buf.push_str("data: [DONE]\n\n");
    buf
}

#[tokio::test]
async fn streaming_turn_emits_token_deltas_and_assembles_response() {
    let server = MockServer::start().await;
    let chunks = ["Hello", ", ", "world", "!"];
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&chunks)),
        )
        .mount(&server)
        .await;

    let cred_store = CloudCredentialStore::in_memory().expect("store");
    let credentials = CloudCredentialTask::start(cred_store);
    credentials
        .upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            ..Default::default()
        })
        .await
        .expect("upsert");

    let store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(store, hasher(), credentials);

    // Create the session up-front so we can subscribe to its events
    // before the turn fires.
    let session = service.new_session(None, None).expect("new session");
    let mut rx = service.events().subscribe(session.id);

    let service_clone = service.clone();
    let session_id = session.id;
    let turn_handle = tokio::spawn(async move {
        service_clone
            .turn(TurnRequest {
                session_id: Some(session_id),
                user_message: "say hi".into(),
                credential: None,
                use_rag: false,
                use_memory: false,
                use_tools: false,
                review: false,
                review_wait_secs: 30,
                stream: true,
                history_window: 0,
                fact_top_k: 0,
                rag_top_k: 0,
                metadata: Default::default(),
                attachments: Default::default(),
                subagent_depth: 0,
                mode: None,
            })
            .await
    });

    // Collect events until the turn completes. Bound the wait so a
    // broken implementation doesn't hang CI.
    let mut deltas: Vec<String> = Vec::new();
    let mut saw_completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_completed {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Ok(TurnEvent::TokenDelta { delta, .. })) => deltas.push(delta),
            Ok(Ok(TurnEvent::TurnCompleted { .. })) => saw_completed = true,
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }

    let result = turn_handle.await.expect("spawn").expect("turn ok");

    assert!(saw_completed, "should have seen a TurnCompleted event");
    assert_eq!(
        deltas.join(""),
        "Hello, world!",
        "token deltas should concatenate to the full reply"
    );
    assert_eq!(result.turn.assistant_response, "Hello, world!");
}
