//! SQLite persistence for the audit chain + anchors.
//!
//! Two tables (defined in `ordo-store`):
//!   - `secrets_audit_chain` â€” the append-only chain rows.
//!   - `secrets_audit_anchors` â€” signed anchors over chain slices.

use chrono::{DateTime, Utc};
use ordo_protocol::{AuditEntry, SealingTier, SecretAuditEventType, TransparencyReceipt};
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum AuditStoreError {
    #[error("storage: {0}")]
    Storage(String),
    #[error("serialization: {0}")]
    Serialization(String),
}

pub type AuditStoreResult<T> = Result<T, AuditStoreError>;

pub struct AuditStore {
    db: OrdoDatabase,
}

impl AuditStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> AuditStoreResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> AuditStoreResult<Self> {
        let db =
            OrdoDatabase::in_memory().map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    /// Read-and-increment: look up the current tip and return the
    /// sequence+prev_hash the NEXT entry should use.
    pub fn next_sequence_and_prev_hash(
        &self,
        workspace_id: &str,
    ) -> AuditStoreResult<(u64, [u8; 32])> {
        let row: Option<(i64, Vec<u8>)> = self
            .db
            .conn()
            .query_row(
                "SELECT sequence, entry_hash FROM secrets_audit_chain
                 WHERE workspace_id = ?1
                 ORDER BY sequence DESC LIMIT 1",
                params![workspace_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        match row {
            None => Ok((1, [0u8; 32])),
            Some((seq, hash_vec)) => {
                if hash_vec.len() != 32 {
                    return Err(AuditStoreError::Storage(format!(
                        "entry_hash at seq {seq} is {}B, expected 32",
                        hash_vec.len()
                    )));
                }
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&hash_vec);
                Ok((seq as u64 + 1, hash))
            }
        }
    }

    pub fn insert_entry(
        &mut self,
        workspace_id: &str,
        entry: &AuditEntry,
        entry_hash: &[u8; 32],
    ) -> AuditStoreResult<()> {
        let payload_json = serde_json::to_string(&entry.payload)
            .map_err(|err| AuditStoreError::Serialization(err.to_string()))?;
        self.db
            .conn()
            .execute(
                "INSERT INTO secrets_audit_chain (
                    id, workspace_id, sequence, prev_hash, entry_hash,
                    timestamp, event_type, payload_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.id,
                    workspace_id,
                    entry.sequence as i64,
                    entry.prev_hash.to_vec(),
                    entry_hash.to_vec(),
                    entry.timestamp.to_rfc3339(),
                    event_type_label(entry.event_type),
                    payload_json,
                ],
            )
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        Ok(())
    }

    pub fn list_all(&self, workspace_id: &str) -> AuditStoreResult<Vec<(AuditEntry, [u8; 32])>> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, workspace_id, sequence, prev_hash, entry_hash,
                        timestamp, event_type, payload_json
                 FROM secrets_audit_chain
                 WHERE workspace_id = ?1
                 ORDER BY sequence ASC",
            )
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![workspace_id], row_to_entry_tuple)
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| AuditStoreError::Storage(err.to_string()))??);
        }
        Ok(out)
    }

    pub fn list_range(
        &self,
        workspace_id: &str,
        first: u64,
        last: u64,
    ) -> AuditStoreResult<Vec<(AuditEntry, [u8; 32])>> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, workspace_id, sequence, prev_hash, entry_hash,
                        timestamp, event_type, payload_json
                 FROM secrets_audit_chain
                 WHERE workspace_id = ?1
                   AND sequence >= ?2 AND sequence <= ?3
                 ORDER BY sequence ASC",
            )
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(
                params![workspace_id, first as i64, last as i64],
                row_to_entry_tuple,
            )
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| AuditStoreError::Storage(err.to_string()))??);
        }
        Ok(out)
    }

    pub fn insert_anchor(
        &mut self,
        workspace_id: &str,
        receipt: &TransparencyReceipt,
    ) -> AuditStoreResult<()> {
        let id = ulid::Ulid::new().to_string();
        self.db
            .conn()
            .execute(
                "INSERT INTO secrets_audit_anchors (
                    id, workspace_id, first_sequence, last_sequence,
                    chain_root, signed_at, cose_sign1, signer_tier,
                    service_id, service_attestation
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id,
                    workspace_id,
                    receipt.anchor.first_sequence as i64,
                    receipt.anchor.last_sequence as i64,
                    receipt.anchor.chain_root.to_vec(),
                    receipt.anchor.signed_at.to_rfc3339(),
                    receipt.anchor.cose_sign1.clone(),
                    tier_label(receipt.anchor.signer_tier),
                    receipt.service_id.clone(),
                    receipt.service_attestation.clone(),
                ],
            )
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        Ok(())
    }

    /// Backdoor for chain-tamper tests. Not exposed in the public
    /// `AuditService` API.
    #[doc(hidden)]
    pub fn test_tamper_payload(
        &mut self,
        workspace_id: &str,
        sequence: u64,
        payload: Value,
    ) -> AuditStoreResult<()> {
        let payload_json = serde_json::to_string(&payload)
            .map_err(|err| AuditStoreError::Serialization(err.to_string()))?;
        self.db
            .conn()
            .execute(
                "UPDATE secrets_audit_chain
                 SET payload_json = ?3
                 WHERE workspace_id = ?1 AND sequence = ?2",
                params![workspace_id, sequence as i64, payload_json],
            )
            .map_err(|err| AuditStoreError::Storage(err.to_string()))?;
        Ok(())
    }
}

fn event_type_label(t: SecretAuditEventType) -> &'static str {
    match t {
        SecretAuditEventType::SecretCreated => "secret_created",
        SecretAuditEventType::SecretRetired => "secret_retired",
        SecretAuditEventType::SecretRotated => "secret_rotated",
        SecretAuditEventType::HandleIssued => "handle_issued",
        SecretAuditEventType::HandleDereferenced => "handle_dereferenced",
        SecretAuditEventType::HandleExpired => "handle_expired",
        SecretAuditEventType::HandleRevoked => "handle_revoked",
        SecretAuditEventType::ThresholdSigningBegan => "threshold_signing_began",
        SecretAuditEventType::ThresholdSigningCompleted => "threshold_signing_completed",
        SecretAuditEventType::ThresholdShareRedistributed => "threshold_share_redistributed",
        SecretAuditEventType::CanaryDetected => "canary_detected",
        SecretAuditEventType::CustodyMismatchDetected => "custody_mismatch_detected",
        SecretAuditEventType::StructuralLimitExceeded => "structural_limit_exceeded",
        SecretAuditEventType::AnchorSigned => "anchor_signed",
        SecretAuditEventType::RotationDue => "rotation_due",
        SecretAuditEventType::SealTierDegraded => "seal_tier_degraded",
    }
}

fn event_type_from_label(label: &str) -> Result<SecretAuditEventType, AuditStoreError> {
    Ok(match label {
        "secret_created" => SecretAuditEventType::SecretCreated,
        "secret_retired" => SecretAuditEventType::SecretRetired,
        "secret_rotated" => SecretAuditEventType::SecretRotated,
        "handle_issued" => SecretAuditEventType::HandleIssued,
        "handle_dereferenced" => SecretAuditEventType::HandleDereferenced,
        "handle_expired" => SecretAuditEventType::HandleExpired,
        "handle_revoked" => SecretAuditEventType::HandleRevoked,
        "threshold_signing_began" => SecretAuditEventType::ThresholdSigningBegan,
        "threshold_signing_completed" => SecretAuditEventType::ThresholdSigningCompleted,
        "threshold_share_redistributed" => SecretAuditEventType::ThresholdShareRedistributed,
        "canary_detected" => SecretAuditEventType::CanaryDetected,
        "custody_mismatch_detected" => SecretAuditEventType::CustodyMismatchDetected,
        "structural_limit_exceeded" => SecretAuditEventType::StructuralLimitExceeded,
        "anchor_signed" => SecretAuditEventType::AnchorSigned,
        "rotation_due" => SecretAuditEventType::RotationDue,
        "seal_tier_degraded" => SecretAuditEventType::SealTierDegraded,
        other => {
            return Err(AuditStoreError::Serialization(format!(
                "unknown event type {other}"
            )))
        }
    })
}

fn tier_label(t: SealingTier) -> &'static str {
    t.label()
}

fn row_to_entry_tuple(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<(AuditEntry, [u8; 32]), AuditStoreError>> {
    let id: String = row.get(0)?;
    let _workspace: String = row.get(1)?;
    let sequence_i64: i64 = row.get(2)?;
    let sequence = sequence_i64 as u64;
    let prev_hash_vec: Vec<u8> = row.get(3)?;
    let entry_hash_vec: Vec<u8> = row.get(4)?;
    let timestamp: String = row.get(5)?;
    let event_type_label: String = row.get(6)?;
    let payload_json: String = row.get(7)?;
    Ok((|| -> Result<(AuditEntry, [u8; 32]), AuditStoreError> {
        if prev_hash_vec.len() != 32 {
            return Err(AuditStoreError::Serialization(format!(
                "prev_hash at seq {sequence} is {}B",
                prev_hash_vec.len()
            )));
        }
        if entry_hash_vec.len() != 32 {
            return Err(AuditStoreError::Serialization(format!(
                "entry_hash at seq {sequence} is {}B",
                entry_hash_vec.len()
            )));
        }
        let mut prev_hash = [0u8; 32];
        prev_hash.copy_from_slice(&prev_hash_vec);
        let mut entry_hash = [0u8; 32];
        entry_hash.copy_from_slice(&entry_hash_vec);
        let timestamp = DateTime::parse_from_rfc3339(&timestamp)
            .map_err(|err| AuditStoreError::Serialization(err.to_string()))?
            .with_timezone(&Utc);
        let event_type = event_type_from_label(&event_type_label)?;
        let payload: Value = serde_json::from_str(&payload_json)
            .map_err(|err| AuditStoreError::Serialization(err.to_string()))?;
        Ok((
            AuditEntry {
                id,
                sequence,
                prev_hash,
                timestamp,
                event_type,
                payload,
            },
            entry_hash,
        ))
    })())
}
