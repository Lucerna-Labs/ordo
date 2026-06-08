//! ConnectionService â€” the orchestrator the control API talks to.
//!
//! Holds a `ConnectionStore` (metadata) + an `Arc<VaultService>`
//! (sealed secret material). Operations:
//!
//!   create â€” insert metadata row, vault-seal the secret if
//!     `requires_secret`, immediately run the test handler so the
//!     operator sees a green/red result without an extra click.
//!   list / get â€” return rows; secrets are NEVER returned through
//!     this API. The studio displays "configured" / "needs setup"
//!     based on `vault_secret_id.is_some()`.
//!   update â€” replace fields + optionally the secret. Status
//!     resets to untested; caller can re-test.
//!   delete â€” remove the row + retire the secret (vault keeps
//!     the row for audit continuity per invariant 23).
//!   test â€” re-run the type's tester against the current fields +
//!     stored secret. Persists status + detail.
//!
//! Provider id used when dereferencing the secret from the vault:
//! `connection:<connection_id>`. That binds each secret to the
//! connection it belongs to â€” the broker's allowed-providers
//! check refuses cross-connection reads.

use std::sync::Arc;

use chrono::Utc;
use ordo_protocol::SecretClass;
use ordo_secrets_vault::{SecureBytes, VaultError, VaultService};
use serde_json::Value;

use crate::store::{ConnectionRow, ConnectionStatus, ConnectionStore, ConnectionStoreError};
use crate::types::{self, ConnectionType, FieldType, TestReport, TestStatus};

#[derive(Debug, thiserror::Error)]
pub enum ConnectionServiceError {
    #[error("store: {0}")]
    Store(#[from] ConnectionStoreError),
    #[error("vault: {0}")]
    Vault(#[from] VaultError),
    #[error("unknown connection type: {0}")]
    UnknownType(String),
    #[error("invalid input: {0}")]
    BadInput(String),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type ConnectionServiceResult<T> = Result<T, ConnectionServiceError>;

pub struct ConnectionService {
    store: tokio::sync::Mutex<ConnectionStore>,
    vault: Arc<VaultService>,
    workspace_id: String,
}

impl ConnectionService {
    pub fn new(store: ConnectionStore, vault: Arc<VaultService>) -> Self {
        Self {
            store: tokio::sync::Mutex::new(store),
            vault,
            workspace_id: "local".to_string(),
        }
    }

    pub fn with_workspace_id(mut self, ws: impl Into<String>) -> Self {
        self.workspace_id = ws.into();
        self
    }

    pub async fn list(&self) -> ConnectionServiceResult<Vec<ConnectionRow>> {
        let store = self.store.lock().await;
        Ok(store.list(&self.workspace_id)?)
    }

    pub async fn get(&self, id: &str) -> ConnectionServiceResult<ConnectionRow> {
        let store = self.store.lock().await;
        store
            .get(id)?
            .ok_or_else(|| ConnectionServiceError::NotFound(id.to_string()))
    }

    /// Create a new connection. If `requires_secret` and a secret
    /// is provided, it's sealed in the vault first, then the
    /// metadata row is written referencing the new sealed_secrets
    /// row. After insert, the type's tester runs automatically and
    /// the result is persisted to the row.
    pub async fn create(
        &self,
        type_id: &str,
        friendly_name: &str,
        fields: Value,
        secret: Option<String>,
    ) -> ConnectionServiceResult<ConnectionRow> {
        let connection_type = types::find(type_id)
            .ok_or_else(|| ConnectionServiceError::UnknownType(type_id.to_string()))?;
        validate_fields(&connection_type, &fields)?;
        let cleaned_secret = secret
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if connection_type.requires_secret && cleaned_secret.is_none() {
            return Err(ConnectionServiceError::BadInput(format!(
                "type `{type_id}` requires a secret"
            )));
        }

        let id = ulid::Ulid::new().to_string();
        let now_ms = Utc::now().timestamp_millis();
        let provider_id = format!("connection:{id}");

        // Seal the secret first so we can store the vault row's
        // id on the connection. If the seal fails we never insert
        // a partial connection row.
        let vault_secret_id = match &cleaned_secret {
            Some(value) => {
                let record = self
                    .vault
                    .put(
                        SecretClass::Generic,
                        format!("connection:{type_id}:{friendly_name}"),
                        vec![provider_id.clone()],
                        SecureBytes::from_slice(value.as_bytes()),
                    )
                    .await?;
                Some(record.id)
            }
            None => None,
        };

        let row = ConnectionRow {
            id: id.clone(),
            workspace_id: self.workspace_id.clone(),
            type_id: type_id.to_string(),
            friendly_name: friendly_name.to_string(),
            fields: fields.clone(),
            vault_secret_id: vault_secret_id.clone(),
            status: ConnectionStatus::Untested,
            status_detail: None,
            last_test_at_ms: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        {
            let mut store = self.store.lock().await;
            store.insert(&row)?;
        }

        // Run the test handler immediately so the operator sees a
        // result without a separate click.
        let _ = self.test(&id).await;
        self.get(&id).await
    }

    pub async fn update(
        &self,
        id: &str,
        friendly_name: &str,
        fields: Value,
        new_secret: Option<String>,
    ) -> ConnectionServiceResult<ConnectionRow> {
        let existing = self.get(id).await?;
        let connection_type = types::find(&existing.type_id)
            .ok_or_else(|| ConnectionServiceError::UnknownType(existing.type_id.clone()))?;
        validate_fields(&connection_type, &fields)?;

        // Replacing the secret: retire the old vault row, seal the
        // new one. Leaving secret unchanged: keep the existing
        // vault_secret_id.
        let cleaned_new = new_secret
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let provider_id = format!("connection:{id}");
        let new_vault_id: Option<String> = if let Some(value) = cleaned_new {
            if let Some(old_id) = &existing.vault_secret_id {
                let _ = self.vault.retire(old_id).await;
            }
            let record = self
                .vault
                .put(
                    SecretClass::Generic,
                    format!("connection:{}:{friendly_name}", existing.type_id),
                    vec![provider_id],
                    SecureBytes::from_slice(value.as_bytes()),
                )
                .await?;
            Some(record.id)
        } else {
            existing.vault_secret_id.clone()
        };

        {
            let mut store = self.store.lock().await;
            store.update_fields_and_secret(id, friendly_name, &fields, new_vault_id.as_deref())?;
        }
        // Auto-test like create.
        let _ = self.test(id).await;
        self.get(id).await
    }

    pub async fn delete(&self, id: &str) -> ConnectionServiceResult<()> {
        let removed = {
            let mut store = self.store.lock().await;
            store.delete(id)?
        };
        if let Some(row) = removed {
            if let Some(secret_id) = row.vault_secret_id {
                // Best-effort retire â€” even if this fails, the
                // connection is gone so the secret is unreachable
                // through normal paths.
                let _ = self.vault.retire(&secret_id).await;
            }
        }
        Ok(())
    }

    /// Re-run the type's tester. Persists the result to the row's
    /// `status` / `status_detail` / `last_test_at_ms`. Returns the
    /// detailed report so the caller can show duration, etc.
    pub async fn test(&self, id: &str) -> ConnectionServiceResult<TestReport> {
        let row = self.get(id).await?;
        let secret = if let Some(secret_id) = &row.vault_secret_id {
            let provider_id = format!("connection:{id}");
            match self.vault.get_for_provider(secret_id, &provider_id).await {
                Ok(bytes) => Some(
                    String::from_utf8(bytes.as_slice().to_vec())
                        .map_err(|err| ConnectionServiceError::BadInput(err.to_string()))?,
                ),
                Err(err) => {
                    let detail = format!("vault dereference failed: {err}");
                    let mut store = self.store.lock().await;
                    store.update_status(id, ConnectionStatus::Error, Some(&detail))?;
                    return Ok(TestReport {
                        status: TestStatus::Error,
                        detail,
                        duration_ms: 0,
                    });
                }
            }
        } else {
            None
        };
        let report = types::run_test(&row.type_id, &row.fields, secret.as_deref()).await;
        let status = match report.status {
            TestStatus::Ok => ConnectionStatus::Ok,
            TestStatus::Error => ConnectionStatus::Error,
            TestStatus::NotApplicable => ConnectionStatus::Untested,
        };
        let mut store = self.store.lock().await;
        store.update_status(id, status, Some(&report.detail))?;
        Ok(report)
    }
}

fn validate_fields(
    connection_type: &ConnectionType,
    fields: &Value,
) -> ConnectionServiceResult<()> {
    let map = fields
        .as_object()
        .ok_or_else(|| ConnectionServiceError::BadInput("fields must be a JSON object".into()))?;
    for field in &connection_type.fields {
        if !field.required {
            continue;
        }
        let present = map
            .get(field.name)
            .map(|v| match field.field_type {
                FieldType::Text | FieldType::Url | FieldType::Email | FieldType::LongText => {
                    v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)
                }
                FieldType::Number => v.is_number(),
            })
            .unwrap_or(false);
        if !present {
            return Err(ConnectionServiceError::BadInput(format!(
                "required field `{}` is missing or empty",
                field.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_secrets_vault::sealer::{MockSealer, Sealer};
    use ordo_secrets_vault::VaultStore;
    use serde_json::json;

    async fn build_service() -> ConnectionService {
        let vault_store = VaultStore::in_memory().unwrap();
        let sealers: Vec<Box<dyn Sealer>> = vec![Box::new(MockSealer)];
        let vault = Arc::new(
            VaultService::builder(vault_store, "local")
                .with_sealers(sealers)
                .build()
                .await
                .unwrap(),
        );
        let store = ConnectionStore::in_memory().unwrap();
        ConnectionService::new(store, vault)
    }

    #[tokio::test]
    async fn create_with_unknown_type_rejected() {
        let svc = build_service().await;
        let err = svc
            .create("nope", "x", json!({}), Some("y".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, ConnectionServiceError::UnknownType(_)));
    }

    #[tokio::test]
    async fn create_secret_required_when_type_requires_one() {
        let svc = build_service().await;
        let err = svc
            .create(
                "ssh",
                "x",
                json!({"host": "example.com", "username": "deploy"}),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ConnectionServiceError::BadInput(_)));
    }

    #[tokio::test]
    async fn create_validates_required_fields() {
        let svc = build_service().await;
        // ssh requires host + username
        let err = svc
            .create(
                "ssh",
                "x",
                json!({"host": "example.com"}),
                Some("password".into()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ConnectionServiceError::BadInput(_)));
    }

    #[tokio::test]
    async fn list_returns_multiple_of_same_type() {
        let svc = build_service().await;
        for i in 0..3 {
            let _ = svc
                .create(
                    "generic_api_key",
                    &format!("Key {i}"),
                    json!({}),
                    Some(format!("secret-{i}")),
                )
                .await
                .unwrap();
        }
        let list = svc.list().await.unwrap();
        assert_eq!(list.len(), 3);
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let svc = build_service().await;
        let row = svc
            .create("generic_api_key", "K", json!({}), Some("v".into()))
            .await
            .unwrap();
        svc.delete(&row.id).await.unwrap();
        let err = svc.get(&row.id).await.unwrap_err();
        assert!(matches!(err, ConnectionServiceError::NotFound(_)));
    }
}
