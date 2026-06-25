//! Fact auto-extractor â€” mines durable facts from recent turns.
//!
//! Runs periodically in the background. For each recent turn that
//! hasn't already been processed, it asks the LLM whether there's
//! anything lasting worth remembering ("Jane prefers terse copy,"
//! "client Acme never wants exclamation points") and upserts the
//! results into the fact store with `source: "auto:<session>:<turn>"`
//! and a lower starting confidence than operator-entered facts.
//!
//! The extractor is *conservative by construction*: the system
//! prompt asks for strict JSON output and only keeps entries whose
//! subject/predicate/object are all non-empty strings. Anything
//! ambiguous is dropped.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ordo_cloud::{CloudCredentialTask, CloudHttp};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::json;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::recall::FactStore;
use crate::store::AssistantStore;
use crate::types::{AssistantError, AssistantResult, NewFact};

pub struct AutoExtractor {
    store: Arc<Mutex<AssistantStore>>,
    facts: FactStore,
    credentials: CloudCredentialTask,
    http: CloudHttp,
    default_service: String,
    /// In-process memory of turn ids we've already processed â€” avoids
    /// re-extracting on every poll. Survives process lifetime only;
    /// a restart would re-extract but idempotency is guaranteed by
    /// the `source` tag (duplicates show up with a bumped count, not
    /// conflicting rows).
    seen_turns: Arc<Mutex<HashSet<uuid::Uuid>>>,
}

impl AutoExtractor {
    pub fn new(
        store: Arc<Mutex<AssistantStore>>,
        facts: FactStore,
        credentials: CloudCredentialTask,
        http: CloudHttp,
        default_service: String,
    ) -> Self {
        Self {
            store,
            facts,
            credentials,
            http,
            default_service,
            seen_turns: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Run the extractor forever. Intended to be spawned as a
    /// background component by the runtime.
    pub async fn run(self, interval: Duration) {
        loop {
            let started = Instant::now();
            if let Err(err) = self.run_once().await {
                debug!(
                    target: "ordo_assistant::extractor",
                    error = %err,
                    "extractor pass failed; will retry"
                );
            }
            let elapsed = started.elapsed();
            if elapsed < interval {
                sleep(interval - elapsed).await;
            }
        }
    }

    /// One pass: grab recent turns we haven't seen, extract facts
    /// from each, upsert results. Returns the number of facts
    /// extracted.
    pub async fn run_once(&self) -> AssistantResult<usize> {
        let recent_sessions = self.store.lock().list_sessions(20)?;
        let mut added = 0usize;
        for session in recent_sessions {
            let turns = self.store.lock().list_turns(session.id)?;
            for turn in turns {
                let seen = {
                    let mut set = self.seen_turns.lock();
                    !set.insert(turn.id)
                };
                if seen {
                    continue;
                }
                match self.extract_for_turn(&turn).await {
                    Ok(count) => added = added.saturating_add(count),
                    Err(err) => warn!(
                        target: "ordo_assistant::extractor",
                        turn_id = %turn.id,
                        error = %err,
                        "failed to extract facts from turn"
                    ),
                }
            }
        }
        Ok(added)
    }

    /// Resolve a usable cloud credential the SAME way the chat / speech
    /// paths do (`AssistantService::speak_text`): the operator's default
    /// credential, then the configured `default_service`, then any
    /// configured credential — instead of a single hardcoded-name lookup.
    ///
    /// This is the fix for the asymmetry where chatting worked on a
    /// non-OpenAI setup (e.g. Ollama Cloud) but fact extraction failed
    /// every turn with `NoCredential("openai")`: the chat loop already
    /// fell back to the default credential, the extractor did not.
    async fn resolve_credential(&self) -> AssistantResult<ordo_cloud::CloudCredential> {
        resolve_credential_from(&self.credentials, &self.default_service).await
    }

    async fn extract_for_turn(&self, turn: &crate::types::Turn) -> AssistantResult<usize> {
        // Don't spend LLM budget on trivial exchanges.
        if turn.user_message.len() < 12 && turn.assistant_response.len() < 12 {
            return Ok(0);
        }
        let credential = self.resolve_credential().await?;

        let messages = json!([
            {
                "role": "system",
                "content": "You mine durable, reusable facts about an operator, their clients, brand, or projects from a single chat exchange. Return STRICT JSON in the shape {\"facts\": [{\"subject\": \"user\"|\"brand\"|\"client:<name>\"|\"project:<slug>\", \"predicate\": \"prefers\"|\"avoids\"|\"location\"|\"role\"|\"fact\", \"object\": \"â€¦\"}]}. If nothing is worth remembering, return {\"facts\": []}. Do NOT invent details. Do NOT extract transient context like 'is drafting X right now'."
            },
            {
                "role": "user",
                "content": format!(
                    "Operator said:\n{}\n\nAssistant replied:\n{}\n\nExtract durable facts (or none).",
                    turn.user_message, turn.assistant_response
                )
            }
        ]);
        let mut chat_args = json!({
            "messages": messages,
            "temperature": 0.0,
        });
        // Honor a per-credential model override (extras.model). Provider-
        // neutral: lets local OpenAI-compatible servers (Ollama, LM Studio)
        // route to whichever model is loaded.
        if let Some(model) = credential.extras.get("model") {
            chat_args["model"] = json!(model);
        }
        let response = if credential.auth_style == "anthropic" {
            ordo_cloud::anthropic::messages(&self.http, &credential, &chat_args)
                .await
                .map_err(|err| AssistantError::LlmFailed(err.to_string()))?
        } else {
            ordo_cloud::openai::chat(&self.http, &credential, &chat_args)
                .await
                .map_err(|err| AssistantError::LlmFailed(err.to_string()))?
        };
        let text = response
            .get("assistant_message")
            .and_then(|v| v.as_str())
            .or_else(|| response.get("assistant_text").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let parsed: ExtractorResponse = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => {
                // Try to salvage JSON hiding inside markdown fences.
                if let Some(json_text) = first_json_object(&text) {
                    serde_json::from_str(json_text).unwrap_or_default()
                } else {
                    ExtractorResponse::default()
                }
            }
        };
        let mut added = 0usize;
        for candidate in parsed.facts {
            if candidate.subject.trim().is_empty()
                || candidate.predicate.trim().is_empty()
                || candidate.object.trim().is_empty()
            {
                continue;
            }
            let new_fact = NewFact {
                subject: candidate.subject.trim().to_string(),
                predicate: candidate.predicate.trim().to_string(),
                object: candidate.object.trim().to_string(),
                source: format!("auto:{}:{}", turn.session_id, turn.id),
                confidence: 0.5,
                // Auto-extracted facts go to global by default. The
                // operator can move them to a tighter scope manually
                // (or a future mode-aware extractor can read the
                // session's mode and tag them with `mode:<id>`).
                scope: None,
            };
            match self.facts.remember(new_fact).await {
                Ok(_) => added += 1,
                Err(err) => warn!(
                    target: "ordo_assistant::extractor",
                    error = %err,
                    "could not persist extracted fact"
                ),
            }
        }
        if added > 0 {
            debug!(
                target: "ordo_assistant::extractor",
                turn_id = %turn.id,
                added,
                "extracted durable facts"
            );
        }
        Ok(added)
    }
}

#[derive(Debug, Default, Deserialize)]
struct ExtractorResponse {
    #[serde(default)]
    facts: Vec<CandidateFact>,
}

#[derive(Debug, Deserialize)]
struct CandidateFact {
    subject: String,
    predicate: String,
    object: String,
}

fn first_json_object(input: &str) -> Option<&str> {
    let start = input.find('{')?;
    // Walk forward counting braces to find the matching close.
    let bytes = input.as_bytes();
    let mut depth = 0i32;
    for (idx, &b) in bytes[start..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&input[start..start + idx + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Resolve a usable credential from the store, preferring the operator's
/// default credential (`get_default()`), then `default_service`, then any
/// configured credential. A free function (not a method) so it can be
/// tested against a real `CloudCredentialTask` without standing up a full
/// `AutoExtractor`.
async fn resolve_credential_from(
    credentials: &CloudCredentialTask,
    default_service: &str,
) -> AssistantResult<ordo_cloud::CloudCredential> {
    let default_name = credentials.get_default().await.ok().flatten();
    let all = credentials
        .list()
        .await
        .map(|creds| creds.into_iter().map(|cred| cred.service).collect())
        .unwrap_or_default();
    for name in candidate_service_names(default_name, default_service, all) {
        if let Ok(Some(credential)) = credentials.get(name).await {
            return Ok(credential);
        }
    }
    Err(AssistantError::NoCredential(default_service.to_string()))
}

/// Ordered, de-duplicated credential names to try when resolving a
/// credential for extraction: the operator's default credential first,
/// then the configured `default_service`, then every other configured
/// credential. Empty names are dropped. Mirrors the chat/speech
/// resolution order so extraction succeeds wherever chatting does.
fn candidate_service_names(
    default_name: Option<String>,
    default_service: &str,
    all: Vec<String>,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(name) = default_name {
        names.push(name);
    }
    names.push(default_service.to_string());
    names.extend(all);

    let mut seen = HashSet::new();
    names.retain(|name| !name.trim().is_empty() && seen.insert(name.clone()));
    names
}

#[cfg(test)]
mod tests {
    use super::{candidate_service_names, resolve_credential_from};
    use ordo_cloud::{CloudCredentialStore, CloudCredentialTask, CloudCredentialUpdate};

    /// The exact failing setup: only an Ollama-compatible credential exists
    /// (no "openai"), and it is the operator default. Extraction must resolve
    /// THROUGH that default instead of erroring on the literal "openai".
    #[tokio::test]
    async fn extraction_uses_default_credential_when_no_openai() {
        let store = CloudCredentialStore::in_memory().expect("store");
        let credentials = CloudCredentialTask::start(store);
        credentials
            .upsert(CloudCredentialUpdate {
                service: "ollama-cloud-api".into(),
                secret: Some("dummy-key".into()),
                base_url: Some("https://ollama.com/v1".into()),
                auth_style: Some("bearer".into()),
                ..Default::default()
            })
            .await
            .expect("upsert credential");
        credentials
            .set_default(Some("ollama-cloud-api".into()))
            .await
            .expect("set default");

        // `default_service` is the legacy literal "openai", which does NOT exist.
        let credential = resolve_credential_from(&credentials, "openai")
            .await
            .expect("extraction should resolve via the default credential");
        assert_eq!(credential.service, "ollama-cloud-api");
    }

    /// With no credentials at all, the error still names the configured
    /// `default_service` so the operator gets an actionable message.
    #[tokio::test]
    async fn extraction_errors_clearly_when_no_credentials() {
        let store = CloudCredentialStore::in_memory().expect("store");
        let credentials = CloudCredentialTask::start(store);
        let err = resolve_credential_from(&credentials, "openai")
            .await
            .expect_err("no credentials -> error");
        assert!(format!("{err}").contains("openai"), "got: {err}");
    }

    #[test]
    fn operator_default_wins_over_legacy_default_service() {
        // The classic failing setup: default_service is the legacy
        // "openai" that doesn't exist, but the operator's default
        // credential is Ollama Cloud. Ollama must be tried FIRST.
        let names = candidate_service_names(
            Some("ollama-cloud-api".into()),
            "openai",
            vec!["ollama-cloud-api".into(), "anthropic-main".into()],
        );
        assert_eq!(
            names,
            vec![
                "ollama-cloud-api".to_string(),
                "openai".to_string(),
                "anthropic-main".to_string()
            ]
        );
    }

    #[test]
    fn falls_back_to_listed_credentials_when_no_default() {
        let names = candidate_service_names(None, "openai", vec!["ollama-cloud-api".into()]);
        assert_eq!(
            names,
            vec!["openai".to_string(), "ollama-cloud-api".to_string()]
        );
    }

    #[test]
    fn empty_names_are_dropped() {
        let names = candidate_service_names(
            Some(String::new()),
            "openai",
            vec![String::new(), "x".into()],
        );
        assert_eq!(names, vec!["openai".to_string(), "x".to_string()]);
    }
}
