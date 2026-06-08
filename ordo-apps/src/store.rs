//! SQLite persistence for apps + their event log.
//!
//! Two tables (Phase 1.1 migration in `ordo-store`):
//!
//!   - `apps` â€” folded current state. Fast to read; authoritative for
//!     "what does app X look like right now".
//!   - `app_events` â€” append-only log. Source of truth for history and
//!     eventual replay / version rewind (Phase 1.2 builds the rewind on
//!     top; the log itself is written from day one so history isn't
//!     lost in the meantime).
//!
//! Every mutation that writes an `apps` row also writes at least one
//! `app_events` row in the same SQLite transaction. The two cannot
//! diverge.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ordo_protocol::{App, AppEvent, AppEventKind, AppStatus};
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use uuid::Uuid;

use crate::types::{AppRef, AppsError, AppsResult};

pub struct AppsStore {
    db: OrdoDatabase,
}

impl AppsStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> AppsResult<Self> {
        let db =
            OrdoDatabase::open(path.as_ref()).map_err(|err| AppsError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> AppsResult<Self> {
        let db = OrdoDatabase::in_memory().map_err(|err| AppsError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    /// Insert a new app + its `Created` event atomically.
    pub fn insert(
        &mut self,
        workspace_id: &str,
        slug: &str,
        name: &str,
        description: &str,
        metadata: &BTreeMap<String, Value>,
        actor: &str,
    ) -> AppsResult<App> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let metadata_json =
            serde_json::to_string(metadata).map_err(|err| AppsError::Storage(err.to_string()))?;
        let conn = self.db.conn_mut();
        let tx = conn
            .transaction()
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        tx.execute(
            "INSERT INTO apps (
                id, workspace_id, slug, name, description, status,
                metadata_json, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6, ?7, ?7)",
            params![
                id.to_string(),
                workspace_id,
                slug,
                name,
                description,
                metadata_json,
                now.to_rfc3339(),
            ],
        )
        .map_err(|err| slug_conflict_or_storage(err, workspace_id, slug))?;
        append_event_tx(
            &tx,
            id,
            workspace_id,
            0,
            actor,
            now,
            &AppEventKind::Created {
                slug: slug.to_string(),
                name: name.to_string(),
                description: description.to_string(),
            },
        )?;
        tx.commit()
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        Ok(App {
            id,
            workspace_id: workspace_id.to_string(),
            slug: slug.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            status: AppStatus::Draft,
            metadata: metadata.clone(),
            created_at: now,
            updated_at: now,
            published_at: None,
            archived_at: None,
        })
    }

    pub fn get(&self, app_ref: &AppRef) -> AppsResult<Option<App>> {
        let conn = self.db.conn();
        let (id, row) = match app_ref {
            AppRef::Id(id) => (
                id.to_string(),
                conn.query_row(
                    "SELECT id, workspace_id, slug, name, description, status,
                            metadata_json, created_at, updated_at, published_at, archived_at
                     FROM apps WHERE id = ?1",
                    params![id.to_string()],
                    row_to_app,
                )
                .optional()
                .map_err(|err| AppsError::Storage(err.to_string()))?,
            ),
            AppRef::Slug { workspace_id, slug } => (
                format!("{workspace_id}/{slug}"),
                conn.query_row(
                    "SELECT id, workspace_id, slug, name, description, status,
                            metadata_json, created_at, updated_at, published_at, archived_at
                     FROM apps WHERE workspace_id = ?1 AND slug = ?2",
                    params![workspace_id, slug],
                    row_to_app,
                )
                .optional()
                .map_err(|err| AppsError::Storage(err.to_string()))?,
            ),
        };
        match row {
            Some(app) => Ok(Some(app)),
            None => {
                let _ = id;
                Ok(None)
            }
        }
    }

    pub fn require(&self, app_ref: &AppRef) -> AppsResult<App> {
        let key = match app_ref {
            AppRef::Id(id) => id.to_string(),
            AppRef::Slug { workspace_id, slug } => format!("{workspace_id}/{slug}"),
        };
        self.get(app_ref)?.ok_or(AppsError::NotFound(key))
    }

    pub fn list(
        &self,
        workspace_id: Option<&str>,
        status: Option<AppStatus>,
        limit: Option<u32>,
    ) -> AppsResult<Vec<App>> {
        let mut sql = String::from(
            "SELECT id, workspace_id, slug, name, description, status,
                    metadata_json, created_at, updated_at, published_at, archived_at
             FROM apps WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(ws) = workspace_id {
            sql.push_str(" AND workspace_id = ?");
            args.push(Box::new(ws.to_string()));
        }
        if let Some(st) = status {
            sql.push_str(" AND status = ?");
            args.push(Box::new(st.label().to_string()));
        }
        sql.push_str(" ORDER BY updated_at DESC");
        if let Some(lim) = limit {
            sql.push_str(" LIMIT ?");
            args.push(Box::new(lim as i64));
        }
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let arg_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let rows = stmt
            .query_map(arg_refs.as_slice(), row_to_app)
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let mut apps = Vec::new();
        for row in rows {
            apps.push(row.map_err(|err| AppsError::Storage(err.to_string()))?);
        }
        Ok(apps)
    }

    /// Apply a set of mutations + write their corresponding events
    /// atomically. `expected_status` gates the operation â€” used by
    /// status transitions so we never publish an archived app, etc.
    pub fn apply_mutations(
        &mut self,
        app_id: Uuid,
        expected_status: Option<AppStatus>,
        mutations: Vec<Mutation>,
        actor: &str,
    ) -> AppsResult<App> {
        let now = Utc::now();
        let conn = self.db.conn_mut();
        let tx = conn
            .transaction()
            .map_err(|err| AppsError::Storage(err.to_string()))?;

        // Reload current state inside the tx.
        let current: App = tx
            .query_row(
                "SELECT id, workspace_id, slug, name, description, status,
                        metadata_json, created_at, updated_at, published_at, archived_at
                 FROM apps WHERE id = ?1",
                params![app_id.to_string()],
                row_to_app,
            )
            .optional()
            .map_err(|err| AppsError::Storage(err.to_string()))?
            .ok_or_else(|| AppsError::NotFound(app_id.to_string()))?;

        if let Some(expected) = expected_status {
            if current.status != expected {
                return Err(AppsError::InvalidTransition {
                    from: current.status.label(),
                    to: "depends on mutation",
                });
            }
        }

        let mut next_seq: u64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM app_events WHERE app_id = ?1",
                params![app_id.to_string()],
                |row| row.get::<_, i64>(0).map(|v| v as u64),
            )
            .map_err(|err| AppsError::Storage(err.to_string()))?;

        let mut name = current.name.clone();
        let mut description = current.description.clone();
        let mut metadata = current.metadata.clone();
        let mut status = current.status;
        let mut published_at = current.published_at;
        let mut archived_at = current.archived_at;

        for mutation in mutations {
            let event_kind = match &mutation {
                Mutation::Rename(new_name) => {
                    let from = name.clone();
                    name = new_name.clone();
                    AppEventKind::Renamed {
                        from,
                        to: new_name.clone(),
                    }
                }
                Mutation::UpdateDescription(new_desc) => {
                    description = new_desc.clone();
                    AppEventKind::DescriptionUpdated {
                        description: new_desc.clone(),
                    }
                }
                Mutation::SetMetadata(key, value) => {
                    metadata.insert(key.clone(), value.clone());
                    AppEventKind::MetadataSet {
                        key: key.clone(),
                        value: value.clone(),
                    }
                }
                Mutation::RemoveMetadata(key) => {
                    metadata.remove(key);
                    AppEventKind::MetadataRemoved { key: key.clone() }
                }
                Mutation::Publish => {
                    status = AppStatus::Published;
                    published_at = Some(now);
                    AppEventKind::Published
                }
                Mutation::Unpublish => {
                    status = AppStatus::Draft;
                    AppEventKind::Unpublished
                }
                Mutation::Archive => {
                    status = AppStatus::Archived;
                    archived_at = Some(now);
                    AppEventKind::Archived
                }
                Mutation::Unarchive => {
                    status = AppStatus::Draft;
                    archived_at = None;
                    AppEventKind::Unarchived
                }
            };
            append_event_tx(
                &tx,
                app_id,
                &current.workspace_id,
                next_seq,
                actor,
                now,
                &event_kind,
            )?;
            next_seq += 1;
        }

        let metadata_json =
            serde_json::to_string(&metadata).map_err(|err| AppsError::Storage(err.to_string()))?;
        tx.execute(
            "UPDATE apps SET name = ?1, description = ?2, status = ?3,
                             metadata_json = ?4, updated_at = ?5,
                             published_at = ?6, archived_at = ?7
             WHERE id = ?8",
            params![
                name,
                description,
                status.label(),
                metadata_json,
                now.to_rfc3339(),
                published_at.map(|dt| dt.to_rfc3339()),
                archived_at.map(|dt| dt.to_rfc3339()),
                app_id.to_string(),
            ],
        )
        .map_err(|err| AppsError::Storage(err.to_string()))?;

        tx.commit()
            .map_err(|err| AppsError::Storage(err.to_string()))?;

        Ok(App {
            id: app_id,
            workspace_id: current.workspace_id,
            slug: current.slug,
            name,
            description,
            status,
            metadata,
            created_at: current.created_at,
            updated_at: now,
            published_at,
            archived_at,
        })
    }

    /// Fold the event stream up to (and including) a given sequence
    /// to reconstruct the historical app state. Returns `None` if the
    /// app has no events at or before that sequence (typically: bad
    /// `up_to_seq`). The folded state carries the same fields as a
    /// live `apps` row would; `updated_at` reflects the last included
    /// event's `created_at`, not the current wall clock.
    ///
    /// This is the foundational primitive for Phase 1.2 "version
    /// rewind": `state_at_version(id, n)` answers "what did this app
    /// look like after event N?" Higher-level features (rollback,
    /// diff) compose on top.
    pub fn state_at_version(&self, app_id: Uuid, up_to_seq: u64) -> AppsResult<Option<App>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, app_id, workspace_id, seq, kind, payload_json, actor, created_at
                 FROM app_events WHERE app_id = ?1 AND seq <= ?2 ORDER BY seq ASC",
            )
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![app_id.to_string(), up_to_seq as i64], row_to_event)
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|err| AppsError::Storage(err.to_string()))?);
        }
        Ok(fold_events_into_state(&events))
    }

    // ---- Deployments (Phase 3.3) ------------------------------------

    /// Create a new deployment pointing at the app's current highest
    /// event sequence. Returns the created record. State starts as
    /// `pending` â€” `promote` or `fail` move it to a terminal state.
    pub fn create_deployment(
        &mut self,
        app_id: Uuid,
        preview_path: Option<String>,
        note: &str,
    ) -> AppsResult<ordo_protocol::Deployment> {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let conn = self.db.conn_mut();
        let tx = conn
            .transaction()
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let workspace_id: String = tx
            .query_row(
                "SELECT workspace_id FROM apps WHERE id = ?1",
                params![app_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| AppsError::Storage(err.to_string()))?
            .ok_or_else(|| AppsError::NotFound(app_id.to_string()))?;
        let max_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), -1) FROM app_events WHERE app_id = ?1",
                params![app_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        if max_seq < 0 {
            return Err(AppsError::InvalidArgument(
                "app has no events â€” cannot create a deployment".into(),
            ));
        }
        tx.execute(
            "INSERT INTO app_deployments (
                id, app_id, workspace_id, app_event_seq, state, preview_path, note, created_at
            ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7)",
            params![
                id.to_string(),
                app_id.to_string(),
                workspace_id,
                max_seq,
                preview_path,
                note,
                now.to_rfc3339(),
            ],
        )
        .map_err(|err| AppsError::Storage(err.to_string()))?;
        tx.commit()
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        Ok(ordo_protocol::Deployment {
            id,
            app_id,
            workspace_id,
            app_event_seq: max_seq as u64,
            state: ordo_protocol::DeploymentState::Pending,
            preview_path,
            note: note.to_string(),
            created_at: now,
            promoted_at: None,
        })
    }

    pub fn set_deployment_state(
        &mut self,
        deployment_id: Uuid,
        state: ordo_protocol::DeploymentState,
    ) -> AppsResult<ordo_protocol::Deployment> {
        let now = Utc::now();
        let conn = self.db.conn_mut();
        let promoted_at = if matches!(state, ordo_protocol::DeploymentState::Live) {
            Some(now.to_rfc3339())
        } else {
            None
        };
        let updated = conn
            .execute(
                "UPDATE app_deployments SET state = ?1, promoted_at = COALESCE(?2, promoted_at)
                 WHERE id = ?3",
                params![state.label(), promoted_at, deployment_id.to_string()],
            )
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(AppsError::NotFound(deployment_id.to_string()));
        }
        self.get_deployment(deployment_id)?
            .ok_or_else(|| AppsError::NotFound(deployment_id.to_string()))
    }

    pub fn get_deployment(
        &self,
        deployment_id: Uuid,
    ) -> AppsResult<Option<ordo_protocol::Deployment>> {
        let conn = self.db.conn();
        conn.query_row(
            "SELECT id, app_id, workspace_id, app_event_seq, state, preview_path, note,
                    created_at, promoted_at
             FROM app_deployments WHERE id = ?1",
            params![deployment_id.to_string()],
            row_to_deployment,
        )
        .optional()
        .map_err(|err| AppsError::Storage(err.to_string()))
    }

    pub fn list_deployments(&self, app_id: Uuid) -> AppsResult<Vec<ordo_protocol::Deployment>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, app_id, workspace_id, app_event_seq, state, preview_path, note,
                        created_at, promoted_at
                 FROM app_deployments WHERE app_id = ?1
                 ORDER BY created_at DESC",
            )
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![app_id.to_string()], row_to_deployment)
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| AppsError::Storage(err.to_string()))?);
        }
        Ok(out)
    }

    /// Return all events for an app in ascending sequence order. Used
    /// for debug / history inspection / Phase 1.2 version rewind.
    pub fn events(&self, app_id: Uuid) -> AppsResult<Vec<AppEvent>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, app_id, workspace_id, seq, kind, payload_json, actor, created_at
                 FROM app_events WHERE app_id = ?1 ORDER BY seq ASC",
            )
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![app_id.to_string()], row_to_event)
            .map_err(|err| AppsError::Storage(err.to_string()))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|err| AppsError::Storage(err.to_string()))?);
        }
        Ok(events)
    }
}

/// Atomic mutation primitives. The service translates public update
/// requests into a sequence of these and hands them to `apply_mutations`.
pub enum Mutation {
    Rename(String),
    UpdateDescription(String),
    SetMetadata(String, Value),
    RemoveMetadata(String),
    Publish,
    Unpublish,
    Archive,
    Unarchive,
}

/// Fold an ordered event stream into the state it describes. Used by
/// `state_at_version`. The first event must be `Created`; anything
/// else is treated as a corrupted stream and returns `None`.
///
/// Any unknown future `AppEventKind` variant is conservatively ignored
/// when folding (no-op on state), so an older binary reading a newer
/// stream still produces a consistent-for-its-era state â€” aligns with
/// Rule 11's additive-variant policy.
fn fold_events_into_state(events: &[AppEvent]) -> Option<App> {
    let first = events.first()?;
    let (slug, name, description) = match &first.event {
        AppEventKind::Created {
            slug,
            name,
            description,
        } => (slug.clone(), name.clone(), description.clone()),
        _ => return None,
    };
    let mut app = App {
        id: first.app_id,
        workspace_id: first.workspace_id.clone(),
        slug,
        name,
        description,
        status: AppStatus::Draft,
        metadata: BTreeMap::new(),
        created_at: first.created_at,
        updated_at: first.created_at,
        published_at: None,
        archived_at: None,
    };
    for event in events.iter().skip(1) {
        app.updated_at = event.created_at;
        match &event.event {
            AppEventKind::Created { .. } => {
                // Second Created event in a stream is malformed.
                return None;
            }
            AppEventKind::Renamed { to, .. } => app.name = to.clone(),
            AppEventKind::DescriptionUpdated { description } => {
                app.description = description.clone()
            }
            AppEventKind::MetadataSet { key, value } => {
                app.metadata.insert(key.clone(), value.clone());
            }
            AppEventKind::MetadataRemoved { key } => {
                app.metadata.remove(key);
            }
            AppEventKind::Published => {
                app.status = AppStatus::Published;
                app.published_at = Some(event.created_at);
            }
            AppEventKind::Unpublished => {
                app.status = AppStatus::Draft;
            }
            AppEventKind::Archived => {
                app.status = AppStatus::Archived;
                app.archived_at = Some(event.created_at);
            }
            AppEventKind::Unarchived => {
                app.status = AppStatus::Draft;
                app.archived_at = None;
            }
        }
    }
    Some(app)
}

fn append_event_tx(
    tx: &rusqlite::Transaction<'_>,
    app_id: Uuid,
    workspace_id: &str,
    seq: u64,
    actor: &str,
    now: DateTime<Utc>,
    kind: &AppEventKind,
) -> AppsResult<()> {
    let payload = serde_json::to_value(kind).map_err(|err| AppsError::Storage(err.to_string()))?;
    // Split the discriminant from the payload body for cheap filtering
    // by kind in future queries.
    let kind_label = payload
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let payload_json =
        serde_json::to_string(&payload).map_err(|err| AppsError::Storage(err.to_string()))?;
    tx.execute(
        "INSERT INTO app_events (id, app_id, workspace_id, seq, kind, payload_json, actor, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            app_id.to_string(),
            workspace_id,
            seq as i64,
            kind_label,
            payload_json,
            actor,
            now.to_rfc3339(),
        ],
    )
    .map_err(|err| AppsError::Storage(err.to_string()))?;
    Ok(())
}

fn slug_conflict_or_storage(err: rusqlite::Error, workspace: &str, slug: &str) -> AppsError {
    // SQLite raises an error code 2067 / string "UNIQUE constraint
    // failed" on a duplicate (workspace, slug).
    let msg = err.to_string();
    if msg.contains("UNIQUE constraint failed") && msg.contains("apps.slug") {
        AppsError::SlugConflict {
            workspace: workspace.to_string(),
            slug: slug.to_string(),
        }
    } else if msg.contains("UNIQUE constraint failed") {
        // Be defensive: some SQLite versions format the column list
        // differently.
        AppsError::SlugConflict {
            workspace: workspace.to_string(),
            slug: slug.to_string(),
        }
    } else {
        AppsError::Storage(msg)
    }
}

fn row_to_app(row: &rusqlite::Row<'_>) -> rusqlite::Result<App> {
    let id_str: String = row.get(0)?;
    let status_str: String = row.get(5)?;
    let metadata_json: String = row.get(6)?;
    let created_at_str: String = row.get(7)?;
    let updated_at_str: String = row.get(8)?;
    let published_at_str: Option<String> = row.get(9)?;
    let archived_at_str: Option<String> = row.get(10)?;

    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let status = AppStatus::from_label(&status_str).unwrap_or(AppStatus::Draft);
    let metadata: BTreeMap<String, Value> =
        serde_json::from_str(&metadata_json).unwrap_or_default();
    let created_at = parse_ts(&created_at_str)?;
    let updated_at = parse_ts(&updated_at_str)?;
    let published_at = published_at_str.as_deref().map(parse_ts).transpose()?;
    let archived_at = archived_at_str.as_deref().map(parse_ts).transpose()?;

    Ok(App {
        id,
        workspace_id: row.get(1)?,
        slug: row.get(2)?,
        name: row.get(3)?,
        description: row.get(4)?,
        status,
        metadata,
        created_at,
        updated_at,
        published_at,
        archived_at,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppEvent> {
    let id_str: String = row.get(0)?;
    let app_id_str: String = row.get(1)?;
    let workspace_id: String = row.get(2)?;
    let seq: i64 = row.get(3)?;
    let _kind_label: String = row.get(4)?;
    let payload_json: String = row.get(5)?;
    let actor: String = row.get(6)?;
    let created_at_str: String = row.get(7)?;

    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let app_id = Uuid::parse_str(&app_id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let event: AppEventKind = serde_json::from_str(&payload_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let created_at = parse_ts(&created_at_str)?;

    Ok(AppEvent {
        id,
        app_id,
        workspace_id,
        seq: seq as u64,
        actor,
        created_at,
        event,
    })
}

fn row_to_deployment(row: &rusqlite::Row<'_>) -> rusqlite::Result<ordo_protocol::Deployment> {
    let id_str: String = row.get(0)?;
    let app_id_str: String = row.get(1)?;
    let workspace_id: String = row.get(2)?;
    let seq: i64 = row.get(3)?;
    let state_str: String = row.get(4)?;
    let preview_path: Option<String> = row.get(5)?;
    let note: String = row.get(6)?;
    let created_at_str: String = row.get(7)?;
    let promoted_at_str: Option<String> = row.get(8)?;

    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let app_id = Uuid::parse_str(&app_id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let state = ordo_protocol::DeploymentState::from_label(&state_str)
        .unwrap_or(ordo_protocol::DeploymentState::Pending);
    let created_at = parse_ts(&created_at_str)?;
    let promoted_at = promoted_at_str.as_deref().map(parse_ts).transpose()?;

    Ok(ordo_protocol::Deployment {
        id,
        app_id,
        workspace_id,
        app_event_seq: seq as u64,
        state,
        preview_path,
        note,
        created_at,
        promoted_at,
    })
}

fn parse_ts(s: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get_roundtrip() {
        let mut store = AppsStore::in_memory().expect("store");
        let meta = BTreeMap::new();
        let app = store
            .insert("local", "hello", "Hello", "", &meta, "operator")
            .expect("insert");
        assert_eq!(app.workspace_id, "local");
        assert_eq!(app.slug, "hello");
        assert_eq!(app.status, AppStatus::Draft);

        let fetched = store
            .get(&AppRef::Id(app.id))
            .expect("get")
            .expect("present");
        assert_eq!(fetched.id, app.id);
        assert_eq!(fetched.name, "Hello");

        let fetched_by_slug = store
            .get(&AppRef::Slug {
                workspace_id: "local".into(),
                slug: "hello".into(),
            })
            .expect("get by slug")
            .expect("present");
        assert_eq!(fetched_by_slug.id, app.id);

        let events = store.events(app.id).expect("events");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, AppEventKind::Created { .. }));
    }

    #[test]
    fn slug_conflict_reported_distinctly() {
        let mut store = AppsStore::in_memory().expect("store");
        let meta = BTreeMap::new();
        store
            .insert("local", "dup", "First", "", &meta, "operator")
            .expect("first");
        let err = store
            .insert("local", "dup", "Second", "", &meta, "operator")
            .expect_err("second should conflict");
        assert!(matches!(err, AppsError::SlugConflict { .. }));
    }

    #[test]
    fn publish_transitions_status_and_writes_event() {
        let mut store = AppsStore::in_memory().expect("store");
        let meta = BTreeMap::new();
        let app = store
            .insert("local", "pub-test", "P", "", &meta, "operator")
            .expect("insert");
        let updated = store
            .apply_mutations(
                app.id,
                Some(AppStatus::Draft),
                vec![Mutation::Publish],
                "operator",
            )
            .expect("publish");
        assert_eq!(updated.status, AppStatus::Published);
        assert!(updated.published_at.is_some());

        let events = store.events(app.id).expect("events");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[1].event, AppEventKind::Published));
        assert_eq!(events[1].seq, 1);
    }

    #[test]
    fn rename_and_metadata_mutations_compose() {
        let mut store = AppsStore::in_memory().expect("store");
        let meta = BTreeMap::new();
        let app = store
            .insert("local", "compose", "Old", "", &meta, "operator")
            .expect("insert");
        let updated = store
            .apply_mutations(
                app.id,
                None,
                vec![
                    Mutation::Rename("New".into()),
                    Mutation::SetMetadata("preview_url".into(), Value::String("http://x".into())),
                ],
                "operator",
            )
            .expect("mutate");
        assert_eq!(updated.name, "New");
        assert_eq!(
            updated.metadata.get("preview_url"),
            Some(&Value::String("http://x".into()))
        );
        let events = store.events(app.id).expect("events");
        assert_eq!(events.len(), 3);
        assert!(matches!(events[1].event, AppEventKind::Renamed { .. }));
        assert!(matches!(events[2].event, AppEventKind::MetadataSet { .. }));
    }

    #[test]
    fn state_at_version_replays_history() {
        let mut store = AppsStore::in_memory().expect("store");
        let meta = BTreeMap::new();
        let app = store
            .insert("local", "rewind", "First", "orig", &meta, "op")
            .expect("insert");
        store
            .apply_mutations(
                app.id,
                None,
                vec![Mutation::Rename("Second".into()), Mutation::Publish],
                "op",
            )
            .expect("mutate");

        let at_created = store
            .state_at_version(app.id, 0)
            .expect("fold 0")
            .expect("present");
        assert_eq!(at_created.name, "First");
        assert_eq!(at_created.status, AppStatus::Draft);

        let at_renamed = store
            .state_at_version(app.id, 1)
            .expect("fold 1")
            .expect("present");
        assert_eq!(at_renamed.name, "Second");
        assert_eq!(at_renamed.status, AppStatus::Draft);

        let at_published = store
            .state_at_version(app.id, 2)
            .expect("fold 2")
            .expect("present");
        assert_eq!(at_published.status, AppStatus::Published);
        assert!(at_published.published_at.is_some());
    }

    #[test]
    fn list_filters_by_workspace_and_status() {
        let mut store = AppsStore::in_memory().expect("store");
        let meta = BTreeMap::new();
        store.insert("local", "a", "A", "", &meta, "op").expect("a");
        let b = store.insert("local", "b", "B", "", &meta, "op").expect("b");
        store.insert("other", "c", "C", "", &meta, "op").expect("c");
        store
            .apply_mutations(b.id, Some(AppStatus::Draft), vec![Mutation::Publish], "op")
            .expect("publish b");

        let local_all = store.list(Some("local"), None, None).expect("local all");
        assert_eq!(local_all.len(), 2);
        let local_pub = store
            .list(Some("local"), Some(AppStatus::Published), None)
            .expect("local pub");
        assert_eq!(local_pub.len(), 1);
        assert_eq!(local_pub[0].slug, "b");
    }
}
