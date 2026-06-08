//! `FilesService` â€” handles bytes-on-disk + metadata-in-SQLite.
//!
//! Storage layout under `user_files/`:
//!
//! ```text
//! user_files/
//!   <workspace_id>/
//!     <file_id>/
//!       <sanitized_original_name>
//! ```
//!
//! The inner file uuid directory keeps collisions impossible even
//! when two uploads share a name. Sanitization strips path separators
//! and control chars from the stored filename.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use ordo_bus::Bus;
use ordo_protocol::{topics, Envelope, FileEntry, NodeId, OrdoMessage};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::store::FilesStore;
use crate::types::{FilesError, FilesQuery, FilesResult, NewUpload};

pub const DEFAULT_WORKSPACE_ID: &str = "local";

#[derive(Clone)]
pub struct FilesService {
    store: Arc<Mutex<FilesStore>>,
    user_files_root: Arc<PathBuf>,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
}

impl FilesService {
    /// `user_files_root` is the directory where bytes live. The
    /// service writes only inside that root; attempts to escape it
    /// are refused.
    pub fn new(store: FilesStore, user_files_root: impl Into<PathBuf>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            user_files_root: Arc::new(user_files_root.into()),
            bus: None,
            node_id: NodeId::new(),
        }
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn user_files_root(&self) -> &Path {
        self.user_files_root.as_path()
    }

    /// Persist `bytes` + metadata. Returns the folded `FileEntry`.
    pub async fn upload(&self, upload: NewUpload, bytes: Vec<u8>) -> FilesResult<FileEntry> {
        if upload.original_name.trim().is_empty() {
            return Err(FilesError::InvalidArgument(
                "original_name is required".into(),
            ));
        }
        let workspace_id = upload
            .workspace_id
            .unwrap_or_else(|| DEFAULT_WORKSPACE_ID.to_string());
        let created_by = upload.created_by.unwrap_or_else(|| "operator".to_string());
        let content_type = upload
            .content_type
            .unwrap_or_else(|| infer_mime(&upload.original_name));
        let safe_name = sanitize_name(&upload.original_name);
        let id = Uuid::new_v4();
        let rel_path = Path::new(&workspace_id)
            .join(id.to_string())
            .join(&safe_name);
        let abs_path = self.user_files_root.join(&rel_path);
        verify_within_root(&self.user_files_root, &abs_path)?;

        if let Some(parent) = abs_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| FilesError::Io(err.to_string()))?;
        }

        let sha = {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex::encode(hasher.finalize())
        };
        let size_bytes = bytes.len() as u64;

        tokio::fs::write(&abs_path, &bytes)
            .await
            .map_err(|err| FilesError::Io(err.to_string()))?;

        let now = Utc::now();
        let storage_path = rel_path.to_string_lossy().replace('\\', "/");
        // Scope the lock guard tightly â€” parking_lot's guard is
        // `!Send`, so holding it across any `await` breaks the handler
        // futures.
        let insert_result = {
            let mut store = self.store.lock();
            store.insert(
                id,
                &workspace_id,
                &upload.original_name,
                &storage_path,
                &content_type,
                size_bytes,
                &sha,
                now,
                &created_by,
                upload.app_id,
            )
        };
        let entry = match insert_result {
            Ok(entry) => entry,
            Err(err) => {
                // Roll back the bytes so we never have a dangling
                // file without a metadata row.
                let _ = tokio::fs::remove_file(&abs_path).await;
                return Err(err);
            }
        };

        self.broadcast(OrdoMessage::FileUploaded(entry.clone()))
            .await;
        Ok(entry)
    }

    pub fn get_metadata(&self, id: Uuid) -> FilesResult<Option<FileEntry>> {
        self.store.lock().get(id)
    }

    pub async fn download(&self, id: Uuid) -> FilesResult<(FileEntry, Vec<u8>)> {
        let entry = self.store.lock().get(id)?.ok_or(FilesError::NotFound(id))?;
        let abs = self.user_files_root.join(&entry.storage_path);
        verify_within_root(&self.user_files_root, &abs)?;
        let bytes = tokio::fs::read(&abs)
            .await
            .map_err(|err| FilesError::Io(err.to_string()))?;
        Ok((entry, bytes))
    }

    pub fn list(&self, query: FilesQuery) -> FilesResult<Vec<FileEntry>> {
        let workspace_id = query.workspace_id.as_deref().or(Some(DEFAULT_WORKSPACE_ID));
        self.store
            .lock()
            .list(workspace_id, query.app_id, query.limit)
    }

    pub async fn delete(&self, id: Uuid) -> FilesResult<Option<FileEntry>> {
        let removed = {
            let mut store = self.store.lock();
            store.delete(id)?
        };
        if let Some(entry) = &removed {
            let abs = self.user_files_root.join(&entry.storage_path);
            if abs.exists() {
                if let Err(err) = tokio::fs::remove_file(&abs).await {
                    tracing::warn!(
                        target: "ordo_files",
                        path = %abs.display(),
                        error = %err,
                        "delete: orphaned bytes on disk (metadata row was removed)"
                    );
                }
                // Best-effort clean up the empty file-id directory.
                if let Some(parent) = abs.parent() {
                    let _ = tokio::fs::remove_dir(parent).await;
                }
            }
            self.broadcast(OrdoMessage::FileDeleted {
                id: entry.id,
                workspace_id: entry.workspace_id.clone(),
            })
            .await;
        }
        Ok(removed)
    }

    async fn broadcast(&self, message: OrdoMessage) {
        let Some(bus) = &self.bus else { return };
        let envelope = Envelope::new(self.node_id.clone(), message);
        if let Err(err) = bus.publish(topics::FILES_EVENT, envelope).await {
            tracing::warn!(target: "ordo_files", error = %err, "files.event publish failed");
        }
    }
}

/// Rejects names that would escape the per-file directory (path sep,
/// `..`, control chars). Keeps extensions.
fn sanitize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        let safe = match c {
            '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        };
        out.push(safe);
    }
    let trimmed = out.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Refuse to operate on any path that doesn't resolve under
/// `user_files_root`. Defends against malicious `storage_path` values
/// if a future migration or manual DB edit injected one.
fn verify_within_root(root: &Path, target: &Path) -> FilesResult<()> {
    // Normalize without requiring the path to exist yet (upload
    // creates the parent later), using a simple `..` count check.
    let mut rel = target.strip_prefix(root).map_err(|_| {
        FilesError::StorageEscape(format!("{} not under {}", target.display(), root.display()))
    })?;
    let mut depth: i32 = 0;
    loop {
        let mut components = rel.components();
        match components.next() {
            Some(std::path::Component::ParentDir) => depth -= 1,
            Some(std::path::Component::Normal(_)) => depth += 1,
            Some(_) => {}
            None => break,
        }
        rel = components.as_path();
    }
    if depth < 0 {
        return Err(FilesError::StorageEscape(format!(
            "{} escapes {}",
            target.display(),
            root.display()
        )));
    }
    Ok(())
}

/// Minimal MIME guess from extension. Not exhaustive â€” callers that
/// care should set `content_type` explicitly.
fn infer_mime(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    let mime = match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "txt" | "md" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "zip" => "application/zip",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn svc() -> (FilesService, tempfile::TempDir) {
        let tmp = tempdir().expect("tmp");
        let store = FilesStore::in_memory().expect("store");
        let service = FilesService::new(store, tmp.path().to_path_buf());
        (service, tmp)
    }

    #[tokio::test]
    async fn upload_roundtrip_writes_bytes_and_metadata() {
        let (svc, _root) = svc();
        let entry = svc
            .upload(
                NewUpload {
                    original_name: "hello.txt".into(),
                    content_type: Some("text/plain".into()),
                    workspace_id: None,
                    created_by: Some("test".into()),
                    app_id: None,
                },
                b"hello world".to_vec(),
            )
            .await
            .expect("upload");
        assert_eq!(entry.size_bytes, 11);
        assert_eq!(entry.content_type, "text/plain");
        assert_eq!(entry.workspace_id, "local");
        assert!(!entry.sha256_hex.is_empty());

        let (fetched, bytes) = svc.download(entry.id).await.expect("download");
        assert_eq!(fetched.id, entry.id);
        assert_eq!(&bytes, b"hello world");
    }

    #[tokio::test]
    async fn infer_mime_from_extension_when_omitted() {
        let (svc, _root) = svc();
        let entry = svc
            .upload(
                NewUpload {
                    original_name: "logo.png".into(),
                    content_type: None,
                    workspace_id: None,
                    created_by: None,
                    app_id: None,
                },
                vec![1, 2, 3],
            )
            .await
            .expect("upload");
        assert_eq!(entry.content_type, "image/png");
    }

    #[tokio::test]
    async fn delete_removes_bytes_and_metadata() {
        let (svc, root) = svc();
        let entry = svc
            .upload(
                NewUpload {
                    original_name: "doomed.txt".into(),
                    content_type: None,
                    workspace_id: None,
                    created_by: None,
                    app_id: None,
                },
                vec![4, 5, 6],
            )
            .await
            .expect("upload");
        let abs = root.path().join(&entry.storage_path);
        assert!(abs.exists());
        let removed = svc.delete(entry.id).await.expect("delete");
        assert!(removed.is_some());
        assert!(!abs.exists(), "bytes should be removed");
        assert!(svc.get_metadata(entry.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn sanitize_rejects_path_separators() {
        // `..` leading dots get trimmed alongside the path separator
        // replacement â€” the goal is "no way to break out of the
        // file-id directory," not preservation of the original bytes.
        assert_eq!(sanitize_name("../etc/passwd"), "_etc_passwd");
        assert_eq!(sanitize_name("normal.txt"), "normal.txt");
        assert_eq!(sanitize_name("   "), "file");
        // Backslashes mapped to underscores (Windows-authored names).
        assert_eq!(sanitize_name("a\\b.png"), "a_b.png");
    }

    #[tokio::test]
    async fn list_filters_by_app_id() {
        let (svc, _root) = svc();
        let app_a = Uuid::new_v4();
        let app_b = Uuid::new_v4();
        svc.upload(
            NewUpload {
                original_name: "a.txt".into(),
                content_type: None,
                workspace_id: None,
                created_by: None,
                app_id: Some(app_a),
            },
            vec![1],
        )
        .await
        .expect("a");
        svc.upload(
            NewUpload {
                original_name: "b.txt".into(),
                content_type: None,
                workspace_id: None,
                created_by: None,
                app_id: Some(app_b),
            },
            vec![2],
        )
        .await
        .expect("b");

        let only_a = svc
            .list(FilesQuery {
                app_id: Some(app_a),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].original_name, "a.txt");
    }
}
