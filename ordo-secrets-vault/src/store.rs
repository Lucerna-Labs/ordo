//! SQLite persistence for the vault.
//!
//! Two tables (defined in `ordo-store`):
//!
//!   - `vault_state` â€” per-workspace master-key record. One row per
//!     workspace. Holds the sealed DEK + generation counter +
//!     active sealer metadata.
//!   - `sealed_secrets` â€” the AEAD-wrapped material rows. Every
//!     secret carries the generation of the DEK that wrapped it so
//!     a half-finished rotation is visible.
//!
//! Invariant 23 is enforced here: `retire_row` overwrites the
//! ciphertext with zeros before flipping `retired_at`.

use chrono::{DateTime, Utc};
use ordo_protocol::{ProtectionLevel, SealingTier, SecretClass, SecretRecord};
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, thiserror::Error)]
pub enum VaultStoreError {
    #[error("storage: {0}")]
    Storage(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("serialization: {0}")]
    Serialization(String),
}

pub type VaultStoreResult<T> = Result<T, VaultStoreError>;

/// Vault master-key row.
#[derive(Debug, Clone)]
pub struct VaultStateRow {
    pub workspace_id: String,
    pub generation: u32,
    pub sealing_tier: SealingTier,
    pub sealer_label: String,
    pub sealed_dek: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
}

/// Sealed secret row. Material-bearing: `ciphertext` is AEAD
/// wrapped under the DEK at the `dek_generation` that rotated it.
#[derive(Debug, Clone)]
pub struct SealedSecretRow {
    pub record: SecretRecord,
    pub sealing_tier: SealingTier,
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 24],
    pub aad: Vec<u8>,
    pub dek_generation: u32,
}

pub struct VaultStore {
    db: OrdoDatabase,
}

impl VaultStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> VaultStoreResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> VaultStoreResult<Self> {
        let db =
            OrdoDatabase::in_memory().map_err(|err| VaultStoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    // -------------------------------------------------------------
    // Vault master-key row
    // -------------------------------------------------------------

    pub fn load_vault_state(&self, workspace_id: &str) -> VaultStoreResult<Option<VaultStateRow>> {
        self.db
            .conn()
            .query_row(
                "SELECT workspace_id, generation, sealing_tier, sealer_label,
                        sealed_dek, created_at, rotated_at
                 FROM vault_state WHERE workspace_id = ?1",
                params![workspace_id],
                row_to_vault_state,
            )
            .optional()
            .map_err(|err| VaultStoreError::Storage(err.to_string()))
    }

    pub fn insert_vault_state(&mut self, row: &VaultStateRow) -> VaultStoreResult<()> {
        self.db
            .conn()
            .execute(
                "INSERT INTO vault_state (
                    workspace_id, generation, sealing_tier, sealer_label,
                    sealed_dek, created_at, rotated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.workspace_id,
                    row.generation,
                    row.sealing_tier.label(),
                    row.sealer_label,
                    row.sealed_dek,
                    row.created_at.to_rfc3339(),
                    row.rotated_at.map(|t| t.to_rfc3339()),
                ],
            )
            .map_err(|err| {
                if err.to_string().contains("UNIQUE") {
                    VaultStoreError::Conflict(format!(
                        "vault_state already exists for workspace {}",
                        row.workspace_id
                    ))
                } else {
                    VaultStoreError::Storage(err.to_string())
                }
            })?;
        Ok(())
    }

    pub fn update_vault_state(&mut self, row: &VaultStateRow) -> VaultStoreResult<()> {
        let updated = self
            .db
            .conn()
            .execute(
                "UPDATE vault_state
                 SET generation = ?2,
                     sealing_tier = ?3,
                     sealer_label = ?4,
                     sealed_dek = ?5,
                     rotated_at = ?6
                 WHERE workspace_id = ?1",
                params![
                    row.workspace_id,
                    row.generation,
                    row.sealing_tier.label(),
                    row.sealer_label,
                    row.sealed_dek,
                    row.rotated_at.map(|t| t.to_rfc3339()),
                ],
            )
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(VaultStoreError::NotFound(format!(
                "vault_state row for workspace {}",
                row.workspace_id
            )));
        }
        Ok(())
    }

    // -------------------------------------------------------------
    // Sealed-secret rows
    // -------------------------------------------------------------

    pub fn insert_sealed(&mut self, row: &SealedSecretRow) -> VaultStoreResult<()> {
        let allowed = serde_json::to_string(&row.record.allowed_providers)
            .map_err(|err| VaultStoreError::Serialization(err.to_string()))?;
        let (kind, t, n) = protection_to_columns(&row.record.protection);
        self.db
            .conn()
            .execute(
                "INSERT INTO sealed_secrets (
                    id, workspace_id, class, protection_kind,
                    protection_threshold_t, protection_threshold_n,
                    label, allowed_providers_json, sealing_tier,
                    ciphertext, nonce, aad, dek_generation,
                    created_at, rotation_due_at, retired_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    row.record.id.to_string(),
                    row.record.workspace_id,
                    row.record.class.label(),
                    kind,
                    t,
                    n,
                    row.record.label,
                    allowed,
                    row.sealing_tier.label(),
                    row.ciphertext,
                    row.nonce.to_vec(),
                    row.aad,
                    row.dek_generation,
                    row.record.created_at.to_rfc3339(),
                    row.record.rotation_due_at.map(|t| t.to_rfc3339()),
                    row.record.retired_at.map(|t| t.to_rfc3339()),
                ],
            )
            .map_err(|err| {
                if err.to_string().contains("UNIQUE") {
                    VaultStoreError::Conflict(format!("secret id {} already exists", row.record.id))
                } else {
                    VaultStoreError::Storage(err.to_string())
                }
            })?;
        Ok(())
    }

    pub fn load_sealed(&self, id: &str) -> VaultStoreResult<Option<SealedSecretRow>> {
        self.db
            .conn()
            .query_row(
                "SELECT id, workspace_id, class, protection_kind,
                        protection_threshold_t, protection_threshold_n,
                        label, allowed_providers_json, sealing_tier,
                        ciphertext, nonce, aad, dek_generation,
                        created_at, rotation_due_at, retired_at
                 FROM sealed_secrets WHERE id = ?1",
                params![id],
                row_to_sealed_secret,
            )
            .optional()
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?
            .transpose()
    }

    pub fn list_active(&self, workspace_id: &str) -> VaultStoreResult<Vec<SealedSecretRow>> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, workspace_id, class, protection_kind,
                        protection_threshold_t, protection_threshold_n,
                        label, allowed_providers_json, sealing_tier,
                        ciphertext, nonce, aad, dek_generation,
                        created_at, rotation_due_at, retired_at
                 FROM sealed_secrets
                 WHERE workspace_id = ?1 AND retired_at IS NULL
                 ORDER BY created_at ASC",
            )
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![workspace_id], row_to_sealed_secret)
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            let row = row.map_err(|err| VaultStoreError::Storage(err.to_string()))?;
            out.push(row?);
        }
        Ok(out)
    }

    /// Rewrap an existing secret under a new DEK: replaces the
    /// ciphertext/nonce/aad/dek_generation in place. Used during
    /// rotation to re-seal every active secret without retiring it.
    pub fn rewrap_in_place(
        &mut self,
        id: &str,
        ciphertext: &[u8],
        nonce: &[u8; 24],
        aad: &[u8],
        dek_generation: u32,
    ) -> VaultStoreResult<()> {
        let updated = self
            .db
            .conn()
            .execute(
                "UPDATE sealed_secrets
                 SET ciphertext = ?2, nonce = ?3, aad = ?4, dek_generation = ?5
                 WHERE id = ?1 AND retired_at IS NULL",
                params![id, ciphertext, nonce.to_vec(), aad, dek_generation],
            )
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(VaultStoreError::NotFound(format!(
                "active secret {id} for rewrap"
            )));
        }
        Ok(())
    }

    /// Invariant 23 enforcement: overwrite ciphertext with zeros
    /// and mark the row retired. The row itself is kept for audit
    /// continuity â€” callers querying by id can see the secret used
    /// to exist but cannot recover the material.
    pub fn retire_row(&mut self, id: &str) -> VaultStoreResult<()> {
        let now = Utc::now().to_rfc3339();
        let tx = self
            .db
            .conn_mut()
            .transaction()
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;

        // Determine ciphertext length first so the replacement is
        // the same shape (easier for forensic diffing).
        let existing_len: i64 = tx
            .query_row(
                "SELECT length(ciphertext) FROM sealed_secrets WHERE id = ?1 AND retired_at IS NULL",
                params![id],
                |row| row.get(0),
            )
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;

        let zeros = vec![0u8; existing_len as usize];
        let zero_nonce = vec![0u8; 24];
        let updated = tx
            .execute(
                "UPDATE sealed_secrets
                 SET ciphertext = ?2, nonce = ?3, aad = x'', retired_at = ?4
                 WHERE id = ?1 AND retired_at IS NULL",
                params![id, zeros, zero_nonce, now],
            )
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;
        if updated == 0 {
            return Err(VaultStoreError::NotFound(format!(
                "active secret {id} for retirement"
            )));
        }
        tx.commit()
            .map_err(|err| VaultStoreError::Storage(err.to_string()))?;
        Ok(())
    }
}

fn protection_to_columns(p: &ProtectionLevel) -> (&'static str, Option<u32>, Option<u32>) {
    match p {
        ProtectionLevel::SingleSealed => ("single_sealed", None, None),
        ProtectionLevel::Threshold { t, n } => ("threshold", Some(*t), Some(*n)),
    }
}

fn columns_to_protection(
    kind: &str,
    t: Option<u32>,
    n: Option<u32>,
) -> Result<ProtectionLevel, VaultStoreError> {
    match kind {
        "single_sealed" => Ok(ProtectionLevel::SingleSealed),
        "threshold" => {
            let (t, n) = t.zip(n).ok_or_else(|| {
                VaultStoreError::Serialization("threshold row missing t/n columns".into())
            })?;
            Ok(ProtectionLevel::Threshold { t, n })
        }
        other => Err(VaultStoreError::Serialization(format!(
            "unknown protection kind {other}"
        ))),
    }
}

fn sealing_tier_from_label(label: &str) -> Result<SealingTier, VaultStoreError> {
    Ok(match label {
        "tier1_hardware" => SealingTier::Tier1Hardware,
        "tier2_secure_element" => SealingTier::Tier2SecureElement,
        "tier3_os_keychain" => SealingTier::Tier3OsKeychain,
        "tier4_software_fallback" => SealingTier::Tier4SoftwareFallback,
        "mock_for_tests" => SealingTier::MockForTests,
        other => {
            return Err(VaultStoreError::Serialization(format!(
                "unknown sealing tier {other}"
            )))
        }
    })
}

fn row_to_vault_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<VaultStateRow> {
    let workspace_id: String = row.get(0)?;
    let generation: u32 = row.get(1)?;
    let tier_label: String = row.get(2)?;
    let sealer_label: String = row.get(3)?;
    let sealed_dek: Vec<u8> = row.get(4)?;
    let created_at: String = row.get(5)?;
    let rotated_at: Option<String> = row.get(6)?;
    let tier = sealing_tier_from_label(&tier_label).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let created_at = DateTime::parse_from_rfc3339(&created_at)
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(err))
        })?
        .with_timezone(&Utc);
    let rotated_at = rotated_at
        .map(|s| DateTime::parse_from_rfc3339(&s).map(|t| t.with_timezone(&Utc)))
        .transpose()
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(err))
        })?;
    Ok(VaultStateRow {
        workspace_id,
        generation,
        sealing_tier: tier,
        sealer_label,
        sealed_dek,
        created_at,
        rotated_at,
    })
}

fn row_to_sealed_secret(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<SealedSecretRow, VaultStoreError>> {
    // Column order: id, workspace_id, class, protection_kind, t, n,
    // label, allowed_providers_json, sealing_tier, ciphertext,
    // nonce, aad, dek_generation, created_at, rotation_due_at, retired_at
    let id_str: String = row.get(0)?;
    let workspace_id: String = row.get(1)?;
    let class_label: String = row.get(2)?;
    let protection_kind: String = row.get(3)?;
    let t: Option<u32> = row.get(4)?;
    let n: Option<u32> = row.get(5)?;
    let label: String = row.get(6)?;
    let allowed_json: String = row.get(7)?;
    let tier_label: String = row.get(8)?;
    let ciphertext: Vec<u8> = row.get(9)?;
    let nonce_vec: Vec<u8> = row.get(10)?;
    let aad: Vec<u8> = row.get(11)?;
    let dek_generation: u32 = row.get(12)?;
    let created_at: String = row.get(13)?;
    let rotation_due_at: Option<String> = row.get(14)?;
    let retired_at: Option<String> = row.get(15)?;

    Ok((|| -> Result<SealedSecretRow, VaultStoreError> {
        let id = id_str;
        let class = SecretClass::from_label(&class_label).ok_or_else(|| {
            VaultStoreError::Serialization(format!("unknown secret class {class_label}"))
        })?;
        let protection = columns_to_protection(&protection_kind, t, n)?;
        let allowed_providers: Vec<String> = serde_json::from_str(&allowed_json)
            .map_err(|err| VaultStoreError::Serialization(err.to_string()))?;
        let sealing_tier = sealing_tier_from_label(&tier_label)?;
        if nonce_vec.len() != 24 {
            return Err(VaultStoreError::Serialization(format!(
                "nonce must be 24 bytes, got {}",
                nonce_vec.len()
            )));
        }
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&nonce_vec);
        let created_at = DateTime::parse_from_rfc3339(&created_at)
            .map_err(|err| VaultStoreError::Serialization(err.to_string()))?
            .with_timezone(&Utc);
        let rotation_due_at = rotation_due_at
            .map(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|t| t.with_timezone(&Utc))
                    .map_err(|err| VaultStoreError::Serialization(err.to_string()))
            })
            .transpose()?;
        let retired_at = retired_at
            .map(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|t| t.with_timezone(&Utc))
                    .map_err(|err| VaultStoreError::Serialization(err.to_string()))
            })
            .transpose()?;
        Ok(SealedSecretRow {
            record: SecretRecord {
                id,
                workspace_id,
                class,
                protection,
                label,
                allowed_providers,
                created_at,
                rotation_due_at,
                retired_at,
            },
            sealing_tier,
            ciphertext,
            nonce,
            aad,
            dek_generation,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row(workspace: &str, label: &str) -> SealedSecretRow {
        SealedSecretRow {
            record: SecretRecord {
                id: ulid::Ulid::new().to_string(),
                workspace_id: workspace.to_string(),
                class: SecretClass::ApiKey,
                protection: ProtectionLevel::SingleSealed,
                label: label.to_string(),
                allowed_providers: vec!["cloud.anthropic".to_string()],
                created_at: Utc::now(),
                rotation_due_at: None,
                retired_at: None,
            },
            sealing_tier: SealingTier::MockForTests,
            ciphertext: vec![1u8; 48],
            nonce: [2u8; 24],
            aad: b"secret_id=x".to_vec(),
            dek_generation: 0,
        }
    }

    #[test]
    fn round_trip_sealed_secret() {
        let mut store = VaultStore::in_memory().unwrap();
        let row = sample_row("local", "openai-prod");
        store.insert_sealed(&row).unwrap();
        let loaded = store.load_sealed(&row.record.id).unwrap().unwrap();
        assert_eq!(loaded.record.label, "openai-prod");
        assert_eq!(loaded.ciphertext, row.ciphertext);
        assert_eq!(loaded.nonce, row.nonce);
        assert_eq!(loaded.dek_generation, 0);
    }

    #[test]
    fn retire_zeros_ciphertext_and_preserves_row() {
        let mut store = VaultStore::in_memory().unwrap();
        let row = sample_row("local", "openai-prod");
        let id = row.record.id.to_string();
        store.insert_sealed(&row).unwrap();
        store.retire_row(&id).unwrap();
        let loaded = store.load_sealed(&id).unwrap().unwrap();
        assert!(loaded.record.retired_at.is_some(), "retired_at must be set");
        assert!(
            loaded.ciphertext.iter().all(|b| *b == 0),
            "invariant 23: retired ciphertext must be zeroed"
        );
        assert_eq!(loaded.nonce, [0u8; 24]);
        assert!(loaded.aad.is_empty());
    }

    #[test]
    fn list_active_excludes_retired() {
        let mut store = VaultStore::in_memory().unwrap();
        let alive = sample_row("local", "alive");
        let dead = sample_row("local", "dead");
        store.insert_sealed(&alive).unwrap();
        store.insert_sealed(&dead).unwrap();
        store.retire_row(&dead.record.id.to_string()).unwrap();
        let active = store.list_active("local").unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record.label, "alive");
    }

    #[test]
    fn rewrap_in_place_updates_ciphertext_and_generation() {
        let mut store = VaultStore::in_memory().unwrap();
        let row = sample_row("local", "openai-prod");
        let id = row.record.id.to_string();
        store.insert_sealed(&row).unwrap();
        store
            .rewrap_in_place(&id, &[9u8; 80], &[3u8; 24], b"new-aad", 1)
            .unwrap();
        let loaded = store.load_sealed(&id).unwrap().unwrap();
        assert_eq!(loaded.ciphertext, vec![9u8; 80]);
        assert_eq!(loaded.nonce, [3u8; 24]);
        assert_eq!(loaded.aad, b"new-aad".to_vec());
        assert_eq!(loaded.dek_generation, 1);
    }

    #[test]
    fn vault_state_round_trip_and_update() {
        let mut store = VaultStore::in_memory().unwrap();
        assert!(store.load_vault_state("local").unwrap().is_none());
        let row = VaultStateRow {
            workspace_id: "local".to_string(),
            generation: 0,
            sealing_tier: SealingTier::Tier4SoftwareFallback,
            sealer_label: "argon2id-default".to_string(),
            sealed_dek: vec![5u8; 90],
            created_at: Utc::now(),
            rotated_at: None,
        };
        store.insert_vault_state(&row).unwrap();
        let loaded = store.load_vault_state("local").unwrap().unwrap();
        assert_eq!(loaded.generation, 0);
        assert_eq!(loaded.sealed_dek, vec![5u8; 90]);
        let mut updated = loaded.clone();
        updated.generation = 1;
        updated.sealed_dek = vec![6u8; 90];
        updated.rotated_at = Some(Utc::now());
        store.update_vault_state(&updated).unwrap();
        let reloaded = store.load_vault_state("local").unwrap().unwrap();
        assert_eq!(reloaded.generation, 1);
        assert!(reloaded.rotated_at.is_some());
    }
}
