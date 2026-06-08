//! Tree node persistence. Tombstone-based soft-delete so replays
//! can load tree-as-of-then.

use ordo_protocol::{RetrievalSemantics, TreeNode};
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, thiserror::Error)]
pub enum TreeStoreError {
    #[error("storage: {0}")]
    Storage(String),
    #[error("node not found: {0}")]
    NotFound(String),
}

pub type TreeStoreResult<T> = Result<T, TreeStoreError>;

pub struct TreeStore {
    db: OrdoDatabase,
}

impl TreeStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> TreeStoreResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| TreeStoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> TreeStoreResult<Self> {
        let db =
            OrdoDatabase::in_memory().map_err(|err| TreeStoreError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn from_database(db: OrdoDatabase) -> Self {
        Self { db }
    }

    /// Insert or replace a tree node. Always updates `updated_at_ms`
    /// (caller provides current wall clock).
    pub fn upsert(
        &mut self,
        workspace_id: &str,
        node: &TreeNode,
    ) -> TreeStoreResult<Option<TreeNode>> {
        let before = self.get(workspace_id, &node.path)?;
        let conn = self.db.conn_mut();
        let hint = node.retrieval_hint.map(|h| {
            match h {
                RetrievalSemantics::Lexical => "lexical",
                RetrievalSemantics::Dense => "dense",
                RetrievalSemantics::Hybrid => "hybrid",
                RetrievalSemantics::Exact => "exact",
            }
            .to_string()
        });
        conn.execute(
            "INSERT INTO memory_tree_nodes (
                path, workspace_id, parent_path, description, retrieval_hint,
                created_at_ms, updated_at_ms, tombstoned
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
            ON CONFLICT(path) DO UPDATE SET
                parent_path = excluded.parent_path,
                description = excluded.description,
                retrieval_hint = excluded.retrieval_hint,
                updated_at_ms = excluded.updated_at_ms,
                tombstoned = 0",
            params![
                node.path,
                workspace_id,
                node.parent_path,
                node.description,
                hint,
                node.created_at_ms,
                node.updated_at_ms,
            ],
        )
        .map_err(|err| TreeStoreError::Storage(err.to_string()))?;
        Ok(before)
    }

    pub fn tombstone(
        &mut self,
        workspace_id: &str,
        path: &str,
        now_ms: i64,
    ) -> TreeStoreResult<Option<TreeNode>> {
        let before = self.get(workspace_id, path)?;
        if before.is_none() {
            return Err(TreeStoreError::NotFound(path.to_string()));
        }
        let conn = self.db.conn_mut();
        conn.execute(
            "UPDATE memory_tree_nodes SET tombstoned = 1, updated_at_ms = ?1
             WHERE path = ?2 AND workspace_id = ?3",
            params![now_ms, path, workspace_id],
        )
        .map_err(|err| TreeStoreError::Storage(err.to_string()))?;
        Ok(before)
    }

    pub fn get(&self, workspace_id: &str, path: &str) -> TreeStoreResult<Option<TreeNode>> {
        let conn = self.db.conn();
        conn.query_row(
            "SELECT path, parent_path, description, retrieval_hint,
                    created_at_ms, updated_at_ms, tombstoned
             FROM memory_tree_nodes WHERE path = ?1 AND workspace_id = ?2",
            params![path, workspace_id],
            row_to_node,
        )
        .optional()
        .map_err(|err| TreeStoreError::Storage(err.to_string()))
    }

    /// Live tree (tombstoned excluded).
    pub fn list_live(&self, workspace_id: &str) -> TreeStoreResult<Vec<TreeNode>> {
        self.list(workspace_id, false)
    }

    pub fn list_all(&self, workspace_id: &str) -> TreeStoreResult<Vec<TreeNode>> {
        self.list(workspace_id, true)
    }

    fn list(&self, workspace_id: &str, include_tombstoned: bool) -> TreeStoreResult<Vec<TreeNode>> {
        let sql = if include_tombstoned {
            "SELECT path, parent_path, description, retrieval_hint,
                    created_at_ms, updated_at_ms, tombstoned
             FROM memory_tree_nodes WHERE workspace_id = ?1
             ORDER BY path ASC"
        } else {
            "SELECT path, parent_path, description, retrieval_hint,
                    created_at_ms, updated_at_ms, tombstoned
             FROM memory_tree_nodes WHERE workspace_id = ?1 AND tombstoned = 0
             ORDER BY path ASC"
        };
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(sql)
            .map_err(|err| TreeStoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![workspace_id], row_to_node)
            .map_err(|err| TreeStoreError::Storage(err.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| TreeStoreError::Storage(err.to_string()))?);
        }
        Ok(out)
    }

    /// Tree state as of `timestamp_ms`: include nodes whose
    /// `created_at_ms <= timestamp_ms` and whose latest live-state
    /// at that timestamp was "not tombstoned." Approximation: uses
    /// `updated_at_ms` as a proxy for when tombstoning happened.
    ///
    /// For replay-exact fidelity the caller should reconstruct from
    /// `system.tree_change` events in the log. This helper is the
    /// fast path that works when the log agrees.
    pub fn list_at_timestamp(
        &self,
        workspace_id: &str,
        timestamp_ms: i64,
    ) -> TreeStoreResult<Vec<TreeNode>> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT path, parent_path, description, retrieval_hint,
                        created_at_ms, updated_at_ms, tombstoned
                 FROM memory_tree_nodes
                 WHERE workspace_id = ?1 AND created_at_ms <= ?2
                   AND NOT (tombstoned = 1 AND updated_at_ms <= ?2)
                 ORDER BY path ASC",
            )
            .map_err(|err| TreeStoreError::Storage(err.to_string()))?;
        let rows = stmt
            .query_map(params![workspace_id, timestamp_ms], row_to_node)
            .map_err(|err| TreeStoreError::Storage(err.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| TreeStoreError::Storage(err.to_string()))?);
        }
        Ok(out)
    }
}

fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<TreeNode> {
    let retrieval_hint_str: Option<String> = row.get(3)?;
    let tombstoned_int: i64 = row.get(6)?;
    let retrieval_hint = retrieval_hint_str.as_deref().and_then(|s| match s {
        "lexical" => Some(RetrievalSemantics::Lexical),
        "dense" => Some(RetrievalSemantics::Dense),
        "hybrid" => Some(RetrievalSemantics::Hybrid),
        "exact" => Some(RetrievalSemantics::Exact),
        _ => None,
    });
    Ok(TreeNode {
        path: row.get(0)?,
        parent_path: row.get(1)?,
        description: row.get(2)?,
        retrieval_hint,
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
        tombstoned: tombstoned_int != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &str, desc: &str, ts: i64) -> TreeNode {
        TreeNode {
            path: path.into(),
            parent_path: None,
            description: desc.into(),
            retrieval_hint: Some(RetrievalSemantics::Hybrid),
            created_at_ms: ts,
            updated_at_ms: ts,
            tombstoned: false,
        }
    }

    #[test]
    fn upsert_and_get() {
        let mut store = TreeStore::in_memory().unwrap();
        let n = node("lucerna/voice", "lucerna brand voice", 1000);
        store.upsert("local", &n).unwrap();
        let fetched = store.get("local", "lucerna/voice").unwrap().unwrap();
        assert_eq!(fetched.description, "lucerna brand voice");
    }

    #[test]
    fn tombstone_hides_from_live_list() {
        let mut store = TreeStore::in_memory().unwrap();
        store.upsert("local", &node("a", "alpha", 100)).unwrap();
        store.upsert("local", &node("b", "beta", 100)).unwrap();
        store.tombstone("local", "a", 200).unwrap();
        let live = store.list_live("local").unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].path, "b");
        // But list_all still shows tombstoned.
        let all = store.list_all("local").unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn list_at_timestamp_reconstructs_past_tree() {
        let mut store = TreeStore::in_memory().unwrap();
        store.upsert("local", &node("old", "", 100)).unwrap();
        store.upsert("local", &node("new", "", 500)).unwrap();
        store.tombstone("local", "old", 600).unwrap();
        // At ts=300 only `old` existed.
        let at_300 = store.list_at_timestamp("local", 300).unwrap();
        assert_eq!(at_300.len(), 1);
        assert_eq!(at_300[0].path, "old");
        // At ts=550 both existed, old not yet tombstoned.
        let at_550 = store.list_at_timestamp("local", 550).unwrap();
        assert_eq!(at_550.len(), 2);
        // At ts=700 only `new` is live.
        let at_700 = store.list_at_timestamp("local", 700).unwrap();
        assert_eq!(at_700.len(), 1);
        assert_eq!(at_700[0].path, "new");
    }
}
