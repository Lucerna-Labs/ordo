//! Share registry â€” metadata-only persistence for threshold
//! shares.
//!
//! Key distinction from the vault: this table holds NO secret
//! material. It records *who holds share index `i` for secret `S`*
//! â€” a device fingerprint, a share index, and the public group
//! metadata. The actual `KeyPackage` bytes live on the holder's
//! device (sealed there by the vault).
//!
//! Use cases driven by the registry:
//!   - The broker needs to know which holders can contribute to a
//!     signing round for secret `S`.
//!   - The rotation scheduler needs to detect when a holder's
//!     share has not signed in a long time (potential device
//!     loss â†’ trigger redistribution).
//!   - A new holder device adopting a share publishes a
//!     `ThresholdShareAnnouncement`; the registry records it.

use chrono::{DateTime, Utc};
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, thiserror::Error)]
pub enum ShareRegistryError {
    #[error("storage: {0}")]
    Storage(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
}

pub type ShareRegistryResult<T> = Result<T, ShareRegistryError>;

#[derive(Debug, Clone)]
pub struct ShareMetadata {
    pub share_id: String,
    pub secret_id: String,
    pub workspace_id: String,
    pub share_index: u32,
    pub total_shares: u32,
    pub holder_fingerprint: [u8; 32],
    pub holder_label: String,
    pub registered_at: DateTime<Utc>,
    pub last_signed_at: Option<DateTime<Utc>>,
    pub retired_at: Option<DateTime<Utc>>,
}

pub struct ShareRegistry {
    db: OrdoDatabase,
}

impl ShareRegistry {
    pub fn open(path: impl AsRef<std::path::Path>) -> ShareRegistryResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| ShareRegistryError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> ShareRegistryResult<Self> {
        let db = OrdoDatabase::in_memory()
            .map_err(|err| ShareRegistryError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    pub fn announce(&mut self, meta: &ShareMetadata) -> ShareRegistryResult<()> {
        self.db
            .conn()
            .execute(
                "INSERT INTO threshold_shares (
                    share_id, secret_id, workspace_id, share_index, total_shares,
                    holder_fingerprint, holder_label, registered_at, last_signed_at, retired_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    meta.share_id,
                    meta.secret_id,
                    meta.workspace_id,
                    meta.share_index,
                    meta.total_shares,
                    meta.holder_fingerprint.to_vec(),
                    meta.holder_label,
                    meta.registered_at.to_rfc3339(),
                    meta.last_signed_at.map(|t| t.to_rfc3339()),
                    meta.retired_at.map(|t| t.to_rfc3339()),
                ],
            )
            .map_err(|err| {
                if err.to_string().contains("UNIQUE") {
                    ShareRegistryError::Conflict(format!(
                        "share {} already announced",
                        meta.share_id
                    ))
                } else {
                    ShareRegistryError::Storage(err.to_string())
                }
            })?;
        Ok(())
    }

    pub fn list_for_secret(&self, secret_id: &str) -> ShareRegistryResult<Vec<ShareMetadata>> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT share_id, secret_id, workspace_id, share_index, total_shares,
                        holder_fingerprint, holder_label, registered_at, last_signed_at, retired_at
                 FROM threshold_shares
                 WHERE secret_id = ?1 AND retired_at IS NULL
                 ORDER BY share_index ASC",
            )
            .map_err(|err| ShareRegistryError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![secret_id], row_to_meta)
            .map_err(|err| ShareRegistryError::Storage(err.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| ShareRegistryError::Storage(err.to_string()))??);
        }
        Ok(out)
    }

    pub fn mark_signed(
        &mut self,
        share_id: &str,
        signed_at: DateTime<Utc>,
    ) -> ShareRegistryResult<()> {
        let updated = self
            .db
            .conn()
            .execute(
                "UPDATE threshold_shares SET last_signed_at = ?2
                 WHERE share_id = ?1 AND retired_at IS NULL",
                params![share_id, signed_at.to_rfc3339()],
            )
            .map_err(|err| ShareRegistryError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(ShareRegistryError::NotFound(share_id.to_string()));
        }
        Ok(())
    }

    pub fn retire(&mut self, share_id: &str) -> ShareRegistryResult<()> {
        let updated = self
            .db
            .conn()
            .execute(
                "UPDATE threshold_shares SET retired_at = ?2
                 WHERE share_id = ?1 AND retired_at IS NULL",
                params![share_id, Utc::now().to_rfc3339()],
            )
            .map_err(|err| ShareRegistryError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(ShareRegistryError::NotFound(share_id.to_string()));
        }
        Ok(())
    }

    pub fn load(&self, share_id: &str) -> ShareRegistryResult<Option<ShareMetadata>> {
        self.db
            .conn()
            .query_row(
                "SELECT share_id, secret_id, workspace_id, share_index, total_shares,
                        holder_fingerprint, holder_label, registered_at, last_signed_at, retired_at
                 FROM threshold_shares WHERE share_id = ?1",
                params![share_id],
                row_to_meta,
            )
            .optional()
            .map_err(|err| ShareRegistryError::Storage(err.to_string()))?
            .transpose()
    }
}

fn row_to_meta(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<ShareMetadata, ShareRegistryError>> {
    let share_id: String = row.get(0)?;
    let secret_id: String = row.get(1)?;
    let workspace_id: String = row.get(2)?;
    let share_index: u32 = row.get(3)?;
    let total_shares: u32 = row.get(4)?;
    let fp_vec: Vec<u8> = row.get(5)?;
    let holder_label: String = row.get(6)?;
    let registered_at: String = row.get(7)?;
    let last_signed_at: Option<String> = row.get(8)?;
    let retired_at: Option<String> = row.get(9)?;
    Ok((|| -> Result<ShareMetadata, ShareRegistryError> {
        if fp_vec.len() != 32 {
            return Err(ShareRegistryError::Storage(format!(
                "fingerprint must be 32 bytes, got {}",
                fp_vec.len()
            )));
        }
        let mut fp = [0u8; 32];
        fp.copy_from_slice(&fp_vec);
        let registered_at = DateTime::parse_from_rfc3339(&registered_at)
            .map_err(|err| ShareRegistryError::Storage(err.to_string()))?
            .with_timezone(&Utc);
        let last_signed_at = last_signed_at
            .map(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|t| t.with_timezone(&Utc))
                    .map_err(|err| ShareRegistryError::Storage(err.to_string()))
            })
            .transpose()?;
        let retired_at = retired_at
            .map(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|t| t.with_timezone(&Utc))
                    .map_err(|err| ShareRegistryError::Storage(err.to_string()))
            })
            .transpose()?;
        Ok(ShareMetadata {
            share_id,
            secret_id,
            workspace_id,
            share_index,
            total_shares,
            holder_fingerprint: fp,
            holder_label,
            registered_at,
            last_signed_at,
            retired_at,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn sample(idx: u32) -> ShareMetadata {
        ShareMetadata {
            share_id: format!("share-{idx}"),
            secret_id: "secret-1".to_string(),
            workspace_id: "local".to_string(),
            share_index: idx,
            total_shares: 3,
            holder_fingerprint: [idx as u8; 32],
            holder_label: format!("device-{idx}"),
            registered_at: Utc::now(),
            last_signed_at: None,
            retired_at: None,
        }
    }

    /// Registry tests need a sealed_secrets row first because of
    /// the FK on `threshold_shares.secret_id`.
    fn fixture_registry_with_parent_secret() -> ShareRegistry {
        let db = OrdoDatabase::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO sealed_secrets (
                    id, workspace_id, class, protection_kind,
                    protection_threshold_t, protection_threshold_n,
                    label, allowed_providers_json, sealing_tier,
                    ciphertext, nonce, aad, dek_generation, created_at
                 ) VALUES (?1, 'local', 'signing_key', 'threshold', 2, 3,
                           'test-secret', '[]', 'mock_for_tests',
                           x'00', x'000000000000000000000000000000000000000000000000',
                           x'', 0, ?2)",
                params!["secret-1", Utc::now().to_rfc3339()],
            )
            .unwrap();
        ShareRegistry::from_database(db)
    }

    #[test]
    fn announce_and_list() {
        let mut reg = fixture_registry_with_parent_secret();
        reg.announce(&sample(1)).unwrap();
        reg.announce(&sample(2)).unwrap();
        reg.announce(&sample(3)).unwrap();
        let list = reg.list_for_secret("secret-1").unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].share_index, 1);
    }

    #[test]
    fn retire_excludes_from_list() {
        let mut reg = fixture_registry_with_parent_secret();
        reg.announce(&sample(1)).unwrap();
        reg.announce(&sample(2)).unwrap();
        reg.retire("share-2").unwrap();
        let list = reg.list_for_secret("secret-1").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].share_id, "share-1");
    }

    #[test]
    fn mark_signed_updates_timestamp() {
        let mut reg = fixture_registry_with_parent_secret();
        reg.announce(&sample(1)).unwrap();
        let when = Utc::now();
        reg.mark_signed("share-1", when).unwrap();
        let loaded = reg.load("share-1").unwrap().unwrap();
        assert!(loaded.last_signed_at.is_some());
    }

    #[test]
    fn announce_conflict_on_duplicate_share_id() {
        let mut reg = fixture_registry_with_parent_secret();
        reg.announce(&sample(1)).unwrap();
        let err = reg.announce(&sample(1)).unwrap_err();
        assert!(matches!(err, ShareRegistryError::Conflict(_)));
    }
}
