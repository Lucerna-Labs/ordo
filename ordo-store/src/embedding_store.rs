//! `EmbeddingStore` trait â€” the pluggability seam for vector storage
//! (Phase 4.3).
//!
//! The current in-tree embedding storage (SQLite BLOB/JSON columns on
//! `rag_chunks`, `assistant_facts`, `assistant_knowledge`) is fine
//! for single-operator deploys up to maybe ~100k vectors. Past that,
//! cosine similarity over a full table scan gets slow, and operators
//! want real vector databases.
//!
//! This trait is the seam: callers request nearest-neighbor lookups
//! through it, and a builder wires in either the default
//! SQLite-backed adapter or a Qdrant / pgvector / Pinecone adapter
//! behind a feature flag.
//!
//! **What ships in Phase 4.3:** the trait + an `InMemoryEmbeddingStore`
//! test helper + adapter entry points. Existing consumers (FactStore,
//! KnowledgeStore, RagStore) are not yet migrated â€” each one has its
//! own in-table embedding pipeline that predates this trait. Incremental
//! adoption is tracked as follow-up work; the trait documents the
//! contract so future adapters plug in without another refactor.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A single embedding record. `id` is caller-assigned (usually the
/// UUID of whatever higher-level entity owns this vector). `payload`
/// carries arbitrary metadata the store echoes back on query â€” keep
/// it small; this is a vector index, not a general blob store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingRecord {
    pub id: String,
    pub vector: Vec<f32>,
    #[serde(default)]
    pub payload: HashMap<String, serde_json::Value>,
}

/// A single nearest-neighbor match with its similarity score.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingMatch {
    pub id: String,
    pub score: f32,
    pub payload: HashMap<String, serde_json::Value>,
}

/// Error shape. Transport / backend errors become `Backend(String)`;
/// capacity / argument errors get typed variants so callers can
/// distinguish.
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingStoreError {
    #[error("vector dimensions don't match the store: got {got}, expected {expected}")]
    DimensionMismatch { got: usize, expected: usize },
    #[error("id '{0}' not found")]
    NotFound(String),
    #[error("backend: {0}")]
    Backend(String),
}

pub type EmbeddingStoreResult<T> = Result<T, EmbeddingStoreError>;

/// Pluggable nearest-neighbor index. Impls MUST be Send + Sync + Clone
/// â€” callers share them via Arc.
#[async_trait]
pub trait EmbeddingStore: Send + Sync {
    /// Human-readable backend name for logs / metrics / the
    /// capability descriptor surfaced to operators.
    fn backend_name(&self) -> &'static str;

    /// Upsert a record. Implementations decide whether to dedupe by
    /// `id` in-place or append-then-prune.
    async fn upsert(&self, record: EmbeddingRecord) -> EmbeddingStoreResult<()>;

    /// Batch upsert â€” impls can override for bulk-ingest efficiency.
    async fn upsert_many(&self, records: Vec<EmbeddingRecord>) -> EmbeddingStoreResult<()> {
        for record in records {
            self.upsert(record).await?;
        }
        Ok(())
    }

    /// Nearest-neighbor search. Returns at most `top_k` matches in
    /// descending score order.
    async fn query(
        &self,
        vector: &[f32],
        top_k: usize,
    ) -> EmbeddingStoreResult<Vec<EmbeddingMatch>>;

    async fn delete(&self, id: &str) -> EmbeddingStoreResult<()>;

    /// Rough count for observability â€” impls that can't afford an
    /// exact count may return an estimate or 0.
    async fn approximate_len(&self) -> EmbeddingStoreResult<u64> {
        Ok(0)
    }
}

/// Default test double â€” keeps records in memory, does brute-force
/// cosine similarity. Used in unit tests; production wiring picks a
/// real adapter.
pub struct InMemoryEmbeddingStore {
    dim: usize,
    inner: parking_lot::RwLock<Vec<EmbeddingRecord>>,
}

impl InMemoryEmbeddingStore {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            inner: parking_lot::RwLock::new(Vec::new()),
        }
    }
}

/// SQLite-backed adapter (Follow-up 2). Namespaced so multiple
/// consumers can share the `vector_index` table without colliding
/// on ids. Uses brute-force cosine similarity â€” fine up to ~100k
/// vectors per namespace; past that, swap in a Qdrant/pgvector
/// adapter. The trait contract is identical.
pub struct SqliteEmbeddingStore {
    db: std::sync::Arc<parking_lot::Mutex<crate::OrdoDatabase>>,
    namespace: String,
    workspace_id: String,
    dim: usize,
}

impl SqliteEmbeddingStore {
    pub fn new(
        db: std::sync::Arc<parking_lot::Mutex<crate::OrdoDatabase>>,
        namespace: impl Into<String>,
        workspace_id: impl Into<String>,
        dim: usize,
    ) -> Self {
        Self {
            db,
            namespace: namespace.into(),
            workspace_id: workspace_id.into(),
            dim,
        }
    }
}

#[async_trait]
impl EmbeddingStore for SqliteEmbeddingStore {
    fn backend_name(&self) -> &'static str {
        "sqlite"
    }

    async fn upsert(&self, record: EmbeddingRecord) -> EmbeddingStoreResult<()> {
        if record.vector.len() != self.dim {
            return Err(EmbeddingStoreError::DimensionMismatch {
                got: record.vector.len(),
                expected: self.dim,
            });
        }
        let vector_json = serde_json::to_string(&record.vector)
            .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?;
        let payload_json = serde_json::to_string(&record.payload)
            .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?;
        let now = chrono::Utc::now().timestamp_millis();
        let db = self.db.clone();
        let namespace = self.namespace.clone();
        let workspace_id = self.workspace_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = db.lock();
            guard
                .conn_mut()
                .execute(
                    "INSERT INTO vector_index (namespace, id, workspace_id, vector_json, payload_json, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(namespace, id) DO UPDATE SET
                        vector_json = excluded.vector_json,
                        payload_json = excluded.payload_json,
                        updated_at_ms = excluded.updated_at_ms",
                    rusqlite::params![
                        namespace,
                        record.id,
                        workspace_id,
                        vector_json,
                        payload_json,
                        now,
                    ],
                )
                .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))
                .map(|_| ())
        })
        .await
        .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?
    }

    async fn query(
        &self,
        vector: &[f32],
        top_k: usize,
    ) -> EmbeddingStoreResult<Vec<EmbeddingMatch>> {
        if vector.len() != self.dim {
            return Err(EmbeddingStoreError::DimensionMismatch {
                got: vector.len(),
                expected: self.dim,
            });
        }
        let query_vec = vector.to_vec();
        let db = self.db.clone();
        let namespace = self.namespace.clone();
        let workspace_id = self.workspace_id.clone();
        let rows = tokio::task::spawn_blocking(move || -> EmbeddingStoreResult<Vec<(String, Vec<f32>, HashMap<String, serde_json::Value>)>> {
            let guard = db.lock();
            let mut stmt = guard
                .conn()
                .prepare(
                    "SELECT id, vector_json, payload_json FROM vector_index
                     WHERE namespace = ?1 AND workspace_id = ?2",
                )
                .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?;
            let row_iter = stmt
                .query_map(rusqlite::params![namespace, workspace_id], |row| {
                    let id: String = row.get(0)?;
                    let vector_json: String = row.get(1)?;
                    let payload_json: String = row.get(2)?;
                    Ok((id, vector_json, payload_json))
                })
                .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?;
            let mut out: Vec<(String, Vec<f32>, HashMap<String, serde_json::Value>)> = Vec::new();
            for row in row_iter {
                let (id, vec_json, payload_json) =
                    row.map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?;
                let v: Vec<f32> = serde_json::from_str(&vec_json).unwrap_or_default();
                let payload: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&payload_json).unwrap_or_default();
                out.push((id, v, payload));
            }
            Ok(out)
        })
        .await
        .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))??;

        let mut scored: Vec<EmbeddingMatch> = rows
            .into_iter()
            .map(|(id, v, payload)| EmbeddingMatch {
                id,
                score: cosine(&query_vec, &v),
                payload,
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        Ok(scored)
    }

    async fn delete(&self, id: &str) -> EmbeddingStoreResult<()> {
        let db = self.db.clone();
        let namespace = self.namespace.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let mut guard = db.lock();
            let removed = guard
                .conn_mut()
                .execute(
                    "DELETE FROM vector_index WHERE namespace = ?1 AND id = ?2",
                    rusqlite::params![namespace, id],
                )
                .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?;
            if removed == 0 {
                return Err(EmbeddingStoreError::NotFound(id));
            }
            Ok(())
        })
        .await
        .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?
    }

    async fn approximate_len(&self) -> EmbeddingStoreResult<u64> {
        let db = self.db.clone();
        let namespace = self.namespace.clone();
        tokio::task::spawn_blocking(move || {
            let guard = db.lock();
            let count: i64 = guard
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM vector_index WHERE namespace = ?1",
                    rusqlite::params![namespace],
                    |row| row.get(0),
                )
                .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?;
            Ok(count.max(0) as u64)
        })
        .await
        .map_err(|err| EmbeddingStoreError::Backend(err.to_string()))?
    }
}

#[async_trait]
impl EmbeddingStore for InMemoryEmbeddingStore {
    fn backend_name(&self) -> &'static str {
        "in-memory"
    }

    async fn upsert(&self, record: EmbeddingRecord) -> EmbeddingStoreResult<()> {
        if record.vector.len() != self.dim {
            return Err(EmbeddingStoreError::DimensionMismatch {
                got: record.vector.len(),
                expected: self.dim,
            });
        }
        let mut guard = self.inner.write();
        if let Some(existing) = guard.iter_mut().find(|r| r.id == record.id) {
            *existing = record;
        } else {
            guard.push(record);
        }
        Ok(())
    }

    async fn query(
        &self,
        vector: &[f32],
        top_k: usize,
    ) -> EmbeddingStoreResult<Vec<EmbeddingMatch>> {
        if vector.len() != self.dim {
            return Err(EmbeddingStoreError::DimensionMismatch {
                got: vector.len(),
                expected: self.dim,
            });
        }
        let guard = self.inner.read();
        let mut scored: Vec<EmbeddingMatch> = guard
            .iter()
            .map(|record| EmbeddingMatch {
                id: record.id.clone(),
                score: cosine(vector, &record.vector),
                payload: record.payload.clone(),
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        Ok(scored)
    }

    async fn delete(&self, id: &str) -> EmbeddingStoreResult<()> {
        let mut guard = self.inner.write();
        let before = guard.len();
        guard.retain(|r| r.id != id);
        if guard.len() == before {
            return Err(EmbeddingStoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    async fn approximate_len(&self) -> EmbeddingStoreResult<u64> {
        Ok(self.inner.read().len() as u64)
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, v: Vec<f32>) -> EmbeddingRecord {
        EmbeddingRecord {
            id: id.into(),
            vector: v,
            payload: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn in_memory_store_returns_nearest_first() {
        let store = InMemoryEmbeddingStore::new(3);
        store.upsert(rec("a", vec![1.0, 0.0, 0.0])).await.unwrap();
        store.upsert(rec("b", vec![0.0, 1.0, 0.0])).await.unwrap();
        store.upsert(rec("c", vec![0.9, 0.1, 0.0])).await.unwrap();

        let hits = store.query(&[1.0, 0.0, 0.0], 2).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "a");
        assert_eq!(hits[1].id, "c");
    }

    #[tokio::test]
    async fn dimension_mismatch_is_reported_distinctly() {
        let store = InMemoryEmbeddingStore::new(3);
        let err = store
            .upsert(rec("x", vec![1.0, 2.0]))
            .await
            .expect_err("dim mismatch");
        assert!(matches!(err, EmbeddingStoreError::DimensionMismatch { .. }));
    }

    #[tokio::test]
    async fn upsert_replaces_existing_id() {
        let store = InMemoryEmbeddingStore::new(2);
        store.upsert(rec("a", vec![1.0, 0.0])).await.unwrap();
        store.upsert(rec("a", vec![0.0, 1.0])).await.unwrap();
        assert_eq!(store.approximate_len().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn delete_reports_not_found_for_missing_id() {
        let store = InMemoryEmbeddingStore::new(2);
        let err = store.delete("missing").await.expect_err("not found");
        assert!(matches!(err, EmbeddingStoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn sqlite_store_round_trips_and_ranks() {
        let db = std::sync::Arc::new(parking_lot::Mutex::new(
            crate::OrdoDatabase::in_memory().expect("db"),
        ));
        let store = SqliteEmbeddingStore::new(db, "rag_test", "local", 3);
        store.upsert(rec("a", vec![1.0, 0.0, 0.0])).await.unwrap();
        store.upsert(rec("b", vec![0.0, 1.0, 0.0])).await.unwrap();
        store.upsert(rec("c", vec![0.9, 0.1, 0.0])).await.unwrap();
        let hits = store.query(&[1.0, 0.0, 0.0], 2).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "a");
        assert_eq!(hits[1].id, "c");
        assert_eq!(store.approximate_len().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn sqlite_store_namespaces_do_not_collide() {
        let db = std::sync::Arc::new(parking_lot::Mutex::new(
            crate::OrdoDatabase::in_memory().expect("db"),
        ));
        let facts = SqliteEmbeddingStore::new(db.clone(), "facts", "local", 2);
        let rag = SqliteEmbeddingStore::new(db, "rag", "local", 2);
        facts.upsert(rec("x", vec![1.0, 0.0])).await.unwrap();
        rag.upsert(rec("x", vec![0.0, 1.0])).await.unwrap();
        // Same id `x` in both namespaces, with different vectors.
        let facts_hit = facts.query(&[1.0, 0.0], 1).await.unwrap();
        let rag_hit = rag.query(&[0.0, 1.0], 1).await.unwrap();
        assert_eq!(facts_hit[0].id, "x");
        assert_eq!(rag_hit[0].id, "x");
        // And neither namespace sees the other's records.
        assert_eq!(facts.approximate_len().await.unwrap(), 1);
        assert_eq!(rag.approximate_len().await.unwrap(), 1);
    }
}
