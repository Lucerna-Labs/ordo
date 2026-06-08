//! SQLite persistence for the `connections` table.
//!
//! Schema lives in `ordo-store::MIGRATIONS_SLICE`. This module
//! only reads/writes â€” it doesn't own the schema.

use chrono::Utc;
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ConnectionStoreError {
    #[error("storage: {0}")]
    Storage(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("serialization: {0}")]
    Serialization(String),
}

pub type ConnectionStoreResult<T> = Result<T, ConnectionStoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Untested,
    Ok,
    Error,
}

impl ConnectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Untested => "untested",
            Self::Ok => "ok",
            Self::Error => "error",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Some(match label {
            "untested" => Self::Untested,
            "ok" => Self::Ok,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionRow {
    pub id: String,
    pub workspace_id: String,
    pub type_id: String,
    pub friendly_name: String,
    /// Non-secret fields specific to this connection type
    /// (model account, SSH host, API endpoint, etc.).
    pub fields: Value,
    /// `Some(...)` -> sealed_secrets row id holding the secret.
    /// `None` for types that don't require a secret.
    #[serde(default)]
    pub vault_secret_id: Option<String>,
    pub status: ConnectionStatus,
    #[serde(default)]
    pub status_detail: Option<String>,
    #[serde(default)]
    pub last_test_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub struct ConnectionStore {
    db: OrdoDatabase,
}

impl ConnectionStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> ConnectionStoreResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| ConnectionStoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> ConnectionStoreResult<Self> {
        let db = OrdoDatabase::in_memory()
            .map_err(|err| ConnectionStoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    pub fn insert(&mut self, row: &ConnectionRow) -> ConnectionStoreResult<()> {
        let fields_json = serde_json::to_string(&row.fields)
            .map_err(|err| ConnectionStoreError::Serialization(err.to_string()))?;
        self.db
            .conn()
            .execute(
                "INSERT INTO connections (
                    id, workspace_id, type_id, friendly_name, fields_json,
                    vault_secret_id, status, status_detail, last_test_at_ms,
                    created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    row.id,
                    row.workspace_id,
                    row.type_id,
                    row.friendly_name,
                    fields_json,
                    row.vault_secret_id,
                    row.status.label(),
                    row.status_detail,
                    row.last_test_at_ms,
                    row.created_at_ms,
                    row.updated_at_ms,
                ],
            )
            .map_err(|err| {
                if err.to_string().contains("UNIQUE") {
                    ConnectionStoreError::Conflict(format!("connection id {} exists", row.id))
                } else {
                    ConnectionStoreError::Storage(err.to_string())
                }
            })?;
        Ok(())
    }

    pub fn update_fields_and_secret(
        &mut self,
        id: &str,
        friendly_name: &str,
        fields: &Value,
        vault_secret_id: Option<&str>,
    ) -> ConnectionStoreResult<()> {
        let fields_json = serde_json::to_string(fields)
            .map_err(|err| ConnectionStoreError::Serialization(err.to_string()))?;
        let now = Utc::now().timestamp_millis();
        let updated = self
            .db
            .conn()
            .execute(
                "UPDATE connections
                 SET friendly_name = ?2, fields_json = ?3, vault_secret_id = ?4,
                     updated_at_ms = ?5,
                     status = 'untested', status_detail = NULL, last_test_at_ms = NULL
                 WHERE id = ?1",
                params![id, friendly_name, fields_json, vault_secret_id, now],
            )
            .map_err(|err| ConnectionStoreError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(ConnectionStoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn update_status(
        &mut self,
        id: &str,
        status: ConnectionStatus,
        detail: Option<&str>,
    ) -> ConnectionStoreResult<()> {
        let now = Utc::now().timestamp_millis();
        let updated = self
            .db
            .conn()
            .execute(
                "UPDATE connections
                 SET status = ?2, status_detail = ?3, last_test_at_ms = ?4,
                     updated_at_ms = ?4
                 WHERE id = ?1",
                params![id, status.label(), detail, now],
            )
            .map_err(|err| ConnectionStoreError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(ConnectionStoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> ConnectionStoreResult<Option<ConnectionRow>> {
        let row = self.get(id)?;
        let removed = self
            .db
            .conn()
            .execute("DELETE FROM connections WHERE id = ?1", params![id])
            .map_err(|err| ConnectionStoreError::Storage(err.to_string()))?;
        if removed == 0 {
            return Ok(None);
        }
        Ok(row)
    }

    pub fn get(&self, id: &str) -> ConnectionStoreResult<Option<ConnectionRow>> {
        self.db
            .conn()
            .query_row(
                "SELECT id, workspace_id, type_id, friendly_name, fields_json,
                        vault_secret_id, status, status_detail, last_test_at_ms,
                        created_at_ms, updated_at_ms
                 FROM connections WHERE id = ?1",
                params![id],
                row_to_connection,
            )
            .optional()
            .map_err(|err| ConnectionStoreError::Storage(err.to_string()))?
            .transpose()
    }

    pub fn list(&self, workspace_id: &str) -> ConnectionStoreResult<Vec<ConnectionRow>> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, workspace_id, type_id, friendly_name, fields_json,
                        vault_secret_id, status, status_detail, last_test_at_ms,
                        created_at_ms, updated_at_ms
                 FROM connections
                 WHERE workspace_id = ?1
                 ORDER BY created_at_ms ASC",
            )
            .map_err(|err| ConnectionStoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![workspace_id], row_to_connection)
            .map_err(|err| ConnectionStoreError::Storage(err.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| ConnectionStoreError::Storage(err.to_string()))??);
        }
        Ok(out)
    }
}

fn row_to_connection(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<ConnectionRow, ConnectionStoreError>> {
    let id: String = row.get(0)?;
    let workspace_id: String = row.get(1)?;
    let type_id: String = row.get(2)?;
    let friendly_name: String = row.get(3)?;
    let fields_json: String = row.get(4)?;
    let vault_secret_id: Option<String> = row.get(5)?;
    let status_label: String = row.get(6)?;
    let status_detail: Option<String> = row.get(7)?;
    let last_test_at_ms: Option<i64> = row.get(8)?;
    let created_at_ms: i64 = row.get(9)?;
    let updated_at_ms: i64 = row.get(10)?;
    Ok((|| -> Result<ConnectionRow, ConnectionStoreError> {
        let fields: Value = serde_json::from_str(&fields_json)
            .map_err(|err| ConnectionStoreError::Serialization(err.to_string()))?;
        let status = ConnectionStatus::from_label(&status_label).ok_or_else(|| {
            ConnectionStoreError::Serialization(format!("unknown status `{status_label}`"))
        })?;
        Ok(ConnectionRow {
            id,
            workspace_id,
            type_id,
            friendly_name,
            fields,
            vault_secret_id,
            status,
            status_detail,
            last_test_at_ms,
            created_at_ms,
            updated_at_ms,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample(id: &str, type_id: &str, name: &str) -> ConnectionRow {
        let now = Utc::now().timestamp_millis();
        ConnectionRow {
            id: id.into(),
            workspace_id: "local".into(),
            type_id: type_id.into(),
            friendly_name: name.into(),
            fields: json!({"site_url": "https://example.com"}),
            vault_secret_id: Some("vault-1".into()),
            status: ConnectionStatus::Untested,
            status_detail: None,
            last_test_at_ms: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    #[test]
    fn insert_and_get_round_trip() {
        let mut store = ConnectionStore::in_memory().unwrap();
        let row = sample("conn-1", "generic_api_key", "Build API");
        store.insert(&row).unwrap();
        let loaded = store.get("conn-1").unwrap().unwrap();
        assert_eq!(loaded.friendly_name, "Build API");
        assert_eq!(loaded.type_id, "generic_api_key");
        assert_eq!(loaded.fields["site_url"], "https://example.com");
        assert!(matches!(loaded.status, ConnectionStatus::Untested));
    }

    #[test]
    fn list_filters_by_workspace_and_orders_by_created() {
        let mut store = ConnectionStore::in_memory().unwrap();
        let mut a = sample("conn-1", "generic_api_key", "Personal");
        let mut b = sample("conn-2", "generic_api_key", "Build");
        a.created_at_ms = 1;
        b.created_at_ms = 2;
        store.insert(&a).unwrap();
        store.insert(&b).unwrap();
        let list = store.list("local").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].friendly_name, "Personal");
        assert_eq!(list[1].friendly_name, "Build");
    }

    #[test]
    fn update_status_resets_when_fields_change() {
        let mut store = ConnectionStore::in_memory().unwrap();
        let row = sample("conn-1", "generic_api_key", "Build API");
        store.insert(&row).unwrap();
        store
            .update_status("conn-1", ConnectionStatus::Ok, Some("verified"))
            .unwrap();
        let after_ok = store.get("conn-1").unwrap().unwrap();
        assert!(matches!(after_ok.status, ConnectionStatus::Ok));

        store
            .update_fields_and_secret(
                "conn-1",
                "Build API v2",
                &json!({"site_url": "https://new.example.com"}),
                Some("vault-2"),
            )
            .unwrap();
        let after_edit = store.get("conn-1").unwrap().unwrap();
        // Fields changed -> status reset to untested.
        assert!(matches!(after_edit.status, ConnectionStatus::Untested));
        assert_eq!(after_edit.friendly_name, "Build API v2");
        assert_eq!(after_edit.fields["site_url"], "https://new.example.com");
    }

    #[test]
    fn delete_returns_the_removed_row() {
        let mut store = ConnectionStore::in_memory().unwrap();
        let row = sample("conn-1", "generic_api_key", "x");
        store.insert(&row).unwrap();
        let deleted = store.delete("conn-1").unwrap().unwrap();
        assert_eq!(deleted.id, "conn-1");
        assert!(store.get("conn-1").unwrap().is_none());
    }

    #[test]
    fn allows_multiple_connections_of_the_same_type() {
        let mut store = ConnectionStore::in_memory().unwrap();
        for i in 0..5 {
            let mut r = sample(
                &format!("conn-{i}"),
                "generic_api_key",
                &format!("Account {i}"),
            );
            r.created_at_ms = i as i64;
            store.insert(&r).unwrap();
        }
        let list = store.list("local").unwrap();
        assert_eq!(list.len(), 5);
    }
}
