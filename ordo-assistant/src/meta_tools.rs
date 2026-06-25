//! Meta-tool methods extracted from service.rs.
//!
//! Progressive-disclosure meta-tools that the assistant exposes to the LLM:
//! semantic memory recall, knowledge lookup, parallel lookup, fact CRUD,
//! and cross-mode consultation.

use crate::service::*;
use crate::types::*;
use ordo_bus::Bus;
use ordo_modes::ModeManifest;
use ordo_protocol::{CorrelationId, Envelope, NodeId, OrdoMessage, Taint};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::collections::HashMap;
use parking_lot::Mutex;
use uuid::Uuid;
use tracing::warn;
use crate::events::TurnEvent;

impl AssistantService {

    pub(crate) async fn meta_recall_memory(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let top_k = arguments
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(8);
        if query.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.recall_memory requires a non-empty `query`".into(),
            ));
        }
        // When the assistant has a mode registry attached AND the
        // session resolves to a mode, scope the recall to that
        // mode's `memory_scope` list. Otherwise (legacy callers,
        // pre-mode tests), fall back to all-scopes recall â€”
        // backward-compat for anything that hasn't migrated.
        let facts = if let Some(manifest) = self.resolve_mode_for_session(session_id) {
            // Per-subagent isolation: a scoped subagent also recalls from
            // its private `agent:<uuid>` scope (its own working memory),
            // on top of the mode's shared scopes.
            let mut scopes = manifest.memory_scope.clone();
            if let Some(tag) = self
                .session_isolation
                .lock()
                .get(&session_id)
                .and_then(|iso| iso.memory_scope.clone())
            {
                if !scopes.contains(&tag) {
                    scopes.push(tag);
                }
            }
            let scoped = self.facts.recall_in_scopes(&query, top_k, &scopes).await?;
            // Telemetry: surface the scope filter to the insight
            // trace so an operator inspecting "why didn't fact X
            // surface?" can see the active scope set + visible
            // count without grepping logs.
            self.events.publish(
                session_id,
                TurnEvent::ModeMemoryScopeApplied {
                    session_id,
                    mode_id: manifest.id.clone(),
                    visible_scopes: manifest.memory_scope.clone(),
                    facts_visible: scoped.len(),
                },
            );
            scoped
        } else {
            self.facts.recall(&query, top_k).await?
        };
        // Reinforce on recall so heavily-used facts accrue confidence.
        for recalled in &facts {
            let _ = self.facts.reinforce(recalled.fact.id);
        }
        Ok(json!({
            "preamble": crate::prompt::MEMORY_PREAMBLE,
            "query": query,
            "top_k": top_k,
            "facts": facts,
            "facts_rendered": crate::prompt::render_facts_block(&facts),
        }))
    }

    pub(crate) async fn meta_knowledge_lookup(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let top_k = arguments
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(5);
        let kind = arguments
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(KnowledgeKind::parse);
        let domain = arguments
            .get("domain")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if query.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.knowledge_lookup requires a non-empty `query`".into(),
            ));
        }

        // Mode-scoped RAG: when a mode is active, validate the
        // requested domain against the mode's `rag_domains` list.
        // If the LLM asks for a domain the mode doesn't permit, we
        // fail loudly with a clear message â€” the model can correct
        // and try a domain it IS allowed to use, or recognize that
        // the lookup isn't appropriate for this workspace.
        if let (Some(mode), Some(domain_id)) =
            (self.resolve_mode_for_session(session_id), domain.as_deref())
        {
            if !mode.rag_domains.iter().any(|d| d == domain_id) {
                self.events.publish(
                    session_id,
                    TurnEvent::ToolCallFailed {
                        session_id,
                        invocation_id: Uuid::new_v4(),
                        capability: "assistant.knowledge_lookup".into(),
                        error: format!(
                            "domain '{domain_id}' not in mode '{}' rag_domains",
                            mode.id
                        ),
                    },
                );
                return Err(AssistantError::InvalidArgument(format!(
                    "RAG domain '{domain_id}' is not available in mode '{}'. \
                     This mode permits: {}",
                    mode.id,
                    if mode.rag_domains.is_empty() {
                        "(no RAG domains in this mode)".to_string()
                    } else {
                        mode.rag_domains.join(", ")
                    },
                )));
            }
        }
        let hits = self
            .knowledge
            .recall(&query, top_k, kind, domain.as_deref())
            .await?;
        for hit in &hits {
            let _ = self.knowledge.reinforce(hit.entry.id);
        }
        Ok(json!({
            "preamble": crate::prompt::KNOWLEDGE_PREAMBLE,
            "query": query,
            "top_k": top_k,
            "kind": kind.map(|k| k.as_str()),
            "domain": domain,
            "hits": hits,
        }))
    }

    /// Fan `knowledge_lookup` across an explicit list of domains concurrently.
    /// The domains must come from the user, active mode, or retrieved knowledge;
    /// Ordo no longer exposes an automatic router to pick them.
    pub(crate) async fn meta_parallel_lookup(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if query.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.parallel_lookup requires a non-empty `query`".into(),
            ));
        }
        let mut domains: Vec<String> = arguments
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
            return Err(AssistantError::InvalidArgument(
                "assistant.parallel_lookup requires at least one entry in `domains`".into(),
            ));
        }
        let requested_domains = domains.clone();
        let mut used_mode_fallback_domains = false;

        // Mode-scoped RAG: when a mode is active, drop any requested
        // domains that aren't in the mode's `rag_domains` list. If
        // ALL get filtered out, fall back to the active mode's own
        // domains instead of broadening access. The caller still gets
        // `blocked_domains` so it can explain what was denied, while
        // the tool remains useful for smoke tests and general-mode
        // retrieval.
        let mut blocked_domains: Vec<String> = Vec::new();
        if let Some(mode) = self.resolve_mode_for_session(session_id) {
            let allowed: std::collections::HashSet<&String> = mode.rag_domains.iter().collect();
            domains.retain(|d| {
                if allowed.contains(d) {
                    true
                } else {
                    blocked_domains.push(d.clone());
                    false
                }
            });
            if domains.is_empty() {
                domains = mode.rag_domains.clone();
                used_mode_fallback_domains = true;
                if domains.is_empty() {
                    return Err(AssistantError::InvalidArgument(format!(
                        "none of the requested RAG domains are available in mode '{}', \
                         and this mode has no fallback RAG domains",
                        mode.id,
                    )));
                }
            }
            if !blocked_domains.is_empty() {
                tracing::info!(
                    target: "ordo_assistant",
                    mode = %mode.id,
                    blocked = ?blocked_domains,
                    kept = ?domains,
                    "parallel_lookup: dropped domains not in mode rag_domains"
                );
            }
        }
        let top_k = arguments
            .get("top_k_per_domain")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(3);
        let kind = arguments
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(KnowledgeKind::parse);

        let knowledge = self.knowledge.clone();
        let query_cloned = query.clone();
        let mut handles = Vec::with_capacity(domains.len());
        for domain in &domains {
            let knowledge = knowledge.clone();
            let query = query_cloned.clone();
            let domain = domain.clone();
            handles.push(tokio::spawn(async move {
                let hits = knowledge
                    .recall(&query, top_k, kind, Some(&domain))
                    .await
                    .unwrap_or_default();
                for hit in &hits {
                    let _ = knowledge.reinforce(hit.entry.id);
                }
                (domain, hits)
            }));
        }
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok((domain, hits)) => results.push(json!({
                    "domain": domain,
                    "count": hits.len(),
                    "hits": hits,
                })),
                Err(err) => {
                    warn!(
                        target: "ordo_assistant",
                        error = %err,
                        "parallel lookup task panicked"
                    );
                }
            }
        }
        Ok(json!({
            "preamble": crate::prompt::KNOWLEDGE_PREAMBLE,
            "query": query,
            "top_k_per_domain": top_k,
            "kind": kind.map(|k| k.as_str()),
            "domains": domains,
            "requested_domains": requested_domains,
            "blocked_domains": blocked_domains,
            "used_mode_fallback_domains": used_mode_fallback_domains,
            "results": results,
        }))
    }

    /// Mode-aware shadow of `assistant.remember_fact`. When the
    /// session is bound to a mode, NEW facts default to
    /// `scope: "mode:<id>"` instead of `"global"` â€” so the brand
    /// preference the LLM learns in Planning mode doesn't pollute
    /// Vibe Coding's recall.
    ///
    /// The LLM CAN override by passing an explicit `scope` field in
    /// the NewFact JSON: `"global"` for cross-mode visibility,
    /// `"mode:<other_id>"` for cross-tagging (legitimate when an
    /// operator dictates "this is a brand fact even though I'm in
    /// Vibe Coding right now"), or any other valid scope tag.
    ///
    /// When no mode is resolved (legacy / no registry attached), the
    /// fact falls through to "global" â€” same shape as the bus path.
    pub(crate) async fn meta_remember_fact(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let mut new_fact: NewFact = serde_json::from_value(arguments).map_err(|err| {
            AssistantError::InvalidArgument(format!(
                "assistant.remember_fact: invalid fact body â€” {err}"
            ))
        })?;

        if let Some(mode) = self.resolve_mode_for_session(session_id) {
            let mode_scope = format!("mode:{}", mode.id);
            if mode.id == "diagnostic" {
                match new_fact.scope.as_deref() {
                    Some(scope) if scope != mode_scope => {
                        return Err(AssistantError::InvalidArgument(format!(
                            "diagnostic mode memory is self-contained; assistant.remember_fact may only write to '{mode_scope}'"
                        )));
                    }
                    Some(_) => {}
                    None => {
                        new_fact.scope = Some(mode_scope);
                    }
                }
            } else {
                // A scoped subagent is CONFINED to its private memory
                // scope (anti-clobber / anti-laundering): it may only
                // write to its own `agent:<uuid>` scope — never to
                // `global`, another mode, or another agent — so an
                // explicit `scope` argument that escapes is rejected.
                // Unscoped (operator) turns keep the mode scope as the
                // default and may target any scope they choose.
                let private_scope = self
                    .session_isolation
                    .lock()
                    .get(&session_id)
                    .and_then(|iso| iso.memory_scope.clone());
                match (private_scope, new_fact.scope.as_deref()) {
                    (Some(private), Some(requested)) if requested != private => {
                        return Err(AssistantError::InvalidArgument(format!(
                            "a scoped subagent may only write to its own memory scope \
                             '{private}', not '{requested}'"
                        )));
                    }
                    (Some(private), _) => {
                        new_fact.scope = Some(private);
                    }
                    (None, _) if new_fact.scope.is_none() => {
                        new_fact.scope = Some(mode_scope);
                    }
                    (None, _) => {}
                }
            }
        }

        let fact = self.facts.remember(new_fact).await?;
        Ok(serde_json::to_value(&fact).unwrap_or(Value::Null))
    }

    pub(crate) async fn meta_list_facts(&self, session_id: Uuid, arguments: Value) -> AssistantResult<Value> {
        let requested_subject = arguments
            .get("subject")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let limit = arguments
            .get("limit")
            .and_then(|value| value.as_u64())
            .map(|value| value.min(500) as usize)
            .unwrap_or(200);
        let mut facts = self.list_facts(requested_subject.as_deref())?;
        if let Some(mode) = self.resolve_mode_for_session(session_id) {
            let allowed_scopes = &mode.memory_scope;
            facts.retain(|fact| allowed_scopes.iter().any(|scope| scope == &fact.scope));
        }
        facts.truncate(limit);
        Ok(json!({
            "facts": facts,
            "count": facts.len(),
            "subject": requested_subject,
            "limit": limit,
        }))
    }

    pub(crate) async fn meta_forget_fact(&self, session_id: Uuid, arguments: Value) -> AssistantResult<Value> {
        let id = arguments
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                AssistantError::InvalidArgument("assistant.forget_fact requires id".into())
            })?;
        let uuid = Uuid::parse_str(id)
            .map_err(|err| AssistantError::InvalidArgument(format!("invalid fact id: {err}")))?;

        if let Some(mode) = self.resolve_mode_for_session(session_id) {
            let visible = self.list_facts(None)?;
            let Some(fact) = visible.iter().find(|fact| fact.id == uuid) else {
                return Err(AssistantError::InvalidArgument(format!(
                    "fact {uuid} is not visible in active mode '{}'",
                    mode.id
                )));
            };
            if !mode.memory_scope.iter().any(|scope| scope == &fact.scope) {
                return Err(AssistantError::InvalidArgument(format!(
                    "fact {uuid} is outside active mode '{}' memory scope",
                    mode.id
                )));
            }
        }

        let removed = self.forget_fact(uuid)?;
        Ok(json!({ "id": uuid, "removed": removed }))
    }

    /// Mode-aware shadow of `assistant.remember_knowledge`. Knowledge writes
    /// are constrained to the active mode's declared RAG domains, so a
    /// diagnostic lesson lands in the diagnostic tree instead of leaking into
    /// the general assistant's self-knowledge.
    pub(crate) async fn meta_remember_knowledge(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        #[derive(Deserialize)]
        struct RememberKnowledgeArgs {
            #[serde(default)]
            kind: Option<String>,
            #[serde(default)]
            domain: Option<String>,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            body: Option<String>,
            #[serde(default)]
            content: Option<String>,
            #[serde(default)]
            note: Option<String>,
            #[serde(default)]
            source: Option<String>,
            #[serde(default)]
            confidence: Option<f32>,
        }

        let args: RememberKnowledgeArgs = serde_json::from_value(arguments).map_err(|err| {
            AssistantError::InvalidArgument(format!(
                "assistant.remember_knowledge: invalid knowledge body - {err}"
            ))
        })?;

        let mode = self.resolve_mode_for_session(session_id).ok_or_else(|| {
            AssistantError::InvalidArgument(
                "assistant.remember_knowledge requires a session bound to a registered mode".into(),
            )
        })?;
        if mode.rag_domains.is_empty() {
            return Err(AssistantError::InvalidArgument(format!(
                "mode '{}' has no writable RAG domains",
                mode.id
            )));
        }

        let requested_domain = args
            .domain
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let domain = if let Some(domain) = requested_domain {
            if !mode.rag_domains.iter().any(|allowed| allowed == domain) {
                return Err(AssistantError::InvalidArgument(format!(
                    "RAG domain '{domain}' is not writable in mode '{}'. This mode permits: {}",
                    mode.id,
                    mode.rag_domains.join(", ")
                )));
            }
            domain.to_string()
        } else if mode.id == "diagnostic"
            && mode
                .rag_domains
                .iter()
                .any(|allowed| allowed == "diagnostic_self_learning_tree")
        {
            "diagnostic_self_learning_tree".to_string()
        } else {
            mode.rag_domains[0].clone()
        };

        let body = args
            .body
            .or(args.content)
            .or(args.note)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AssistantError::InvalidArgument(
                    "assistant.remember_knowledge requires non-empty body, content, or note".into(),
                )
            })?;
        let title = args
            .title
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                body.lines()
                    .next()
                    .unwrap_or("Mode lesson")
                    .chars()
                    .take(96)
                    .collect()
            });
        let kind = args
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                KnowledgeKind::parse(value).ok_or_else(|| {
                    AssistantError::InvalidArgument(format!(
                        "assistant.remember_knowledge kind must be one of skill, persona, tool_note, observation, note; got '{value}'"
                    ))
                })
            })
            .transpose()?
            .unwrap_or(KnowledgeKind::Observation);
        let source = args
            .source
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("assistant:mode:{}:learned", mode.id));
        let confidence = args.confidence.unwrap_or(1.0).clamp(0.0, 1.0);

        let entry = self
            .knowledge
            .remember(NewKnowledge {
                kind,
                domain: Some(domain.clone()),
                title,
                body,
                source,
                confidence,
            })
            .await?;

        Ok(json!({
            "preamble": crate::prompt::KNOWLEDGE_PREAMBLE,
            "mode": mode.id,
            "domain": domain,
            "entry": entry,
        }))
    }


    pub(crate) async fn meta_consult_mode_agent(
        &self,
        session_id: Uuid,
        arguments: Value,
    ) -> AssistantResult<Value> {
        let target_mode_id = arguments
            .get("target_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let reason = arguments
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let question = arguments
            .get("question")
            .or_else(|| arguments.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let max_iterations = arguments
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(2)
            .clamp(1, 5);

        if target_mode_id.is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.consult_mode_agent requires a `target_mode`".into(),
            ));
        }
        if reason.is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.consult_mode_agent requires a `reason` for the audit log".into(),
            ));
        }
        if question.is_empty() {
            return Err(AssistantError::InvalidArgument(
                "assistant.consult_mode_agent requires a `question`".into(),
            ));
        }

        let active_manifest = self.resolve_mode_for_session(session_id).ok_or_else(|| {
            AssistantError::InvalidArgument(
                "this assistant has no mode registry attached; cross-mode consultation requires modes to be loaded"
                    .into(),
            )
        })?;
        let active_mode_id = active_manifest.id.clone();
        if target_mode_id == active_mode_id {
            return Err(AssistantError::InvalidArgument(format!(
                "consult target '{target_mode_id}' is the active mode; answer directly instead"
            )));
        }

        let target_manifest = self.get_mode(&target_mode_id).ok_or_else(|| {
            AssistantError::InvalidArgument(format!(
                "consult target '{target_mode_id}' is not a registered mode"
            ))
        })?;

        self.events.publish(
            session_id,
            TurnEvent::CrossModeConsultRequested {
                session_id,
                active_mode: active_mode_id.clone(),
                target_mode: target_mode_id.clone(),
                reason: reason.clone(),
                question: question.clone(),
            },
        );

        if !target_manifest.allows_consult_from() {
            let denial_reason = format!(
                "mode '{}' has cross_mode_consult_policy = deny; consultation requires switching to that mode (start a new chat in {})",
                target_manifest.id, target_manifest.label
            );
            self.events.publish(
                session_id,
                TurnEvent::CrossModeConsultDenied {
                    session_id,
                    active_mode: active_mode_id.clone(),
                    target_mode: target_mode_id.clone(),
                    reason: denial_reason.clone(),
                },
            );
            return Err(AssistantError::InvalidArgument(denial_reason));
        }

        self.events.publish(
            session_id,
            TurnEvent::CrossModeConsultApproved {
                session_id,
                active_mode: active_mode_id.clone(),
                target_mode: target_mode_id.clone(),
            },
        );

        let consult_goal = format!(
            "You are being consulted as the '{}' mode by the active '{}' mode.\n\
             Reason: {}\n\
             Question: {}\n\n\
             Return a concise, bounded answer for the active mode to consider. \
             Do not ask to read or write the active mode's memory. Do not modify durable state \
             unless the operator explicitly requested it.",
            target_manifest.label, active_manifest.label, reason, question
        );
        // A scoped-subagent parent confines the consult child to its own
        // narrowed lanes, so `consult` can't be used to widen capability
        // (e.g. a web-only subagent consulting a code-capable mode).
        let (parent_lanes, parent_depth) = {
            let map = self.session_isolation.lock();
            let iso = map.get(&session_id);
            (
                iso.and_then(|i| i.allowed_lanes.clone()),
                iso.map(|i| i.depth).unwrap_or(0),
            )
        };
        let result = self
            .spawn_subagent_in_mode(
                // Accumulate depth across consult hops so MAX_SUBAGENT_DEPTH
                // actually bounds nesting (was a hardcoded 0, which reset
                // the counter every hop).
                parent_depth,
                consult_goal,
                Some(max_iterations),
                Some(target_mode_id.clone()),
                SubagentScope {
                    // Downward: propagate the parent session's taint so a
                    // consult can't launder untrusted content INTO the child.
                    inherit_taint: self.session_taints(session_id),
                    allowed_lanes: parent_lanes,
                    ..SubagentScope::default()
                },
            )
            .await?;
        // Upward: if the consult child ingested untrusted content, taint
        // flows back so a clean parent can't launder it in as "clean"
        // consult output. (taint_session de-dups, so re-seeding is free.)
        for taint in self.session_taints(result.session_id) {
            self.taint_session(session_id, taint);
        }

        self.events.publish(
            session_id,
            TurnEvent::CrossModeConsultCompleted {
                session_id,
                active_mode: active_mode_id,
                target_mode: target_mode_id.clone(),
                turn_id: result.turn.id,
            },
        );

        Ok(json!({
            "preamble": "Cross-mode consultation returns another mode agent's bounded answer. It does not expose that mode's raw RAG or memory.",
            "target_mode": target_mode_id,
            "target_label": target_manifest.label,
            "reason": reason,
            "question": question,
            "max_iterations": max_iterations,
            "consulted_turn": result.turn,
            "answer": result.turn.assistant_response,
        }))
    }
}
