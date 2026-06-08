//! End-to-end assistant turn test (push 3 â€” progressive disclosure).
//!
//! Under push 3 the prompt is a thin bootstrap that advertises the
//! meta-tools; facts and RAG are no longer stuffed into the prompt up
//! front. The LLM pulls them on demand via `assistant.recall_memory`
//! and `assistant.knowledge_lookup`. These
//! tests therefore assert:
//!   1. the bootstrap prompt is what goes on the wire (not a fact dump)
//!   2. facts are still reachable via the recall API (debug path)
//!   3. session history persists across turns
//!   4. a missing credential fails cleanly

use std::sync::Arc;

use ordo_assistant::{AssistantService, AssistantStore, NewFact, TurnContext, TurnRequest};
use ordo_cloud::{CloudCredentialStore, CloudCredentialTask, CloudCredentialUpdate};
use ordo_models::{EmbeddingClient, HashingEmbedder};
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

#[tokio::test]
async fn turn_grounds_on_remembered_fact_and_persists_context() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock-gpt",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Understood â€” I'll keep your brand voice terse and grounded."
                },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let credentials = credentials_for(&server.uri()).await;
    let store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(store, hasher(), credentials);

    // Teach it a fact first.
    let fact = service
        .remember_fact(NewFact {
            subject: "brand".into(),
            predicate: "avoids".into(),
            object: "marketing clichÃ©s and exclamation points".into(),
            source: "operator".into(),
            confidence: 1.0,
            scope: None,
        })
        .await
        .expect("remember fact");
    assert_eq!(fact.subject, "brand");

    // Take a turn. The router should recall the brand fact because
    // the prompt is about brand voice.
    let result = service
        .turn(TurnRequest {
            session_id: None,
            user_message: "what should our brand voice sound like?".into(),
            credential: None,
            use_rag: false, // there's no RAG peer in this test
            use_memory: true,
            use_tools: false,
            review: false,
            review_wait_secs: 300,
            stream: false, // tool use not under test here
            history_window: 6,
            fact_top_k: 5,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect("turn");

    // Push 3: pre-retrieval is gone, so `retrieved_facts` is empty.
    // The LLM would reach memory via `assistant.recall_memory`.
    assert!(
        result.retrieved_facts.is_empty(),
        "progressive disclosure: facts are not pre-loaded anymore"
    );
    assert!(result.turn.assistant_response.contains("terse"));
    assert_eq!(result.turn.credential_service.as_deref(), Some("openai"));

    // The fact *is* still recall-able via the debug path â€” which is
    // what the `assistant.recall_memory` meta-tool wraps.
    let recalled = service
        .recall("brand voice", 5)
        .await
        .expect("recall debug path");
    assert!(
        recalled.iter().any(|r| r.fact.id == fact.id),
        "brand fact should be recallable on demand"
    );

    // Inspect what was actually sent to the LLM: the bootstrap prompt
    // advertises the meta-tools without dumping the fact inline.
    let received = server.received_requests().await.expect("received list");
    assert_eq!(received.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&received[0].body).expect("request body json");
    let messages = body["messages"].as_array().expect("messages array");
    let serialized = serde_json::to_string(messages).expect("serialize");
    assert!(
        !serialized.contains("marketing clichÃ©s"),
        "fact should NOT be dumped into the prompt anymore: {serialized}"
    );
    assert!(
        serialized.contains("assistant.recall_memory"),
        "bootstrap prompt should advertise the memory meta-tool"
    );
    assert!(
        serialized.contains("brand voice"),
        "user message should appear in the prompt"
    );
}

#[tokio::test]
async fn second_turn_in_same_session_includes_prior_history() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock-gpt",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Got it." },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;
    let credentials = credentials_for(&server.uri()).await;
    let store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(store, hasher(), credentials);

    let first = service
        .turn(TurnRequest {
            session_id: None,
            user_message: "my name is Jane and I run Acme Running".into(),
            credential: None,
            use_rag: false,
            use_memory: false,
            use_tools: false,
            review: false,
            review_wait_secs: 300,
            stream: false,
            history_window: 6,
            fact_top_k: 0,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect("first turn");
    let session_id = first.session_id;

    let _second = service
        .turn(TurnRequest {
            session_id: Some(session_id),
            user_message: "remind me of my company".into(),
            credential: None,
            use_rag: false,
            use_memory: false,
            use_tools: false,
            review: false,
            review_wait_secs: 300,
            stream: false,
            history_window: 6,
            fact_top_k: 0,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect("second turn");

    let received = server.received_requests().await.expect("received list");
    assert_eq!(received.len(), 2);
    // Second request should include the first exchange in its
    // messages array so the LLM has context.
    let body: serde_json::Value = serde_json::from_slice(&received[1].body).expect("body json");
    let messages = body["messages"].as_array().expect("messages");
    let serialized = serde_json::to_string(messages).expect("serialize");
    assert!(
        serialized.contains("Jane"),
        "history should include prior turn: {serialized}"
    );
    assert!(
        serialized.contains("Acme Running"),
        "history should include prior turn content"
    );

    // And the session should now have two turns persisted.
    let turns = service.list_turns(session_id).expect("list turns");
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].index, 0);
    assert_eq!(turns[1].index, 1);
}

#[tokio::test]
async fn call_time_failover_retries_next_credential_on_5xx() {
    // Follow-up 1: when the primary LLM returns 5xx, the assistant
    // should retry with the next credential in the failover chain
    // (same turn â€” not caller-visible retry).
    use wiremock::matchers::{body_string_contains, header_exists};
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock-secondary",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "served-by-secondary"},
                "finish_reason": "stop"
            }]
        })))
        .mount(&secondary)
        .await;
    // Suppress unused warnings for the matcher helpers â€” they're
    // documented here for future callers mirroring this pattern.
    let _ = (body_string_contains("x"), header_exists("x"));

    let cred_store = CloudCredentialStore::in_memory().expect("cred store");
    let credentials = CloudCredentialTask::start(cred_store);
    credentials
        .upsert(CloudCredentialUpdate {
            service: "openai".into(), // primary (will 500)
            auth_style: Some("bearer".into()),
            secret: Some("sk-primary".into()),
            base_url: Some(format!("{}/", primary.uri())),
            ..Default::default()
        })
        .await
        .expect("upsert primary");
    credentials
        .upsert(CloudCredentialUpdate {
            service: "backup".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-backup".into()),
            base_url: Some(format!("{}/", secondary.uri())),
            ..Default::default()
        })
        .await
        .expect("upsert backup");

    let assistant_store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(assistant_store, hasher(), credentials)
        .with_failover_chain(vec!["backup".into()]);

    let result = service
        .turn(TurnRequest {
            session_id: None,
            user_message: "please answer".into(),
            credential: None,
            use_rag: false,
            use_memory: false,
            use_tools: false,
            review: false,
            review_wait_secs: 10,
            stream: false,
            history_window: 0,
            fact_top_k: 0,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect("turn should succeed via call-time failover");
    assert_eq!(result.turn.credential_service.as_deref(), Some("backup"));
    assert!(result
        .turn
        .assistant_response
        .contains("served-by-secondary"));
}

#[tokio::test]
async fn failover_chain_reaches_secondary_when_primary_credential_absent() {
    // Phase 4.5 guard: when `default_service` has no credential but
    // the failover chain does, the turn reaches the secondary. No
    // fall-through to a generic "credential missing" error.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock-gpt",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "fallback-answered"},
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    // Register the secondary credential under a non-default name.
    let cred_store = CloudCredentialStore::in_memory().expect("cred store");
    let credentials = CloudCredentialTask::start(cred_store);
    credentials
        .upsert(CloudCredentialUpdate {
            service: "secondary".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-secondary".into()),
            base_url: Some(format!("{}/", server.uri())),
            extras: Some(std::collections::HashMap::from([(
                "provider_kind".to_string(),
                "cloud_model".to_string(),
            )])),
            ..Default::default()
        })
        .await
        .expect("upsert secondary");

    let assistant_store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(assistant_store, hasher(), credentials)
        .with_failover_chain(vec!["secondary".into()]);

    let result = service
        .turn(TurnRequest {
            session_id: None,
            user_message: "hi".into(),
            credential: None, // no explicit; default is "openai" which is absent
            use_rag: false,
            use_memory: false,
            use_tools: false,
            review: false,
            review_wait_secs: 10,
            stream: false,
            history_window: 0,
            fact_top_k: 0,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect("turn should succeed via failover");
    assert_eq!(result.turn.credential_service.as_deref(), Some("secondary"));
    assert!(result.turn.assistant_response.contains("fallback-answered"));
}

#[tokio::test]
async fn turn_appends_user_message_and_chained_agent_response_to_memory_log() {
    // Integration: when a memory log is wired, every turn appends
    // one `user.message` event + one `agent.response` event chained
    // to it via `parent_id`. Proves the DPM substrate is fed on
    // every turn.
    use ordo_memory_log::{MemoryLogService, MemoryLogStore};
    use ordo_protocol::MemoryEventType;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock-gpt",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "noted."},
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let cred_store = CloudCredentialStore::in_memory().expect("cred store");
    let credentials = CloudCredentialTask::start(cred_store);
    credentials
        .upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-log-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            extras: Some(std::collections::HashMap::from([(
                "provider_kind".to_string(),
                "cloud_model".to_string(),
            )])),
            ..Default::default()
        })
        .await
        .expect("upsert credential");

    let memory_log = MemoryLogService::new(MemoryLogStore::in_memory().expect("log"), "local");
    let assistant_store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(assistant_store, hasher(), credentials)
        .with_memory_log(memory_log.clone());

    let _ = service
        .turn(TurnRequest {
            session_id: None,
            user_message: "remember me".into(),
            credential: None,
            use_rag: false,
            use_memory: false,
            use_tools: false,
            review: false,
            review_wait_secs: 10,
            stream: false,
            history_window: 0,
            fact_top_k: 0,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect("turn");

    // Both events persisted?
    let events = memory_log
        .query_by_range(0, i64::MAX, &[], None)
        .expect("query")
        .events;
    assert_eq!(events.len(), 2, "expected user.message + agent.response");

    // Order is by timestamp ASC, so user.message is first.
    let user_event = &events[0];
    let agent_event = &events[1];
    assert_eq!(user_event.event_type, MemoryEventType::UserMessage);
    assert_eq!(agent_event.event_type, MemoryEventType::AgentResponse);
    assert_eq!(user_event.actor, "operator");
    assert_eq!(agent_event.actor, "assistant");

    // Agent response is chained to the user message.
    assert_eq!(
        agent_event.parent_id.as_deref(),
        Some(user_event.id.as_str()),
        "agent.response should be parented to user.message for replay"
    );

    // Payloads carry the turn contents.
    assert_eq!(user_event.payload["text"], "remember me");
    assert_eq!(agent_event.payload["text"], "noted.");
    assert_eq!(agent_event.payload["credential_service"], "openai");
}

#[tokio::test]
async fn turn_events_share_a_turn_id_and_two_turns_have_distinct_ids() {
    // Blueprint concern 2: all events from a single turn share a
    // turn_id; subsequent turns get their own. Guards the grouping
    // primitive that intra-turn events (tool calls, router
    // decisions) will inherit once wired.
    use ordo_memory_log::{MemoryLogService, MemoryLogStore};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
        })))
        .mount(&server)
        .await;
    let cred_store = CloudCredentialStore::in_memory().unwrap();
    let credentials = CloudCredentialTask::start(cred_store);
    credentials
        .upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-x".into()),
            base_url: Some(format!("{}/", server.uri())),
            extras: Some(std::collections::HashMap::from([(
                "provider_kind".to_string(),
                "cloud_model".to_string(),
            )])),
            ..Default::default()
        })
        .await
        .unwrap();
    let log = MemoryLogService::new(MemoryLogStore::in_memory().unwrap(), "local");
    let assistant_store = AssistantStore::in_memory().unwrap();
    let service =
        AssistantService::new(assistant_store, hasher(), credentials).with_memory_log(log.clone());

    let first = service
        .turn(TurnRequest {
            session_id: None,
            user_message: "first".into(),
            credential: None,
            use_rag: false,
            use_memory: false,
            use_tools: false,
            review: false,
            review_wait_secs: 10,
            stream: false,
            history_window: 0,
            fact_top_k: 0,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect("first turn");
    let _second = service
        .turn(TurnRequest {
            session_id: Some(first.session_id),
            user_message: "second".into(),
            credential: None,
            use_rag: false,
            use_memory: false,
            use_tools: false,
            review: false,
            review_wait_secs: 10,
            stream: false,
            history_window: 0,
            fact_top_k: 0,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect("second turn");

    // Four events total, two pairs by turn_id.
    let all = log.query_by_range(0, i64::MAX, &[], None).unwrap().events;
    assert_eq!(all.len(), 4, "two turns * (user + agent) = 4 events");

    // Collect unique turn_ids.
    let mut turn_ids: std::collections::HashSet<String> =
        all.iter().filter_map(|e| e.turn_id.clone()).collect();
    assert_eq!(turn_ids.len(), 2, "two distinct turn_ids; got {turn_ids:?}");

    // For each turn_id, query_by_turn returns exactly the user+agent
    // pair for that turn.
    let first_id = turn_ids.iter().next().cloned().unwrap();
    turn_ids.remove(&first_id);
    let second_id = turn_ids.into_iter().next().unwrap();
    for turn in [&first_id, &second_id] {
        let events = log.query_by_turn(turn).unwrap();
        assert_eq!(events.len(), 2, "turn {turn} should have 2 events");
        // Both events share this turn_id.
        for e in &events {
            assert_eq!(e.turn_id.as_deref(), Some(turn.as_str()));
        }
    }
}

#[tokio::test]
async fn subagent_rejects_past_recursion_budget() {
    use ordo_assistant::MAX_SUBAGENT_DEPTH;
    // Turns arriving with depth > MAX are rejected BEFORE any LLM
    // call. Guards Phase 4.1's anti-infinite-loop contract.
    let assistant_store = AssistantStore::in_memory().expect("store");
    let cred_store = CloudCredentialStore::in_memory().expect("cred store");
    let credentials = CloudCredentialTask::start(cred_store);
    let service = AssistantService::new(assistant_store, hasher(), credentials);

    let req = TurnRequest {
        user_message: "anything".into(),
        subagent_depth: MAX_SUBAGENT_DEPTH + 1,
        ..TurnRequest::default()
    };
    let err = service.turn(req).await.expect_err("should reject");
    assert!(
        matches!(
            err,
            ordo_assistant::AssistantError::SubagentBudgetExceeded(_, _)
        ),
        "expected SubagentBudgetExceeded, got: {err}"
    );
}

#[tokio::test]
async fn spawn_subagent_errors_when_would_exceed_budget() {
    use ordo_assistant::MAX_SUBAGENT_DEPTH;
    let assistant_store = AssistantStore::in_memory().expect("store");
    let cred_store = CloudCredentialStore::in_memory().expect("cred store");
    let credentials = CloudCredentialTask::start(cred_store);
    let service = AssistantService::new(assistant_store, hasher(), credentials);

    // Parent is already at the cap â†’ child would be cap + 1.
    let err = service
        .spawn_subagent(MAX_SUBAGENT_DEPTH, "plan it".into(), Some(1))
        .await
        .expect_err("should reject");
    assert!(matches!(
        err,
        ordo_assistant::AssistantError::SubagentBudgetExceeded(_, _)
    ));
}

#[tokio::test]
async fn diagnostic_mode_denies_cloud_models_by_default() {
    let server = MockServer::start().await;
    let store = CloudCredentialStore::in_memory().expect("store");
    let credentials = CloudCredentialTask::start(store);
    credentials
        .upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            extras: Some(std::collections::HashMap::from([(
                "provider_kind".to_string(),
                "cloud_model".to_string(),
            )])),
            ..Default::default()
        })
        .await
        .expect("upsert credential");
    let assistant_store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(assistant_store, hasher(), credentials)
        .with_modes(ordo_modes::ModeRegistry::from_defaults().expect("modes"));

    let err = service
        .turn(TurnRequest {
            user_message: "diagnose provider".into(),
            credential: Some("openai".into()),
            use_rag: false,
            use_memory: false,
            use_tools: false,
            stream: false,
            mode: Some("diagnostic".into()),
            ..TurnRequest::default()
        })
        .await
        .expect_err("diagnostic should deny cloud by default");
    let msg = err.to_string();
    assert!(
        msg.contains("diagnostic mode can only use local model credentials"),
        "got: {msg}"
    );
}

#[tokio::test]
async fn diagnostic_mode_can_use_cloud_when_operator_allows_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mock-gpt",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Diagnostic cloud turn accepted." },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let store = CloudCredentialStore::in_memory().expect("store");
    let credentials = CloudCredentialTask::start(store);
    credentials
        .upsert(CloudCredentialUpdate {
            service: "openai".into(),
            auth_style: Some("bearer".into()),
            secret: Some("sk-test".into()),
            base_url: Some(format!("{}/", server.uri())),
            extras: Some(std::collections::HashMap::from([(
                "provider_kind".to_string(),
                "cloud_model".to_string(),
            )])),
            ..Default::default()
        })
        .await
        .expect("upsert credential");
    let assistant_store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(assistant_store, hasher(), credentials)
        .with_modes(ordo_modes::ModeRegistry::from_defaults().expect("modes"));

    let result = service
        .turn(TurnRequest {
            user_message: "diagnose provider".into(),
            credential: Some("openai".into()),
            use_rag: false,
            use_memory: false,
            use_tools: false,
            stream: false,
            mode: Some("diagnostic".into()),
            metadata: std::collections::HashMap::from([(
                "diagnostic".to_string(),
                json!({ "allow_cloud_models": true }),
            )]),
            ..TurnRequest::default()
        })
        .await
        .expect("diagnostic cloud should be allowed by explicit operator metadata");
    assert!(result
        .turn
        .assistant_response
        .contains("Diagnostic cloud turn accepted."));
}

#[tokio::test]
async fn denied_turn_returns_error_when_no_credential_configured() {
    let store = CloudCredentialStore::in_memory().expect("store");
    let credentials = CloudCredentialTask::start(store); // empty
    let assistant_store = AssistantStore::in_memory().expect("store");
    let service = AssistantService::new(assistant_store, hasher(), credentials);
    let err = service
        .turn(TurnRequest {
            session_id: None,
            user_message: "hello".into(),
            credential: None,
            use_rag: false,
            use_memory: false,
            use_tools: false,
            review: false,
            review_wait_secs: 300,
            stream: false,
            history_window: 0,
            fact_top_k: 0,
            rag_top_k: 0,
            metadata: Default::default(),
            attachments: Default::default(),
            subagent_depth: 0,
            mode: None,
        })
        .await
        .expect_err("should fail");
    let msg = err.to_string();
    assert!(msg.contains("credential"), "got: {msg}");
}

#[test]
fn recalled_facts_round_trip_through_turn_context() {
    // Sanity-check the context shape (this is serialized into SQLite
    // and later read by the studio).
    let ctx = TurnContext {
        facts: vec![],
        rag_hits: vec![],
        tool_calls: vec![],
        history_window: 0,
    };
    let json = serde_json::to_string(&ctx).unwrap();
    let parsed: TurnContext = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.history_window, 0);
}
