use crate::*;

// =========================================================================
// Assistant provider — exposes `ordo-assistant` on the capability bus.
// Same "provider-in-mcp, service-in-its-own-crate" pattern as review,
// to avoid a cycle between `ordo-assistant` and `ordo-mcp-host`.
// =========================================================================

pub const ASSISTANT_TURN: &str = "assistant.turn";
pub const ASSISTANT_NEW_SESSION: &str = "assistant.new_session";
pub const ASSISTANT_LIST_SESSIONS: &str = "assistant.list_sessions";
pub const ASSISTANT_GET_SESSION: &str = "assistant.get_session";
pub const ASSISTANT_REMEMBER_FACT: &str = "assistant.remember_fact";
pub const ASSISTANT_FORGET_FACT: &str = "assistant.forget_fact";
pub const ASSISTANT_LIST_FACTS: &str = "assistant.list_facts";
pub const ASSISTANT_RECALL: &str = "assistant.recall";
// Push 3: progressive-disclosure meta-tools + self-knowledge CRUD.
pub const ASSISTANT_RECALL_MEMORY: &str = "assistant.recall_memory";
pub const ASSISTANT_KNOWLEDGE_LOOKUP: &str = "assistant.knowledge_lookup";
pub const ASSISTANT_PARALLEL_LOOKUP: &str = "assistant.parallel_lookup";
pub const ASSISTANT_REMEMBER_KNOWLEDGE: &str = "assistant.remember_knowledge";
pub const ASSISTANT_FORGET_KNOWLEDGE: &str = "assistant.forget_knowledge";
pub const ASSISTANT_LIST_KNOWLEDGE: &str = "assistant.list_knowledge";

const ASSISTANT_CAPABILITIES: &[&str] = &[
    ASSISTANT_TURN,
    ASSISTANT_NEW_SESSION,
    ASSISTANT_LIST_SESSIONS,
    ASSISTANT_GET_SESSION,
    ASSISTANT_REMEMBER_FACT,
    ASSISTANT_FORGET_FACT,
    ASSISTANT_LIST_FACTS,
    ASSISTANT_RECALL,
    ASSISTANT_RECALL_MEMORY,
    ASSISTANT_KNOWLEDGE_LOOKUP,
    ASSISTANT_PARALLEL_LOOKUP,
    ASSISTANT_REMEMBER_KNOWLEDGE,
    ASSISTANT_FORGET_KNOWLEDGE,
    ASSISTANT_LIST_KNOWLEDGE,
];

fn assistant_description(capability: &str) -> &'static str {
    match capability {
        ASSISTANT_TURN => {
            "Process a user turn. Routes through the fact store, the local RAG lane, and the configured cloud LLM; persists the turn. Primary entry point for conversational use."
        }
        ASSISTANT_NEW_SESSION => "Create a new conversation session.",
        ASSISTANT_LIST_SESSIONS => "List recent conversation sessions.",
        ASSISTANT_GET_SESSION => "Load a session with its full turn history.",
        ASSISTANT_REMEMBER_FACT => {
            "Teach the assistant a durable fact about the operator, a client, the operator profile, or a project."
        }
        ASSISTANT_FORGET_FACT => "Remove a stored fact by id.",
        ASSISTANT_LIST_FACTS => "List stored facts, optionally filtered by subject.",
        ASSISTANT_RECALL => {
            "Return the top-K facts most relevant to an arbitrary query, without consulting an LLM."
        }
        ASSISTANT_RECALL_MEMORY => {
            "Meta-tool: semantic recall over persistent fact memory. Returns facts with a read-only preamble describing how to use the memory layer. Called by the assistant itself during a turn; operators can call it directly for debugging."
        }
        ASSISTANT_KNOWLEDGE_LOOKUP => {
            "Meta-tool: semantic recall over the assistant's self-knowledge RAG (skills, personas, tool notes, observations). Optionally filter by kind and/or domain. Results include the self-knowledge-layer preamble."
        }
        ASSISTANT_PARALLEL_LOOKUP => {
            "Meta-tool: fan knowledge_lookup across an explicit list of user-, mode-, or knowledge-selected domains concurrently."
        }
        ASSISTANT_REMEMBER_KNOWLEDGE => {
            "Add an entry to the assistant's self-knowledge RAG (skill card, persona guide, tool note, observation, or free-form note)."
        }
        ASSISTANT_FORGET_KNOWLEDGE => "Remove a stored knowledge entry by id.",
        ASSISTANT_LIST_KNOWLEDGE => {
            "List stored knowledge entries, optionally filtered by kind and/or domain."
        }
        _ => "Assistant capability.",
    }
}

pub struct AssistantProvider {
    service: ordo_assistant::AssistantService,
}

impl AssistantProvider {
    pub fn new(service: ordo_assistant::AssistantService) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &ordo_assistant::AssistantService {
        &self.service
    }
}

#[async_trait]
impl CapabilityProvider for AssistantProvider {
    fn name(&self) -> &str {
        "assistant"
    }

    fn capabilities(&self) -> Vec<String> {
        ASSISTANT_CAPABILITIES
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ASSISTANT_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    assistant_description(capability),
                    CapabilityTier::Core,
                    CapabilityActivation::Eager,
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
        let outcome: Result<Value, ordo_assistant::AssistantError> = match capability {
            ASSISTANT_TURN => assistant_do_turn(&self.service, arguments).await,
            ASSISTANT_NEW_SESSION => assistant_do_new_session(&self.service, arguments),
            ASSISTANT_LIST_SESSIONS => assistant_do_list_sessions(&self.service, arguments),
            ASSISTANT_GET_SESSION => assistant_do_get_session(&self.service, arguments),
            ASSISTANT_REMEMBER_FACT => assistant_do_remember(&self.service, arguments).await,
            ASSISTANT_FORGET_FACT => assistant_do_forget(&self.service, arguments),
            ASSISTANT_LIST_FACTS => assistant_do_list_facts(&self.service, arguments),
            ASSISTANT_RECALL => assistant_do_recall(&self.service, arguments).await,
            ASSISTANT_RECALL_MEMORY => assistant_do_recall_memory(&self.service, arguments).await,
            ASSISTANT_KNOWLEDGE_LOOKUP => {
                assistant_do_knowledge_lookup(&self.service, arguments).await
            }
            ASSISTANT_PARALLEL_LOOKUP => {
                assistant_do_parallel_lookup(&self.service, arguments).await
            }
            ASSISTANT_REMEMBER_KNOWLEDGE => {
                assistant_do_remember_knowledge(&self.service, arguments).await
            }
            ASSISTANT_FORGET_KNOWLEDGE => assistant_do_forget_knowledge(&self.service, arguments),
            ASSISTANT_LIST_KNOWLEDGE => assistant_do_list_knowledge(&self.service, arguments),
            _ => return None,
        };
        Some(match outcome {
            Ok(value) => ToolCallResult::Completed { result: value },
            Err(err) => ToolCallResult::Failed {
                error: err.to_string(),
            },
        })
    }
}

async fn assistant_do_turn(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let request: ordo_assistant::TurnRequest = serde_json::from_value(arguments.clone())
        .map_err(|err| ordo_assistant::AssistantError::InvalidArgument(err.to_string()))?;
    // `assistant.turn` is an untrusted bus/MCP boundary: the caller is not
    // an authenticated session owner. Sanitize the request so a caller
    // cannot target/hijack an existing session id (it always runs on a
    // fresh session) or set the internal isolation fields. Trusted callers
    // (control API, in-process spawns) call `service.turn` directly.
    let request = ordo_assistant::sanitize_untrusted_turn_request(request);
    let result = service.turn(request).await?;
    Ok(serde_json::to_value(&result).unwrap_or(Value::Null))
}

fn assistant_do_new_session(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let title = arguments
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let mode = arguments
        .get("mode")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let session = service.new_session(title, mode)?;
    Ok(serde_json::to_value(&session).unwrap_or(Value::Null))
}

fn assistant_do_list_sessions(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;
    let sessions = service.list_sessions(limit)?;
    Ok(json!({ "count": sessions.len(), "sessions": sessions }))
}

fn assistant_do_get_session(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let id = assistant_parse_id(arguments, "session_id")?;
    let session = service.get_session(id)?;
    Ok(serde_json::to_value(&session).unwrap_or(Value::Null))
}

async fn assistant_do_remember(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let mut new_fact: ordo_assistant::NewFact = serde_json::from_value(arguments.clone())
        .map_err(|err| ordo_assistant::AssistantError::InvalidArgument(err.to_string()))?;

    // Mode-aware bus path: external callers MAY pass an optional
    // `session_id` so the new fact lands in that session's mode
    // scope rather than the legacy global default. The LLM path
    // (dispatch_tool's `assistant.remember_fact` shadow) already
    // does this implicitly; the bus exposes it explicitly because
    // external MCP clients don't have a "current mode" by default.
    //
    // Resolution rules (mirror the meta-tool's):
    //   - If the fact already has an explicit `scope`, that wins.
    //   - Else if `session_id` is supplied AND resolves to a mode,
    //     the fact gets `scope: "mode:<id>"`.
    //   - Else the fact falls through to NewFact's serde default
    //     ("global"), preserving every legacy caller.
    if new_fact.scope.is_none() {
        if let Some(sid_str) = arguments.get("session_id").and_then(|v| v.as_str()) {
            if let Ok(sid) = uuid::Uuid::parse_str(sid_str) {
                if let Some(mode) = service.resolve_session_mode_manifest(sid) {
                    new_fact.scope = Some(format!("mode:{}", mode.id));
                }
            }
        }
    }

    let fact = service.remember_fact(new_fact).await?;
    let summary = ordo_assistant::FactSummary::from(&fact);
    Ok(serde_json::to_value(&summary).unwrap_or(Value::Null))
}

fn assistant_do_forget(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let id = assistant_parse_id(arguments, "id")?;
    let removed = service.forget_fact(id)?;
    Ok(json!({ "id": id, "removed": removed }))
}

fn assistant_do_list_facts(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let subject = arguments
        .get("subject")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let facts = service.list_facts(subject)?;
    Ok(json!({ "count": facts.len(), "facts": facts }))
}

async fn assistant_do_recall(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_assistant::AssistantError::InvalidArgument("missing 'query'".into()))?
        .to_string();
    let top_k = arguments
        .get("top_k")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(50) as usize;
    let recalled = service.recall(&query, top_k).await?;
    Ok(json!({
        "query": query,
        "count": recalled.len(),
        "facts": recalled,
    }))
}

// ---- push 3 meta-tool + knowledge handlers ----------------------------

async fn assistant_do_recall_memory(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_assistant::AssistantError::InvalidArgument("missing 'query'".into()))?
        .to_string();
    let top_k = arguments
        .get("top_k")
        .and_then(|v| v.as_u64())
        .unwrap_or(8)
        .min(50) as usize;
    let facts = service.facts().recall(&query, top_k).await?;
    Ok(json!({
        "preamble": ordo_assistant::MEMORY_PREAMBLE,
        "query": query,
        "top_k": top_k,
        "facts": facts,
    }))
}

async fn assistant_do_knowledge_lookup(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_assistant::AssistantError::InvalidArgument("missing 'query'".into()))?
        .to_string();
    let top_k = arguments
        .get("top_k")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(50) as usize;
    let kind = arguments
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(ordo_assistant::KnowledgeKind::parse);
    let domain = arguments
        .get("domain")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let hits = service
        .knowledge()
        .recall(&query, top_k, kind, domain.as_deref())
        .await?;
    Ok(json!({
        "preamble": ordo_assistant::KNOWLEDGE_PREAMBLE,
        "query": query,
        "top_k": top_k,
        "kind": kind.map(|k| k.as_str()),
        "domain": domain,
        "hits": hits,
    }))
}

async fn assistant_do_parallel_lookup(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_assistant::AssistantError::InvalidArgument("missing 'query'".into()))?
        .to_string();
    let domains: Vec<String> = arguments
        .get("domains")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if domains.is_empty() {
        return Err(ordo_assistant::AssistantError::InvalidArgument(
            "assistant.parallel_lookup requires at least one entry in `domains`".into(),
        ));
    }
    let top_k = arguments
        .get("top_k_per_domain")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .min(50) as usize;
    let kind = arguments
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(ordo_assistant::KnowledgeKind::parse);

    // Run the fanout concurrently — mirrors the in-turn meta-tool.
    let knowledge = service.knowledge().clone();
    let mut handles = Vec::with_capacity(domains.len());
    for domain in &domains {
        let knowledge = knowledge.clone();
        let query = query.clone();
        let domain = domain.clone();
        handles.push(tokio::spawn(async move {
            let hits = knowledge
                .recall(&query, top_k, kind, Some(&domain))
                .await
                .unwrap_or_default();
            (domain, hits)
        }));
    }
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok((domain, hits)) = handle.await {
            results.push(json!({
                "domain": domain,
                "count": hits.len(),
                "hits": hits,
            }));
        }
    }
    Ok(json!({
        "preamble": ordo_assistant::KNOWLEDGE_PREAMBLE,
        "query": query,
        "top_k_per_domain": top_k,
        "kind": kind.map(|k| k.as_str()),
        "domains": domains,
        "results": results,
    }))
}

async fn assistant_do_remember_knowledge(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let new_entry: ordo_assistant::NewKnowledge = serde_json::from_value(arguments.clone())
        .map_err(|err| ordo_assistant::AssistantError::InvalidArgument(err.to_string()))?;
    let entry = service.knowledge().remember(new_entry).await?;
    let summary = ordo_assistant::KnowledgeSummary::from(&entry);
    Ok(serde_json::to_value(&summary).unwrap_or(Value::Null))
}

fn assistant_do_forget_knowledge(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let id = assistant_parse_id(arguments, "id")?;
    let removed = service.knowledge().forget(id)?;
    Ok(json!({ "id": id, "removed": removed }))
}

fn assistant_do_list_knowledge(
    service: &ordo_assistant::AssistantService,
    arguments: &Value,
) -> Result<Value, ordo_assistant::AssistantError> {
    let kind = arguments
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(ordo_assistant::KnowledgeKind::parse);
    let domain = arguments
        .get("domain")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let entries = service.knowledge().list(kind, domain.as_deref())?;
    let summaries: Vec<ordo_assistant::KnowledgeSummary> = entries
        .iter()
        .map(ordo_assistant::KnowledgeSummary::from)
        .collect();
    Ok(json!({
        "count": summaries.len(),
        "entries": summaries,
    }))
}

fn assistant_parse_id(
    arguments: &Value,
    field: &str,
) -> Result<uuid::Uuid, ordo_assistant::AssistantError> {
    let raw = arguments
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ordo_assistant::AssistantError::InvalidArgument(format!("missing '{field}'"))
        })?;
    uuid::Uuid::parse_str(raw)
        .map_err(|err| ordo_assistant::AssistantError::InvalidArgument(err.to_string()))
}
