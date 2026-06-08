use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use futures::StreamExt;
use ordo_bus::Bus;
use ordo_protocol::{topics, Envelope, MemoryTier, NodeId, OrdoMessage};
use ordo_store::{OrdoDatabase, StorageTask, StorageTaskError};
use rusqlite::params;
use serde::{Deserialize, Serialize};

type DynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryRecord {
    stored_at: DateTime<Utc>,
    content: String,
}

pub struct MemoryStore {
    db: OrdoDatabase,
    budgets: MemoryBudgets,
}

#[derive(Clone)]
pub struct MemoryStorageTask {
    path: Option<PathBuf>,
    inner: StorageTask<MemoryStore>,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryBudgets {
    pub working_bytes: usize,
    pub pinned_bytes: usize,
}

impl Default for MemoryBudgets {
    fn default() -> Self {
        Self {
            working_bytes: 10 * 1024 * 1024 * 1024,
            pinned_bytes: 50 * 1024 * 1024 * 1024,
        }
    }
}

impl MemoryStore {
    pub fn in_memory() -> Self {
        Self::in_memory_with_budgets(MemoryBudgets::default())
    }

    pub fn in_memory_with_budgets(budgets: MemoryBudgets) -> Self {
        Self {
            db: OrdoDatabase::in_memory().expect("open in-memory sqlite database"),
            budgets,
        }
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        Self::open_with_budgets(path, MemoryBudgets::default())
    }

    pub fn open_with_budgets(
        path: impl Into<PathBuf>,
        budgets: MemoryBudgets,
    ) -> Result<Self, DynError> {
        Ok(Self {
            db: OrdoDatabase::open(path)?,
            budgets,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.db.path()
    }

    pub fn len(&self) -> usize {
        self.count().expect("count memory records")
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn archive(&mut self, content: String) -> Result<(), DynError> {
        self.archive_with_tier(content, MemoryTier::Working)
    }

    pub fn archive_pinned(&mut self, content: String) -> Result<(), DynError> {
        self.archive_with_tier(content, MemoryTier::Pinned)
    }

    pub fn archive_pinned_if_missing(&mut self, content: String) -> Result<bool, DynError> {
        self.archive_if_missing_with_tier(content, MemoryTier::Pinned)
    }

    pub fn archive_with_tier(&mut self, content: String, tier: MemoryTier) -> Result<(), DynError> {
        let stored_at = Utc::now().to_rfc3339();
        let size_bytes = content.len() as i64;
        self.db.conn_mut().execute(
            "INSERT INTO memory_records (stored_at, content, tier, size_bytes) VALUES (?1, ?2, ?3, ?4)",
            params![stored_at, content, tier_to_str(tier), size_bytes],
        )?;
        self.prune_to_budget(tier)?;
        Ok(())
    }

    pub fn remove_with_tier(&mut self, content: &str, tier: MemoryTier) -> Result<bool, DynError> {
        let deleted = self.db.conn_mut().execute(
            "
            DELETE FROM memory_records
            WHERE id IN (
                SELECT id
                FROM memory_records
                WHERE tier = ?1 AND content = ?2
                ORDER BY stored_at DESC, id DESC
                LIMIT 1
            )
            ",
            params![tier_to_str(tier), content],
        )?;
        Ok(deleted > 0)
    }

    fn archive_if_missing_with_tier(
        &mut self,
        content: String,
        tier: MemoryTier,
    ) -> Result<bool, DynError> {
        if self.contains_content(&content, tier)? {
            return Ok(false);
        }

        self.archive_with_tier(content, tier)?;
        Ok(true)
    }

    pub fn search(&self, query: &str) -> Vec<String> {
        self.search_result(query).expect("query memory records")
    }

    pub fn list_recent(&self, tier: MemoryTier, limit: usize) -> Vec<String> {
        self.list_recent_result(tier, limit)
            .expect("list recent memory records")
    }

    pub fn import_legacy_jsonl(&mut self, path: &Path) -> Result<usize, DynError> {
        if !path.exists() {
            return Ok(0);
        }

        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let tx = self.db.conn_mut().transaction()?;
        let mut imported = 0usize;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let record: MemoryRecord = serde_json::from_str(&line)?;
            let content = record.content;
            let size_bytes = content.len() as i64;
            tx.execute(
                "INSERT INTO memory_records (stored_at, content, tier, size_bytes) VALUES (?1, ?2, ?3, ?4)",
                params![
                    record.stored_at.to_rfc3339(),
                    content,
                    tier_to_str(MemoryTier::Working),
                    size_bytes,
                ],
            )?;
            imported += 1;
        }

        tx.commit()?;
        Ok(imported)
    }

    fn count(&self) -> Result<usize, DynError> {
        let count = self
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM memory_records", [], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(count as usize)
    }

    fn search_result(&self, query: &str) -> Result<Vec<String>, DynError> {
        let needle = format!("%{}%", query.to_ascii_lowercase());
        let mut stmt = self.db.conn().prepare(
            "
            SELECT content, tier, MIN(id) AS first_seen
            FROM memory_records
            WHERE lower(content) LIKE ?1
            GROUP BY content
            ORDER BY CASE tier WHEN 'pinned' THEN 0 ELSE 1 END ASC, first_seen ASC
            LIMIT 5
            ",
        )?;
        let rows = stmt.query_map(params![needle], |row| row.get::<_, String>(0))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    fn contains_content(&self, content: &str, tier: MemoryTier) -> Result<bool, DynError> {
        let existing = self.db.conn().query_row(
            "
            SELECT 1
            FROM memory_records
            WHERE content = ?1 AND tier = ?2
            LIMIT 1
            ",
            params![content, tier_to_str(tier)],
            |_row| Ok(()),
        );

        match existing {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(err) => Err(Box::new(err)),
        }
    }

    fn list_recent_result(&self, tier: MemoryTier, limit: usize) -> Result<Vec<String>, DynError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut stmt = self.db.conn().prepare(
            "
            SELECT content
            FROM memory_records
            WHERE tier = ?1
            ORDER BY stored_at DESC, id DESC
            LIMIT ?2
            ",
        )?;
        let rows = stmt.query_map(params![tier_to_str(tier), limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    fn prune_to_budget(&mut self, tier: MemoryTier) -> Result<(), DynError> {
        let budget = match tier {
            MemoryTier::Working => self.budgets.working_bytes,
            MemoryTier::Pinned => self.budgets.pinned_bytes,
        } as i64;

        loop {
            let current_bytes: i64 = self.db.conn().query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM memory_records WHERE tier = ?1",
                params![tier_to_str(tier)],
                |row| row.get(0),
            )?;

            if current_bytes <= budget {
                break;
            }

            let deleted = self.db.conn_mut().execute(
                "
                DELETE FROM memory_records
                WHERE id IN (
                    SELECT id FROM memory_records
                    WHERE tier = ?1
                    ORDER BY stored_at ASC, id ASC
                    LIMIT 1
                )
                ",
                params![tier_to_str(tier)],
            )?;

            if deleted == 0 {
                break;
            }
        }

        Ok(())
    }
}

impl MemoryStorageTask {
    pub fn from_store(store: MemoryStore) -> Self {
        Self {
            path: store.path().map(PathBuf::from),
            inner: StorageTask::start("memory-store", store),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub async fn len(&self) -> Result<usize, StorageTaskError> {
        self.inner.call(|store| Ok(store.len())).await
    }

    pub async fn is_empty(&self) -> Result<bool, StorageTaskError> {
        self.inner.call(|store| Ok(store.len() == 0)).await
    }

    pub async fn archive_with_tier(
        &self,
        content: String,
        tier: MemoryTier,
    ) -> Result<(), StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .archive_with_tier(content, tier)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })
            .await
    }

    pub async fn archive_pinned_if_missing(
        &self,
        content: String,
    ) -> Result<bool, StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .archive_pinned_if_missing(content)
                    .map_err(|err| err.to_string())
            })
            .await
    }

    pub async fn remove_with_tier(
        &self,
        content: String,
        tier: MemoryTier,
    ) -> Result<bool, StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .remove_with_tier(&content, tier)
                    .map_err(|err| err.to_string())
            })
            .await
    }

    pub async fn list_recent(
        &self,
        tier: MemoryTier,
        limit: usize,
    ) -> Result<Vec<String>, StorageTaskError> {
        self.inner
            .call(move |store| Ok(store.list_recent(tier, limit)))
            .await
    }

    pub async fn search(&self, query: String) -> Result<Vec<String>, StorageTaskError> {
        self.inner.call(move |store| Ok(store.search(&query))).await
    }
}

pub struct MemoryPeer {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
    store: MemoryStorageTask,
}

impl MemoryPeer {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self::with_store(bus, MemoryStore::in_memory())
    }

    pub fn with_store(bus: Arc<dyn Bus>, store: MemoryStore) -> Self {
        Self::with_storage(bus, MemoryStorageTask::from_store(store))
    }

    pub fn with_storage(bus: Arc<dyn Bus>, store: MemoryStorageTask) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
            store,
        }
    }

    pub async fn run(&mut self) -> Result<(), DynError> {
        let mut sub = self.bus.subscribe(topics::ALL).await?;
        let node_id = self.node_id.clone();
        let bus = self.bus.clone();
        let stored_count = self.store.len().await.map_err(storage_error)?;

        match self.store.path() {
            Some(path) => println!(
                "[Memory] Peer online with {} persisted record(s) at {}",
                stored_count,
                path.display()
            ),
            None => println!(
                "[Memory] Peer online with {} in-memory record(s)",
                stored_count
            ),
        }

        while let Some(envelope) = sub.next().await {
            let correlation_id = envelope.correlation_id.clone();
            match envelope.payload {
                OrdoMessage::RequirementMessage { requirement } => {
                    self.archive(requirement).await?;
                }
                OrdoMessage::CapabilityMessage {
                    capability,
                    description,
                } => {
                    self.archive(format!("{capability}: {description}")).await?;
                }
                OrdoMessage::MemoryStored { content, tier } => {
                    self.archive_with_tier(content, tier).await?;
                }
                OrdoMessage::MemoryStoreRequested { content, tier } => {
                    let stored = match tier {
                        MemoryTier::Pinned => {
                            println!("[Memory] Pinning memory: {}", content);
                            self.store
                                .archive_pinned_if_missing(content.clone())
                                .await
                                .map_err(storage_error)?
                        }
                        MemoryTier::Working => {
                            self.archive_with_tier(content.clone(), tier).await?;
                            true
                        }
                    };
                    let res = Envelope::new(
                        node_id.clone(),
                        OrdoMessage::MemoryStoreCompleted {
                            content,
                            tier,
                            stored,
                        },
                    );
                    let res = match correlation_id {
                        Some(cid) => res.with_correlation(cid),
                        None => res,
                    };
                    let _ = bus.publish(topics::MEMORY_STORE_RESPONSE, res).await;
                }
                OrdoMessage::MemoryRemoveRequested { content, tier } => {
                    let removed = self
                        .store
                        .remove_with_tier(content.clone(), tier)
                        .await
                        .map_err(storage_error)?;
                    let res = Envelope::new(
                        node_id.clone(),
                        OrdoMessage::MemoryRemoveCompleted {
                            content,
                            tier,
                            removed,
                        },
                    );
                    let res = match correlation_id {
                        Some(cid) => res.with_correlation(cid),
                        None => res,
                    };
                    let _ = bus.publish(topics::MEMORY_REMOVE_RESPONSE, res).await;
                }
                OrdoMessage::MemoryRemoveCompleted { .. } => {}
                OrdoMessage::MemoryListRequested { tier, limit } => {
                    let results = self
                        .store
                        .list_recent(tier, limit)
                        .await
                        .map_err(storage_error)?;
                    let res =
                        Envelope::new(node_id.clone(), OrdoMessage::MemoryListed { tier, results });
                    let res = match correlation_id {
                        Some(cid) => res.with_correlation(cid),
                        None => res,
                    };
                    let _ = bus.publish(topics::MEMORY_LIST_RESPONSE, res).await;
                }
                OrdoMessage::RunRequested { goal, plan, .. } => {
                    let plan_suffix = plan
                        .as_ref()
                        .map(|plan| {
                            let capabilities = plan
                                .steps
                                .iter()
                                .map(|step| step.capability.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!(
                                " planned_steps={} capabilities=[{}]",
                                plan.steps.len(),
                                capabilities
                            )
                        })
                        .unwrap_or_default();
                    self.archive(format!("run requested: {goal}{plan_suffix}"))
                        .await?;
                }
                OrdoMessage::StepCompleted { output, .. } => {
                    self.archive(format!("run completed: {output}")).await?;
                }
                OrdoMessage::StepFailed { error, .. } => {
                    self.archive(format!("run failed: {error}")).await?;
                }
                OrdoMessage::RunFinished {
                    run_id,
                    status,
                    completed_steps,
                } => {
                    self.archive(format!(
                        "run finished: {:?} {:?} after {} step(s)",
                        run_id, status, completed_steps
                    ))
                    .await?;
                }
                OrdoMessage::RagDocumentIndexed {
                    document_id,
                    chunk_count,
                } => {
                    self.archive(format!("rag indexed: {document_id} ({chunk_count} chunks)"))
                        .await?;
                }
                OrdoMessage::RagQueryCompleted { query, hits } => {
                    self.archive(format!("rag query: {query} -> {} hit(s)", hits.len()))
                        .await?;
                }
                OrdoMessage::ToolCallRequested { capability, .. } => {
                    self.archive(format!("tool requested: {capability}"))
                        .await?;
                }
                OrdoMessage::ToolCallCompleted {
                    capability, result, ..
                } => {
                    self.archive(format!("tool completed: {capability} -> {result}"))
                        .await?;
                }
                OrdoMessage::ToolCallFailed {
                    capability, error, ..
                } => {
                    self.archive(format!("tool failed: {capability} -> {error}"))
                        .await?;
                }
                OrdoMessage::SelfHealRequested { incident } => {
                    self.archive(format!(
                        "self-heal requested: {} [{}] {}",
                        incident.component, incident.fingerprint, incident.symptom
                    ))
                    .await?;
                }
                OrdoMessage::SelfHealPlanned {
                    fingerprint, plan, ..
                } => {
                    self.archive(format!(
                        "self-heal planned: {} -> {} via {:?}",
                        fingerprint, plan.summary, plan.source
                    ))
                    .await?;
                }
                OrdoMessage::MemoryQuery { query } => {
                    println!("[Memory] Received query: '{}'", query);
                    let results = self
                        .store
                        .search(query.clone())
                        .await
                        .map_err(storage_error)?;
                    let res = Envelope::new(
                        node_id.clone(),
                        OrdoMessage::MemoryQueried { query, results },
                    );
                    let res = match correlation_id {
                        Some(cid) => res.with_correlation(cid),
                        None => res,
                    };
                    let _ = bus.publish(topics::MEMORY_RESPONSE, res).await;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn archive(&mut self, content: String) -> Result<(), DynError> {
        println!("[Memory] Archiving: {}", content);
        self.store
            .archive_with_tier(content, MemoryTier::Working)
            .await
            .map_err(storage_error)
    }

    async fn archive_with_tier(
        &mut self,
        content: String,
        tier: MemoryTier,
    ) -> Result<(), DynError> {
        println!("[Memory] Archiving ({:?}): {}", tier, content);
        self.store
            .archive_with_tier(content, tier)
            .await
            .map_err(storage_error)
    }
}

fn storage_error(error: StorageTaskError) -> DynError {
    Box::new(std::io::Error::other(error.to_string()))
}

fn tier_to_str(tier: MemoryTier) -> &'static str {
    match tier {
        MemoryTier::Working => "working",
        MemoryTier::Pinned => "pinned",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{MemoryBudgets, MemoryStore};

    fn temp_memory_path() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("codex-ordo-memory-{stamp}.db"))
    }

    fn remove_sqlite_artifacts(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));
    }

    #[test]
    fn persisted_store_round_trips_records() {
        let path = temp_memory_path();

        {
            let mut store = MemoryStore::open(&path).expect("create store");
            store
                .archive("I need to read file config.json".to_string())
                .expect("archive config");
            store
                .archive("filesystem.read_file: Reads files from the local disk.".to_string())
                .expect("archive capability");
        }

        let reopened = MemoryStore::open(&path).expect("reopen store");
        assert_eq!(reopened.len(), 2);
        assert_eq!(
            reopened.search("config"),
            vec!["I need to read file config.json".to_string()]
        );

        drop(reopened);
        remove_sqlite_artifacts(&path);
    }

    #[test]
    fn working_budget_prunes_before_pinned_budget() {
        let mut store = MemoryStore::in_memory_with_budgets(MemoryBudgets {
            working_bytes: 10,
            pinned_bytes: 64,
        });

        store.archive("123456".to_string()).expect("first working");
        store.archive("abcdef".to_string()).expect("second working");
        store
            .archive_pinned("important anchor".to_string())
            .expect("pinned");

        let working_hits = store.search("123456");
        assert!(working_hits.is_empty());
        let second_hit = store.search("abcdef");
        assert_eq!(second_hit, vec!["abcdef".to_string()]);
        let pinned_hit = store.search("important");
        assert_eq!(pinned_hit, vec!["important anchor".to_string()]);
    }

    #[test]
    fn pinned_bootstrap_is_idempotent() {
        let mut store = MemoryStore::in_memory();

        let first = store
            .archive_pinned_if_missing("official architecture memory".to_string())
            .expect("first archive");
        let second = store
            .archive_pinned_if_missing("official architecture memory".to_string())
            .expect("second archive");

        assert!(first);
        assert!(!second);
        assert_eq!(
            store.search("official architecture"),
            vec!["official architecture memory".to_string()]
        );
    }

    #[test]
    fn list_recent_returns_latest_entries_for_tier() {
        let mut store = MemoryStore::in_memory();
        store
            .archive_pinned("first pinned memory".to_string())
            .expect("first pinned");
        store
            .archive_pinned("second pinned memory".to_string())
            .expect("second pinned");

        let recent = store.list_recent(ordo_protocol::MemoryTier::Pinned, 2);
        assert_eq!(
            recent,
            vec![
                "second pinned memory".to_string(),
                "first pinned memory".to_string()
            ]
        );
    }

    #[test]
    fn remove_with_tier_deletes_matching_entry() {
        let mut store = MemoryStore::in_memory();
        store
            .archive_pinned("first pinned memory".to_string())
            .expect("first pinned");
        store
            .archive_pinned("second pinned memory".to_string())
            .expect("second pinned");

        let removed = store
            .remove_with_tier("second pinned memory", ordo_protocol::MemoryTier::Pinned)
            .expect("remove matching entry");
        assert!(removed);
        assert_eq!(
            store.list_recent(ordo_protocol::MemoryTier::Pinned, 5),
            vec!["first pinned memory".to_string()]
        );
    }
}
