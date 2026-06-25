use crate::BuildLedger;
use ordo_store::{StorageTask, StorageTaskError};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum BuildLedgerStoreError {
    #[error("build ledger io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("build ledger json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("build ledger {0} was not found")]
    NotFound(Uuid),
}

pub struct BuildLedgerStore {
    root: PathBuf,
}

impl BuildLedgerStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, BuildLedgerStoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save(&self, ledger: &BuildLedger) -> Result<(), BuildLedgerStoreError> {
        let path = self.path_for(ledger.build_id);
        let bytes = serde_json::to_vec_pretty(ledger)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(&self, build_id: Uuid) -> Result<BuildLedger, BuildLedgerStoreError> {
        let path = self.path_for(build_id);
        if !path.exists() {
            return Err(BuildLedgerStoreError::NotFound(build_id));
        }
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn list(&self) -> Result<Vec<BuildLedger>, BuildLedgerStoreError> {
        let mut ledgers: Vec<BuildLedger> = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(path)?;
            ledgers.push(serde_json::from_slice(&bytes)?);
        }
        ledgers.sort_by_key(|l| std::cmp::Reverse(l.updated_at));
        Ok(ledgers)
    }

    fn path_for(&self, build_id: Uuid) -> PathBuf {
        self.root.join(format!("{build_id}.json"))
    }
}

#[derive(Clone)]
pub struct BuildLedgerTask {
    inner: StorageTask<BuildLedgerStore>,
}

impl BuildLedgerTask {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, BuildLedgerStoreError> {
        Ok(Self::from_store(BuildLedgerStore::open(root)?))
    }

    pub fn from_store(store: BuildLedgerStore) -> Self {
        Self {
            inner: StorageTask::start("build-ledger-store", store),
        }
    }

    pub async fn save(&self, ledger: BuildLedger) -> Result<(), StorageTaskError> {
        self.inner
            .call(move |store| store.save(&ledger).map_err(|err| err.to_string()))
            .await
    }

    pub async fn load(&self, build_id: Uuid) -> Result<BuildLedger, StorageTaskError> {
        self.inner
            .call(move |store| store.load(build_id).map_err(|err| err.to_string()))
            .await
    }

    pub async fn list(&self) -> Result<Vec<BuildLedger>, StorageTaskError> {
        self.inner
            .call(move |store| store.list().map_err(|err| err.to_string()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildLedgerStore, BuildLedgerTask};
    use crate::{BuildLedger, BuildRunStatus};

    fn unique_root(name: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("ordo-build-ledger-{name}-{stamp}"))
    }

    #[test]
    fn store_saves_loads_and_lists_ledgers() {
        let root = unique_root("sync");
        let store = BuildLedgerStore::open(&root).expect("store");
        let mut ledger = BuildLedger::new("demo");
        ledger.status = BuildRunStatus::Halted;
        let build_id = ledger.build_id;

        store.save(&ledger).expect("save ledger");
        let loaded = store.load(build_id).expect("load ledger");
        let listed = store.list().expect("list ledgers");

        assert_eq!(loaded.project_id, "demo");
        assert_eq!(loaded.status, BuildRunStatus::Halted);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].build_id, build_id);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn task_serializes_store_access() {
        let root = unique_root("task");
        let task = BuildLedgerTask::open(&root).expect("task");
        let ledger = BuildLedger::new("demo");
        let build_id = ledger.build_id;

        task.save(ledger).await.expect("save ledger");
        let loaded = task.load(build_id).await.expect("load ledger");
        let listed = task.list().await.expect("list ledgers");

        assert_eq!(loaded.build_id, build_id);
        assert_eq!(listed.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
