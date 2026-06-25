use crate::*;
use crate::helpers::*;

/// LLM-backed variant of the orchestration lane.
///
/// This provider bridges deterministic `OrdoOpsProvider` calls and the
/// `cloud.*` lane for operator-reviewed orchestration notes. The capability
/// is opt-in and degrades gracefully: if no cloud credential is configured,
/// the call returns a structured error rather than panicking.
///
/// Capability:
/// - `orchestration.draft_notes` - drafts reviewer notes or revision rationales
pub struct OrdoLlmProvider {
    credentials: ordo_cloud::CloudCredentialTask,
    http: ordo_cloud::CloudHttp,
    default_service: String,
    /// When set, the provider will hydrate the LLM prompt with RAG
    /// snippets taken from the local retrieval lane. This turns generic
    /// LLM output into operator-consistent output without any caller
    /// coordination.
    bus: Option<Arc<dyn Bus>>,
    rag_top_k: usize,
    /// When set, calls with `review: true` queue the draft for operator
    /// approval and block until a decision arrives. Denied drafts are
    /// returned to the agent as a `Failed` result; edits are
    /// transparently substituted into the response.
    review: Option<ordo_review::ReviewService>,
    /// Maximum time to wait for the operator before expiring the
    /// request. Zero = block forever (not recommended).
    review_wait: std::time::Duration,
}

impl OrdoLlmProvider {
    /// Build an LLM provider that talks to a configured cloud service. The
    /// default service name is `openai` but individual calls can override
    /// it via the `credential` argument, matching the `cloud.*` pattern.
    pub fn new(credentials: ordo_cloud::CloudCredentialTask) -> Self {
        Self {
            credentials,
            http: ordo_cloud::CloudHttp::new(),
            default_service: "openai".to_string(),
            bus: None,
            rag_top_k: 3,
            review: None,
            review_wait: std::time::Duration::from_secs(300),
        }
    }

    pub fn with_default_service(mut self, service: impl Into<String>) -> Self {
        self.default_service = service.into();
        self
    }

    /// Enable human-in-the-loop review. When the caller sets
    /// `review: true`, the provider queues the draft and blocks until
    /// the operator approves / edits / denies.
    pub fn with_review(mut self, service: ordo_review::ReviewService) -> Self {
        self.review = Some(service);
        self
    }

    pub fn with_review_wait(mut self, wait: std::time::Duration) -> Self {
        self.review_wait = wait;
        self
    }

    pub fn with_http(mut self, http: ordo_cloud::CloudHttp) -> Self {
        self.http = http;
        self
    }

    /// Enable automatic RAG context injection. Every call to
    /// `orchestration.draft_notes` will pre-query the local retrieval lane
    /// using the caller-supplied `rag_query`
    /// (falling back to the prompt itself) and prepend the top-K
    /// snippets to the system message.
    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_rag_top_k(mut self, top_k: usize) -> Self {
        self.rag_top_k = top_k;
        self
    }
}

const ORCHESTRATION_DRAFT_NOTES: &str = "orchestration.draft_notes";

const ORDO_LLM_CAPABILITIES: &[&str] = &[ORCHESTRATION_DRAFT_NOTES];

fn planning_llm_description(capability: &str) -> &'static str {
    match capability {
        ORCHESTRATION_DRAFT_NOTES => {
            "Drafts reviewer notes or revision rationale using a configured cloud LLM credential."
        }
        _ => "Ordo LLM capability.",
    }
}

fn planning_llm_system_prompt(capability: &str) -> &'static str {
    match capability {
        ORCHESTRATION_DRAFT_NOTES => {
            "You are Ordo's orchestration reviewer. Draft clear, kind, \
             specific reviewer notes or revision rationale. Cite the brief \
             fields you are responding to. Keep it short."
        }
        _ => "You are a helpful assistant.",
    }
}

async fn run_planning_llm_call(
    provider: &OrdoLlmProvider,
    capability: &str,
    arguments: &Value,
) -> Option<ToolCallResult> {
    let service = arguments
        .get("credential")
        .and_then(|value| value.as_str())
        .unwrap_or(&provider.default_service)
        .to_string();

    let credential = match provider.credentials.get(service.clone()).await {
        Ok(Some(credential)) => credential,
        Ok(None) => {
            return Some(ToolCallResult::Failed {
                error: format!(
                    "credential for service '{service}' is not configured; \
                     call cloud.credentials.upsert first to enable {capability}"
                ),
            });
        }
        Err(err) => {
            return Some(ToolCallResult::Failed {
                error: err.to_string(),
            });
        }
    };

    // Build a chat-style prompt. We forward any caller-supplied
    // arguments as the user message body so downstream prompts can
    // pass structured data (briefs, records, etc.) through unchanged.
    let prompt = arguments
        .get("prompt")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(arguments).unwrap_or_default());
    let base_system = planning_llm_system_prompt(capability);

    // Pre-query the local RAG lane and inject the top hits as a second
    // system message so the LLM stays grounded in the operator profile's own
    // corpus. Caller can set `rag_query` explicitly or let the prompt
    // itself be used. `rag=false` disables the prefetch for this call.
    let rag_enabled = arguments
        .get("rag")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let rag_query = arguments
        .get("rag_query")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| prompt.clone());
    let rag_collections = arguments
        .get("rag_collections")
        .and_then(|value| value.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let rag_hits = if rag_enabled {
        fetch_rag_context(provider, &rag_query, &rag_collections).await
    } else {
        Vec::new()
    };

    let mut messages = Vec::new();
    messages.push(json!({ "role": "system", "content": base_system }));
    if !rag_hits.is_empty() {
        let context = render_rag_context(&rag_hits);
        messages.push(json!({
            "role": "system",
            "content": format!(
                "Relevant context from the local Ordo operator corpus. \
                 Use this to stay consistent with existing operator style, \
                 product names, and positioning. Do not invent facts that \
                 are not supported.\n\n{context}"
            ),
        }));
    }
    messages.push(json!({ "role": "user", "content": prompt }));

    let mut chat_args = json!({
        "messages": messages,
        "temperature": arguments.get("temperature").cloned().unwrap_or(json!(0.4)),
    });
    // Honor a per-credential model override (Cloud tab → "model" field,
    // stored in extras). Lets local providers (Ollama / LM Studio) hit
    // whichever model the operator has loaded.
    if let Some(model) = credential.extras.get("model") {
        chat_args["model"] = json!(model);
    }

    match provider
        .credentials
        .enforce_single_local_model(
            &provider.http,
            Some(&credential.service),
            "planning_llm_call",
        )
        .await
    {
        Ok(report) => {
            log_cloud_lifecycle_report("planning llm call", &report);
            if let Some(error) = lifecycle_error(&report) {
                return Some(ToolCallResult::Failed { error });
            }
        }
        Err(err) => {
            return Some(ToolCallResult::Failed {
                error: err.to_string(),
            });
        }
    }

    // Dispatch to whichever provider is configured. Anthropic credentials
    // get the `messages` endpoint; everything else flows through OpenAI
    // chat.
    let result = if credential.auth_style == "anthropic" {
        ordo_cloud::anthropic::messages(&provider.http, &credential, &chat_args).await
    } else {
        ordo_cloud::openai::chat(&provider.http, &credential, &chat_args).await
    };

    let review_requested = arguments
        .get("review")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Some(match result {
        Ok(mut value) => {
            // Make the domain capability identifiable in the output, and
            // report how many RAG hits grounded the prompt so the
            // operator can see whether the answer was operator-context-
            // aware or fell back to pure LLM output.
            if let Some(object) = value.as_object_mut() {
                object.insert("capability".into(), Value::String(capability.to_string()));
                object.insert("credential_service".into(), Value::String(service.clone()));
                object.insert(
                    "rag_context_hits".into(),
                    Value::Number(serde_json::Number::from(rag_hits.len() as u64)),
                );
                if !rag_hits.is_empty() {
                    object.insert(
                        "rag_context_sources".into(),
                        Value::Array(
                            rag_hits
                                .iter()
                                .map(|hit| {
                                    json!({
                                        "document_id": hit.document_id,
                                        "title": hit.title,
                                        "collection": hit.collection,
                                    })
                                })
                                .collect(),
                        ),
                    );
                }
            }

            // Optional human-in-the-loop review step. We queue the
            // LLM's draft for operator approval and (if the review
            // service is configured) block until a decision arrives.
            // Deny → Failed; Edit → substitute the edited text in the
            // output so downstream agents see the operator's version.
            if review_requested {
                match (&provider.review, extract_review_draft(capability, &value)) {
                    (Some(review_service), Some(draft)) => {
                        let metadata = std::collections::HashMap::from_iter([
                            (
                                "capability".to_string(),
                                Value::String(capability.to_string()),
                            ),
                            (
                                "credential_service".to_string(),
                                Value::String(service.clone()),
                            ),
                            (
                                "rag_context_hits".to_string(),
                                Value::Number(serde_json::Number::from(rag_hits.len() as u64)),
                            ),
                        ]);
                        let new_request = ordo_review::NewReviewRequest {
                            origin_capability: capability.to_string(),
                            origin_plugin: None,
                            title: review_title(capability, arguments),
                            content_type: review_content_type(capability),
                            content: draft.clone(),
                            metadata,
                        };
                        match review_service
                            .request_and_wait(new_request, provider.review_wait)
                            .await
                        {
                            Ok(resolved) => {
                                use ordo_review::ReviewState::*;
                                match resolved.state {
                                    Approved | EditedAndApproved => {
                                        substitute_review_output(
                                            capability,
                                            &mut value,
                                            resolved.effective_content(),
                                        );
                                        if let Some(object) = value.as_object_mut() {
                                            object.insert(
                                                "review".into(),
                                                json!({
                                                    "state": resolved.state.label(),
                                                    "id": resolved.id,
                                                    "edited": matches!(resolved.state, EditedAndApproved),
                                                    "note": resolved.decision_note,
                                                }),
                                            );
                                        }
                                        ToolCallResult::Completed { result: value }
                                    }
                                    Denied => ToolCallResult::Failed {
                                        error: format!(
                                            "operator denied review {} ({}){}",
                                            resolved.id,
                                            capability,
                                            resolved
                                                .decision_note
                                                .map(|note| format!(": {note}"))
                                                .unwrap_or_default(),
                                        ),
                                    },
                                    Expired => ToolCallResult::Failed {
                                        error: format!(
                                            "review for '{capability}' expired before the operator acted"
                                        ),
                                    },
                                    Open => ToolCallResult::Failed {
                                        error: "review returned in Open state (runtime bug)"
                                            .to_string(),
                                    },
                                }
                            }
                            Err(err) => ToolCallResult::Failed {
                                error: format!("review service error: {err}"),
                            },
                        }
                    }
                    (None, _) => {
                        // Requested review but nothing's wired — be honest
                        // rather than silently skipping.
                        ToolCallResult::Failed {
                            error: "review requested but no review service is configured"
                                .to_string(),
                        }
                    }
                    (Some(_), None) => ToolCallResult::Completed { result: value },
                }
            } else {
                ToolCallResult::Completed { result: value }
            }
        }
        Err(err) => ToolCallResult::Failed {
            error: err.to_string(),
        },
    })
}

/// Extract the reviewable draft from a ordo-llm response. For
/// OpenAI-style chats we prefer `assistant_message`; for Anthropic, we
/// prefer `assistant_text`; otherwise we fall back to the full JSON
/// payload so the operator at least sees something.
fn extract_review_draft(_capability: &str, value: &Value) -> Option<String> {
    if let Some(text) = value.get("assistant_message").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }
    if let Some(text) = value.get("assistant_text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }
    serde_json::to_string_pretty(value).ok()
}

fn substitute_review_output(_capability: &str, value: &mut Value, approved: &str) {
    if let Some(object) = value.as_object_mut() {
        if object.contains_key("assistant_message") {
            object.insert(
                "assistant_message".into(),
                Value::String(approved.to_string()),
            );
        } else if object.contains_key("assistant_text") {
            object.insert("assistant_text".into(), Value::String(approved.to_string()));
        } else {
            object.insert("text".into(), Value::String(approved.to_string()));
        }
    }
}

fn review_title(capability: &str, arguments: &Value) -> String {
    // Prefer a caller-supplied hint so the review panel has a human
    // label. Fall back to the capability name + short prompt excerpt.
    if let Some(title) = arguments.get("review_title").and_then(|v| v.as_str()) {
        return title.to_string();
    }
    if let Some(prompt) = arguments.get("prompt").and_then(|v| v.as_str()) {
        let snippet = prompt.chars().take(64).collect::<String>();
        return format!("{capability}: {snippet}");
    }
    capability.to_string()
}

fn review_content_type(_capability: &str) -> String {
    "text/markdown".to_string()
}

/// Publish a RAG query on the bus and wait briefly for hits. Returns an
/// empty vec if the bus is not configured or if the retrieval lane does
/// not respond in time — this is best-effort grounding, never a hard
/// dependency.
async fn fetch_rag_context(
    provider: &OrdoLlmProvider,
    query: &str,
    collections: &[String],
) -> Vec<RagHit> {
    use futures::StreamExt;
    use std::time::Duration;
    use tokio::time::timeout;

    let Some(bus) = provider.bus.as_ref() else {
        return Vec::new();
    };
    if query.trim().is_empty() || provider.rag_top_k == 0 {
        return Vec::new();
    }

    let correlation_id = CorrelationId::new();
    let envelope = Envelope::new(
        NodeId::new(),
        OrdoMessage::RagQueryRequested {
            query: query.to_string(),
            top_k: provider.rag_top_k,
            collections: collections.to_vec(),
        },
    )
    .with_correlation(correlation_id.clone());

    let mut sub = match bus.subscribe(topics::RAG_QUERY_RESPONSE).await {
        Ok(sub) => sub,
        Err(_) => return Vec::new(),
    };
    if bus
        .publish(topics::RAG_QUERY_REQUEST, envelope)
        .await
        .is_err()
    {
        return Vec::new();
    }

    // Give the RAG lane a short window — we never want to block a user-
    // facing LLM call on a slow retrieval round trip. 750 ms matches the
    // Brain's internal budget for context hydration.
    let wait = Duration::from_millis(750);
    loop {
        match timeout(wait, sub.next()).await {
            Ok(Some(event)) => {
                if event.correlation_id.as_ref() != Some(&correlation_id) {
                    continue;
                }
                if let OrdoMessage::RagQueryCompleted { query: seen, hits } = event.payload {
                    if seen == query {
                        return hits;
                    }
                }
            }
            _ => return Vec::new(),
        }
    }
}

fn render_rag_context(hits: &[RagHit]) -> String {
    let mut out = String::new();
    for (idx, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "[{n}] ({collection}/{doc} #{chunk}, score={score:.2})\n{snippet}\n\n",
            n = idx + 1,
            collection = hit.collection,
            doc = hit.document_id,
            chunk = hit.chunk_index,
            score = hit.score,
            snippet = hit.snippet.trim(),
        ));
    }
    out
}

#[async_trait]
impl CapabilityProvider for OrdoLlmProvider {
    fn name(&self) -> &str {
        "ordo-llm"
    }

    fn capabilities(&self) -> Vec<String> {
        ORDO_LLM_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ORDO_LLM_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    planning_llm_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        if !ORDO_LLM_CAPABILITIES.contains(&capability) {
            return None;
        }
        run_planning_llm_call(self, capability, arguments).await
    }
}

