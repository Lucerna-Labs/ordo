//! SQLite persistence for memory events. The append is HARD â€” once
//! a row is here, only soft-delete can hide it.

use chrono::{DateTime, Utc};
use ordo_protocol::{MemoryEvent, MemoryEventType, MemoryLogFilter, RetentionTier};
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};

pub struct MemoryLogStore {
    db: OrdoDatabase,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("storage: {0}")]
    Storage(String),
    #[error("event not found: {0}")]
    NotFound(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

impl MemoryLogStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> StoreResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> StoreResult<Self> {
        let db = OrdoDatabase::in_memory().map_err(|err| StoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    /// Crate-private escape hatch used by the health-task integration
    /// test to simulate a write-path failure. Not `#[cfg(test)]` so
    /// integration-test binaries (which compile the lib without the
    /// test cfg) can still reach it via the service's public
    /// `drop_events_table_for_tests` helper.
    pub(crate) fn db_mut_for_tests(&mut self) -> &mut rusqlite::Connection {
        self.db.conn_mut()
    }

    /// Insert. Caller is responsible for ULID generation + payload
    /// hashing; the store validates presence / uniqueness but not
    /// semantics. Returns `true` on new insert, `false` if the
    /// exact (id, payload_hash) was already present.
    pub fn insert(&mut self, event: &MemoryEvent, workspace_id: &str) -> StoreResult<bool> {
        let payload_json = serde_json::to_string(&event.payload)
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let conn = self.db.conn_mut();
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO memory_events (
                id, workspace_id, timestamp_ms, event_type, actor,
                domain, category, parent_id, payload_json, payload_hash,
                tier, pinned, soft_deleted, turn_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13)",
                params![
                    event.id,
                    workspace_id,
                    event.timestamp_ms,
                    event.event_type.label(),
                    event.actor,
                    event.domain,
                    event.category,
                    event.parent_id,
                    payload_json,
                    event.payload_hash,
                    event.tier.label(),
                    if event.pinned { 1i64 } else { 0 },
                    event.turn_id,
                ],
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        Ok(inserted == 1)
    }

    pub fn get_by_id(&self, id: &str) -> StoreResult<Option<MemoryEvent>> {
        let conn = self.db.conn();
        conn.query_row(
            "SELECT id, timestamp_ms, event_type, actor, domain, category, parent_id,
                    payload_json, payload_hash, tier, pinned, soft_deleted,
                    soft_deleted_at, soft_deleted_reason, turn_id
             FROM memory_events WHERE id = ?1",
            params![id],
            row_to_event,
        )
        .optional()
        .map_err(|err| StoreError::Storage(err.to_string()))
    }

    /// Look up by payload_hash within a recent-events window.
    /// Used to implement idempotent append (dedupe retries).
    pub fn recent_by_hash(
        &self,
        workspace_id: &str,
        payload_hash: &str,
        window_start_ms: i64,
    ) -> StoreResult<Option<MemoryEvent>> {
        let conn = self.db.conn();
        conn.query_row(
            "SELECT id, timestamp_ms, event_type, actor, domain, category, parent_id,
                    payload_json, payload_hash, tier, pinned, soft_deleted,
                    soft_deleted_at, soft_deleted_reason, turn_id
             FROM memory_events
             WHERE workspace_id = ?1 AND payload_hash = ?2 AND timestamp_ms >= ?3
             ORDER BY timestamp_ms DESC LIMIT 1",
            params![workspace_id, payload_hash, window_start_ms],
            row_to_event,
        )
        .optional()
        .map_err(|err| StoreError::Storage(err.to_string()))
    }

    pub fn parent_exists(&self, parent_id: &str) -> StoreResult<bool> {
        let conn = self.db.conn();
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM memory_events WHERE id = ?1",
                params![parent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        Ok(exists.is_some())
    }

    pub fn query_by_range(
        &self,
        workspace_id: &str,
        start_ms: i64,
        end_ms: i64,
        filters: &[MemoryLogFilter],
        limit: Option<u32>,
    ) -> StoreResult<(Vec<MemoryEvent>, bool)> {
        let mut sql = String::from(
            "SELECT id, timestamp_ms, event_type, actor, domain, category, parent_id,
                    payload_json, payload_hash, tier, pinned, soft_deleted,
                    soft_deleted_at, soft_deleted_reason, turn_id
             FROM memory_events
             WHERE workspace_id = ?1 AND timestamp_ms >= ?2 AND timestamp_ms <= ?3
               AND soft_deleted = 0",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(workspace_id.to_string()),
            Box::new(start_ms),
            Box::new(end_ms),
        ];
        for filter in filters {
            match filter {
                MemoryLogFilter::Domain(v) => {
                    sql.push_str(" AND domain = ?");
                    args.push(Box::new(v.clone()));
                }
                MemoryLogFilter::Category(v) => {
                    sql.push_str(" AND category = ?");
                    args.push(Box::new(v.clone()));
                }
                MemoryLogFilter::EventType(et) => {
                    sql.push_str(" AND event_type = ?");
                    args.push(Box::new(et.label().to_string()));
                }
                MemoryLogFilter::Actor(v) => {
                    sql.push_str(" AND actor = ?");
                    args.push(Box::new(v.clone()));
                }
            }
        }
        sql.push_str(" ORDER BY timestamp_ms ASC");
        let applied_limit = limit.unwrap_or(10_000);
        // Fetch one extra so we can signal `truncated`.
        sql.push_str(" LIMIT ?");
        args.push(Box::new((applied_limit + 1) as i64));

        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let arg_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let rows = stmt
            .query_map(arg_refs.as_slice(), row_to_event)
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|err| StoreError::Storage(err.to_string()))?);
        }
        let truncated = events.len() > applied_limit as usize;
        if truncated {
            events.truncate(applied_limit as usize);
        }
        Ok((events, truncated))
    }

    /// Return every event stamped with `turn_id`, in timestamp
    /// ascending order. Empty vec if the id is unknown; not an error
    /// (queries for in-flight turns should return whatever has been
    /// logged so far).
    pub fn query_by_turn(&self, turn_id: &str) -> StoreResult<Vec<MemoryEvent>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp_ms, event_type, actor, domain, category, parent_id,
                        payload_json, payload_hash, tier, pinned, soft_deleted,
                        soft_deleted_at, soft_deleted_reason, turn_id
                 FROM memory_events
                 WHERE turn_id = ?1 AND soft_deleted = 0
                 ORDER BY timestamp_ms ASC",
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![turn_id], row_to_event)
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|err| StoreError::Storage(err.to_string()))?);
        }
        Ok(events)
    }

    /// Walk every live event, recomputing `payload_hash` and comparing
    /// against the persisted hash. Returns `(checked, mismatches)`.
    /// Used at startup and on demand; cheap enough to run often
    /// because blake3 hashes tens of thousands of rows in well under
    /// a second on local SSD.
    pub fn walk_for_integrity(&self, workspace_id: &str) -> StoreResult<(u64, u64)> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, payload_json, payload_hash FROM memory_events
                 WHERE workspace_id = ?1 AND soft_deleted = 0",
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![workspace_id], |row| {
                let id: String = row.get(0)?;
                let payload_json: String = row.get(1)?;
                let persisted_hash: String = row.get(2)?;
                Ok((id, payload_json, persisted_hash))
            })
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let mut checked: u64 = 0;
        let mut mismatches: u64 = 0;
        for row in rows {
            let (_id, payload_json, persisted) =
                row.map_err(|err| StoreError::Storage(err.to_string()))?;
            checked += 1;
            // Recompute from the stored canonical-JSON bytes the same
            // way the service produced them at append time.
            let parsed: serde_json::Value =
                serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null);
            let recomputed = super::service::MemoryLogService::compute_payload_hash(&parsed);
            if recomputed != persisted {
                mismatches += 1;
            }
        }
        Ok((checked, mismatches))
    }

    /// Soft-delete health-probe canaries older than the given
    /// cutoff. Counts deletions for observability. Preserves DPM:
    /// soft-delete, never DROP.
    pub fn sweep_stale_canaries(
        &mut self,
        workspace_id: &str,
        older_than_ms: i64,
    ) -> StoreResult<u64> {
        let now = Utc::now().to_rfc3339();
        let conn = self.db.conn_mut();
        let deleted = conn
            .execute(
                "UPDATE memory_events
                 SET soft_deleted = 1,
                     soft_deleted_at = ?1,
                     soft_deleted_reason = 'stale_canary'
                 WHERE workspace_id = ?2
                   AND event_type = 'system.health_probe'
                   AND soft_deleted = 0
                   AND timestamp_ms < ?3",
                params![now, workspace_id, older_than_ms],
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        Ok(deleted as u64)
    }

    pub fn query_by_parent(&self, parent_id: &str) -> StoreResult<Vec<MemoryEvent>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp_ms, event_type, actor, domain, category, parent_id,
                        payload_json, payload_hash, tier, pinned, soft_deleted,
                        soft_deleted_at, soft_deleted_reason, turn_id
                 FROM memory_events WHERE parent_id = ?1 AND soft_deleted = 0
                 ORDER BY timestamp_ms ASC",
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![parent_id], row_to_event)
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|err| StoreError::Storage(err.to_string()))?);
        }
        Ok(events)
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> StoreResult<()> {
        let conn = self.db.conn_mut();
        let updated = conn
            .execute(
                "UPDATE memory_events SET pinned = ?1 WHERE id = ?2",
                params![if pinned { 1i64 } else { 0 }, id],
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn soft_delete(&mut self, id: &str, reason: &str) -> StoreResult<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.db.conn_mut();
        let updated = conn
            .execute(
                "UPDATE memory_events
                 SET soft_deleted = 1, soft_deleted_at = ?1, soft_deleted_reason = ?2
                 WHERE id = ?3",
                params![now, reason, id],
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn transition_tier(&mut self, id: &str, tier: RetentionTier) -> StoreResult<()> {
        let conn = self.db.conn_mut();
        let updated = conn
            .execute(
                "UPDATE memory_events SET tier = ?1 WHERE id = ?2",
                params![tier.label(), id],
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn count(&self, workspace_id: &str) -> StoreResult<u64> {
        let conn = self.db.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_events WHERE workspace_id = ?1",
                params![workspace_id],
                |row| row.get(0),
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        Ok(count.max(0) as u64)
    }

    /// Compute a hash summarizing the log state at or before the
    /// given timestamp. Used by replay verification â€” a matching
    /// snapshot hash implies the underlying events are identical.
    pub fn snapshot_hash(&self, workspace_id: &str, up_to_ms: i64) -> StoreResult<String> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, payload_hash FROM memory_events
                 WHERE workspace_id = ?1 AND timestamp_ms <= ?2
                 ORDER BY timestamp_ms ASC, id ASC",
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let mut hasher = blake3::Hasher::new();
        let rows = stmt
            .query_map(params![workspace_id, up_to_ms], |row| {
                let id: String = row.get(0)?;
                let payload_hash: String = row.get(1)?;
                Ok((id, payload_hash))
            })
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        for row in rows {
            let (id, payload_hash) = row.map_err(|err| StoreError::Storage(err.to_string()))?;
            hasher.update(id.as_bytes());
            hasher.update(b"|");
            hasher.update(payload_hash.as_bytes());
            hasher.update(b"\n");
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    /// Export every event for a workspace as newline-delimited JSON,
    /// for backup / migration. Preserves soft-deleted rows so
    /// replay continues to work on the restored instance.
    pub fn export_jsonl(&self, workspace_id: &str) -> StoreResult<String> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp_ms, event_type, actor, domain, category, parent_id,
                        payload_json, payload_hash, tier, pinned, soft_deleted,
                        soft_deleted_at, soft_deleted_reason, turn_id
                 FROM memory_events WHERE workspace_id = ?1
                 ORDER BY timestamp_ms ASC, id ASC",
            )
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![workspace_id], row_to_event)
            .map_err(|err| StoreError::Storage(err.to_string()))?;
        let mut out = String::new();
        for row in rows {
            let event = row.map_err(|err| StoreError::Storage(err.to_string()))?;
            let line = serde_json::to_string(&event)
                .map_err(|err| StoreError::Storage(err.to_string()))?;
            out.push_str(&line);
            out.push('\n');
        }
        Ok(out)
    }
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryEvent> {
    let event_type_str: String = row.get(2)?;
    let payload_json: String = row.get(7)?;
    let tier_str: String = row.get(9)?;
    let pinned_int: i64 = row.get(10)?;
    let soft_deleted_int: i64 = row.get(11)?;
    let soft_deleted_at_str: Option<String> = row.get(12)?;
    let turn_id: Option<String> = row.get(14)?;

    let event_type = MemoryEventType::from_label(&event_type_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            format!("unknown event_type `{event_type_str}`").into(),
        )
    })?;
    let payload: serde_json::Value =
        serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null);
    let tier = match tier_str.as_str() {
        "hot" => RetentionTier::Hot,
        "warm" => RetentionTier::Warm,
        "cold" => RetentionTier::Cold,
        "pinned" => RetentionTier::Pinned,
        _ => RetentionTier::Hot,
    };
    let soft_deleted_at = soft_deleted_at_str
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Ok(MemoryEvent {
        id: row.get(0)?,
        timestamp_ms: row.get(1)?,
        event_type,
        actor: row.get(3)?,
        domain: row.get(4)?,
        category: row.get(5)?,
        parent_id: row.get(6)?,
        turn_id,
        payload,
        payload_hash: row.get(8)?,
        tier,
        pinned: pinned_int != 0,
        soft_deleted: soft_deleted_int != 0,
        soft_deleted_at,
        soft_deleted_reason: row.get(13)?,
    })
}
