//! SQLite persistence for webhook subscriptions.

use chrono::{DateTime, Utc};
use ordo_protocol::WebhookSubscription;
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::types::{WebhookError, WebhookResult};

pub struct WebhookStore {
    db: OrdoDatabase,
}

impl WebhookStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> WebhookResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| WebhookError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> WebhookResult<Self> {
        let db = OrdoDatabase::in_memory().map_err(|err| WebhookError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert(
        &mut self,
        id: Uuid,
        workspace_id: &str,
        target_url: &str,
        secret: &str,
        topics: &[String],
        description: &str,
        created_at: DateTime<Utc>,
    ) -> WebhookResult<WebhookSubscription> {
        let topics_json =
            serde_json::to_string(topics).map_err(|err| WebhookError::Storage(err.to_string()))?;
        let conn = self.db.conn_mut();
        conn.execute(
            "INSERT INTO webhook_subscriptions (
                id, workspace_id, target_url, secret, topics_json, description,
                active, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
            params![
                id.to_string(),
                workspace_id,
                target_url,
                secret,
                topics_json,
                description,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(|err| WebhookError::Storage(err.to_string()))?;
        Ok(WebhookSubscription {
            id,
            workspace_id: workspace_id.to_string(),
            target_url: target_url.to_string(),
            secret: secret.to_string(),
            topics: topics.to_vec(),
            description: description.to_string(),
            active: true,
            created_at,
            last_delivery_at: None,
            last_delivery_status: None,
        })
    }

    pub fn get(&self, id: Uuid) -> WebhookResult<Option<WebhookSubscription>> {
        let conn = self.db.conn();
        conn.query_row(
            "SELECT id, workspace_id, target_url, secret, topics_json, description,
                    active, created_at, last_delivery_at, last_delivery_status
             FROM webhook_subscriptions WHERE id = ?1",
            params![id.to_string()],
            row_to_subscription,
        )
        .optional()
        .map_err(|err| WebhookError::Storage(err.to_string()))
    }

    pub fn list(&self, workspace_id: Option<&str>) -> WebhookResult<Vec<WebhookSubscription>> {
        let conn = self.db.conn();
        let (sql, has_arg) = match workspace_id {
            Some(_) => (
                "SELECT id, workspace_id, target_url, secret, topics_json, description,
                        active, created_at, last_delivery_at, last_delivery_status
                 FROM webhook_subscriptions WHERE workspace_id = ?1
                 ORDER BY created_at DESC",
                true,
            ),
            None => (
                "SELECT id, workspace_id, target_url, secret, topics_json, description,
                        active, created_at, last_delivery_at, last_delivery_status
                 FROM webhook_subscriptions
                 ORDER BY created_at DESC",
                false,
            ),
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|err| WebhookError::Storage(err.to_string()))?;
        let rows: Vec<_> = if has_arg {
            stmt.query_map(params![workspace_id.unwrap()], row_to_subscription)
                .map_err(|err| WebhookError::Storage(err.to_string()))?
                .collect()
        } else {
            stmt.query_map([], row_to_subscription)
                .map_err(|err| WebhookError::Storage(err.to_string()))?
                .collect()
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row.map_err(|err| WebhookError::Storage(err.to_string()))?);
        }
        Ok(out)
    }

    /// Active subscriptions keyed for delivery. Dispatcher uses this.
    pub fn active(&self) -> WebhookResult<Vec<WebhookSubscription>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, workspace_id, target_url, secret, topics_json, description,
                        active, created_at, last_delivery_at, last_delivery_status
                 FROM webhook_subscriptions WHERE active = 1",
            )
            .map_err(|err| WebhookError::Storage(err.to_string()))?;
        let rows: Vec<_> = stmt
            .query_map([], row_to_subscription)
            .map_err(|err| WebhookError::Storage(err.to_string()))?
            .collect();
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row.map_err(|err| WebhookError::Storage(err.to_string()))?);
        }
        Ok(out)
    }

    pub fn delete(&mut self, id: Uuid) -> WebhookResult<bool> {
        let conn = self.db.conn_mut();
        let removed = conn
            .execute(
                "DELETE FROM webhook_subscriptions WHERE id = ?1",
                params![id.to_string()],
            )
            .map_err(|err| WebhookError::Storage(err.to_string()))?;
        Ok(removed > 0)
    }

    pub fn set_active(&mut self, id: Uuid, active: bool) -> WebhookResult<()> {
        let conn = self.db.conn_mut();
        let updated = conn
            .execute(
                "UPDATE webhook_subscriptions SET active = ?1 WHERE id = ?2",
                params![if active { 1 } else { 0 }, id.to_string()],
            )
            .map_err(|err| WebhookError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(WebhookError::NotFound(id));
        }
        Ok(())
    }

    pub fn record_delivery(
        &mut self,
        id: Uuid,
        when: DateTime<Utc>,
        status: u16,
    ) -> WebhookResult<()> {
        let conn = self.db.conn_mut();
        conn.execute(
            "UPDATE webhook_subscriptions
             SET last_delivery_at = ?1, last_delivery_status = ?2
             WHERE id = ?3",
            params![when.to_rfc3339(), status as i64, id.to_string()],
        )
        .map_err(|err| WebhookError::Storage(err.to_string()))?;
        Ok(())
    }
}

fn row_to_subscription(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebhookSubscription> {
    let id_str: String = row.get(0)?;
    let topics_json: String = row.get(4)?;
    let created_at_str: String = row.get(7)?;
    let last_delivery_at_str: Option<String> = row.get(8)?;
    let last_delivery_status: Option<i64> = row.get(9)?;
    let active_int: i64 = row.get(6)?;

    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let topics: Vec<String> = serde_json::from_str(&topics_json).unwrap_or_default();
    let created_at = parse_ts(&created_at_str)?;
    let last_delivery_at = last_delivery_at_str.as_deref().map(parse_ts).transpose()?;

    Ok(WebhookSubscription {
        id,
        workspace_id: row.get(1)?,
        target_url: row.get(2)?,
        secret: row.get(3)?,
        topics,
        description: row.get(5)?,
        active: active_int != 0,
        created_at,
        last_delivery_at,
        last_delivery_status: last_delivery_status.map(|v| v.clamp(0, u16::MAX as i64) as u16),
    })
}

fn parse_ts(s: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })
}
