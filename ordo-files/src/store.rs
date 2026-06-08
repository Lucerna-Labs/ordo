//! SQLite persistence for file metadata. Bytes live on disk â€”
//! `FilesService` owns the filesystem plumbing; this store only
//! tracks the metadata row.

use chrono::{DateTime, Utc};
use ordo_protocol::FileEntry;
use ordo_store::OrdoDatabase;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::types::{FilesError, FilesResult};

pub struct FilesStore {
    db: OrdoDatabase,
}

impl FilesStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> FilesResult<Self> {
        let db = OrdoDatabase::open(path.as_ref())
            .map_err(|err| FilesError::Storage(err.to_string()))?;
        Ok(Self { db })
    }

    pub fn in_memory() -> FilesResult<Self> {
        let db = OrdoDatabase::in_memory().map_err(|err| FilesError::Storage(err.to_string()))?;
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
        original_name: &str,
        storage_path: &str,
        content_type: &str,
        size_bytes: u64,
        sha256_hex: &str,
        created_at: DateTime<Utc>,
        created_by: &str,
        app_id: Option<Uuid>,
    ) -> FilesResult<FileEntry> {
        let conn = self.db.conn_mut();
        conn.execute(
            "INSERT INTO files (
                id, workspace_id, original_name, storage_path, content_type,
                size_bytes, sha256_hex, created_at, created_by, app_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id.to_string(),
                workspace_id,
                original_name,
                storage_path,
                content_type,
                size_bytes as i64,
                sha256_hex,
                created_at.to_rfc3339(),
                created_by,
                app_id.map(|id| id.to_string()),
            ],
        )
        .map_err(|err| FilesError::Storage(err.to_string()))?;
        Ok(FileEntry {
            id,
            workspace_id: workspace_id.to_string(),
            original_name: original_name.to_string(),
            storage_path: storage_path.to_string(),
            content_type: content_type.to_string(),
            size_bytes,
            sha256_hex: sha256_hex.to_string(),
            created_at,
            created_by: created_by.to_string(),
            app_id,
        })
    }

    pub fn get(&self, id: Uuid) -> FilesResult<Option<FileEntry>> {
        let conn = self.db.conn();
        conn.query_row(
            "SELECT id, workspace_id, original_name, storage_path, content_type,
                    size_bytes, sha256_hex, created_at, created_by, app_id
             FROM files WHERE id = ?1",
            params![id.to_string()],
            row_to_entry,
        )
        .optional()
        .map_err(|err| FilesError::Storage(err.to_string()))
    }

    pub fn list(
        &self,
        workspace_id: Option<&str>,
        app_id: Option<Uuid>,
        limit: Option<u32>,
    ) -> FilesResult<Vec<FileEntry>> {
        let mut sql = String::from(
            "SELECT id, workspace_id, original_name, storage_path, content_type,
                    size_bytes, sha256_hex, created_at, created_by, app_id
             FROM files WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(ws) = workspace_id {
            sql.push_str(" AND workspace_id = ?");
            args.push(Box::new(ws.to_string()));
        }
        if let Some(id) = app_id {
            sql.push_str(" AND app_id = ?");
            args.push(Box::new(id.to_string()));
        }
        sql.push_str(" ORDER BY created_at DESC");
        if let Some(lim) = limit {
            sql.push_str(" LIMIT ?");
            args.push(Box::new(lim as i64));
        }
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| FilesError::Storage(err.to_string()))?;
        let arg_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let rows = stmt
            .query_map(arg_refs.as_slice(), row_to_entry)
            .map_err(|err| FilesError::Storage(err.to_string()))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|err| FilesError::Storage(err.to_string()))?);
        }
        Ok(entries)
    }

    pub fn delete(&mut self, id: Uuid) -> FilesResult<Option<FileEntry>> {
        let existing = self.get(id)?;
        if existing.is_none() {
            return Ok(None);
        }
        let conn = self.db.conn_mut();
        conn.execute("DELETE FROM files WHERE id = ?1", params![id.to_string()])
            .map_err(|err| FilesError::Storage(err.to_string()))?;
        Ok(existing)
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileEntry> {
    let id_str: String = row.get(0)?;
    let created_at_str: String = row.get(7)?;
    let app_id_str: Option<String> = row.get(9)?;
    let size_bytes: i64 = row.get(5)?;

    let id = Uuid::parse_str(&id_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(err))
        })?;
    let app_id = app_id_str
        .map(|s| Uuid::parse_str(&s))
        .transpose()
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(err))
        })?;
    Ok(FileEntry {
        id,
        workspace_id: row.get(1)?,
        original_name: row.get(2)?,
        storage_path: row.get(3)?,
        content_type: row.get(4)?,
        size_bytes: size_bytes.max(0) as u64,
        sha256_hex: row.get(6)?,
        created_at,
        created_by: row.get(8)?,
        app_id,
    })
}
