use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub mod build;
pub mod cloud;
pub mod mcp;
pub mod memory;
pub mod secrets;
pub use build::{
    build_topics, BuildArtifactRef, BuildErrorClass, BuildGateEvidence, BuildGateResult,
    BuildPlannerEvent, BuildStep, BuildStepCompletedSignal, GateOutcome,
};
pub use cloud::{cloud_topics, CloudCredentialFull, CloudCredentialView};
pub use mcp::{
    mcp_topics, ArgumentConstraint, AttenuationConstraints, Attestation, CapabilityDeclaration,
    DpopProof, HostCallOutcome, HostCallRecord, McpExtractionError, McpExtractionResult,
    McpServerLockfile, PrivilegeTier, ProvenanceCheckRequest, ProvenanceCheckResult,
    ResourceLimits, ResourceUsage, ServerIdentity, ServerTrustState, Taint, ToolRiskLevel,
    ToolSchema, TrustClaim,
};
pub use memory::{
    memory_topics, ClassifierNodeChoice, ClassifierOutput, CostHint, FeedbackPolarity,
    FeedbackSignal, FeedbackSource, MemoryEvent, MemoryEventType, MemoryLogFilter, MemoryLogHealth,
    MemoryLogIntegrityReport, MemoryLogQueryByRange, MemoryLogQueryResult, ProjectionBuilt,
    ProjectionRequest, ProtocolViolation, ProtocolViolationType, ProviderRegistration,
    ReplayDegradedReason, RetentionTier, RetrievalSemantics, RouteDecided, RouteMode, Severity,
    TreeNode, Ulid,
};
pub use secrets::{
    secrets_topics, AnchorStatement, AuditEntry, CapabilityCanary, CapabilityHandle, InputCustody,
    NonceCommitment, ProtectionLevel, RotationDue, RotationPolicy, RotationReason, SealingTier,
    SecretAuditEventType, SecretClass, SecretRecord, StructuralOutputCheck,
    ThresholdShareAnnouncement, ThresholdSigningRequest, TransparencyReceipt,
};

pub mod topics {
    pub const ALL: &str = "ordo.*";
    pub const HEARTBEAT: &str = "ordo.heartbeat";
    pub const REQUIREMENT: &str = "ordo.requirement";
    pub const CAPABILITY_RESPONSE: &str = "ordo.capability.response";
    pub const CAPABILITY_INVENTORY_REQUEST: &str = "ordo.capability.inventory.request";
    pub const CAPABILITY_INVENTORY_RESPONSE: &str = "ordo.capability.inventory.response";
    pub const RUN_REQUEST: &str = "ordo.run.request";
    pub const RUN_EVENT: &str = "ordo.run.event";
    pub const BUILD_STEP_COMPLETED: &str = crate::build_topics::STEP_COMPLETED;
    pub const BUILD_GATE_RESULT: &str = crate::build_topics::GATE_RESULT;
    pub const BUILD_PLANNER_EVENT: &str = crate::build_topics::PLANNER_EVENT;
    pub const RAG_INGEST_REQUEST: &str = "ordo.rag.ingest.request";
    pub const RAG_INGEST_RESPONSE: &str = "ordo.rag.ingest.response";
    pub const RAG_COLLECTIONS_REQUEST: &str = "ordo.rag.collections.request";
    pub const RAG_COLLECTIONS_RESPONSE: &str = "ordo.rag.collections.response";
    pub const RAG_QUERY_REQUEST: &str = "ordo.rag.query.request";
    pub const RAG_QUERY_RESPONSE: &str = "ordo.rag.query.response";
    pub const TOOL_REQUEST: &str = "ordo.tool.request";
    pub const TOOL_RESPONSE: &str = "ordo.tool.response";
    pub const MEMORY_STORE_REQUEST: &str = "ordo.memory.store.request";
    pub const MEMORY_STORE_RESPONSE: &str = "ordo.memory.store.response";
    pub const MEMORY_REMOVE_REQUEST: &str = "ordo.memory.remove.request";
    pub const MEMORY_REMOVE_RESPONSE: &str = "ordo.memory.remove.response";
    pub const MEMORY_LIST_REQUEST: &str = "ordo.memory.list.request";
    pub const MEMORY_LIST_RESPONSE: &str = "ordo.memory.list.response";
    pub const MEMORY_QUERY: &str = "ordo.memory.query";
    pub const MEMORY_RESPONSE: &str = "ordo.memory.response";
    pub const SELF_HEAL_REQUEST: &str = "ordo.self_heal.request";
    pub const SELF_HEAL_RESPONSE: &str = "ordo.self_heal.response";
    // Apps primitive (Phase 1.1). `apps.event` is the append-only
    // stream of lifecycle events; one envelope per persisted event.
    pub const APPS_EVENT: &str = "ordo.apps.event";
    // Files primitive (Phase 1.4).
    pub const FILES_EVENT: &str = "ordo.files.event";
    // System state (supervisor groundwork). Single broadcast topic
    // carrying the rolled-up health + activity axes. Publisher
    // (`ordo-supervisor`, separate PR) emits on transitions only;
    // subscribers reflect the latest state. No publisher exists at
    // the time this constant lands — subscribing today blocks until
    // the supervisor PR ships.
    pub const SYSTEM_STATE: &str = "ordo.system.state";
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct NodeId(pub Uuid);

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CorrelationId(pub Uuid);

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrelationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SessionId(pub Uuid);

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub sender: NodeId,
    pub timestamp: DateTime<Utc>,
    pub correlation_id: Option<CorrelationId>,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(sender: NodeId, payload: T) -> Self {
        Self {
            sender,
            timestamp: Utc::now(),
            correlation_id: None,
            payload,
        }
    }

    pub fn with_correlation(mut self, cid: CorrelationId) -> Self {
        self.correlation_id = Some(cid);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrdoMessage {
    // Discovery & Health
    Heartbeat(NodeStatus),
    HealthProbe,
    HealthSnapshot(NodeStatus),

    // System state (supervisor groundwork). Two-axis: `health` is
    // slow-moving, derived from degradation signals (`*.degraded`
    // topics, heartbeat staleness, self-heal outcomes); `activity`
    // is fast-moving, tied to in-flight runs / turns / RAG ingest.
    // Axes are orthogonal — a system can process while degraded —
    // so they're carried separately rather than collapsed. `reason`
    // is human-readable narration for logs + operator surfaces; no
    // thresholds or policy data on the wire (the supervisor owns
    // derivation policy). See `ordo-protocol/CHANGELOG.md`.
    SystemStateChanged {
        health: HealthState,
        activity: ActivityState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    // Cloud credentials (Cycle 2 of 4 for the Cloud tab work).
    // List/Upsert/Remove/Test/SetDefault paired as
    // request+response or request+event. Publisher
    // (`ordo-cloud-bridge`, Cycle 3) and consumer
    // (`ordo-uxi::tab_cloud`, Cycle 4) ship separately;
    // dormant until Cycle 3 lands. See
    // `ordo-protocol/CHANGELOG.md` + `cloud_topics`.
    CloudCredentialsListRequest,
    CloudCredentialsListResponse {
        credentials: Vec<CloudCredentialView>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_service: Option<String>,
    },
    CloudCredentialUpsertRequest {
        credential: CloudCredentialFull,
    },
    CloudCredentialUpserted(CloudCredentialView),
    CloudCredentialRemoveRequest {
        service: String,
    },
    CloudCredentialRemoved {
        service: String,
    },
    CloudCredentialTestRequest {
        service: String,
    },
    CloudCredentialTestResult {
        service: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    CloudCredentialSetDefaultRequest {
        service: String,
    },
    CloudCredentialDefaultChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        service: Option<String>,
    },

    // Build pipeline spine
    BuildStepCompleted(BuildStepCompletedSignal),
    BuildGateResult(BuildGateResult),
    BuildPlannerEvent(BuildPlannerEvent),

    // Orchestration
    RequirementMessage {
        requirement: String,
    },
    CapabilityMessage {
        capability: String,
        description: String,
    },
    CapabilityInventoryRequested,
    CapabilityInventorySnapshot {
        capabilities: Vec<String>,
        descriptors: Vec<CapabilityDescriptor>,
    },

    // Run Lifecycle
    RunRequested {
        run_id: Uuid,
        goal: String,
        context: Vec<RagHit>,
        plan: Option<ExecutionPlan>,
    },
    RunAccepted {
        run_id: Uuid,
    },
    StepStarted {
        run_id: Uuid,
        step_id: Uuid,
        name: String,
    },
    StepCompleted {
        run_id: Uuid,
        step_id: Uuid,
        output: String,
    },
    StepFailed {
        run_id: Uuid,
        step_id: Uuid,
        error: String,
    },
    RunFinished {
        run_id: Uuid,
        status: RunStatus,
        completed_steps: usize,
    },

    // Orchestration (Goal / Task / Policy)
    GoalSubmitted {
        goal_id: Uuid,
        description: String,
    },
    PlanCreated {
        goal_id: Uuid,
        task_count: usize,
        agent_count: usize,
    },
    TaskQueued {
        goal_id: Uuid,
        task_id: Uuid,
        task_type: String,
        assigned_agent: Option<Uuid>,
    },
    TaskStarted {
        goal_id: Uuid,
        task_id: Uuid,
        agent_id: Uuid,
    },
    TaskCompleted {
        goal_id: Uuid,
        task_id: Uuid,
        output: String,
    },
    TaskFailed {
        goal_id: Uuid,
        task_id: Uuid,
        error: String,
    },
    PolicyCheckRequired {
        agent_id: Uuid,
        task_id: Option<Uuid>,
        capability: String,
        risk_level: u8,
    },
    UserApprovalRequired {
        approval_id: Uuid,
        agent_id: Uuid,
        capability: String,
        summary: String,
    },
    GoalCompleted {
        goal_id: Uuid,
        succeeded: bool,
        task_count: usize,
    },

    // Autonomous Jobs
    JobScheduled {
        job_id: Uuid,
        name: String,
        next_run: Option<String>,
    },
    JobTriggered {
        job_id: Uuid,
        name: String,
    },
    JobCompleted {
        job_id: Uuid,
        output: String,
    },
    JobFailed {
        job_id: Uuid,
        error: String,
    },

    // Retrieval-Augmented Generation
    RagIngestRequested {
        document: RagDocument,
    },
    RagDocumentIndexed {
        document_id: String,
        chunk_count: usize,
    },
    RagCollectionsRequested,
    RagCollectionsListed {
        collections: Vec<RagCollectionSummary>,
    },
    RagQueryRequested {
        query: String,
        top_k: usize,
        #[serde(default)]
        collections: Vec<String>,
    },
    RagQueryCompleted {
        query: String,
        hits: Vec<RagHit>,
    },

    // Tool Invocation
    ToolCallRequested {
        invocation_id: Uuid,
        capability: String,
        arguments: Value,
    },
    ToolCallCompleted {
        invocation_id: Uuid,
        capability: String,
        result: Value,
    },
    ToolCallFailed {
        invocation_id: Uuid,
        capability: String,
        error: String,
    },

    // Memory
    MemoryStored {
        content: String,
        tier: MemoryTier,
    },
    MemoryStoreRequested {
        content: String,
        tier: MemoryTier,
    },
    MemoryStoreCompleted {
        content: String,
        tier: MemoryTier,
        stored: bool,
    },
    MemoryRemoveRequested {
        content: String,
        tier: MemoryTier,
    },
    MemoryRemoveCompleted {
        content: String,
        tier: MemoryTier,
        removed: bool,
    },
    MemoryListRequested {
        tier: MemoryTier,
        limit: usize,
    },
    MemoryListed {
        tier: MemoryTier,
        results: Vec<String>,
    },
    MemoryQuery {
        query: String,
    },
    MemoryQueried {
        query: String,
        results: Vec<String>,
    },

    // Self-heal
    SelfHealRequested {
        incident: SelfHealIncident,
    },
    SelfHealPlanned {
        incident_id: Uuid,
        fingerprint: String,
        plan: SelfHealPlan,
    },

    // Model Invocation
    ModelInvocationRequested {
        prompt: String,
    },
    ModelInvocationCompleted {
        response: String,
    },

    // Apps primitive (Phase 1.1). One envelope per persisted `AppEvent`
    // â€” carries the full event so consumers don't need to re-query.
    AppsEvent(AppEvent),

    // Files primitive (Phase 1.4). Fired on upload / delete so
    // subscribers (webhooks, external MCP clients, the studio) see
    // the artifact change live.
    FileUploaded(FileEntry),
    FileDeleted {
        id: Uuid,
        workspace_id: String,
    },

    // -- Hierarchical memory (blueprint v2) ---------------------------
    //
    // Append-only event log, tree-routed retrieval, deterministic
    // projection. Request/response pairs are the common pattern; fire-
    // and-forget notifications have no paired Response variant.

    // Log ops
    MemoryLogAppendRequest {
        event: MemoryEvent,
    },
    MemoryLogAppendResponse {
        event_id: Ulid,
        /// True if the event was already present (idempotent replay
        /// within the dedupe window); caller should not double-count
        /// side effects.
        deduplicated: bool,
    },
    MemoryLogAppended {
        event_id: Ulid,
        event_type: MemoryEventType,
        domain: Option<String>,
    },
    MemoryLogQueryRequest(MemoryLogQueryByRange),
    MemoryLogQueryResponse(MemoryLogQueryResult),
    MemoryLogColdQuery {
        query: MemoryLogQueryByRange,
    },
    MemoryRetentionTransition {
        event_id: Ulid,
        from_tier: RetentionTier,
        to_tier: RetentionTier,
    },

    // Router ops
    MemoryRouteRequest {
        query_id: String,
        query: String,
        domain_hint: Option<String>,
        mode: RouteMode,
        max_providers: u32,
    },
    MemoryRouteResponse(RouteDecided),
    MemoryRouteDecided(RouteDecided),
    MemoryRouteLowConfidence {
        query_id: String,
        best_classifier_confidence: f32,
    },
    MemoryProviderRegister(ProviderRegistration),
    MemoryProviderDeregister {
        provider_id: String,
    },
    MemoryProviderHeartbeat {
        provider_id: String,
        healthy: bool,
    },
    MemoryTreeChange {
        path: String,
        change_type: TreeChangeType,
        before: Option<TreeNode>,
        after: Option<TreeNode>,
    },

    // Projection ops
    MemoryProjectionBuildRequest(ProjectionRequest),
    MemoryProjectionBuildResponse(ProjectionBuilt),
    MemoryProjectionBuilt(ProjectionBuilt),
    MemoryProjectionIdentityOverBudget {
        projection_id: String,
        required_tokens: u32,
        available_tokens: u32,
    },
    MemoryProjectionReplayDegraded {
        projection_id: String,
        reason: ReplayDegradedReason,
    },

    // Feedback
    MemoryFeedbackSignal(FeedbackSignal),

    // Protocol violations (first-class, auto-pinned in the log)
    MemoryProtocolViolation(ProtocolViolation),

    // Memory log health (concern 1). Request/Response is the
    // introspection path; Ok/Degraded are the periodic canary
    // broadcasts that rescue subscribes to. IntegrityResult fires
    // once on startup.
    MemoryLogHealthRequest,
    MemoryLogHealthResponse(MemoryLogHealth),
    MemoryLogHealthOk(MemoryLogHealth),
    MemoryLogHealthDegraded {
        reason: String,
        health: MemoryLogHealth,
    },
    MemoryLogIntegrityResult(MemoryLogIntegrityReport),

    // Turn-scoped query (concern 2).
    MemoryLogQueryByTurnRequest {
        turn_id: String,
    },
    MemoryLogQueryByTurnResponse(MemoryLogQueryResult),

    // -- Secrets (blueprint v2 â€” complete). Every mutation to
    // secret state, every capability issuance, every dereference,
    // every threshold signing step, every audit-chain advance is
    // a first-class bus envelope. Rule 4 + blueprint invariant
    // "every write emits a bus event" applied to secrets
    // specifically.
    SecretsCapabilityIssued {
        handle: CapabilityHandle,
        canary: CapabilityCanary,
    },
    SecretsCapabilityRevoked {
        capability_id: String,
        reason: String,
    },
    SecretsCanaryDetected {
        capability_id: String,
        where_detected: String,
    },
    SecretsCustodyMismatch(InputCustody),
    SecretsStructuralRejection(StructuralOutputCheck),
    SecretsSealTierDegraded {
        from: SealingTier,
        to: SealingTier,
        reason: String,
    },
    SecretsRotationDue(RotationDue),
    SecretsRotationCompleted {
        secret_id: String,
        new_record_id: String,
    },
    SecretsThresholdShareAnnouncement(ThresholdShareAnnouncement),
    SecretsThresholdSigningRequest(ThresholdSigningRequest),
    SecretsThresholdSigningCompleted {
        operation_id: String,
        secret_id: String,
        /// Blake3 of the signed message. The signature itself is
        /// delivered to the requester via a direct correlated
        /// reply, not broadcast â€” invariant: quorum-produced
        /// material never hits pub/sub.
        message_hash: [u8; 32],
    },
    SecretsAuditEntryAppended {
        entry_id: String,
        sequence: u64,
        event_type: SecretAuditEventType,
    },
    SecretsAuditAnchorSigned(AnchorStatement),

    // -------------------------------------------------------------
    // MCP security architecture (Crates Aâ€“E: worker, sandbox,
    // client, registry, provenance). Every new surface is a
    // first-class bus envelope â€” same discipline as the secrets
    // blueprint.
    // -------------------------------------------------------------
    McpWorkerExtract {
        raw_response: serde_json::Value,
        expected_schema: serde_json::Value,
        tool_id: String,
        server_id: String,
        invocation_id: String,
    },
    McpWorkerExtractResult {
        invocation_id: String,
        result: Result<McpExtractionResult, McpExtractionError>,
    },
    McpWorkerStatus {
        worker_id: String,
        active: bool,
        uses_since_spawn: u32,
    },

    McpSandboxInstalled {
        server_id: String,
        lockfile_hash: [u8; 32],
    },
    McpSandboxUninstalled {
        server_id: String,
    },
    McpSandboxInvoke {
        server_id: String,
        invocation_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    McpSandboxInvokeResult {
        invocation_id: String,
        raw_response: Result<serde_json::Value, String>,
        resource_usage: ResourceUsage,
    },
    McpSandboxHostCall(HostCallRecord),
    McpSandboxViolation {
        server_id: String,
        invocation_id: String,
        details: String,
    },

    McpClientInvokeAccepted {
        invocation_id: String,
        server_id: String,
        tool_name: String,
        privilege_tier: PrivilegeTier,
    },
    McpClientInvokeResult {
        invocation_id: String,
        extracted_data: Result<serde_json::Value, String>,
    },
    McpClientAuthDegraded {
        server_id: String,
        reason: String,
    },

    McpRegistryTrustChanged {
        server_id: String,
        from: ServerTrustState,
        to: ServerTrustState,
        reason: String,
    },
    McpRegistryDriftDetected {
        server_id: String,
        details: String,
    },
    McpRegistryQuarantined {
        server_id: String,
        reason: String,
    },
    McpRegistryReAuthorized {
        server_id: String,
        new_lockfile_hash: [u8; 32],
    },

    McpProvenanceCheckRequest(ProvenanceCheckRequest),
    McpProvenanceCheckResult(ProvenanceCheckResult),
    McpProvenanceSanitized {
        event_id: String,
        justification: String,
    },
    McpProvenanceUserConfirmed {
        action_id: String,
    },
    McpProvenanceSensitiveBlocked {
        action: String,
        taint_path: Vec<String>,
    },

    // -- Email remote-control channel -----------------------------
    // ordo-email polls an IMAP inbox, finds commands from
    // authorized senders, publishes them here. The brain/assistant
    // processes them and requests replies via EmailReplyRequested.
    EmailCommandReceived {
        email_id: String,
        from_address: String,
        subject: String,
        body_plain: String,
        #[serde(default)]
        body_html: Option<String>,
        received_at: DateTime<Utc>,
    },
    EmailReplyRequested {
        email_id: String,
        to_address: String,
        subject: String,
        body_plain: String,
        #[serde(default)]
        body_html: Option<String>,
        #[serde(default)]
        in_reply_to_subject: Option<String>,
    },
}

/// Tree mutation kind carried on `MemoryTreeChange` envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeChangeType {
    Upsert,
    Tombstone,
}

pub type BusEnvelope = Envelope<OrdoMessage>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagDocument {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    pub tags: Vec<String>,
    #[serde(default = "default_rag_collection_name")]
    pub collection: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RagHit {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    pub chunk_index: usize,
    pub score: f32,
    pub snippet: String,
    pub tags: Vec<String>,
    #[serde(default = "default_rag_collection_name")]
    pub collection: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RagCollectionGroup {
    Shared,
    Domain,
    Interface,
    Custom,
}

impl RagCollectionGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Shared => "Shared",
            Self::Domain => "Domain",
            Self::Interface => "Interface",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagCollectionSummary {
    pub name: String,
    pub label: String,
    pub group: RagCollectionGroup,
    pub document_count: usize,
    pub chunk_count: usize,
    pub sample_titles: Vec<String>,
}

pub const RAG_COLLECTION_MAIN: &str = "main";
pub const RAG_COLLECTION_PLANNING: &str = "planning";
pub const RAG_COLLECTION_ORCHESTRATION: &str = "orchestration";
pub const RAG_COLLECTION_RESEARCH: &str = "research";
pub const RAG_COLLECTION_CONTENT_STORE: &str = "content_store";
pub const RAG_COLLECTION_SSH: &str = "ssh";
pub const RAG_COLLECTION_API: &str = "api";
pub const RAG_COLLECTION_REST: &str = "rest";

/// Reserved but unnamed domain RAG slots. The platform pre-provisions
/// ten domain collections so operators can claim them later without a
/// schema migration; naming them is deferred until there's an actual
/// orchestration to attach. Slots 1â€“4 are filled by planning / orchestration /
/// research / content_store today; 5â€“10 are empty until an operator opts in.
pub const RAG_COLLECTION_DOMAIN_SLOT_5: &str = "domain_slot_5";
pub const RAG_COLLECTION_DOMAIN_SLOT_6: &str = "domain_slot_6";
pub const RAG_COLLECTION_DOMAIN_SLOT_7: &str = "domain_slot_7";
pub const RAG_COLLECTION_DOMAIN_SLOT_8: &str = "domain_slot_8";
pub const RAG_COLLECTION_DOMAIN_SLOT_9: &str = "domain_slot_9";
pub const RAG_COLLECTION_DOMAIN_SLOT_10: &str = "domain_slot_10";

/// All ten domain RAG slots in the canonical display order. The first
/// four are named; the remaining six are reserved placeholders.
pub const RAG_DOMAIN_SLOTS: &[&str] = &[
    RAG_COLLECTION_PLANNING,
    RAG_COLLECTION_ORCHESTRATION,
    RAG_COLLECTION_RESEARCH,
    RAG_COLLECTION_CONTENT_STORE,
    RAG_COLLECTION_DOMAIN_SLOT_5,
    RAG_COLLECTION_DOMAIN_SLOT_6,
    RAG_COLLECTION_DOMAIN_SLOT_7,
    RAG_COLLECTION_DOMAIN_SLOT_8,
    RAG_COLLECTION_DOMAIN_SLOT_9,
    RAG_COLLECTION_DOMAIN_SLOT_10,
];

pub fn default_rag_collection_name() -> String {
    RAG_COLLECTION_MAIN.to_string()
}

pub fn normalize_rag_collection_name(name: &str) -> String {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        default_rag_collection_name()
    } else {
        normalized
    }
}

pub fn normalize_rag_collections(collections: &[String]) -> Vec<String> {
    let mut normalized = collections
        .iter()
        .map(|collection| normalize_rag_collection_name(collection))
        .collect::<Vec<_>>();
    normalized.sort_by(|left, right| {
        rag_collection_rank(left)
            .cmp(&rag_collection_rank(right))
            .then_with(|| left.cmp(right))
    });
    normalized.dedup();
    normalized
}

pub fn rag_collection_label(name: &str) -> &'static str {
    match name {
        RAG_COLLECTION_MAIN => "Main",
        RAG_COLLECTION_PLANNING => "Planning",
        RAG_COLLECTION_ORCHESTRATION => "Orchestration",
        RAG_COLLECTION_RESEARCH => "Research",
        RAG_COLLECTION_CONTENT_STORE => "Content Store",
        RAG_COLLECTION_SSH => "SSH",
        RAG_COLLECTION_API => "API",
        RAG_COLLECTION_REST => "REST API",
        RAG_COLLECTION_DOMAIN_SLOT_5 => "Domain Slot 5",
        RAG_COLLECTION_DOMAIN_SLOT_6 => "Domain Slot 6",
        RAG_COLLECTION_DOMAIN_SLOT_7 => "Domain Slot 7",
        RAG_COLLECTION_DOMAIN_SLOT_8 => "Domain Slot 8",
        RAG_COLLECTION_DOMAIN_SLOT_9 => "Domain Slot 9",
        RAG_COLLECTION_DOMAIN_SLOT_10 => "Domain Slot 10",
        _ => "Custom",
    }
}

pub fn rag_collection_group(name: &str) -> RagCollectionGroup {
    match name {
        RAG_COLLECTION_MAIN => RagCollectionGroup::Shared,
        RAG_COLLECTION_PLANNING
        | RAG_COLLECTION_ORCHESTRATION
        | RAG_COLLECTION_RESEARCH
        | RAG_COLLECTION_CONTENT_STORE
        | RAG_COLLECTION_DOMAIN_SLOT_5
        | RAG_COLLECTION_DOMAIN_SLOT_6
        | RAG_COLLECTION_DOMAIN_SLOT_7
        | RAG_COLLECTION_DOMAIN_SLOT_8
        | RAG_COLLECTION_DOMAIN_SLOT_9
        | RAG_COLLECTION_DOMAIN_SLOT_10 => RagCollectionGroup::Domain,
        RAG_COLLECTION_SSH | RAG_COLLECTION_API | RAG_COLLECTION_REST => {
            RagCollectionGroup::Interface
        }
        _ => RagCollectionGroup::Custom,
    }
}

fn rag_collection_rank(name: &str) -> u8 {
    match name {
        RAG_COLLECTION_MAIN => 0,
        RAG_COLLECTION_PLANNING => 1,
        RAG_COLLECTION_ORCHESTRATION => 2,
        RAG_COLLECTION_RESEARCH => 3,
        RAG_COLLECTION_CONTENT_STORE => 4,
        RAG_COLLECTION_DOMAIN_SLOT_5 => 5,
        RAG_COLLECTION_DOMAIN_SLOT_6 => 6,
        RAG_COLLECTION_DOMAIN_SLOT_7 => 7,
        RAG_COLLECTION_DOMAIN_SLOT_8 => 8,
        RAG_COLLECTION_DOMAIN_SLOT_9 => 9,
        RAG_COLLECTION_DOMAIN_SLOT_10 => 10,
        RAG_COLLECTION_SSH => 20,
        RAG_COLLECTION_API => 21,
        RAG_COLLECTION_REST => 22,
        _ => 100,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionPlan {
    pub plan_id: Uuid,
    pub goal: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanStep {
    pub capability: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CapabilityTier {
    Core,
    Optional,
    Heavy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CapabilityActivation {
    Eager,
    Lazy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityLaneGroup {
    Domain,
    Interface,
    System,
}

impl CapabilityLaneGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Domain => "Domain",
            Self::Interface => "Interface",
            Self::System => "System",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CapabilityLane {
    pub group: CapabilityLaneGroup,
    pub name: String,
    pub label: String,
}

impl CapabilityLane {
    pub fn from_capability(capability: &str) -> Self {
        let name = capability
            .split('.')
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or("system");
        let group = capability_lane_group(name);
        let label = capability_lane_label(name);

        Self {
            group,
            name: name.to_string(),
            label,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityLaneSummary {
    pub group: CapabilityLaneGroup,
    pub name: String,
    pub label: String,
    pub count: usize,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    pub capability: String,
    pub provider: String,
    pub description: String,
    pub tier: CapabilityTier,
    pub activation: CapabilityActivation,
    pub lane: CapabilityLane,
    /// Optional JSON Schema describing the shape of `arguments` the
    /// provider expects. Purely **descriptive** â€” consumed by LLM tool
    /// advertisement and the MCP bridge's `tools/list`. Runtime tool
    /// dispatch stays `Value`-in / `Value`-out (Rule 9 â€”
    /// see docs/architecture-contract.md). `None` means "no schema
    /// published"; callers must not infer anything from absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
}

impl CapabilityDescriptor {
    pub fn new(
        capability: impl Into<String>,
        provider: impl Into<String>,
        description: impl Into<String>,
        tier: CapabilityTier,
        activation: CapabilityActivation,
    ) -> Self {
        let capability = capability.into();
        Self {
            lane: CapabilityLane::from_capability(&capability),
            capability,
            provider: provider.into(),
            description: description.into(),
            tier,
            activation,
            input_schema: None,
        }
    }

    /// Attach a JSON Schema describing the argument shape. Providers
    /// can build this with `schemars::schema_for!(MyArgs)` and
    /// `serde_json::to_value(...)` when they have a typed args struct,
    /// or hand-roll it for dynamic tools.
    pub fn with_input_schema(mut self, schema: serde_json::Value) -> Self {
        self.input_schema = Some(schema);
        self
    }
}

pub fn summarize_capability_lanes(
    descriptors: &[CapabilityDescriptor],
) -> Vec<CapabilityLaneSummary> {
    let mut summaries = BTreeMap::<(u8, u8, String), CapabilityLaneSummary>::new();

    for descriptor in descriptors {
        let key = (
            capability_lane_group_rank(descriptor.lane.group),
            capability_lane_name_rank(&descriptor.lane.name),
            descriptor.lane.name.clone(),
        );
        let entry = summaries
            .entry(key)
            .or_insert_with(|| CapabilityLaneSummary {
                group: descriptor.lane.group,
                name: descriptor.lane.name.clone(),
                label: descriptor.lane.label.clone(),
                count: 0,
                capabilities: Vec::new(),
            });
        entry.count += 1;
        entry.capabilities.push(descriptor.capability.clone());
    }

    summaries
        .into_values()
        .map(|mut summary| {
            summary.capabilities.sort();
            summary.capabilities.dedup();
            summary
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum KnowledgeTask {
    Summarize,
    AnswerQuestion,
    CompareSources,
    IdentifyFollowUps,
}

impl KnowledgeTask {
    pub const ALL: [Self; 4] = [
        Self::Summarize,
        Self::AnswerQuestion,
        Self::CompareSources,
        Self::IdentifyFollowUps,
    ];

    pub fn capability(self) -> &'static str {
        match self {
            Self::Summarize => "knowledge.summarize",
            Self::AnswerQuestion => "knowledge.answer_question",
            Self::CompareSources => "knowledge.compare_sources",
            Self::IdentifyFollowUps => "knowledge.identify_followups",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Summarize => "Summarizes retrieved context from the local knowledge index.",
            Self::AnswerQuestion => {
                "Answers a question using retrieved context from the local knowledge index."
            }
            Self::CompareSources => {
                "Compares retrieved context slices to highlight differences and overlap."
            }
            Self::IdentifyFollowUps => {
                "Identifies likely follow-ups or next steps from retrieved context."
            }
        }
    }
}

pub fn infer_knowledge_task(goal: &str) -> Option<KnowledgeTask> {
    let tokens = goal_tokens(goal);

    if has_any_token(
        &tokens,
        &["compare", "difference", "differences", "versus", "vs"],
    ) {
        return Some(KnowledgeTask::CompareSources);
    }

    if has_any_token(
        &tokens,
        &["revisit", "todo", "todos", "followup", "followups"],
    ) || has_pair(&tokens, "next", "step")
        || has_pair(&tokens, "next", "steps")
        || has_pair(&tokens, "follow", "up")
        || has_pair(&tokens, "open", "question")
        || has_pair(&tokens, "open", "questions")
        || has_triple(&tokens, "build", "next", "for")
        || has_triple(&tokens, "build", "next", "on")
    {
        return Some(KnowledgeTask::IdentifyFollowUps);
    }

    if goal.contains('?')
        || has_any_token(
            &tokens,
            &["why", "how", "what", "when", "where", "who", "explain"],
        )
    {
        return Some(KnowledgeTask::AnswerQuestion);
    }

    if has_any_token(
        &tokens,
        &[
            "summarize",
            "summary",
            "overview",
            "architecture",
            "design",
            "transport",
            "rag",
            "knowledge",
        ],
    ) {
        return Some(KnowledgeTask::Summarize);
    }

    None
}

pub fn is_knowledge_goal(goal: &str) -> bool {
    infer_knowledge_task(goal).is_some()
}

pub fn infer_rag_collections(goal: &str) -> Vec<String> {
    let tokens = goal_tokens(goal);
    let mut collections = vec![RAG_COLLECTION_MAIN.to_string()];

    if has_any_token(
        &tokens,
        &[
            "planning",
            "brief",
            "initiative",
            "resource",
            "resources",
            "operator profile",
            "ordo",
            "deliverable",
            "deliverables",
        ],
    ) {
        collections.push(RAG_COLLECTION_PLANNING.to_string());
    }

    if has_any_token(
        &tokens,
        &[
            "orchestration",
            "review",
            "approval",
            "approvals",
            "handoff",
            "handoffs",
            "revision",
            "revisions",
            "pipeline",
            "pipelines",
            "stage",
            "stages",
        ],
    ) {
        collections.push(RAG_COLLECTION_ORCHESTRATION.to_string());
    }

    if has_any_token(
        &tokens,
        &[
            "research", "metadata", "meta", "search", "serp", "slug", "keyword", "keywords",
            "intent", "ranking",
        ],
    ) {
        collections.push(RAG_COLLECTION_RESEARCH.to_string());
    }

    if has_any_token(
        &tokens,
        &[
            "content_store",
            "taxonomy",
            "taxonomies",
            "template",
            "templates",
            "publish",
            "publishing",
            "entry",
            "entries",
            "field",
            "fields",
        ],
    ) {
        collections.push(RAG_COLLECTION_CONTENT_STORE.to_string());
    }

    if has_any_token(
        &tokens,
        &["ssh", "remote", "shell", "host", "server", "terminal"],
    ) {
        collections.push(RAG_COLLECTION_SSH.to_string());
    }

    if has_any_token(
        &tokens,
        &[
            "api",
            "integration",
            "integrations",
            "sdk",
            "webhook",
            "service",
        ],
    ) {
        collections.push(RAG_COLLECTION_API.to_string());
    }

    if has_any_token(
        &tokens,
        &[
            "rest",
            "endpoint",
            "endpoints",
            "http",
            "resource",
            "request",
        ],
    ) {
        collections.push(RAG_COLLECTION_REST.to_string());
    }

    normalize_rag_collections(&collections)
}

fn goal_tokens(goal: &str) -> Vec<String> {
    goal.to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn has_any_token(tokens: &[String], candidates: &[&str]) -> bool {
    tokens
        .iter()
        .any(|token| candidates.iter().any(|candidate| token == candidate))
}

fn has_pair(tokens: &[String], left: &str, right: &str) -> bool {
    tokens
        .windows(2)
        .any(|window| window[0] == left && window[1] == right)
}

fn has_triple(tokens: &[String], first: &str, second: &str, third: &str) -> bool {
    tokens
        .windows(3)
        .any(|window| window[0] == first && window[1] == second && window[2] == third)
}

fn capability_lane_group(name: &str) -> CapabilityLaneGroup {
    match name {
        "planning" | "orchestration" | "research" | "content_store" => CapabilityLaneGroup::Domain,
        "ssh" | "api" | "rest" => CapabilityLaneGroup::Interface,
        _ => CapabilityLaneGroup::System,
    }
}

fn capability_lane_label(name: &str) -> String {
    match name {
        "research" => "Research".to_string(),
        "content_store" => "Content Store".to_string(),
        "ssh" => "SSH".to_string(),
        "api" => "API".to_string(),
        "rest" => "REST API".to_string(),
        "self_heal" => "Self-Heal".to_string(),
        _ => humanize_capability_namespace(name),
    }
}

fn humanize_capability_namespace(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = String::new();
                    word.extend(first.to_uppercase());
                    word.push_str(chars.as_str());
                    word
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn capability_lane_group_rank(group: CapabilityLaneGroup) -> u8 {
    match group {
        CapabilityLaneGroup::Domain => 0,
        CapabilityLaneGroup::Interface => 1,
        CapabilityLaneGroup::System => 2,
    }
}

fn capability_lane_name_rank(name: &str) -> u8 {
    match name {
        "planning" => 0,
        "orchestration" => 1,
        "research" => 2,
        "content_store" => 3,
        "ssh" => 10,
        "api" => 11,
        "rest" => 12,
        "runtime" => 20,
        "filesystem" => 21,
        "knowledge" => 22,
        "memory" => 23,
        "self_heal" => 24,
        _ => 100,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryTier {
    Working,
    Pinned,
}

/// System health axis (supervisor groundwork). Slow-moving,
/// derived from degradation signals — `*.degraded` topics,
/// heartbeat staleness, self-heal outcomes. Carried on
/// `OrdoMessage::SystemStateChanged`. Wire labels are stable;
/// new variants are additive.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Healthy,
    Rescue,
    Critical,
}

/// System activity axis (supervisor groundwork). Fast-moving,
/// tied to in-flight work — runs, turns, RAG ingest. Orthogonal
/// to [`HealthState`]; a system can process while degraded.
/// Carried on `OrdoMessage::SystemStateChanged`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    Idle,
    Processing,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SelfHealUrgency {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SelfHealSource {
    MemoryReuse,
    LlamaCpp,
    DeterministicFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfHealIncident {
    pub incident_id: Uuid,
    pub component: String,
    pub symptom: String,
    pub fingerprint: String,
    pub urgency: SelfHealUrgency,
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfHealPlan {
    pub summary: String,
    pub why: String,
    pub actions: Vec<String>,
    pub source: SelfHealSource,
    pub reused_previous_fix: bool,
    pub memory_hits: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransportKind {
    InProcess,
    Quic,
    TcpNoise,
    RelayQuic,
    UpdatePush,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NatKind {
    Unknown,
    OpenInternet,
    Cone,
    Symmetric,
    RelayOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrustTier {
    LocalProcess,
    PairedPeer,
    TrustedPeer,
    UnknownPeer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PairingMode {
    LocalOnly,
    PairingRequired,
    TrustedOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CryptoSuite {
    InProcess,
    NoiseX25519,
    HybridPqNoiseX25519,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrafficClass {
    Interactive,
    Background,
    Control,
    Replication,
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionTarget {
    LocalOnly,
    BestPeer,
    SpecificPeer(NodeId),
    Broadcast,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerPresence {
    pub id: NodeId,
    pub label: String,
    pub protocol_version: String,
    pub trust_tier: TrustTier,
    pub pairing_mode: PairingMode,
    pub nat_kind: NatKind,
    pub transports: Vec<TransportKind>,
    pub crypto_suites: Vec<CryptoSuite>,
    pub endpoints: Vec<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteDirective {
    pub traffic_class: TrafficClass,
    pub execution_target: ExecutionTarget,
    pub required_capabilities: Vec<String>,
    pub prefer_pq: bool,
    pub allow_relay_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerHello {
    pub peer: PeerPresence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeSelection {
    pub transport: TransportKind,
    pub crypto_suite: CryptoSuite,
    pub relay_required: bool,
    pub pairing_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub id: NodeId,
    pub name: String,
    pub uptime_secs: u64,
    pub version: String,
    pub capabilities: Vec<String>,
}

// -----------------------------------------------------------------------------
// Apps primitive (Phase 1.1).
//
// Wire-shared types for the `ordo-apps` service. Kept here per Rule 11:
// anything that crosses a process boundary (bus, HTTP, MCP bridge,
// webhooks) lives in `ordo-protocol`, not in the emitting crate.
//
// Design notes:
//  - `App` is the folded current state (mirrors the `apps` row).
//  - `AppEvent` is the append-only lifecycle event (mirrors an
//    `app_events` row). Event sourcing is Phase 1.2 â€” the types ship
//    now so downstream consumers can build against a stable shape.
//  - `workspace_id` is on both (Rule 6 â€” multi-tenant from day one).
// -----------------------------------------------------------------------------

/// App lifecycle state. Terminal states are `Archived`; `Published` is
/// reversible to `Draft` via an `unpublish` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    Draft,
    Published,
    Archived,
}

impl AppStatus {
    pub fn label(self) -> &'static str {
        match self {
            AppStatus::Draft => "draft",
            AppStatus::Published => "published",
            AppStatus::Archived => "archived",
        }
    }

    pub fn from_label(label: &str) -> Option<AppStatus> {
        match label {
            "draft" => Some(AppStatus::Draft),
            "published" => Some(AppStatus::Published),
            "archived" => Some(AppStatus::Archived),
            _ => None,
        }
    }
}

/// Folded current state of an app. Reconstructible from its event
/// stream; persisted separately for fast reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct App {
    pub id: Uuid,
    pub workspace_id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub status: AppStatus,
    /// Free-form metadata the provider wants to keep on the app â€”
    /// linked UI extension name, linked plugin names, preview URL,
    /// build artifact path, etc. Untyped per Rule 9.
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub published_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
}

/// Append-only event kind. Add new variants additively; never
/// reassign existing labels (they persist in SQLite).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppEventKind {
    Created {
        slug: String,
        name: String,
        description: String,
    },
    Renamed {
        from: String,
        to: String,
    },
    DescriptionUpdated {
        description: String,
    },
    MetadataSet {
        key: String,
        value: Value,
    },
    MetadataRemoved {
        key: String,
    },
    Published,
    Unpublished,
    Archived,
    Unarchived,
}

/// Multimodal user-message attachment (Phase 1.3).
///
/// Carried alongside `TurnRequest.user_message` so the assistant turn
/// can include images (and later audio/video/files) in the user-role
/// message. Provider-neutral on the wire; each cloud translator maps
/// it to the provider's native shape (OpenAI `image_url` block,
/// Anthropic `image` block).
///
/// New variants are additive â€” consumers that don't recognize a
/// variant should skip it rather than failing the turn. See the
/// translator handling in `ordo-cloud` for the conservative path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserAttachment {
    /// Remote-URL image (typically https). The LLM provider fetches
    /// the bytes directly. Works with OpenAI vision; Anthropic
    /// requires base64, so the translator rejects URL images and
    /// surfaces a clear error (avoids silently dropping the image).
    ImageUrl { url: String },
    /// Inline base64-encoded image. `media_type` is an IANA MIME
    /// (e.g. `image/png`, `image/jpeg`). Works with both OpenAI and
    /// Anthropic.
    ImageBase64 { data: String, media_type: String },
}

/// Deployment lifecycle state (Phase 3.3). A deployment is pending
/// at creation, moves to `live` when promoted, or `failed` if the
/// promotion checks rejected it. Terminal states are `live` and
/// `failed`; a new deployment record is created to supersede.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentState {
    Pending,
    Live,
    Failed,
}

impl DeploymentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Live => "live",
            Self::Failed => "failed",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "pending" => Some(Self::Pending),
            "live" => Some(Self::Live),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// App deployment record (Phase 3.3). Ties a point in an app's event
/// stream (via `app_event_seq`) to an externally-addressable release.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Deployment {
    pub id: Uuid,
    pub app_id: Uuid,
    pub workspace_id: String,
    /// Event sequence that this deployment snapshots. Used with
    /// `state_at_version` to reconstruct exactly what was promoted.
    pub app_event_seq: u64,
    pub state: DeploymentState,
    /// Optional path (under `user_files/`) to a preview bundle â€”
    /// empty when the deployment isn't backed by static resources.
    #[serde(default)]
    pub preview_path: Option<String>,
    pub note: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub promoted_at: Option<DateTime<Utc>>,
}

/// Webhook subscription (Phase 3.1). External subscriber opts in to
/// matching bus events and receives HMAC-signed POSTs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebhookSubscription {
    pub id: Uuid,
    pub workspace_id: String,
    pub target_url: String,
    /// HMAC-SHA256 secret. Never returned in list/read responses
    /// (`skip_serializing_if` is enforced by the service, not the
    /// type â€” keep both in sync).
    pub secret: String,
    pub topics: Vec<String>,
    pub description: String,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_delivery_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_delivery_status: Option<u16>,
}

/// Metadata for a persisted file (Phase 1.4). Bytes live on disk
/// under the runtime's `user_files/` root at `storage_path`; the
/// `files` SQLite row holds everything the platform queries
/// frequently.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub id: Uuid,
    pub workspace_id: String,
    /// Display name as uploaded (with extension, no path separators).
    pub original_name: String,
    /// Path relative to the runtime's `user_files/` root â€” so the DB
    /// remains portable across machines.
    pub storage_path: String,
    /// MIME type. Defaults to `application/octet-stream` when not
    /// declared by the uploader.
    pub content_type: String,
    pub size_bytes: u64,
    /// Lowercase hex-encoded SHA-256 of the bytes. Used for dedupe
    /// and end-to-end integrity checks.
    pub sha256_hex: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    /// Optional owning app (Phase 1.4 â€” keeps uploaded resources in
    /// scope of a single app when the uploader is agent-driven).
    #[serde(default)]
    pub app_id: Option<Uuid>,
}

/// Envelope for a persisted app event. Carries enough context for
/// replay across processes (workspace_id + actor + created_at).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppEvent {
    pub id: Uuid,
    pub app_id: Uuid,
    pub workspace_id: String,
    /// Monotonic per-app sequence; 0 = Created, subsequent events
    /// increment by 1. Gaps signal a corrupted stream.
    pub seq: u64,
    pub actor: String,
    pub created_at: DateTime<Utc>,
    #[serde(flatten)]
    pub event: AppEventKind,
}

#[cfg(test)]
mod tests {
    use super::{
        infer_knowledge_task, infer_rag_collections, is_knowledge_goal, rag_collection_group,
        rag_collection_label, summarize_capability_lanes, CapabilityActivation,
        CapabilityDescriptor, CapabilityLaneGroup, CapabilityTier, KnowledgeTask,
        RagCollectionGroup, RAG_COLLECTION_MAIN,
    };

    #[test]
    fn detects_summary_knowledge_goal() {
        assert_eq!(
            infer_knowledge_task("summarize transport design"),
            Some(KnowledgeTask::Summarize)
        );
    }

    #[test]
    fn detects_question_knowledge_goal_without_matching_show() {
        assert_eq!(
            infer_knowledge_task("why is retrieval lazy in the standard profile?"),
            Some(KnowledgeTask::AnswerQuestion)
        );
        assert_eq!(infer_knowledge_task("show pinned memory"), None);
    }

    #[test]
    fn detects_compare_knowledge_goal() {
        assert_eq!(
            infer_knowledge_task("compare transport and retrieval approaches"),
            Some(KnowledgeTask::CompareSources)
        );
    }

    #[test]
    fn detects_follow_up_knowledge_goal() {
        assert_eq!(
            infer_knowledge_task("what are the next steps for transport?"),
            Some(KnowledgeTask::IdentifyFollowUps)
        );
    }

    #[test]
    fn distinguishes_non_knowledge_goal() {
        assert!(!is_knowledge_goal(r#"read file "Cargo.toml""#));
    }

    #[test]
    fn classifies_capability_lanes_from_namespace() {
        let planning = CapabilityDescriptor::new(
            "planning.capture_brief",
            "planning",
            "Captures a planning brief.",
            CapabilityTier::Core,
            CapabilityActivation::Eager,
        );
        let rest = CapabilityDescriptor::new(
            "rest.prepare_request",
            "rest",
            "Builds a REST request.",
            CapabilityTier::Optional,
            CapabilityActivation::Lazy,
        );
        let repair = CapabilityDescriptor::new(
            "self_heal.export_case",
            "self-heal",
            "Exports a remembered repair pack.",
            CapabilityTier::Core,
            CapabilityActivation::Eager,
        );

        assert_eq!(planning.lane.group, CapabilityLaneGroup::Domain);
        assert_eq!(planning.lane.label, "Planning");
        assert_eq!(rest.lane.group, CapabilityLaneGroup::Interface);
        assert_eq!(rest.lane.label, "REST API");
        assert_eq!(repair.lane.group, CapabilityLaneGroup::System);
        assert_eq!(repair.lane.label, "Self-Heal");
    }

    #[test]
    fn summarizes_capability_lanes_in_group_order() {
        let descriptors = vec![
            CapabilityDescriptor::new(
                "knowledge.summarize",
                "knowledge",
                "Summarizes context.",
                CapabilityTier::Optional,
                CapabilityActivation::Lazy,
            ),
            CapabilityDescriptor::new(
                "research.package_metadata",
                "research",
                "Packages Research metadata.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "research.audit_readiness",
                "research",
                "Audits Research readiness.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "ssh.run_remote_command",
                "ssh",
                "Runs a remote SSH command.",
                CapabilityTier::Optional,
                CapabilityActivation::Lazy,
            ),
        ];

        let lanes = summarize_capability_lanes(&descriptors);

        assert_eq!(lanes.len(), 3);
        assert_eq!(lanes[0].group, CapabilityLaneGroup::Domain);
        assert_eq!(lanes[0].label, "Research");
        assert_eq!(lanes[0].count, 2);
        assert_eq!(lanes[1].group, CapabilityLaneGroup::Interface);
        assert_eq!(lanes[1].label, "SSH");
        assert_eq!(lanes[2].group, CapabilityLaneGroup::System);
        assert_eq!(lanes[2].label, "Knowledge");
    }

    #[test]
    fn infers_main_and_domain_rag_collections() {
        let collections = infer_rag_collections(
            "prepare Research metadata and Content Store publish orchestration for this release",
        );

        assert_eq!(collections[0], RAG_COLLECTION_MAIN);
        assert!(collections.iter().any(|value| value == "research"));
        assert!(collections.iter().any(|value| value == "content_store"));
        assert!(collections.iter().any(|value| value == "orchestration"));
    }

    #[test]
    fn labels_known_rag_collections() {
        assert_eq!(rag_collection_label("main"), "Main");
        assert_eq!(rag_collection_label("research"), "Research");
        assert_eq!(rag_collection_label("rest"), "REST API");
    }

    #[test]
    fn groups_known_rag_collections() {
        assert_eq!(rag_collection_group("main"), RagCollectionGroup::Shared);
        assert_eq!(rag_collection_group("planning"), RagCollectionGroup::Domain);
        assert_eq!(rag_collection_group("api"), RagCollectionGroup::Interface);
        assert_eq!(rag_collection_group("custom"), RagCollectionGroup::Custom);
    }
}
