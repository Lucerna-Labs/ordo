//! SQLite-backed persistence for sessions, turns, and facts.

use std::path::Path;

use chrono::{DateTime, Utc};
use ordo_store::OrdoDatabase;
use rusqlite::params;
use uuid::Uuid;

use crate::types::{
    AssistantError, AssistantResult, AssistantSession, Fact, FactSummary, KnowledgeEntry,
    KnowledgeKind, NewFact, NewKnowledge, SessionWithTurns, Turn, TurnContext,
};

pub struct AssistantStore {
    db: OrdoDatabase,
}

impl AssistantStore {
    pub fn open(path: impl AsRef<Path>) -> AssistantResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> AssistantResult<Self> {
        let db =
            OrdoDatabase::in_memory().map_err(|err| AssistantError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    // ---- sessions --------------------------------------------------

    pub fn create_session(
        &mut self,
        title: Option<&str>,
        mode: &str,
    ) -> AssistantResult<AssistantSession> {
        if mode.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "mode must not be empty".into(),
            ));
        }
        let now = Utc::now();
        let id = Uuid::new_v4();
        let conn = self.db.conn_mut();
        conn.execute(
            "INSERT INTO assistant_sessions (id, created_at, updated_at, title, turn_count, mode)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![
                id.to_string(),
                now.to_rfc3339(),
                now.to_rfc3339(),
                title,
                mode,
            ],
        )
        .map_err(|err| AssistantError::Storage(err.to_string()))?;
        Ok(AssistantSession {
            id,
            created_at: now,
            updated_at: now,
            title: title.map(str::to_string),
            turn_count: 0,
            mode: mode.to_string(),
        })
    }

    pub fn get_session(&self, id: Uuid) -> AssistantResult<Option<AssistantSession>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, updated_at, title, turn_count, mode
                 FROM assistant_sessions WHERE id = ?1",
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let mut rows = stmt
            .query_map(params![id.to_string()], row_to_session)
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        match rows.next() {
            Some(row) => Ok(Some(
                row.map_err(|err| AssistantError::Storage(err.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    pub fn list_sessions(&self, limit: usize) -> AssistantResult<Vec<AssistantSession>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, updated_at, title, turn_count, mode
                 FROM assistant_sessions
                 ORDER BY updated_at DESC LIMIT ?1",
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_session)
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(|err| AssistantError::Storage(err.to_string()))?);
        }
        Ok(sessions)
    }

    pub fn load_session_with_turns(&self, id: Uuid) -> AssistantResult<Option<SessionWithTurns>> {
        let session = match self.get_session(id)? {
            Some(session) => session,
            None => return Ok(None),
        };
        let turns = self.list_turns(id)?;
        Ok(Some(SessionWithTurns { session, turns }))
    }

    pub fn list_turns(&self, session_id: Uuid) -> AssistantResult<Vec<Turn>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, turn_index, created_at, user_message,
                        assistant_response, context_json, model, credential_service
                 FROM assistant_turns
                 WHERE session_id = ?1
                 ORDER BY turn_index ASC",
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![session_id.to_string()], row_to_turn)
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let mut turns = Vec::new();
        for row in rows {
            turns.push(row.map_err(|err| AssistantError::Storage(err.to_string()))?);
        }
        Ok(turns)
    }

    pub fn insert_turn(
        &mut self,
        session_id: Uuid,
        user_message: &str,
        assistant_response: &str,
        context: &TurnContext,
        model: Option<&str>,
        credential_service: Option<&str>,
    ) -> AssistantResult<Turn> {
        let session = self
            .get_session(session_id)?
            .ok_or(AssistantError::SessionNotFound(session_id))?;
        let now = Utc::now();
        let id = Uuid::new_v4();
        let index = session.turn_count;
        let context_json = serde_json::to_string(context)
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let conn = self.db.conn_mut();
        conn.execute(
            "INSERT INTO assistant_turns (
                id, session_id, turn_index, created_at, user_message,
                assistant_response, context_json, model, credential_service
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id.to_string(),
                session_id.to_string(),
                index as i64,
                now.to_rfc3339(),
                user_message,
                assistant_response,
                context_json,
                model,
                credential_service,
            ],
        )
        .map_err(|err| AssistantError::Storage(err.to_string()))?;

        // Bump the session's counter + updated_at; promote the first
        // user message into the session title if none is set.
        let mut new_title = session.title.clone();
        if new_title.is_none() {
            let snippet = user_message
                .trim()
                .chars()
                .take(48)
                .collect::<String>()
                .trim_end_matches(',')
                .to_string();
            if !snippet.is_empty() {
                new_title = Some(snippet);
            }
        }
        conn.execute(
            "UPDATE assistant_sessions
             SET turn_count = turn_count + 1, updated_at = ?1, title = COALESCE(title, ?2)
             WHERE id = ?3",
            params![now.to_rfc3339(), new_title, session_id.to_string()],
        )
        .map_err(|err| AssistantError::Storage(err.to_string()))?;

        Ok(Turn {
            id,
            session_id,
            index,
            created_at: now,
            user_message: user_message.to_string(),
            assistant_response: assistant_response.to_string(),
            context: context.clone(),
            model: model.map(str::to_string),
            credential_service: credential_service.map(str::to_string),
        })
    }

    // ---- facts -----------------------------------------------------

    pub fn insert_fact(&mut self, new_fact: NewFact, embedding: Vec<f32>) -> AssistantResult<Fact> {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let bytes = embedding_to_bytes(&embedding);
        let scope = new_fact.scope.unwrap_or_else(|| "global".to_string());
        if scope.trim().is_empty() {
            return Err(AssistantError::InvalidArgument(
                "fact scope must not be empty".into(),
            ));
        }
        let conn = self.db.conn_mut();
        conn.execute(
            "INSERT INTO assistant_facts (
                id, subject, predicate, object, source, confidence,
                created_at, reinforced_at, embedding, scope
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id.to_string(),
                new_fact.subject,
                new_fact.predicate,
                new_fact.object,
                new_fact.source,
                new_fact.confidence as f64,
                now.to_rfc3339(),
                now.to_rfc3339(),
                bytes,
                scope,
            ],
        )
        .map_err(|err| AssistantError::Storage(err.to_string()))?;
        Ok(Fact {
            id,
            subject: new_fact.subject,
            predicate: new_fact.predicate,
            object: new_fact.object,
            source: new_fact.source,
            confidence: new_fact.confidence,
            created_at: now,
            reinforced_at: now,
            scope,
            embedding,
        })
    }

    pub fn get_fact(&self, id: Uuid) -> AssistantResult<Option<Fact>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, subject, predicate, object, source, confidence,
                        created_at, reinforced_at, embedding, scope
                 FROM assistant_facts WHERE id = ?1",
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let mut rows = stmt
            .query_map(params![id.to_string()], row_to_fact)
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        match rows.next() {
            Some(row) => Ok(Some(
                row.map_err(|err| AssistantError::Storage(err.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    pub fn list_facts(&self, subject: Option<&str>) -> AssistantResult<Vec<Fact>> {
        let conn = self.db.conn();
        let sql = if subject.is_some() {
            "SELECT id, subject, predicate, object, source, confidence,
                    created_at, reinforced_at, embedding, scope
             FROM assistant_facts WHERE subject = ?1
             ORDER BY reinforced_at DESC"
        } else {
            "SELECT id, subject, predicate, object, source, confidence,
                    created_at, reinforced_at, embedding, scope
             FROM assistant_facts
             ORDER BY reinforced_at DESC"
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let rows = match subject {
            Some(subject) => stmt
                .query_map(params![subject], row_to_fact)
                .map_err(|err| AssistantError::Storage(err.to_string()))?
                .collect::<Vec<_>>(),
            None => stmt
                .query_map([], row_to_fact)
                .map_err(|err| AssistantError::Storage(err.to_string()))?
                .collect::<Vec<_>>(),
        };
        let mut facts = Vec::new();
        for row in rows {
            facts.push(row.map_err(|err| AssistantError::Storage(err.to_string()))?);
        }
        Ok(facts)
    }

    /// List facts whose `scope` is in the supplied set. When `scopes`
    /// is empty, returns no facts (fail-closed — caller should pass
    /// at least `["global"]`). Optional `subject` narrows further.
    ///
    /// Used by the recall path with the active mode's `memory_scope`
    /// list. Cross-mode borrowing extends this list per-request.
    pub fn list_facts_in_scopes(
        &self,
        subject: Option<&str>,
        scopes: &[String],
    ) -> AssistantResult<Vec<Fact>> {
        if scopes.is_empty() {
            return Ok(Vec::new());
        }
        // Build placeholders: ?1, ?2, ... matching scope count, with
        // an extra leading placeholder when subject is set.
        let scope_offset = if subject.is_some() { 2 } else { 1 };
        let placeholders: Vec<String> = (scope_offset..scope_offset + scopes.len())
            .map(|i| format!("?{i}"))
            .collect();
        let scopes_clause = placeholders.join(", ");
        let sql = if subject.is_some() {
            format!(
                "SELECT id, subject, predicate, object, source, confidence,
                        created_at, reinforced_at, embedding, scope
                 FROM assistant_facts
                 WHERE subject = ?1 AND scope IN ({scopes_clause})
                 ORDER BY reinforced_at DESC"
            )
        } else {
            format!(
                "SELECT id, subject, predicate, object, source, confidence,
                        created_at, reinforced_at, embedding, scope
                 FROM assistant_facts
                 WHERE scope IN ({scopes_clause})
                 ORDER BY reinforced_at DESC"
            )
        };

        // Build owned String parameters in one Vec to sidestep
        // the `Vec<&dyn ToSql>` lifetime trap. Subject (when set)
        // goes first; scopes follow in declared order to match the
        // placeholder positions in `sql`.
        let mut bound: Vec<String> = Vec::with_capacity(scopes.len() + 1);
        if let Some(s) = subject {
            bound.push(s.to_string());
        }
        bound.extend(scopes.iter().cloned());

        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bound.iter()), row_to_fact)
            .map_err(|err| AssistantError::Storage(err.to_string()))?
            .collect::<Vec<_>>();
        let mut facts = Vec::new();
        for row in rows {
            facts.push(row.map_err(|err| AssistantError::Storage(err.to_string()))?);
        }
        Ok(facts)
    }

    pub fn delete_fact(&mut self, id: Uuid) -> AssistantResult<bool> {
        let conn = self.db.conn_mut();
        let removed = conn
            .execute(
                "DELETE FROM assistant_facts WHERE id = ?1",
                params![id.to_string()],
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        Ok(removed > 0)
    }

    pub fn reinforce_fact(&mut self, id: Uuid) -> AssistantResult<Option<Fact>> {
        let now = Utc::now();
        let conn = self.db.conn_mut();
        let updated = conn
            .execute(
                "UPDATE assistant_facts
                 SET reinforced_at = ?1,
                     confidence = MIN(1.0, confidence + 0.05)
                 WHERE id = ?2",
                params![now.to_rfc3339(), id.to_string()],
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        if updated == 0 {
            return Ok(None);
        }
        self.get_fact(id)
    }

    // ---- knowledge (push 3) ----------------------------------------

    pub fn insert_knowledge(
        &mut self,
        new_entry: NewKnowledge,
        embedding: Vec<f32>,
    ) -> AssistantResult<KnowledgeEntry> {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let bytes = embedding_to_bytes(&embedding);
        let conn = self.db.conn_mut();
        conn.execute(
            "INSERT INTO assistant_knowledge (
                id, kind, domain, title, body, source, confidence,
                created_at, updated_at, reinforced_at, embedding
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id.to_string(),
                new_entry.kind.as_str(),
                new_entry.domain,
                new_entry.title,
                new_entry.body,
                new_entry.source,
                new_entry.confidence as f64,
                now.to_rfc3339(),
                now.to_rfc3339(),
                now.to_rfc3339(),
                bytes,
            ],
        )
        .map_err(|err| AssistantError::Storage(err.to_string()))?;
        Ok(KnowledgeEntry {
            id,
            kind: new_entry.kind,
            domain: new_entry.domain,
            title: new_entry.title,
            body: new_entry.body,
            source: new_entry.source,
            confidence: new_entry.confidence,
            created_at: now,
            updated_at: now,
            reinforced_at: now,
            embedding,
        })
    }

    pub fn list_knowledge(
        &self,
        kind: Option<KnowledgeKind>,
        domain: Option<&str>,
    ) -> AssistantResult<Vec<KnowledgeEntry>> {
        let conn = self.db.conn();
        let base = "SELECT id, kind, domain, title, body, source, confidence,
                           created_at, updated_at, reinforced_at, embedding
                    FROM assistant_knowledge";
        let (sql, entries) = match (kind, domain) {
            (Some(kind), Some(domain)) => {
                let sql =
                    format!("{base} WHERE kind = ?1 AND domain = ?2 ORDER BY reinforced_at DESC");
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|err| AssistantError::Storage(err.to_string()))?;
                let rows = stmt
                    .query_map(params![kind.as_str(), domain], row_to_knowledge)
                    .map_err(|err| AssistantError::Storage(err.to_string()))?;
                let mut entries = Vec::new();
                for row in rows {
                    entries.push(row.map_err(|err| AssistantError::Storage(err.to_string()))?);
                }
                (sql, entries)
            }
            (Some(kind), None) => {
                let sql = format!("{base} WHERE kind = ?1 ORDER BY reinforced_at DESC");
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|err| AssistantError::Storage(err.to_string()))?;
                let rows = stmt
                    .query_map(params![kind.as_str()], row_to_knowledge)
                    .map_err(|err| AssistantError::Storage(err.to_string()))?;
                let mut entries = Vec::new();
                for row in rows {
                    entries.push(row.map_err(|err| AssistantError::Storage(err.to_string()))?);
                }
                (sql, entries)
            }
            (None, Some(domain)) => {
                let sql = format!("{base} WHERE domain = ?1 ORDER BY reinforced_at DESC");
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|err| AssistantError::Storage(err.to_string()))?;
                let rows = stmt
                    .query_map(params![domain], row_to_knowledge)
                    .map_err(|err| AssistantError::Storage(err.to_string()))?;
                let mut entries = Vec::new();
                for row in rows {
                    entries.push(row.map_err(|err| AssistantError::Storage(err.to_string()))?);
                }
                (sql, entries)
            }
            (None, None) => {
                let sql = format!("{base} ORDER BY reinforced_at DESC");
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|err| AssistantError::Storage(err.to_string()))?;
                let rows = stmt
                    .query_map([], row_to_knowledge)
                    .map_err(|err| AssistantError::Storage(err.to_string()))?;
                let mut entries = Vec::new();
                for row in rows {
                    entries.push(row.map_err(|err| AssistantError::Storage(err.to_string()))?);
                }
                (sql, entries)
            }
        };
        let _ = sql;
        Ok(entries)
    }

    pub fn get_knowledge(&self, id: Uuid) -> AssistantResult<Option<KnowledgeEntry>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, domain, title, body, source, confidence,
                        created_at, updated_at, reinforced_at, embedding
                 FROM assistant_knowledge WHERE id = ?1",
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        let mut rows = stmt
            .query_map(params![id.to_string()], row_to_knowledge)
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        match rows.next() {
            Some(row) => Ok(Some(
                row.map_err(|err| AssistantError::Storage(err.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    /// Upsert a knowledge entry keyed by `source`. If a row already
    /// exists with the same `source`, its body/title/embedding are
    /// refreshed in place; otherwise a new row is inserted. This is
    /// what the boot-time seeder uses so restarting the runtime
    /// doesn't duplicate skill cards.
    pub fn upsert_knowledge_by_source(
        &mut self,
        new_entry: NewKnowledge,
        embedding: Vec<f32>,
    ) -> AssistantResult<KnowledgeEntry> {
        let existing_id: Option<String> = {
            let conn = self.db.conn();
            conn.query_row(
                "SELECT id FROM assistant_knowledge WHERE source = ?1 LIMIT 1",
                params![new_entry.source],
                |row| row.get::<_, String>(0),
            )
            .ok()
        };
        let now = Utc::now();
        let bytes = embedding_to_bytes(&embedding);
        match existing_id {
            Some(id_str) => {
                let id = Uuid::parse_str(&id_str).map_err(|err| {
                    AssistantError::Storage(format!("invalid stored uuid: {err}"))
                })?;
                let conn = self.db.conn_mut();
                conn.execute(
                    "UPDATE assistant_knowledge
                     SET kind = ?1, domain = ?2, title = ?3, body = ?4,
                         confidence = ?5, updated_at = ?6, embedding = ?7
                     WHERE id = ?8",
                    params![
                        new_entry.kind.as_str(),
                        new_entry.domain,
                        new_entry.title,
                        new_entry.body,
                        new_entry.confidence as f64,
                        now.to_rfc3339(),
                        bytes,
                        id_str,
                    ],
                )
                .map_err(|err| AssistantError::Storage(err.to_string()))?;
                Ok(self
                    .get_knowledge(id)?
                    .expect("row just upserted should exist"))
            }
            None => self.insert_knowledge(new_entry, embedding),
        }
    }

    pub fn delete_knowledge(&mut self, id: Uuid) -> AssistantResult<bool> {
        let conn = self.db.conn_mut();
        let removed = conn
            .execute(
                "DELETE FROM assistant_knowledge WHERE id = ?1",
                params![id.to_string()],
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        Ok(removed > 0)
    }

    pub fn reinforce_knowledge(&mut self, id: Uuid) -> AssistantResult<Option<KnowledgeEntry>> {
        let now = Utc::now();
        let conn = self.db.conn_mut();
        let updated = conn
            .execute(
                "UPDATE assistant_knowledge
                 SET reinforced_at = ?1,
                     confidence = MIN(1.0, confidence + 0.05)
                 WHERE id = ?2",
                params![now.to_rfc3339(), id.to_string()],
            )
            .map_err(|err| AssistantError::Storage(err.to_string()))?;
        if updated == 0 {
            return Ok(None);
        }
        self.get_knowledge(id)
    }
}

// ---- helpers ----------------------------------------------------------

pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(embedding.len() * 4);
    for value in embedding {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub fn embedding_from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<AssistantSession> {
    let id_str: String = row.get(0)?;
    let created_at_str: String = row.get(1)?;
    let updated_at_str: String = row.get(2)?;
    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let created_at = parse_rfc3339(1, &created_at_str)?;
    let updated_at = parse_rfc3339(2, &updated_at_str)?;
    let turn_count: i64 = row.get(4)?;
    let mode: String = row.get(5)?;
    Ok(AssistantSession {
        id,
        created_at,
        updated_at,
        title: row.get(3)?,
        turn_count: turn_count.max(0) as u32,
        mode,
    })
}

fn row_to_turn(row: &rusqlite::Row<'_>) -> rusqlite::Result<Turn> {
    let id_str: String = row.get(0)?;
    let session_id_str: String = row.get(1)?;
    let turn_index: i64 = row.get(2)?;
    let created_at_str: String = row.get(3)?;
    let context_json: String = row.get(6)?;
    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let session_id = Uuid::parse_str(&session_id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let created_at = parse_rfc3339(3, &created_at_str)?;
    let context: TurnContext = serde_json::from_str(&context_json).unwrap_or_default();
    Ok(Turn {
        id,
        session_id,
        index: turn_index.max(0) as u32,
        created_at,
        user_message: row.get(4)?,
        assistant_response: row.get(5)?,
        context,
        model: row.get(7)?,
        credential_service: row.get(8)?,
    })
}

fn row_to_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    let id_str: String = row.get(0)?;
    let created_at_str: String = row.get(6)?;
    let reinforced_at_str: String = row.get(7)?;
    let confidence: f64 = row.get(5)?;
    let embedding_bytes: Vec<u8> = row.get(8)?;
    let scope: String = row.get(9)?;
    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let created_at = parse_rfc3339(6, &created_at_str)?;
    let reinforced_at = parse_rfc3339(7, &reinforced_at_str)?;
    Ok(Fact {
        id,
        subject: row.get(1)?,
        predicate: row.get(2)?,
        object: row.get(3)?,
        source: row.get(4)?,
        confidence: confidence as f32,
        created_at,
        reinforced_at,
        scope,
        embedding: embedding_from_bytes(&embedding_bytes),
    })
}

fn row_to_knowledge(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeEntry> {
    let id_str: String = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let domain: Option<String> = row.get(2)?;
    let title: String = row.get(3)?;
    let body: String = row.get(4)?;
    let source: String = row.get(5)?;
    let confidence: f64 = row.get(6)?;
    let created_at_str: String = row.get(7)?;
    let updated_at_str: String = row.get(8)?;
    let reinforced_at_str: String = row.get(9)?;
    let embedding_bytes: Vec<u8> = row.get(10)?;
    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let kind = KnowledgeKind::parse(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "unknown knowledge kind: {kind_str}"
            )),
        )
    })?;
    let created_at = parse_rfc3339(7, &created_at_str)?;
    let updated_at = parse_rfc3339(8, &updated_at_str)?;
    let reinforced_at = parse_rfc3339(9, &reinforced_at_str)?;
    Ok(KnowledgeEntry {
        id,
        kind,
        domain,
        title,
        body,
        source,
        confidence: confidence as f32,
        created_at,
        updated_at,
        reinforced_at,
        embedding: embedding_from_bytes(&embedding_bytes),
    })
}

fn parse_rfc3339(col: usize, value: &str) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                col,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })
}

/// API-safe fact view that drops the embedding.
pub fn fact_summaries(facts: &[Fact]) -> Vec<FactSummary> {
    facts.iter().map(FactSummary::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_and_turn_round_trip() {
        let mut store = AssistantStore::in_memory().expect("store");
        let session = store
            .create_session(None, "general")
            .expect("create session");
        assert_eq!(session.turn_count, 0);
        assert_eq!(session.mode, "general");
        assert!(session.title.is_none());

        let turn = store
            .insert_turn(
                session.id,
                "hello there",
                "hello back",
                &TurnContext::default(),
                Some("gpt-mock"),
                Some("openai"),
            )
            .expect("insert turn");
        assert_eq!(turn.index, 0);

        let reloaded = store
            .get_session(session.id)
            .expect("get")
            .expect("present");
        assert_eq!(reloaded.turn_count, 1);
        assert_eq!(reloaded.title.as_deref(), Some("hello there"));
    }

    #[test]
    fn fact_round_trip_with_embedding() {
        let mut store = AssistantStore::in_memory().expect("store");
        let embedding = vec![0.1, 0.2, -0.3, 0.4];
        let fact = store
            .insert_fact(
                NewFact {
                    subject: "user".into(),
                    predicate: "prefers".into(),
                    object: "terse copy".into(),
                    source: "operator".into(),
                    confidence: 0.9,
                    scope: None,
                },
                embedding.clone(),
            )
            .expect("insert fact");

        let reloaded = store.get_fact(fact.id).expect("get").expect("present");
        assert_eq!(reloaded.embedding, embedding);

        let reinforced = store
            .reinforce_fact(fact.id)
            .expect("reinforce")
            .expect("still present");
        assert!(reinforced.confidence >= 0.9);
        assert!(reinforced.reinforced_at >= fact.reinforced_at);

        assert!(store.delete_fact(fact.id).expect("delete"));
        assert!(store.get_fact(fact.id).expect("get").is_none());
    }

    #[test]
    fn create_session_persists_mode() {
        let mut store = AssistantStore::in_memory().expect("store");
        let session = store
            .create_session(Some("debug session"), "vibe_coding")
            .expect("create");
        assert_eq!(session.mode, "vibe_coding");

        // get_session round-trips the mode.
        let reloaded = store
            .get_session(session.id)
            .expect("get")
            .expect("present");
        assert_eq!(reloaded.mode, "vibe_coding");

        // list_sessions also returns it.
        let listed = store.list_sessions(10).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].mode, "vibe_coding");
    }

    #[test]
    fn create_session_rejects_empty_mode() {
        let mut store = AssistantStore::in_memory().expect("store");
        let err = store.create_session(None, "");
        assert!(err.is_err());
    }

    #[test]
    fn legacy_sessions_default_to_general_mode() {
        // The migration backfills `mode = 'general'` on existing rows.
        // Sanity-check that path: insert a row with no mode column
        // value (the schema has DEFAULT 'general' so this works) and
        // verify the read path returns 'general'.
        let mut store = AssistantStore::in_memory().expect("store");
        let now = Utc::now();
        let id = Uuid::new_v4();
        store
            .db
            .conn_mut()
            .execute(
                "INSERT INTO assistant_sessions (id, created_at, updated_at, title, turn_count)
                 VALUES (?1, ?2, ?3, NULL, 0)",
                params![id.to_string(), now.to_rfc3339(), now.to_rfc3339()],
            )
            .expect("legacy-shape insert");
        let reloaded = store.get_session(id).expect("get").expect("present");
        assert_eq!(reloaded.mode, "general");
    }
}
