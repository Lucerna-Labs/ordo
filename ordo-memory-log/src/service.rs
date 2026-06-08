//! `MemoryLogService` â€” the public surface of the append-only log.

use std::sync::Arc;

use std::collections::VecDeque;

use chrono::Utc;
use ordo_bus::Bus;
use ordo_protocol::{
    memory_topics, Envelope, MemoryEvent, MemoryEventType, MemoryLogFilter, MemoryLogHealth,
    MemoryLogIntegrityReport, MemoryLogQueryResult, NodeId, OrdoMessage, ProtocolViolation,
    ProtocolViolationType, RetentionTier, Severity,
};
use parking_lot::Mutex;
use serde_json::json;
use ulid::Ulid;

use crate::store::MemoryLogStore;

/// Dedupe window for idempotent append on `payload_hash`. Tuned to
/// cover realistic bus-retry scenarios without hiding legitimate
/// repeats of the same user action spaced longer than this.
const DEDUPE_WINDOW_MS: i64 = 5_000;

/// Age at which health-probe canaries are soft-deleted by the
/// periodic sweep. 24 hours per the blueprint â€” long enough to
/// debug an incident by walking the recent canary stream; short
/// enough that canary rows never dominate the table.
pub const CANARY_TTL_MS: i64 = 24 * 60 * 60 * 1_000;

/// Rolling window for the `appends_failed_last_hour` counter.
const FAILURE_WINDOW_MS: i64 = 60 * 60 * 1_000;

/// In-memory counters that feed `MemoryLogHealth`. Updated on every
/// `append()` attempt; published on `health()` and on the periodic
/// canary broadcasts.
#[derive(Default)]
struct HealthCounters {
    appends_attempted: u64,
    appends_succeeded: u64,
    /// Timestamps (epoch ms) of the last hour's failures, oldest
    /// first. `appends_failed_last_hour` is the length of this deque
    /// after the current-call prune.
    recent_failures_ms: VecDeque<i64>,
    last_successful_append_at_ms: Option<i64>,
    last_failure_reason: Option<String>,
}

impl HealthCounters {
    fn record_success(&mut self, now_ms: i64) {
        self.appends_attempted += 1;
        self.appends_succeeded += 1;
        self.last_successful_append_at_ms = Some(now_ms);
    }

    fn record_failure(&mut self, now_ms: i64, reason: impl Into<String>) {
        self.appends_attempted += 1;
        self.prune_failures(now_ms);
        self.recent_failures_ms.push_back(now_ms);
        self.last_failure_reason = Some(reason.into());
    }

    fn prune_failures(&mut self, now_ms: i64) {
        let cutoff = now_ms - FAILURE_WINDOW_MS;
        while matches!(self.recent_failures_ms.front(), Some(&ts) if ts < cutoff) {
            self.recent_failures_ms.pop_front();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryLogError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("event not found: {0}")]
    NotFound(String),
    #[error("payload hash mismatch: expected {expected}, got {computed}")]
    PayloadHashMismatch { expected: String, computed: String },
    #[error("parent {0} does not exist")]
    ParentMissing(String),
    #[error("storage: {0}")]
    Storage(String),
    #[error("bus: {0}")]
    Bus(String),
}

pub type MemoryLogResult<T> = Result<T, MemoryLogError>;

/// Outcome of an append. `deduplicated` is true when the same
/// `(workspace_id, payload_hash)` arrived within the dedupe window;
/// callers can use this to avoid double-counting side effects.
#[derive(Debug, Clone)]
pub struct AppendResult {
    pub event: MemoryEvent,
    pub deduplicated: bool,
}

#[derive(Clone)]
pub struct MemoryLogService {
    store: Arc<Mutex<MemoryLogStore>>,
    workspace_id: String,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
    /// Blueprint concern 1: in-memory counters so a write-path
    /// failure is visible via `memory.log.health` within a poll
    /// interval, not after the next replay-debug session.
    health_counters: Arc<Mutex<HealthCounters>>,
}

impl MemoryLogService {
    pub fn new(store: MemoryLogStore, workspace_id: impl Into<String>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            workspace_id: workspace_id.into(),
            bus: None,
            node_id: NodeId::new(),
            health_counters: Arc::new(Mutex::new(HealthCounters::default())),
        }
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Generate a fresh ULID for a new event. Exposed so callers
    /// can attach the id to related events (parent/child chains)
    /// before the append actually happens.
    pub fn new_event_id() -> String {
        Ulid::new().to_string()
    }

    /// Compute the canonical payload hash â€” lowercase hex blake3 of
    /// the canonical JSON bytes of `payload`. Callers MUST use this
    /// when constructing events; the service verifies at append
    /// time and rejects mismatches as protocol violations.
    pub fn compute_payload_hash(payload: &serde_json::Value) -> String {
        let bytes = canonical_json(payload);
        blake3::hash(&bytes).to_hex().to_string()
    }

    /// Append an event. Enforces every protocol invariant:
    ///   - payload_hash is recomputed and must match
    ///   - parent_id, if set, must reference an existing event
    ///   - id must look like a ULID (26 chars, base32-ish)
    ///   - event_type is the string representation validated by
    ///     `MemoryEventType::from_label` on load
    ///
    /// Idempotent on `payload_hash` within the dedupe window:
    /// replaying the exact same payload within 5 seconds returns
    /// the original event with `deduplicated: true`.
    ///
    /// Auto-pin rules:
    ///   - `identity.assertion` events auto-pin
    ///   - `system.protocol_violation` events auto-pin (evidence)
    ///
    /// Emits `ordo.memory.log.appended` on the bus on success (not
    /// on dedupe hit â€” the subscriber already saw it).
    pub async fn append(&self, mut event: MemoryEvent) -> MemoryLogResult<AppendResult> {
        // --- ULID shape check --------------------------------------
        if event.id.is_empty() {
            event.id = Self::new_event_id();
        } else if Ulid::from_string(&event.id).is_err() {
            self.emit_violation(
                ProtocolViolationType::InvalidEventId,
                Some(event.id.clone()),
                "event id is not a ULID".into(),
                Severity::Error,
            )
            .await;
            return Err(MemoryLogError::InvalidArgument(format!(
                "event id `{}` is not a ULID",
                event.id
            )));
        }

        // --- Payload hash verification ------------------------------
        let computed = Self::compute_payload_hash(&event.payload);
        if event.payload_hash.is_empty() {
            event.payload_hash = computed;
        } else if event.payload_hash.to_lowercase() != computed {
            self.emit_violation(
                ProtocolViolationType::PayloadHashInvalid,
                Some(event.id.clone()),
                format!(
                    "payload_hash mismatch: expected {}, got {}",
                    computed, event.payload_hash
                ),
                Severity::Error,
            )
            .await;
            return Err(MemoryLogError::PayloadHashMismatch {
                expected: computed,
                computed: event.payload_hash,
            });
        }

        // --- Parent reference check ---------------------------------
        if let Some(parent) = &event.parent_id {
            let parent_ok = self
                .store
                .lock()
                .parent_exists(parent)
                .map_err(|err| MemoryLogError::Storage(err.to_string()))?;
            if !parent_ok {
                let parent_id = parent.clone();
                self.emit_violation(
                    ProtocolViolationType::ParentReferenceInvalid,
                    Some(event.id.clone()),
                    format!("parent event `{parent_id}` does not exist"),
                    Severity::Error,
                )
                .await;
                return Err(MemoryLogError::ParentMissing(parent_id));
            }
        }

        // --- Auto-pin rules -----------------------------------------
        if event.event_type.auto_pins() {
            event.pinned = true;
        }
        if matches!(event.event_type, MemoryEventType::WorkflowCheckpoint) {
            event.tier = RetentionTier::Warm;
        }

        // --- Dedupe check (within window) ---------------------------
        let now_ms = event.timestamp_ms;
        let window_start = now_ms - DEDUPE_WINDOW_MS;
        let duplicate = match self.store.lock().recent_by_hash(
            &self.workspace_id,
            &event.payload_hash,
            window_start,
        ) {
            Ok(d) => d,
            Err(err) => {
                let reason = err.to_string();
                self.health_counters
                    .lock()
                    .record_failure(now_ms, reason.clone());
                return Err(MemoryLogError::Storage(reason));
            }
        };
        if let Some(existing) = duplicate {
            // Dedupe hit is a successful outcome â€” the caller's
            // event is already durable.
            self.health_counters.lock().record_success(now_ms);
            return Ok(AppendResult {
                event: existing,
                deduplicated: true,
            });
        }

        // --- Insert -------------------------------------------------
        let insert_result = {
            let mut store = self.store.lock();
            store.insert(&event, &self.workspace_id)
        };
        let inserted = match insert_result {
            Ok(v) => v,
            Err(err) => {
                let reason = err.to_string();
                self.health_counters
                    .lock()
                    .record_failure(now_ms, reason.clone());
                return Err(MemoryLogError::Storage(reason));
            }
        };
        if !inserted {
            // Primary-key collision on the id (unlikely with ULID
            // but possible with caller-provided ids). Treat as a
            // duplicate.
            if let Some(existing) = self
                .store
                .lock()
                .get_by_id(&event.id)
                .map_err(|err| MemoryLogError::Storage(err.to_string()))?
            {
                self.health_counters.lock().record_success(now_ms);
                return Ok(AppendResult {
                    event: existing,
                    deduplicated: true,
                });
            }
        }
        self.health_counters.lock().record_success(now_ms);

        // --- Broadcast ----------------------------------------------
        if let Some(bus) = &self.bus {
            let envelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::MemoryLogAppended {
                    event_id: event.id.clone(),
                    event_type: event.event_type,
                    domain: event.domain.clone(),
                },
            );
            if let Err(err) = bus.publish(memory_topics::LOG_APPENDED, envelope).await {
                tracing::warn!(
                    target: "ordo_memory_log",
                    error = %err,
                    "appended-event broadcast failed (event is persisted)"
                );
            }
        }

        Ok(AppendResult {
            event,
            deduplicated: false,
        })
    }

    pub fn get_by_id(&self, id: &str) -> MemoryLogResult<Option<MemoryEvent>> {
        self.store
            .lock()
            .get_by_id(id)
            .map_err(|err| MemoryLogError::Storage(err.to_string()))
    }

    /// Query the hot + warm tiers (cold is explicit â€” separate
    /// method to make the audit trail obvious). Returns results and
    /// a truncation flag.
    pub fn query_by_range(
        &self,
        start_ms: i64,
        end_ms: i64,
        filters: &[MemoryLogFilter],
        limit: Option<u32>,
    ) -> MemoryLogResult<MemoryLogQueryResult> {
        let (events, truncated) = self
            .store
            .lock()
            .query_by_range(&self.workspace_id, start_ms, end_ms, filters, limit)
            .map_err(|err| MemoryLogError::Storage(err.to_string()))?;
        Ok(MemoryLogQueryResult {
            events,
            truncated,
            cold_queried: false,
        })
    }

    pub fn query_by_parent(&self, parent_id: &str) -> MemoryLogResult<Vec<MemoryEvent>> {
        self.store
            .lock()
            .query_by_parent(parent_id)
            .map_err(|err| MemoryLogError::Storage(err.to_string()))
    }

    pub fn pin(&self, id: &str) -> MemoryLogResult<()> {
        self.store
            .lock()
            .set_pinned(id, true)
            .map_err(|err| match err {
                crate::store::StoreError::NotFound(id) => MemoryLogError::NotFound(id),
                other => MemoryLogError::Storage(other.to_string()),
            })
    }

    pub fn unpin(&self, id: &str) -> MemoryLogResult<()> {
        self.store
            .lock()
            .set_pinned(id, false)
            .map_err(|err| match err {
                crate::store::StoreError::NotFound(id) => MemoryLogError::NotFound(id),
                other => MemoryLogError::Storage(other.to_string()),
            })
    }

    pub fn soft_delete(&self, id: &str, reason: &str) -> MemoryLogResult<()> {
        self.store
            .lock()
            .soft_delete(id, reason)
            .map_err(|err| match err {
                crate::store::StoreError::NotFound(id) => MemoryLogError::NotFound(id),
                other => MemoryLogError::Storage(other.to_string()),
            })
    }

    pub fn query_by_turn(&self, turn_id: &str) -> MemoryLogResult<Vec<MemoryEvent>> {
        self.store
            .lock()
            .query_by_turn(turn_id)
            .map_err(|err| MemoryLogError::Storage(err.to_string()))
    }

    /// TEST-ONLY: drop the `memory_events` table, simulating a
    /// write-path catastrophe (disk eaten, permissions flipped,
    /// schema corrupted). Used by the health-task integration test
    /// to verify the degraded event fires. Not part of the public
    /// API surface â€” the `#[doc(hidden)]` annotation keeps it off
    /// the generated docs.
    #[doc(hidden)]
    pub fn drop_events_table_for_tests(&self) {
        let mut store = self.store.lock();
        let conn = store.db_mut_for_tests();
        let _ = conn.execute("DROP TABLE memory_events", []);
    }

    /// Snapshot of the in-memory health counters. Cheap; holds a
    /// short lock.
    pub fn health(&self) -> MemoryLogHealth {
        let counters = self.health_counters.lock();
        let events_total = self.store.lock().count(&self.workspace_id).unwrap_or(0);
        MemoryLogHealth {
            appends_attempted: counters.appends_attempted,
            appends_succeeded: counters.appends_succeeded,
            appends_failed_last_hour: counters.recent_failures_ms.len() as u64,
            last_successful_append_at_ms: counters.last_successful_append_at_ms,
            last_failure_reason: counters.last_failure_reason.clone(),
            events_total,
        }
    }

    /// Append a canary event. Returns the full outcome â€” callers
    /// typically check `AppendResult::event.event_type` to confirm
    /// `SystemHealthProbe` and treat any `Err` as a degraded signal.
    ///
    /// The canary payload includes a wall-clock nano counter so two
    /// canaries in the same second hash differently and the dedupe
    /// window doesn't collapse them into one.
    pub async fn canary_probe(&self) -> MemoryLogResult<AppendResult> {
        let now = Utc::now();
        let payload = json!({
            "probe": true,
            "emitted_at_ns": now.timestamp_nanos_opt().unwrap_or(0),
        });
        let payload_hash = Self::compute_payload_hash(&payload);
        let event = MemoryEvent {
            id: Self::new_event_id(),
            timestamp_ms: now.timestamp_millis(),
            event_type: MemoryEventType::SystemHealthProbe,
            actor: "system.health".into(),
            domain: None,
            category: Some("health_probe".into()),
            parent_id: None,
            turn_id: None,
            payload,
            payload_hash,
            tier: RetentionTier::Hot,
            pinned: true,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        };
        self.append(event).await
    }

    /// Soft-delete canary events older than `CANARY_TTL_MS`. Used by
    /// the periodic health task to keep canary rows bounded without
    /// violating the "soft-delete, never DROP" invariant.
    pub fn sweep_stale_canaries(&self) -> MemoryLogResult<u64> {
        let cutoff = Utc::now().timestamp_millis() - CANARY_TTL_MS;
        self.store
            .lock()
            .sweep_stale_canaries(&self.workspace_id, cutoff)
            .map_err(|err| MemoryLogError::Storage(err.to_string()))
    }

    /// Run the integrity sweep: walk every live event, recompute
    /// `payload_hash`, compare. Emits
    /// `ordo.memory.log.integrity.result` on the bus, auto-pins a
    /// protocol violation event for each mismatch batch (one
    /// violation with the count; individual row ids aren't collected
    /// to keep memory bounded).
    pub async fn run_integrity_sweep(&self) -> MemoryLogIntegrityReport {
        let (checked, mismatches) = match self.store.lock().walk_for_integrity(&self.workspace_id) {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(
                    target: "ordo_memory_log::integrity",
                    error = %err,
                    "integrity sweep aborted (storage error)"
                );
                return MemoryLogIntegrityReport {
                    passed: false,
                    mismatches_found: 0,
                    checked_count: 0,
                };
            }
        };
        let report = MemoryLogIntegrityReport {
            passed: mismatches == 0,
            mismatches_found: mismatches,
            checked_count: checked,
        };
        if let Some(bus) = &self.bus {
            let envelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::MemoryLogIntegrityResult(report.clone()),
            );
            let _ = bus
                .publish(memory_topics::LOG_INTEGRITY_RESULT, envelope)
                .await;
        }
        if !report.passed {
            self.emit_violation(
                ProtocolViolationType::PayloadHashInvalid,
                None,
                format!(
                    "integrity sweep: {} of {} checked events failed hash verification",
                    mismatches, checked
                ),
                Severity::Error,
            )
            .await;
        }
        report
    }

    pub fn snapshot_hash(&self, up_to_ms: i64) -> MemoryLogResult<String> {
        self.store
            .lock()
            .snapshot_hash(&self.workspace_id, up_to_ms)
            .map_err(|err| MemoryLogError::Storage(err.to_string()))
    }

    pub fn count(&self) -> MemoryLogResult<u64> {
        self.store
            .lock()
            .count(&self.workspace_id)
            .map_err(|err| MemoryLogError::Storage(err.to_string()))
    }

    pub fn export_jsonl(&self) -> MemoryLogResult<String> {
        self.store
            .lock()
            .export_jsonl(&self.workspace_id)
            .map_err(|err| MemoryLogError::Storage(err.to_string()))
    }

    /// Emit a protocol violation as its own persisted event. Also
    /// broadcasts on the bus. Auto-pinned (evidence).
    async fn emit_violation(
        &self,
        violation_type: ProtocolViolationType,
        offending_id: Option<String>,
        details: String,
        severity: Severity,
    ) {
        let violation = ProtocolViolation {
            violation_type,
            offending_id,
            details,
            severity,
        };
        if let Some(bus) = &self.bus {
            let envelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::MemoryProtocolViolation(violation.clone()),
            );
            let _ = bus
                .publish(memory_topics::PROTOCOL_VIOLATION, envelope)
                .await;
        }
        // Persist too â€” but carefully. The persist path uses
        // payload_hash verification; a violation event building
        // another violation event would loop. So we skip the
        // service path and go direct to the store.
        let payload = serde_json::to_value(&violation).unwrap_or(serde_json::Value::Null);
        let payload_hash = Self::compute_payload_hash(&payload);
        let event = MemoryEvent {
            id: Self::new_event_id(),
            timestamp_ms: Utc::now().timestamp_millis(),
            event_type: MemoryEventType::SystemProtocolViolation,
            actor: "system".into(),
            domain: None,
            category: Some("protocol_violation".into()),
            parent_id: None,
            turn_id: None,
            payload,
            payload_hash,
            tier: RetentionTier::Hot,
            pinned: true,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        };
        let _ = self.store.lock().insert(&event, &self.workspace_id);
    }
}

/// Canonical JSON encoder â€” sorts object keys so the same logical
/// payload always hashes to the same bytes regardless of source
/// map ordering. Arrays preserve their order (they're ordered by
/// convention).
fn canonical_json(value: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &serde_json::Value, out: &mut Vec<u8>) {
    match value {
        serde_json::Value::Null => out.extend_from_slice(b"null"),
        serde_json::Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        serde_json::Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        serde_json::Value::String(s) => {
            out.push(b'"');
            for c in s.chars() {
                match c {
                    '"' => out.extend_from_slice(b"\\\""),
                    '\\' => out.extend_from_slice(b"\\\\"),
                    '\n' => out.extend_from_slice(b"\\n"),
                    '\r' => out.extend_from_slice(b"\\r"),
                    '\t' => out.extend_from_slice(b"\\t"),
                    c if (c as u32) < 0x20 => {
                        out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
                    }
                    c => {
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                }
            }
            out.push(b'"');
        }
        serde_json::Value::Array(arr) => {
            out.push(b'[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        serde_json::Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.push(b'"');
                out.extend_from_slice(key.as_bytes());
                out.push(b'"');
                out.push(b':');
                write_canonical(&map[key], out);
            }
            out.push(b'}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn svc() -> MemoryLogService {
        let store = MemoryLogStore::in_memory().expect("store");
        MemoryLogService::new(store, "local")
    }

    fn event_with(payload: serde_json::Value) -> MemoryEvent {
        let hash = MemoryLogService::compute_payload_hash(&payload);
        MemoryEvent {
            id: MemoryLogService::new_event_id(),
            timestamp_ms: Utc::now().timestamp_millis(),
            event_type: MemoryEventType::UserMessage,
            actor: "operator".into(),
            domain: Some("test".into()),
            category: None,
            parent_id: None,
            turn_id: None,
            payload,
            payload_hash: hash,
            tier: RetentionTier::Hot,
            pinned: false,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        }
    }

    #[tokio::test]
    async fn append_stores_and_returns_event() {
        let svc = svc();
        let result = svc
            .append(event_with(json!({"text": "hello"})))
            .await
            .expect("append");
        assert!(!result.deduplicated);
        let fetched = svc
            .get_by_id(&result.event.id)
            .expect("get")
            .expect("present");
        assert_eq!(fetched.payload["text"], "hello");
    }

    #[tokio::test]
    async fn append_is_idempotent_on_payload_hash_within_window() {
        let svc = svc();
        let payload = json!({"text": "same"});
        let a = svc.append(event_with(payload.clone())).await.unwrap();
        let b = svc.append(event_with(payload)).await.unwrap();
        assert_eq!(a.event.id, b.event.id, "same payload hash â†’ same row");
        assert!(b.deduplicated);
        assert_eq!(svc.count().unwrap(), 1);
    }

    #[tokio::test]
    async fn append_rejects_bad_payload_hash() {
        let svc = svc();
        let mut event = event_with(json!({"text": "hello"}));
        event.payload_hash = "0".repeat(64); // wrong
        let err = svc.append(event).await.expect_err("should reject");
        assert!(matches!(err, MemoryLogError::PayloadHashMismatch { .. }));
    }

    #[tokio::test]
    async fn append_rejects_invalid_parent_reference() {
        let svc = svc();
        let mut event = event_with(json!({"text": "orphan"}));
        event.parent_id = Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()); // valid ULID, unknown event
        let err = svc.append(event).await.expect_err("should reject");
        assert!(matches!(err, MemoryLogError::ParentMissing(_)));
    }

    #[tokio::test]
    async fn append_rejects_non_ulid_id() {
        let svc = svc();
        let mut event = event_with(json!({"text": "x"}));
        event.id = "not-a-ulid".into();
        let err = svc.append(event).await.expect_err("should reject");
        assert!(matches!(err, MemoryLogError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn identity_assertion_auto_pins() {
        let svc = svc();
        let mut event = event_with(json!({"voice": "terse"}));
        event.event_type = MemoryEventType::IdentityAssertion;
        event.pinned = false; // service should override
        let result = svc.append(event).await.expect("append");
        assert!(result.event.pinned);
    }

    #[tokio::test]
    async fn workflow_checkpoint_auto_warm_tier() {
        let svc = svc();
        let mut event = event_with(json!({"step": 3}));
        event.event_type = MemoryEventType::WorkflowCheckpoint;
        event.tier = RetentionTier::Hot;
        let result = svc.append(event).await.expect("append");
        assert_eq!(result.event.tier, RetentionTier::Warm);
    }

    #[tokio::test]
    async fn soft_delete_hides_from_queries_but_preserves_row() {
        let svc = svc();
        let result = svc.append(event_with(json!({"x": 1}))).await.unwrap();
        svc.soft_delete(&result.event.id, "test").unwrap();
        let range = svc.query_by_range(0, i64::MAX, &[], None).unwrap();
        assert!(range.events.is_empty());
        // But the row is still physically there.
        assert_eq!(svc.count().unwrap(), 1);
    }

    #[tokio::test]
    async fn snapshot_hash_is_deterministic_for_same_log_state() {
        let svc = svc();
        let now_ms = Utc::now().timestamp_millis();
        svc.append(event_with(json!({"a": 1}))).await.unwrap();
        svc.append(event_with(json!({"b": 2}))).await.unwrap();
        let h1 = svc.snapshot_hash(now_ms + 10_000).unwrap();
        let h2 = svc.snapshot_hash(now_ms + 10_000).unwrap();
        assert_eq!(h1, h2);
        // A fresh append changes the hash.
        svc.append(event_with(json!({"c": 3}))).await.unwrap();
        let h3 = svc.snapshot_hash(now_ms + 10_000).unwrap();
        assert_ne!(h1, h3);
    }

    #[tokio::test]
    async fn query_by_parent_returns_chain() {
        let svc = svc();
        let root = svc.append(event_with(json!({"root": true}))).await.unwrap();
        let mut child = event_with(json!({"child": 1}));
        child.parent_id = Some(root.event.id.clone());
        svc.append(child).await.unwrap();
        let chain = svc.query_by_parent(&root.event.id).unwrap();
        assert_eq!(chain.len(), 1);
    }

    #[tokio::test]
    async fn canonical_json_key_order_independent() {
        let a = canonical_json(&json!({"x": 1, "y": 2}));
        let b = canonical_json(&json!({"y": 2, "x": 1}));
        assert_eq!(a, b);
    }

    // ---- Health + integrity + turn_id tests (concerns 1 + 2) ------

    #[tokio::test]
    async fn health_counters_increment_on_success_and_dedupe() {
        let svc = svc();
        let payload = json!({"text": "x"});
        let first = svc.append(event_with(payload.clone())).await.expect("a");
        assert!(!first.deduplicated);
        let second = svc.append(event_with(payload)).await.expect("b");
        assert!(second.deduplicated);
        let health = svc.health();
        // Two calls, both succeeded (one stored, one deduped).
        assert_eq!(health.appends_attempted, 2);
        assert_eq!(health.appends_succeeded, 2);
        assert_eq!(health.appends_failed_last_hour, 0);
        assert!(health.last_successful_append_at_ms.is_some());
        assert_eq!(health.events_total, 1);
    }

    #[tokio::test]
    async fn canary_probe_persists_an_auto_pinned_health_probe_event() {
        let svc = svc();
        let outcome = svc.canary_probe().await.expect("canary");
        assert_eq!(outcome.event.event_type, MemoryEventType::SystemHealthProbe);
        assert!(outcome.event.pinned);
        assert_eq!(outcome.event.actor, "system.health");
    }

    #[tokio::test]
    async fn sweep_stale_canaries_soft_deletes_old_probes_only() {
        let svc = svc();
        // Probe now + an artificially-old canary.
        let fresh = svc.canary_probe().await.expect("fresh").event;
        // Backdate one probe by rewriting its timestamp via an
        // intentional append (the service accepts caller-supplied
        // timestamps).
        let old_payload = json!({"probe": true, "emitted_at_ns": -1i64});
        let old_hash = MemoryLogService::compute_payload_hash(&old_payload);
        let old_event = MemoryEvent {
            id: MemoryLogService::new_event_id(),
            timestamp_ms: 0, // epoch: guaranteed > CANARY_TTL_MS old
            event_type: MemoryEventType::SystemHealthProbe,
            actor: "system.health".into(),
            domain: None,
            category: Some("health_probe".into()),
            parent_id: None,
            turn_id: None,
            payload: old_payload,
            payload_hash: old_hash,
            tier: RetentionTier::Hot,
            pinned: true,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        };
        let stale = svc.append(old_event).await.expect("stale insert");

        let deleted = svc.sweep_stale_canaries().expect("sweep");
        assert_eq!(deleted, 1, "only the old one is stale");
        // Fresh canary still visible, stale one not.
        let visible = svc
            .query_by_range(0, i64::MAX, &[], None)
            .expect("query")
            .events;
        assert!(visible.iter().any(|e| e.id == fresh.id));
        assert!(!visible.iter().any(|e| e.id == stale.event.id));
    }

    #[tokio::test]
    async fn run_integrity_sweep_flags_hash_mismatches() {
        let svc = svc();
        svc.append(event_with(json!({"ok": 1}))).await.unwrap();
        svc.append(event_with(json!({"ok": 2}))).await.unwrap();
        // Clean log: sweep passes.
        let pass = svc.run_integrity_sweep().await;
        assert!(pass.passed, "clean log should pass");
        assert_eq!(pass.checked_count, 2);
        assert_eq!(pass.mismatches_found, 0);

        // Corrupt a row via direct SQL (simulating disk corruption
        // or a buggy out-of-band writer). We smuggle in a bad hash
        // by reaching into the store's DB. The goal is to prove the
        // sweep DETECTS the mismatch, not to encourage the pattern.
        {
            let mut store = svc.store.lock();
            let conn = store.db_mut_for_tests();
            conn.execute(
                "UPDATE memory_events SET payload_hash = ?1 WHERE rowid = 1",
                rusqlite::params!["0".repeat(64)],
            )
            .expect("corrupt");
        }
        let fail = svc.run_integrity_sweep().await;
        assert!(!fail.passed);
        assert_eq!(fail.mismatches_found, 1);
    }

    #[tokio::test]
    async fn query_by_turn_groups_events_by_turn_id_and_excludes_other_turns() {
        let svc = svc();
        let turn_a = MemoryLogService::new_event_id();
        let turn_b = MemoryLogService::new_event_id();
        for (turn, texts) in [(&turn_a, &["a1", "a2"][..]), (&turn_b, &["b1"][..])] {
            for text in texts {
                let payload = json!({"text": text});
                let hash = MemoryLogService::compute_payload_hash(&payload);
                let event = MemoryEvent {
                    id: MemoryLogService::new_event_id(),
                    timestamp_ms: Utc::now().timestamp_millis(),
                    event_type: MemoryEventType::UserMessage,
                    actor: "operator".into(),
                    domain: None,
                    category: None,
                    parent_id: None,
                    turn_id: Some(turn.clone()),
                    payload,
                    payload_hash: hash,
                    tier: RetentionTier::Hot,
                    pinned: false,
                    soft_deleted: false,
                    soft_deleted_at: None,
                    soft_deleted_reason: None,
                };
                svc.append(event).await.expect("append");
            }
        }
        let a_events = svc.query_by_turn(&turn_a).expect("turn a");
        assert_eq!(a_events.len(), 2);
        assert!(a_events
            .iter()
            .all(|e| e.turn_id.as_deref() == Some(turn_a.as_str())));
        let b_events = svc.query_by_turn(&turn_b).expect("turn b");
        assert_eq!(b_events.len(), 1);
        // Unknown turn_id: empty, not error.
        let missing = svc
            .query_by_turn("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .expect("missing");
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn failure_counter_records_reasons_when_store_errors() {
        // Simulate a failure by corrupting the event's own stored
        // payload_hash after recompute succeeds â€” the store layer
        // itself doesn't easily fail from in-process tests, but we
        // can force a primary-key collision which flows through the
        // !inserted branch and then falls to success via get_by_id.
        //
        // More useful: verify that when we intentionally drop the
        // table and try to append, the failure is recorded.
        let svc = svc();
        // Drop the table under the service's feet.
        {
            let mut store = svc.store.lock();
            let conn = store.db_mut_for_tests();
            conn.execute("DROP TABLE memory_events", []).expect("drop");
        }
        let err = svc
            .append(event_with(json!({"x": 1})))
            .await
            .expect_err("should fail");
        assert!(matches!(err, MemoryLogError::Storage(_)));
        let health = svc.health();
        assert_eq!(health.appends_attempted, 1);
        assert_eq!(health.appends_succeeded, 0);
        assert_eq!(health.appends_failed_last_hour, 1);
        assert!(health.last_failure_reason.is_some());
    }

    #[tokio::test]
    async fn export_jsonl_round_trips() {
        let svc = svc();
        svc.append(event_with(json!({"m": 1}))).await.unwrap();
        svc.append(event_with(json!({"m": 2}))).await.unwrap();
        let jsonl = svc.export_jsonl().unwrap();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let _: MemoryEvent = serde_json::from_str(line).expect("parseable");
        }
    }
}
