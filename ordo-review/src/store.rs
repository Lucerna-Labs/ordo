//! SQLite persistence for review requests.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use ordo_store::OrdoDatabase;
use rusqlite::params;
use uuid::Uuid;

use crate::types::{NewReviewRequest, ReviewError, ReviewRequest, ReviewResult, ReviewState};

pub struct ReviewStore {
    db: OrdoDatabase,
}

impl ReviewStore {
    pub fn open(path: impl AsRef<Path>) -> ReviewResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> ReviewResult<Self> {
        let db = OrdoDatabase::in_memory().map_err(|err| ReviewError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    pub fn insert(&mut self, new_request: NewReviewRequest) -> ReviewResult<ReviewRequest> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let metadata_json = serde_json::to_string(&new_request.metadata)
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        let conn = self.db.conn_mut();
        conn.execute(
            "INSERT INTO review_requests (
                id, created_at, resolved_at, origin_capability, origin_plugin,
                title, content_type, content, metadata_json, state,
                edited_content, decision_note
            ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, 'open', NULL, NULL)",
            params![
                id.to_string(),
                now.to_rfc3339(),
                new_request.origin_capability,
                new_request.origin_plugin,
                new_request.title,
                new_request.content_type,
                new_request.content,
                metadata_json,
            ],
        )
        .map_err(|err| ReviewError::Storage(err.to_string()))?;
        Ok(ReviewRequest {
            id,
            created_at: now,
            resolved_at: None,
            origin_capability: new_request.origin_capability,
            origin_plugin: new_request.origin_plugin,
            title: new_request.title,
            content_type: new_request.content_type,
            content: new_request.content,
            metadata: new_request.metadata,
            state: ReviewState::Open,
            edited_content: None,
            decision_note: None,
        })
    }

    pub fn get(&self, id: Uuid) -> ReviewResult<Option<ReviewRequest>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, resolved_at, origin_capability, origin_plugin,
                        title, content_type, content, metadata_json, state,
                        edited_content, decision_note
                 FROM review_requests WHERE id = ?1",
            )
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        let mut rows = stmt
            .query_map(params![id.to_string()], row_to_request)
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        match rows.next() {
            Some(row) => Ok(Some(
                row.map_err(|err| ReviewError::Storage(err.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    pub fn pending(&self) -> ReviewResult<Vec<ReviewRequest>> {
        self.query_by_state("open")
    }

    pub fn recent(&self, limit: usize) -> ReviewResult<Vec<ReviewRequest>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, resolved_at, origin_capability, origin_plugin,
                        title, content_type, content, metadata_json, state,
                        edited_content, decision_note
                 FROM review_requests ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_request)
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        let mut requests = Vec::new();
        for row in rows {
            requests.push(row.map_err(|err| ReviewError::Storage(err.to_string()))?);
        }
        Ok(requests)
    }

    fn query_by_state(&self, state: &str) -> ReviewResult<Vec<ReviewRequest>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, resolved_at, origin_capability, origin_plugin,
                        title, content_type, content, metadata_json, state,
                        edited_content, decision_note
                 FROM review_requests WHERE state = ?1 ORDER BY created_at ASC",
            )
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![state], row_to_request)
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        let mut requests = Vec::new();
        for row in rows {
            requests.push(row.map_err(|err| ReviewError::Storage(err.to_string()))?);
        }
        Ok(requests)
    }

    /// Atomically mark a request as resolved. Returns the updated
    /// request or an error if it was already resolved.
    pub fn resolve(
        &mut self,
        id: Uuid,
        state: ReviewState,
        edited_content: Option<&str>,
        decision_note: Option<&str>,
    ) -> ReviewResult<ReviewRequest> {
        if !state.is_terminal() {
            return Err(ReviewError::InvalidArgument(
                "cannot resolve into a non-terminal state".into(),
            ));
        }
        let current = self.get(id)?.ok_or(ReviewError::NotFound(id))?;
        if current.state.is_terminal() {
            return Err(ReviewError::AlreadyResolved(id, current.state.label()));
        }
        let now = Utc::now();
        let conn = self.db.conn_mut();
        let updated = conn
            .execute(
                "UPDATE review_requests
                 SET resolved_at = ?1, state = ?2, edited_content = ?3, decision_note = ?4
                 WHERE id = ?5 AND state = 'open'",
                params![
                    now.to_rfc3339(),
                    state.label(),
                    edited_content,
                    decision_note,
                    id.to_string(),
                ],
            )
            .map_err(|err| ReviewError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(ReviewError::AlreadyResolved(id, "open"));
        }
        self.get(id)?.ok_or(ReviewError::NotFound(id))
    }
}

fn row_to_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewRequest> {
    let id_str: String = row.get(0)?;
    let created_at_str: String = row.get(1)?;
    let resolved_at_str: Option<String> = row.get(2)?;
    let metadata_json: String = row.get(8)?;
    let state_str: String = row.get(9)?;
    let metadata: HashMap<String, serde_json::Value> =
        serde_json::from_str(&metadata_json).unwrap_or_default();
    let state = ReviewState::from_label(&state_str).unwrap_or(ReviewState::Open);
    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
        })?;
    let resolved_at = resolved_at_str
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })
        })
        .transpose()?;
    Ok(ReviewRequest {
        id,
        created_at,
        resolved_at,
        origin_capability: row.get(3)?,
        origin_plugin: row.get(4)?,
        title: row.get(5)?,
        content_type: row.get(6)?,
        content: row.get(7)?,
        metadata,
        state,
        edited_content: row.get(10)?,
        decision_note: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_lists_pending_and_resolves() {
        let mut store = ReviewStore::in_memory().expect("store");
        let request = store
            .insert(NewReviewRequest {
                origin_capability: "workflow.generate_copy".into(),
                origin_plugin: None,
                title: "Spring Colorway draft".into(),
                content_type: "text/markdown".into(),
                content: "# Hello".into(),
                metadata: HashMap::new(),
            })
            .expect("insert");
        assert_eq!(request.state, ReviewState::Open);

        let pending = store.pending().expect("pending");
        assert_eq!(pending.len(), 1);

        let resolved = store
            .resolve(request.id, ReviewState::Approved, None, Some("looks good"))
            .expect("resolve");
        assert_eq!(resolved.state, ReviewState::Approved);
        assert_eq!(resolved.decision_note.as_deref(), Some("looks good"));
        assert!(store.pending().expect("pending after resolve").is_empty());
    }

    #[test]
    fn double_resolve_is_rejected() {
        let mut store = ReviewStore::in_memory().expect("store");
        let request = store
            .insert(NewReviewRequest {
                origin_capability: "x".into(),
                origin_plugin: None,
                title: "t".into(),
                content_type: "text/plain".into(),
                content: "x".into(),
                metadata: HashMap::new(),
            })
            .expect("insert");
        store
            .resolve(request.id, ReviewState::Denied, None, None)
            .expect("first resolve");
        let err = store
            .resolve(request.id, ReviewState::Approved, None, None)
            .expect_err("second resolve should fail");
        assert!(matches!(err, ReviewError::AlreadyResolved(_, _)));
    }
}
