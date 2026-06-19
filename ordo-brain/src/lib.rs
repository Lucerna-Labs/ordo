use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures::StreamExt;
use ordo_bus::Bus;
use ordo_planner::RuleBasedPlanner;
use ordo_protocol::{
    infer_rag_collections, is_knowledge_goal, topics, CapabilityDescriptor, CorrelationId,
    Envelope, ExecutionPlan, NodeId, OrdoMessage, RagCollectionSummary, RagDocument, RagHit,
    RunStatus, SelfHealIncident, SelfHealPlan,
};
use serde_json::Value;
use thiserror::Error;
use tokio::time::timeout;
use uuid::Uuid;

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// Wait budget for waiting on a tool-call response on the bus. Tools
/// can include LLM round-trips (now sometimes hybrid: a formalize
/// call + a fallback rhetorical call), so this needs to comfortably
/// exceed any reasonable model latency. Aligned with the cloud
/// layer's 5-minute default — when those settled at 300s, this
/// stayed at 180s and started pre-empting otherwise-fine hybrid
/// paths on slow local models.
const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Wait budget for run lifecycle / step events. Same rationale as
/// `TOOL_CALL_TIMEOUT` since steps run capabilities.
const RUN_EVENT_TIMEOUT: Duration = Duration::from_secs(300);

/// Runaway guard for `invoke_tool`. The control API exposes `invoke_tool`
/// over HTTP with NO per-client throttling, so a buggy or fixated client
/// can drive unbounded bus + native work through it — the 62x
/// `cloud.credentials.list` storm that preceded the 2026-06-07 runtime
/// termination is the canonical example. More than `TOOL_CALL_RATE_MAX`
/// calls to a single capability within `TOOL_CALL_RATE_WINDOW` are
/// rejected with `BrainError::ToolCallRateLimited` BEFORE any bus traffic
/// is generated. The ceiling is deliberately generous: legitimate
/// UI-driven bursts are unaffected and it only trips on a fast runaway.
const TOOL_CALL_RATE_WINDOW: Duration = Duration::from_secs(10);
const TOOL_CALL_RATE_MAX: usize = 120;

/// A *slow* runaway (the observed storm was only ~1 call / 6s, far under
/// the rate cap) is caught for visibility rather than blocked: every time
/// the same capability+arguments is invoked another `TOOL_CALL_WARN_STRIDE`
/// times in a row with nothing else in between, a warning is logged so the
/// runaway is obvious in real time instead of having to be reconstructed
/// from raw logs after a crash. This is intentionally warn-only — blocking
/// identical calls would break legitimate idle polling, and the real fix
/// for a fixated client lives in the client, not the transport layer.
const TOOL_CALL_WARN_STRIDE: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStepSummary {
    pub step_id: Uuid,
    pub name: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PreparedGoal {
    pub goal: String,
    pub context: Vec<RagHit>,
    pub plan: Option<ExecutionPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: Uuid,
    pub goal: String,
    pub accepted: bool,
    pub finished: bool,
    pub succeeded: bool,
    pub used_explicit_plan: bool,
    pub planned_steps: usize,
    pub planned_capabilities: Vec<String>,
    pub completed_steps: usize,
    pub context_hits: usize,
    pub steps: Vec<RunStepSummary>,
}

#[derive(Debug, Error)]
pub enum BrainError {
    #[error("timed out waiting for a matching capability response")]
    CapabilityResponseTimedOut,
    #[error("capability inventory request timed out")]
    CapabilityInventoryTimedOut,
    #[error("run {run_id} timed out waiting for lifecycle completion")]
    RunTimedOut { run_id: Uuid },
    #[error("rag ingest timed out for document '{document_id}'")]
    RagIngestTimedOut { document_id: String },
    #[error("rag query timed out for '{query}'")]
    RagQueryTimedOut { query: String },
    #[error("rag collection inventory request timed out")]
    RagCollectionsTimedOut,
    #[error("goal planning failed for '{goal}': {error}")]
    GoalPlanningFailed { goal: String, error: String },
    #[error("tool call timed out for capability '{capability}'")]
    ToolCallTimedOut { capability: String },
    #[error("tool call failed for capability '{capability}': {error}")]
    ToolCallFailed { capability: String, error: String },
    #[error("self-heal request timed out for fingerprint '{fingerprint}'")]
    SelfHealTimedOut { fingerprint: String },
    #[error(
        "tool call rejected for capability '{capability}': {count} calls within {window_secs}s \
         tripped the runaway guard"
    )]
    ToolCallRateLimited {
        capability: String,
        count: usize,
        window_secs: u64,
    },
}

/// In-memory bookkeeping for the `invoke_tool` runaway guard. See the
/// `TOOL_CALL_RATE_*` / `TOOL_CALL_WARN_STRIDE` constants for the rationale.
#[derive(Default)]
struct ToolCallGuard {
    /// Recent invocation instants per capability, pruned to the rate window
    /// on each call. Used for the hard rate cap.
    recent: HashMap<String, VecDeque<Instant>>,
    /// Fingerprint (capability + arguments) of the previous call, and how
    /// many times it has now repeated back-to-back. Used for the warn-only
    /// slow-runaway signal.
    last_fingerprint: Option<u64>,
    consecutive: usize,
}

/// Outcome of consulting the guard for one `invoke_tool` call.
struct ToolCallDecision {
    /// Number of consecutive identical (capability + arguments) calls
    /// including this one.
    consecutive: usize,
    /// `Some(count)` when this call exceeded the per-capability rate cap and
    /// must be rejected without touching the bus.
    rate_limited: Option<usize>,
}

impl ToolCallGuard {
    fn observe(&mut self, capability: &str, fingerprint: u64, now: Instant) -> ToolCallDecision {
        // Hard rate cap over a sliding window, per capability.
        let window = self.recent.entry(capability.to_string()).or_default();
        while let Some(front) = window.front() {
            if now.duration_since(*front) > TOOL_CALL_RATE_WINDOW {
                window.pop_front();
            } else {
                break;
            }
        }
        let rate_limited = if window.len() >= TOOL_CALL_RATE_MAX {
            Some(window.len())
        } else {
            window.push_back(now);
            None
        };

        // Warn-only consecutive-identical detection (independent of the time
        // window). Only tracked for ADMITTED calls: a rejected call is not
        // being served, so emitting the "still serving" runaway warning for
        // it would be misleading — the rate-limit warning covers that case.
        let consecutive = if rate_limited.is_some() {
            0
        } else if self.last_fingerprint == Some(fingerprint) {
            self.consecutive = self.consecutive.saturating_add(1);
            self.consecutive
        } else {
            self.last_fingerprint = Some(fingerprint);
            self.consecutive = 1;
            1
        };

        ToolCallDecision {
            consecutive,
            rate_limited,
        }
    }
}

/// Stable-enough proxy for "the same tool call with the same arguments".
/// `serde_json::Value` is not `Hash`, so we hash its canonical string form
/// alongside the capability name.
fn fingerprint_tool_call(capability: &str, arguments: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    capability.hash(&mut hasher);
    arguments.to_string().hash(&mut hasher);
    hasher.finish()
}

pub struct Brain {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
    /// Guards the shared `invoke_tool` path against runaway clients. Behind
    /// a `Mutex` because `invoke_tool` takes `&self` and the control API
    /// drives it concurrently; the critical section is tiny and never held
    /// across an `.await`.
    tool_guard: Mutex<ToolCallGuard>,
}

impl Brain {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
            tool_guard: Mutex::new(ToolCallGuard::default()),
        }
    }

    /// Consult the runaway guard before doing any work for a tool call.
    /// Returns `Err(ToolCallRateLimited)` (without publishing to the bus)
    /// when the per-capability rate cap is exceeded, and logs a warning when
    /// a capability is being hammered with identical arguments. See the
    /// `TOOL_CALL_RATE_*` constants.
    fn admit_tool_call(&self, capability: &str, arguments: &Value) -> Result<(), DynError> {
        let fingerprint = fingerprint_tool_call(capability, arguments);
        let decision = {
            let mut guard = self
                .tool_guard
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.observe(capability, fingerprint, Instant::now())
        };

        if decision.consecutive > 0 && decision.consecutive % TOOL_CALL_WARN_STRIDE == 0 {
            eprintln!(
                "[Brain] WARN tool '{capability}' invoked {} times in a row with identical \
                 arguments — possible runaway client (still serving the call)",
                decision.consecutive
            );
        }

        if let Some(count) = decision.rate_limited {
            eprintln!(
                "[Brain] WARN rejecting tool '{capability}': {count} calls hit the \
                 {TOOL_CALL_RATE_MAX}/{}s cap — possible runaway client",
                TOOL_CALL_RATE_WINDOW.as_secs()
            );
            return Err(Box::new(BrainError::ToolCallRateLimited {
                capability: capability.to_string(),
                count,
                window_secs: TOOL_CALL_RATE_WINDOW.as_secs(),
            }));
        }

        Ok(())
    }

    pub async fn run_goal(&self, goal: &str) -> Result<RunSummary, DynError> {
        let prepared = self.prepare_goal_request(goal).await?;
        self.run_prepared_goal(prepared).await
    }

    pub async fn prepare_goal_request(&self, goal: &str) -> Result<PreparedGoal, DynError> {
        let (context, plan) = self.prepare_goal(goal).await?;
        Ok(PreparedGoal {
            goal: goal.to_string(),
            context,
            plan,
        })
    }

    pub async fn run_prepared_goal(&self, prepared: PreparedGoal) -> Result<RunSummary, DynError> {
        let PreparedGoal {
            goal,
            context,
            plan,
        } = prepared;
        let run_id = Uuid::new_v4();
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::RunRequested {
                run_id,
                goal: goal.clone(),
                context: context.clone(),
                plan: plan.clone(),
            },
        )
        .with_correlation(correlation_id.clone());

        println!("[Brain] Requesting run: '{}'", goal);

        let mut sub = self.bus.subscribe(topics::RUN_EVENT).await?;
        self.bus.publish(topics::RUN_REQUEST, envelope).await?;

        let mut summary = RunSummary {
            run_id,
            goal,
            accepted: false,
            finished: false,
            succeeded: false,
            used_explicit_plan: plan.is_some(),
            planned_steps: plan.as_ref().map(|plan| plan.steps.len()).unwrap_or(0),
            planned_capabilities: plan
                .as_ref()
                .map(|plan| {
                    plan.steps
                        .iter()
                        .map(|step| step.capability.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            completed_steps: 0,
            context_hits: context.len(),
            steps: Vec::new(),
        };

        loop {
            match timeout(RUN_EVENT_TIMEOUT, sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    match event.payload {
                        OrdoMessage::RunAccepted {
                            run_id: event_run_id,
                        } if event_run_id == run_id => {
                            summary.accepted = true;
                            println!("[Brain] Run accepted: {:?}", run_id);
                        }
                        OrdoMessage::StepStarted {
                            run_id: event_run_id,
                            step_id,
                            name,
                        } if event_run_id == run_id => {
                            let step = ensure_step(&mut summary.steps, step_id, name);
                            println!("[Brain] Step started: {}", step.name);
                        }
                        OrdoMessage::StepCompleted {
                            run_id: event_run_id,
                            step_id,
                            output,
                        } if event_run_id == run_id => {
                            let step =
                                ensure_step(&mut summary.steps, step_id, format!("step-{step_id}"));
                            step.output = Some(output.clone());
                            step.error = None;
                            println!("[Brain] Step completed: {}", step.name);
                        }
                        OrdoMessage::StepFailed {
                            run_id: event_run_id,
                            step_id,
                            error,
                        } if event_run_id == run_id => {
                            let step =
                                ensure_step(&mut summary.steps, step_id, format!("step-{step_id}"));
                            step.error = Some(error.clone());
                            step.output = None;
                            println!("[Brain] Step failed: {}", step.name);
                        }
                        OrdoMessage::RunFinished {
                            run_id: event_run_id,
                            status,
                            completed_steps,
                        } if event_run_id == run_id => {
                            summary.finished = true;
                            summary.completed_steps = completed_steps;
                            summary.succeeded = matches!(status, RunStatus::Succeeded);
                            println!(
                                "[Brain] Run finished: {:?} completed_steps={}",
                                status, completed_steps
                            );
                            return Ok(summary);
                        }
                        _ => {}
                    }
                }
                Ok(None) | Err(_) => {
                    return Err(Box::new(BrainError::RunTimedOut { run_id }));
                }
            }
        }
    }

    pub async fn ingest_document(&self, document: RagDocument) -> Result<usize, DynError> {
        let correlation_id = CorrelationId::new();
        let document_id = document.document_id.clone();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::RagIngestRequested { document },
        )
        .with_correlation(correlation_id.clone());

        println!("[Brain] Sending document '{}' to RAG", document_id);

        let mut sub = self.bus.subscribe(topics::RAG_INGEST_RESPONSE).await?;
        self.bus
            .publish(topics::RAG_INGEST_REQUEST, envelope)
            .await?;

        loop {
            match timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::RagDocumentIndexed {
                        document_id: indexed_document_id,
                        chunk_count,
                    } = event.payload
                    {
                        if indexed_document_id == document_id {
                            println!(
                                "[Brain] RAG indexed '{}' into {} chunk(s)",
                                indexed_document_id, chunk_count
                            );
                            return Ok(chunk_count);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err(Box::new(BrainError::RagIngestTimedOut { document_id }));
                }
            }
        }
    }

    pub async fn plan_goal(&self, goal: &str) -> Result<ExecutionPlan, DynError> {
        let prepared = self.prepare_goal_request(goal).await?;
        prepared.plan.ok_or_else(|| {
            Box::new(BrainError::GoalPlanningFailed {
                goal: goal.to_string(),
                error: "planner did not produce an execution plan".to_string(),
            }) as DynError
        })
    }

    pub async fn query_rag(&self, query: &str, top_k: usize) -> Result<Vec<RagHit>, DynError> {
        self.query_rag_in_collections(query, &[], top_k).await
    }

    pub async fn query_rag_in_collections(
        &self,
        query: &str,
        collections: &[String],
        top_k: usize,
    ) -> Result<Vec<RagHit>, DynError> {
        self.query_rag_with_timeout(query, collections, top_k, Duration::from_secs(5))
            .await
    }

    pub async fn query_capabilities(&self) -> Result<Vec<String>, DynError> {
        self.query_capabilities_with_timeout(Duration::from_secs(1))
            .await
    }

    pub async fn query_capability_descriptors(
        &self,
    ) -> Result<Vec<CapabilityDescriptor>, DynError> {
        self.query_capability_descriptors_with_timeout(Duration::from_secs(1))
            .await
    }

    pub async fn query_rag_collections(&self) -> Result<Vec<RagCollectionSummary>, DynError> {
        self.query_rag_collections_with_timeout(Duration::from_secs(1))
            .await
    }

    pub async fn invoke_tool(&self, capability: &str, arguments: Value) -> Result<Value, DynError> {
        self.admit_tool_call(capability, &arguments)?;
        let invocation_id = Uuid::new_v4();
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::ToolCallRequested {
                invocation_id,
                capability: capability.to_string(),
                arguments,
            },
        )
        .with_correlation(correlation_id.clone());

        println!("[Brain] Invoking tool '{}'", capability);

        let mut sub = self.bus.subscribe(topics::TOOL_RESPONSE).await?;
        self.bus.publish(topics::TOOL_REQUEST, envelope).await?;

        loop {
            match timeout(TOOL_CALL_TIMEOUT, sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    match event.payload {
                        OrdoMessage::ToolCallCompleted {
                            invocation_id: seen_id,
                            capability: seen_capability,
                            result,
                        } if seen_id == invocation_id && seen_capability == capability => {
                            println!("[Brain] Tool '{}' completed", capability);
                            return Ok(result);
                        }
                        OrdoMessage::ToolCallFailed {
                            invocation_id: seen_id,
                            capability: seen_capability,
                            error,
                        } if seen_id == invocation_id && seen_capability == capability => {
                            return Err(Box::new(BrainError::ToolCallFailed {
                                capability: capability.to_string(),
                                error,
                            }));
                        }
                        _ => {}
                    }
                }
                Ok(None) | Err(_) => {
                    return Err(Box::new(BrainError::ToolCallTimedOut {
                        capability: capability.to_string(),
                    }));
                }
            }
        }
    }

    /// Request a self-heal plan and also pre-query RAG so the returned plan's
    /// summary notes how many retrieved context hits were considered. The
    /// underlying self-heal provider does not yet consume the context (that
    /// would require a protocol change), but this surfaces the retrieval that
    /// informs the caller's decision to apply the fix.
    pub async fn request_self_heal_with_context(
        &self,
        incident: SelfHealIncident,
    ) -> Result<SelfHealPlan, DynError> {
        let query = format!("{} {}", incident.symptom, incident.fingerprint);
        let collections = infer_rag_collections(&query);
        let context = self
            .query_rag_with_timeout(&query, &collections, 3, Duration::from_millis(500))
            .await
            .unwrap_or_default();
        if !context.is_empty() {
            println!(
                "[Brain] Self-heal informed by {} RAG context hit(s) from {:?}",
                context.len(),
                collections
            );
        }

        let mut plan = self.request_self_heal(incident).await?;
        if !context.is_empty() {
            plan.summary = format!(
                "{} (informed by {} retrieved context hit{})",
                plan.summary,
                context.len(),
                if context.len() == 1 { "" } else { "s" }
            );
        }
        Ok(plan)
    }

    pub async fn request_self_heal(
        &self,
        incident: SelfHealIncident,
    ) -> Result<SelfHealPlan, DynError> {
        let correlation_id = CorrelationId::new();
        let fingerprint = incident.fingerprint.clone();
        let incident_id = incident.incident_id;
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::SelfHealRequested { incident },
        )
        .with_correlation(correlation_id.clone());

        println!("[Brain] Requesting self-heal for '{}'", fingerprint);

        let mut sub = self.bus.subscribe(topics::SELF_HEAL_RESPONSE).await?;
        self.bus
            .publish(topics::SELF_HEAL_REQUEST, envelope)
            .await?;

        loop {
            match timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::SelfHealPlanned {
                        incident_id: seen_incident_id,
                        fingerprint: seen_fingerprint,
                        plan,
                    } = event.payload
                    {
                        if seen_incident_id == incident_id && seen_fingerprint == fingerprint {
                            println!(
                                "[Brain] Self-heal plan ready for '{}' via {:?}",
                                seen_fingerprint, plan.source
                            );
                            return Ok(plan);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err(Box::new(BrainError::SelfHealTimedOut { fingerprint }));
                }
            }
        }
    }

    async fn query_rag_with_timeout(
        &self,
        query: &str,
        collections: &[String],
        top_k: usize,
        wait: Duration,
    ) -> Result<Vec<RagHit>, DynError> {
        let collections = collections.to_vec();
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::RagQueryRequested {
                query: query.to_string(),
                top_k,
                collections: collections.clone(),
            },
        )
        .with_correlation(correlation_id.clone());

        if collections.is_empty() {
            println!("[Brain] Querying RAG: '{}' collections=all", query);
        } else {
            println!(
                "[Brain] Querying RAG: '{}' collections={:?}",
                query, collections
            );
        }

        let mut sub = self.bus.subscribe(topics::RAG_QUERY_RESPONSE).await?;
        self.bus
            .publish(topics::RAG_QUERY_REQUEST, envelope)
            .await?;

        loop {
            match timeout(wait, sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::RagQueryCompleted {
                        query: seen_query,
                        hits,
                    } = event.payload
                    {
                        if seen_query == query {
                            println!(
                                "[Brain] RAG returned {} hit(s) for '{}'",
                                hits.len(),
                                seen_query
                            );
                            return Ok(hits);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err(Box::new(BrainError::RagQueryTimedOut {
                        query: query.to_string(),
                    }));
                }
            }
        }
    }

    async fn query_capabilities_with_timeout(
        &self,
        wait: Duration,
    ) -> Result<Vec<String>, DynError> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::CapabilityInventoryRequested,
        )
        .with_correlation(correlation_id.clone());

        let mut sub = self
            .bus
            .subscribe(topics::CAPABILITY_INVENTORY_RESPONSE)
            .await?;
        self.bus
            .publish(topics::CAPABILITY_INVENTORY_REQUEST, envelope)
            .await?;

        loop {
            match timeout(wait, sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::CapabilityInventorySnapshot { capabilities, .. } =
                        event.payload
                    {
                        println!(
                            "[Brain] Capability inventory returned {} capability(s)",
                            capabilities.len()
                        );
                        return Ok(capabilities);
                    }
                }
                Ok(None) | Err(_) => {
                    return Err(Box::new(BrainError::CapabilityInventoryTimedOut));
                }
            }
        }
    }

    async fn query_capability_descriptors_with_timeout(
        &self,
        wait: Duration,
    ) -> Result<Vec<CapabilityDescriptor>, DynError> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::CapabilityInventoryRequested,
        )
        .with_correlation(correlation_id.clone());

        let mut sub = self
            .bus
            .subscribe(topics::CAPABILITY_INVENTORY_RESPONSE)
            .await?;
        self.bus
            .publish(topics::CAPABILITY_INVENTORY_REQUEST, envelope)
            .await?;

        loop {
            match timeout(wait, sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::CapabilityInventorySnapshot { descriptors, .. } =
                        event.payload
                    {
                        println!(
                            "[Brain] Capability inventory returned {} descriptor(s)",
                            descriptors.len()
                        );
                        return Ok(descriptors);
                    }
                }
                Ok(None) | Err(_) => {
                    return Err(Box::new(BrainError::CapabilityInventoryTimedOut));
                }
            }
        }
    }

    async fn query_rag_collections_with_timeout(
        &self,
        wait: Duration,
    ) -> Result<Vec<RagCollectionSummary>, DynError> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(self.node_id.clone(), OrdoMessage::RagCollectionsRequested)
            .with_correlation(correlation_id.clone());

        let mut sub = self.bus.subscribe(topics::RAG_COLLECTIONS_RESPONSE).await?;
        self.bus
            .publish(topics::RAG_COLLECTIONS_REQUEST, envelope)
            .await?;

        loop {
            match timeout(wait, sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::RagCollectionsListed { collections } = event.payload {
                        println!(
                            "[Brain] RAG collection inventory returned {} collection(s)",
                            collections.len()
                        );
                        return Ok(collections);
                    }
                }
                Ok(None) | Err(_) => {
                    return Err(Box::new(BrainError::RagCollectionsTimedOut));
                }
            }
        }
    }

    async fn prepare_goal(
        &self,
        goal: &str,
    ) -> Result<(Vec<RagHit>, Option<ExecutionPlan>), DynError> {
        let context = if should_use_rag_context(goal) {
            let collections = infer_rag_collections(goal);
            match self
                .query_rag_with_timeout(goal, &collections, 3, Duration::from_millis(500))
                .await
            {
                Ok(hits) => {
                    if !hits.is_empty() {
                        println!(
                            "[Brain] Using {} RAG context hit(s) for run from {:?}",
                            hits.len(),
                            collections
                        );
                    }
                    hits
                }
                Err(_) => {
                    println!("[Brain] No RAG context available for run; continuing bare");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        let available_capabilities = match self
            .query_capabilities_with_timeout(Duration::from_millis(500))
            .await
        {
            Ok(capabilities) => capabilities,
            Err(_) => {
                println!(
                    "[Brain] Capability inventory unavailable; planning against local heuristics"
                );
                Vec::new()
            }
        };
        let planner = RuleBasedPlanner;
        let plan = match planner.plan_with_capabilities(goal, &context, &available_capabilities) {
            Ok(plan) => {
                println!("[Brain] Planned {} step(s) for run", plan.steps.len());
                Some(plan)
            }
            Err(err) => {
                println!(
                    "[Brain] Planner could not structure run; falling back: {}",
                    err
                );
                None
            }
        };

        Ok((context, plan))
    }

    pub async fn listen_heartbeats(&self) -> Result<(), DynError> {
        let mut sub = self.bus.subscribe(topics::HEARTBEAT).await?;
        println!("[Brain] Listening for node heartbeats...");

        while let Some(envelope) = sub.next().await {
            if let OrdoMessage::Heartbeat(status) = envelope.payload {
                println!(
                    "[Brain] Node discovered: {} ({:?}) with capabilities: {:?}",
                    status.name, status.id.0, status.capabilities
                );
            }
        }

        Ok(())
    }
}

fn ensure_step(
    steps: &mut Vec<RunStepSummary>,
    step_id: Uuid,
    fallback_name: impl Into<String>,
) -> &mut RunStepSummary {
    let fallback_name = fallback_name.into();
    if let Some(index) = steps.iter().position(|step| step.step_id == step_id) {
        return &mut steps[index];
    }

    steps.push(RunStepSummary {
        step_id,
        name: fallback_name,
        output: None,
        error: None,
    });
    steps
        .last_mut()
        .expect("step summary should exist after push")
}

fn should_use_rag_context(goal: &str) -> bool {
    let lowered = goal.to_ascii_lowercase();
    let is_file_operation = lowered.contains("read file")
        || lowered.contains("write file")
        || lowered.starts_with("read ")
        || lowered.starts_with("write ");
    // Knowledge goals always benefit from RAG context. File operations also
    // benefit: retrieved snippets travel alongside the filesystem step so the
    // caller sees what the runtime was informed by when it executed the step.
    is_file_operation || is_knowledge_goal(goal)
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use ordo_bus::{Bus, InProcessBus};
    use ordo_mcp_host::{FilesystemProvider, McpHost, RuntimeInfoProvider, RuntimePolicySnapshot};
    use ordo_memory::MemoryPeer;
    use ordo_protocol::{CapabilityLaneGroup, RagDocument};
    use ordo_rag::RagPeer;
    use serde_json::json;

    use super::Brain;

    #[tokio::test]
    async fn run_goal_collects_lifecycle_events() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(FilesystemProvider::default()));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let temp_path = std::env::temp_dir().join("ordo-run-goal-test.txt");
        std::fs::write(&temp_path, "hello from ordo").expect("write test file");

        let brain = Brain::new(bus);
        let summary = brain
            .run_goal(&format!("read file \"{}\"", temp_path.display()))
            .await
            .expect("run summary");

        assert!(summary.accepted);
        assert!(summary.finished);
        assert!(summary.succeeded);
        assert!(summary.used_explicit_plan);
        assert_eq!(summary.planned_steps, 1);
        assert_eq!(
            summary.planned_capabilities,
            vec!["filesystem.read_file".to_string()]
        );
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.context_hits, 0);
        assert_eq!(summary.steps.len(), 1);
        assert_eq!(summary.steps[0].name, "filesystem.read_file");
        assert!(summary.steps[0]
            .output
            .as_ref()
            .expect("step output")
            .contains("hello from ordo"));

        host_task.abort();
        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn plan_goal_structures_write_file_task() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let brain = Brain::new(bus);

        let plan = brain
            .plan_goal(r#"write file "notes.txt" with "hello world""#)
            .await
            .expect("write plan");

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].capability, "filesystem.write_file");
        assert_eq!(
            plan.steps[0]
                .arguments
                .get("content")
                .and_then(|value| value.as_str()),
            Some("hello world")
        );
    }

    #[tokio::test]
    async fn plan_goal_routes_question_into_answer_capability() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let brain = Brain::new(bus);

        let plan = brain
            .plan_goal("why is retrieval lazy in the standard profile?")
            .await
            .expect("question plan");

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].capability, "knowledge.answer_question");
    }

    #[tokio::test]
    async fn invoke_tool_reads_file_via_capability_surface() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(FilesystemProvider::default()));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let temp_path = std::env::temp_dir().join("codex-ordo-tool-read-test.txt");
        std::fs::write(&temp_path, "tool invocation path").expect("write test file");
        let expected_path = temp_path.display().to_string();

        let brain = Brain::new(bus);
        let result = brain
            .invoke_tool("filesystem.read_file", json!({ "path": expected_path }))
            .await
            .expect("tool result");

        assert_eq!(
            result.get("path").and_then(|value| value.as_str()),
            Some(expected_path.as_str())
        );
        assert!(result
            .get("preview")
            .and_then(|value| value.as_str())
            .expect("preview")
            .contains("tool invocation path"));

        host_task.abort();
        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn filesystem_read_surfaces_planner_attached_context() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(FilesystemProvider::default()));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let temp_path = std::env::temp_dir().join("codex-ordo-tool-context-test.txt");
        std::fs::write(&temp_path, "contextual read").expect("write test file");
        let expected_path = temp_path.display().to_string();

        let brain = Brain::new(bus);
        let result = brain
            .invoke_tool(
                "filesystem.read_file",
                json!({
                    "path": expected_path,
                    "context_hits": 2,
                    "context_sources": ["brief#0", "spec#1"],
                    "context_snippets": ["the brief says X", "the spec says Y"],
                }),
            )
            .await
            .expect("tool result");

        assert_eq!(
            result.get("context_hits").and_then(|value| value.as_u64()),
            Some(2)
        );
        let sources = result
            .get("context_sources")
            .and_then(|value| value.as_array())
            .expect("context_sources");
        assert_eq!(sources.len(), 2);

        host_task.abort();
        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn invoke_tool_answers_question_via_knowledge_surface() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(ordo_mcp_host::KnowledgeProvider));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        let result = brain
            .invoke_tool(
                "knowledge.answer_question",
                json!({
                    "goal": "why is retrieval lazy in the standard profile?",
                    "snippets": [
                        "The default standard profile keeps retrieval configured and discoverable, but only boots the retrieval peer when retrieval is actually used."
                    ],
                    "sources": ["README#0"],
                }),
            )
            .await
            .expect("knowledge answer result");

        assert!(result
            .get("answer")
            .and_then(|value| value.as_str())
            .expect("answer")
            .contains("retrieval peer"));

        host_task.abort();
    }

    #[tokio::test]
    async fn invoke_tool_reads_runtime_policy_surface() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(RuntimeInfoProvider::new(RuntimePolicySnapshot {
            profile: "standard".to_string(),
            control_api_bind: Some("127.0.0.1:4141".to_string()),
            rag_enabled: true,
            knowledge_enabled: true,
            rag_activation_mode: "lazy".to_string(),
            knowledge_activation_mode: "lazy".to_string(),
            rag_budget_bytes: 1024,
            memory_working_budget_bytes: 2048,
            memory_pinned_budget_bytes: 4096,
            self_heal_history_budget_bytes: 512,
            self_heal_llama_cpp_binary: None,
            self_heal_model_path: None,
            self_heal_model_context_size: 4096,
            self_heal_model_max_tokens: 384,
            self_heal_model_temperature: 0.1,
            llama_cpp_configured: false,
            embedding_backend: "hashing".to_string(),
            embedding_dimensions: 96,
            embedding_llama_cpp_binary: None,
            embedding_model_path: None,
            embedding_context_size: 512,
            embedding_ollama_url: None,
            embedding_ollama_model: None,
        })));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        let result = brain
            .invoke_tool("runtime.describe_profile", json!({}))
            .await
            .expect("runtime profile result");

        assert_eq!(
            result.get("profile").and_then(|value| value.as_str()),
            Some("standard")
        );
        assert_eq!(
            result.get("rag_enabled").and_then(|value| value.as_bool()),
            Some(true)
        );

        host_task.abort();
    }

    #[tokio::test]
    async fn invoke_tool_updates_persisted_runtime_settings() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let settings_path = std::env::temp_dir().join("codex-ordo-runtime-settings-test.db");
        let _ = std::fs::remove_file(&settings_path);

        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(RuntimeInfoProvider::with_settings_path(
            RuntimePolicySnapshot {
                profile: "standard".to_string(),
                control_api_bind: Some("127.0.0.1:4141".to_string()),
                rag_enabled: true,
                knowledge_enabled: true,
                rag_activation_mode: "lazy".to_string(),
                knowledge_activation_mode: "lazy".to_string(),
                rag_budget_bytes: 1024,
                memory_working_budget_bytes: 2048,
                memory_pinned_budget_bytes: 4096,
                self_heal_history_budget_bytes: 512,
                self_heal_llama_cpp_binary: None,
                self_heal_model_path: None,
                self_heal_model_context_size: 4096,
                self_heal_model_max_tokens: 384,
                self_heal_model_temperature: 0.1,
                llama_cpp_configured: false,
                embedding_backend: "hashing".to_string(),
                embedding_dimensions: 96,
                embedding_llama_cpp_binary: None,
                embedding_model_path: None,
                embedding_context_size: 512,
                embedding_ollama_url: None,
                embedding_ollama_model: None,
            },
            settings_path.clone(),
        )));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        let update_result = brain
            .invoke_tool(
                "runtime.update_settings",
                json!({
                    "profile": "full",
                    "rag_budget_bytes": 8192,
                    "self_heal_llama_cpp_binary": "C:/llama/llama-cli.exe",
                    "self_heal_model_path": "C:/models/repair.gguf",
                    "self_heal_model_context_size": 8192,
                    "self_heal_model_max_tokens": 512,
                    "self_heal_model_temperature": 0.2,
                }),
            )
            .await
            .expect("runtime settings update");

        assert_eq!(
            update_result
                .get("restart_required")
                .and_then(|value| value.as_bool()),
            Some(true)
        );

        let describe_result = brain
            .invoke_tool("runtime.describe_settings", json!({}))
            .await
            .expect("runtime settings description");

        assert_eq!(
            describe_result
                .get("persisted")
                .and_then(|value| value.get("profile"))
                .and_then(|value| value.as_str()),
            Some("full")
        );
        assert_eq!(
            describe_result
                .get("persisted")
                .and_then(|value| value.get("rag_budget_bytes"))
                .and_then(|value| value.as_u64()),
            Some(8192)
        );
        assert_eq!(
            describe_result
                .get("persisted")
                .and_then(|value| value.get("self_heal_llama_cpp_binary"))
                .and_then(|value| value.as_str()),
            Some("C:/llama/llama-cli.exe")
        );
        assert_eq!(
            describe_result
                .get("persisted")
                .and_then(|value| value.get("self_heal_model_path"))
                .and_then(|value| value.as_str()),
            Some("C:/models/repair.gguf")
        );
        assert_eq!(
            describe_result
                .get("persisted")
                .and_then(|value| value.get("self_heal_model_context_size"))
                .and_then(|value| value.as_u64()),
            Some(8192)
        );
        assert_eq!(
            describe_result
                .get("persisted")
                .and_then(|value| value.get("self_heal_model_max_tokens"))
                .and_then(|value| value.as_u64()),
            Some(512)
        );
        assert_eq!(
            describe_result
                .get("persisted")
                .and_then(|value| value.get("self_heal_model_temperature"))
                .and_then(|value| value.as_f64()),
            Some(0.2)
        );

        host_task.abort();
        let _ = std::fs::remove_file(&settings_path);
    }

    #[tokio::test]
    async fn invoke_tool_pins_and_lists_memory() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut memory = MemoryPeer::new(bus.clone());
        let memory_task = tokio::spawn(async move {
            let _ = memory.run().await;
        });

        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(ordo_mcp_host::MemoryToolsProvider::new(
            bus.clone(),
        )));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        let pin_result = brain
            .invoke_tool(
                "memory.pin_note",
                json!({ "content": "Keep 50 GB pinned for critical platform memory." }),
            )
            .await
            .expect("pin memory result");

        assert_eq!(
            pin_result.get("tier").and_then(|value| value.as_str()),
            Some("pinned")
        );

        let list_result = brain
            .invoke_tool("memory.list_pinned", json!({ "limit": 5 }))
            .await
            .expect("list pinned memory result");

        assert_eq!(
            list_result.get("tier").and_then(|value| value.as_str()),
            Some("pinned")
        );
        assert!(
            list_result
                .get("results")
                .and_then(|value| value.as_array())
                .expect("pinned list")
                .iter()
                .any(|value| value.as_str()
                    == Some("Keep 50 GB pinned for critical platform memory."))
        );

        let unpin_result = brain
            .invoke_tool(
                "memory.unpin_note",
                json!({ "content": "Keep 50 GB pinned for critical platform memory." }),
            )
            .await
            .expect("unpin memory result");

        assert_eq!(
            unpin_result
                .get("removed")
                .and_then(|value| value.as_bool()),
            Some(true)
        );

        let list_after_unpin = brain
            .invoke_tool("memory.list_pinned", json!({ "limit": 5 }))
            .await
            .expect("list pinned memory after unpin");

        assert!(!list_after_unpin
            .get("results")
            .and_then(|value| value.as_array())
            .expect("results after unpin")
            .iter()
            .any(|value| {
                value.as_str() == Some("Keep 50 GB pinned for critical platform memory.")
            }));

        host_task.abort();
        memory_task.abort();
    }

    #[tokio::test]
    async fn query_capabilities_reads_live_inventory() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(FilesystemProvider::default()));
        host.add_provider(Arc::new(RuntimeInfoProvider::new(RuntimePolicySnapshot {
            profile: "standard".to_string(),
            control_api_bind: Some("127.0.0.1:4141".to_string()),
            rag_enabled: true,
            knowledge_enabled: true,
            rag_activation_mode: "lazy".to_string(),
            knowledge_activation_mode: "lazy".to_string(),
            rag_budget_bytes: 1024,
            memory_working_budget_bytes: 2048,
            memory_pinned_budget_bytes: 4096,
            self_heal_history_budget_bytes: 512,
            self_heal_llama_cpp_binary: None,
            self_heal_model_path: None,
            self_heal_model_context_size: 4096,
            self_heal_model_max_tokens: 384,
            self_heal_model_temperature: 0.1,
            llama_cpp_configured: false,
            embedding_backend: "hashing".to_string(),
            embedding_dimensions: 96,
            embedding_llama_cpp_binary: None,
            embedding_model_path: None,
            embedding_context_size: 512,
            embedding_ollama_url: None,
            embedding_ollama_model: None,
        })));
        host.add_provider(Arc::new(ordo_mcp_host::KnowledgeProvider));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        let capabilities = brain.query_capabilities().await.expect("inventory");

        assert!(capabilities.contains(&"filesystem.read_file".to_string()));
        assert!(capabilities.contains(&"knowledge.summarize".to_string()));
        assert!(capabilities.contains(&"knowledge.answer_question".to_string()));
        assert!(capabilities.contains(&"knowledge.compare_sources".to_string()));
        assert!(capabilities.contains(&"knowledge.identify_followups".to_string()));
        assert!(capabilities.contains(&"runtime.describe_profile".to_string()));

        host_task.abort();
    }

    #[tokio::test]
    async fn query_capability_descriptors_reads_metadata() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(FilesystemProvider::default()));
        host.add_provider(Arc::new(RuntimeInfoProvider::new(RuntimePolicySnapshot {
            profile: "standard".to_string(),
            control_api_bind: Some("127.0.0.1:4141".to_string()),
            rag_enabled: true,
            knowledge_enabled: true,
            rag_activation_mode: "lazy".to_string(),
            knowledge_activation_mode: "lazy".to_string(),
            rag_budget_bytes: 1024,
            memory_working_budget_bytes: 2048,
            memory_pinned_budget_bytes: 4096,
            self_heal_history_budget_bytes: 512,
            self_heal_llama_cpp_binary: None,
            self_heal_model_path: None,
            self_heal_model_context_size: 4096,
            self_heal_model_max_tokens: 384,
            self_heal_model_temperature: 0.1,
            llama_cpp_configured: false,
            embedding_backend: "hashing".to_string(),
            embedding_dimensions: 96,
            embedding_llama_cpp_binary: None,
            embedding_model_path: None,
            embedding_context_size: 512,
            embedding_ollama_url: None,
            embedding_ollama_model: None,
        })));
        host.add_provider(Arc::new(ordo_mcp_host::KnowledgeProvider));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        let descriptors = brain
            .query_capability_descriptors()
            .await
            .expect("descriptor inventory");

        assert!(descriptors.iter().any(|descriptor| {
            descriptor.capability == "filesystem.read_file"
                && descriptor.provider == "filesystem"
                && descriptor.lane.group == CapabilityLaneGroup::System
                && descriptor.lane.label == "Filesystem"
        }));
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.capability == "knowledge.summarize"
                && descriptor.provider == "knowledge"
                && descriptor.lane.group == CapabilityLaneGroup::System
                && descriptor.lane.label == "Knowledge"
        }));
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.capability == "knowledge.answer_question"
                && descriptor.provider == "knowledge"
                && descriptor.lane.group == CapabilityLaneGroup::System
        }));

        host_task.abort();
    }

    #[tokio::test]
    async fn rag_round_trip_collects_hits() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut rag = RagPeer::new(bus.clone());
        let rag_task = tokio::spawn(async move {
            let _ = rag.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        let chunk_count = brain
            .ingest_document(RagDocument {
                document_id: "readme".to_string(),
                uri: "README.md".to_string(),
                title: "Readme".to_string(),
                tags: vec!["docs".to_string()],
                collection: "main".to_string(),
                content: "Tokio bus routing and retrieval make the platform local first."
                    .to_string(),
            })
            .await
            .expect("ingest doc");
        assert!(chunk_count > 0);

        let hits = brain
            .query_rag("tokio retrieval", 3)
            .await
            .expect("rag query");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].document_id, "readme");

        rag_task.abort();
    }

    #[tokio::test]
    async fn run_goal_uses_rag_context_for_knowledge_summary() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(FilesystemProvider::default()));
        host.add_provider(Arc::new(ordo_mcp_host::KnowledgeProvider));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        let mut rag = RagPeer::new(bus.clone());
        let rag_task = tokio::spawn(async move {
            let _ = rag.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        brain
            .ingest_document(RagDocument {
                document_id: "architecture".to_string(),
                uri: "docs/architecture.md".to_string(),
                title: "Architecture".to_string(),
                tags: vec!["docs".to_string(), "architecture".to_string()],
                collection: "main".to_string(),
                content: "Transport adapters allow relay fallback without rebuilding a gateway."
                    .to_string(),
            })
            .await
            .expect("ingest architecture");

        let summary = brain
            .run_goal("summarize transport adapter relay fallback design")
            .await
            .expect("knowledge run");

        assert!(summary.accepted);
        assert!(summary.finished);
        assert!(summary.succeeded);
        assert!(summary.used_explicit_plan);
        assert!(summary.context_hits > 0);
        assert_eq!(summary.planned_steps, 1);
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.steps.len(), 1);
        assert_eq!(summary.steps[0].name, "knowledge.summarize");
        assert!(summary.steps[0]
            .output
            .as_ref()
            .expect("knowledge output")
            .contains("Transport adapters"));

        rag_task.abort();
        host_task.abort();
    }

    #[tokio::test]
    async fn run_goal_uses_rag_context_for_follow_up_extraction() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut host = McpHost::new(bus.clone());
        host.add_provider(Arc::new(FilesystemProvider::default()));
        host.add_provider(Arc::new(ordo_mcp_host::KnowledgeProvider));
        let host_task = tokio::spawn(async move {
            let _ = host.run().await;
        });

        let mut rag = RagPeer::new(bus.clone());
        let rag_task = tokio::spawn(async move {
            let _ = rag.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let brain = Brain::new(bus);
        brain
            .ingest_document(RagDocument {
                document_id: "milestones".to_string(),
                uri: "docs/architecture.md".to_string(),
                title: "Milestones".to_string(),
                tags: vec!["docs".to_string(), "planning".to_string()],
                collection: "main".to_string(),
                content: "Replace the simulated relay adapter with a real relay transport. Feed retrieved context into more providers than the current knowledge task family."
                    .to_string(),
            })
            .await
            .expect("ingest milestones");

        let summary = brain
            .run_goal("what are the next steps for transport?")
            .await
            .expect("follow-up knowledge run");

        assert!(summary.accepted);
        assert!(summary.finished);
        assert!(summary.succeeded);
        assert!(summary.used_explicit_plan);
        assert!(summary.context_hits > 0);
        assert_eq!(summary.planned_steps, 1);
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.steps.len(), 1);
        assert_eq!(summary.steps[0].name, "knowledge.identify_followups");
        assert!(summary.steps[0]
            .output
            .as_ref()
            .expect("follow-up output")
            .contains("Replace the simulated relay adapter"));

        rag_task.abort();
        host_task.abort();
    }
}

#[cfg(test)]
mod guard_tests {
    use std::time::{Duration, Instant};

    use super::{ToolCallGuard, TOOL_CALL_RATE_MAX, TOOL_CALL_RATE_WINDOW};

    #[test]
    fn rate_cap_rejects_fast_storms() {
        let mut guard = ToolCallGuard::default();
        let now = Instant::now();
        // Everything up to the cap is admitted.
        for _ in 0..TOOL_CALL_RATE_MAX {
            assert!(guard.observe("cap.read", 1, now).rate_limited.is_none());
        }
        // One past the cap, still inside the window, is rejected.
        assert_eq!(
            guard.observe("cap.read", 1, now).rate_limited,
            Some(TOOL_CALL_RATE_MAX)
        );
    }

    #[test]
    fn window_pruning_lets_traffic_resume() {
        let mut guard = ToolCallGuard::default();
        let t0 = Instant::now();
        for _ in 0..TOOL_CALL_RATE_MAX {
            let _ = guard.observe("cap.read", 1, t0);
        }
        // Once the window has elapsed the old timestamps prune and calls flow again.
        let later = t0 + TOOL_CALL_RATE_WINDOW + Duration::from_secs(1);
        assert!(guard.observe("cap.read", 1, later).rate_limited.is_none());
    }

    #[test]
    fn consecutive_identical_calls_are_counted_and_reset() {
        let mut guard = ToolCallGuard::default();
        let now = Instant::now();
        assert_eq!(guard.observe("cap.read", 7, now).consecutive, 1);
        assert_eq!(guard.observe("cap.read", 7, now).consecutive, 2);
        assert_eq!(guard.observe("cap.read", 7, now).consecutive, 3);
        // A different fingerprint (different capability or arguments) resets the streak.
        assert_eq!(guard.observe("cap.read", 8, now).consecutive, 1);
    }

    #[test]
    fn rate_limited_calls_do_not_report_consecutive() {
        let mut guard = ToolCallGuard::default();
        let now = Instant::now();
        for _ in 0..TOOL_CALL_RATE_MAX {
            let _ = guard.observe("cap.read", 7, now);
        }
        // The rejected call must NOT advance the consecutive counter, so the
        // misleading "still serving the call" warning is never emitted for it.
        let decision = guard.observe("cap.read", 7, now);
        assert_eq!(decision.rate_limited, Some(TOOL_CALL_RATE_MAX));
        assert_eq!(decision.consecutive, 0);
    }
}
