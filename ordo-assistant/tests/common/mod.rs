//! Shared eval harness for ordo-assistant integration tests.
//!
//! Minimal on purpose (Phase 0.6). Shape:
//!   - `Scenario` â€” declarative: facts, mock LLM responses, turns.
//!   - `RunResult::assert_trace` â€” light-touch structural asserts on
//!     what happened, not byte-for-byte equality (LLM outputs are
//!     already deterministic because we script them, but tool-call
//!     shapes and prompt contents are what we actually care about).
//!
//! This exists to guard architectural invariants (Rule 8 especially:
//! the prompt does not pre-stuff retrieved context) across future
//! phases. Grow it; don't re-invent it.
//!
//! See `docs/architecture-contract.md`.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ordo_assistant::{AssistantService, AssistantStore, NewFact, TurnRequest, TurnResult};
use ordo_cloud::{CloudCredentialStore, CloudCredentialTask, CloudCredentialUpdate};
use ordo_models::{EmbeddingClient, HashingEmbedder};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Serves a fixed queue of LLM responses. Each POST to
/// `/chat/completions` pops the next entry.
pub struct ScriptedLlm {
    queue: Arc<Mutex<VecDeque<Value>>>,
}

impl Respond for ScriptedLlm {
    fn respond(&self, _req: &Request) -> ResponseTemplate {
        let mut queue = self.queue.lock().expect("eval queue lock");
        let body = queue.pop_front().unwrap_or_else(|| {
            json!({
                "error": "scripted LLM exhausted â€” add more responses to the scenario",
            })
        });
        ResponseTemplate::new(200).set_body_json(body)
    }
}

/// Convenience: OpenAI-shaped text completion.
pub fn openai_text(content: &str) -> Value {
    json!({
        "model": "mock-gpt",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }]
    })
}

/// Default TurnRequest with everything disabled. Enable what each
/// scenario actually exercises â€” keeps the prompt under test small.
pub fn bare_turn(user_message: &str) -> TurnRequest {
    TurnRequest {
        session_id: None,
        user_message: user_message.into(),
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
        ..Default::default()
    }
}

pub struct Scenario {
    pub name: &'static str,
    pub facts: Vec<NewFact>,
    pub llm_responses: Vec<Value>,
    pub turns: Vec<TurnRequest>,
}

pub struct RunResult {
    pub turns: Vec<TurnResult>,
    pub llm_requests: Vec<Value>,
}

impl Scenario {
    pub async fn run(self) -> RunResult {
        let server = MockServer::start().await;
        let queue = Arc::new(Mutex::new(VecDeque::from(self.llm_responses)));
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ScriptedLlm {
                queue: queue.clone(),
            })
            .mount(&server)
            .await;

        let cred_store = CloudCredentialStore::in_memory().expect("cred store");
        let credentials = CloudCredentialTask::start(cred_store);
        credentials
            .upsert(CloudCredentialUpdate {
                service: "openai".into(),
                auth_style: Some("bearer".into()),
                secret: Some("sk-eval".into()),
                base_url: Some(format!("{}/", server.uri())),
                ..Default::default()
            })
            .await
            .expect("upsert eval credential");

        let hasher: Arc<dyn EmbeddingClient> = Arc::new(HashingEmbedder::new(96));
        let store = AssistantStore::in_memory().expect("assistant store");
        let service = AssistantService::new(store, hasher, credentials);

        for fact in self.facts {
            service
                .remember_fact(fact)
                .await
                .expect("remember eval fact");
        }

        let mut results = Vec::new();
        for turn in self.turns {
            results.push(service.turn(turn).await.expect("eval turn"));
        }

        let captured = server.received_requests().await.expect("wiremock received");
        let llm_requests = captured
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap_or(Value::Null))
            .collect();

        RunResult {
            turns: results,
            llm_requests,
        }
    }
}

/// Lightweight assertion sheet â€” grows as new phases add signals to
/// track (token streaming, tool traces, cache hits, subagent spawns).
#[derive(Default)]
pub struct Expect {
    pub response_contains: Vec<&'static str>,
    pub response_does_not_contain: Vec<&'static str>,
    pub prompt_contains: Vec<&'static str>,
    pub prompt_does_not_contain: Vec<&'static str>,
}

impl RunResult {
    pub fn assert_trace(&self, turn_idx: usize, expect: &Expect) {
        let turn = self
            .turns
            .get(turn_idx)
            .unwrap_or_else(|| panic!("turn {turn_idx} missing"));
        let response = &turn.turn.assistant_response;
        for needle in &expect.response_contains {
            assert!(
                response.contains(needle),
                "turn {turn_idx}: response missing '{needle}' â€” got: {response}"
            );
        }
        for needle in &expect.response_does_not_contain {
            assert!(
                !response.contains(needle),
                "turn {turn_idx}: response unexpectedly contained '{needle}' â€” got: {response}"
            );
        }
        let req = self
            .llm_requests
            .get(turn_idx)
            .unwrap_or_else(|| panic!("llm request {turn_idx} missing"));
        let prompt = serde_json::to_string(req).unwrap_or_default();
        for needle in &expect.prompt_contains {
            assert!(
                prompt.contains(needle),
                "turn {turn_idx}: prompt missing '{needle}'"
            );
        }
        for needle in &expect.prompt_does_not_contain {
            assert!(
                !prompt.contains(needle),
                "turn {turn_idx}: prompt unexpectedly contained '{needle}' \
                 (Rule 8 violation? â€” see docs/architecture-contract.md)"
            );
        }
    }
}
