//! `ModeRegistry` — loads defaults + on-disk overrides, hands out
//! manifests by id.
//!
//! ## Lifecycle
//!
//! 1. At runtime startup, the registry is constructed with a base
//!    directory (typically `<runtime>/user-files/modes`).
//! 2. The registry materializes any compiled-in defaults that don't
//!    yet exist on disk — first-run population. Defaults that DO
//!    exist on disk are NOT overwritten; the operator's edits win.
//! 3. The registry then reads every `*.json` in the directory.
//!    Each file becomes a registered mode. Files that fail
//!    validation are logged and skipped — a single broken manifest
//!    can't crash the whole runtime.
//! 4. Compiled-in defaults that aren't ALSO on disk (because they
//!    were just materialized) are loaded from the on-disk copies,
//!    not from memory — same path either way.
//!
//! ## Hot reload
//!
//! Not implemented. Mode manifests change rarely (operator action,
//! once per session at most). A runtime restart re-reads them. If
//! we ever need hot reload, it's a `notify`-watcher addition over
//! this same data path; the public API stays stable.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;
use tracing::{info, warn};

use crate::defaults;
use crate::manifest::{ModeManifest, ModeManifestError};

#[derive(Debug, Error)]
pub enum ModeRegistryError {
    #[error("modes directory '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read manifest at '{path}': {source}")]
    ReadManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("compiled-in defaults failed to validate: {0}")]
    BadDefault(#[from] ModeManifestError),
}

/// Errors from operator-driven mode lifecycle actions (create / delete /
/// update). Distinct from load errors so the control layer can map them to
/// precise HTTP statuses (see `docs/mode-lifecycle.md`): AlreadyExists → 409,
/// NotFound → 404, Protected → 403, Invalid → 400, Persist → 500.
#[derive(Debug, Error)]
pub enum ModeMutationError {
    #[error("a mode with id '{0}' already exists")]
    AlreadyExists(String),
    #[error("no mode with id '{0}'")]
    NotFound(String),
    #[error("mode '{0}' is protected and cannot be deleted")]
    Protected(String),
    #[error("invalid mode: {0}")]
    Invalid(#[from] ModeManifestError),
    #[error("failed to persist mode '{id}': {source}")]
    Persist {
        id: String,
        #[source]
        source: std::io::Error,
    },
}

/// Stats from a load pass — useful for the operator-facing
/// "modes panel" and for log lines after startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryStats {
    /// Number of compiled-in defaults written to disk this load
    /// (i.e., first-run materialization or a re-write after deletion).
    pub defaults_materialized: usize,
    /// Number of `*.json` files successfully loaded.
    pub files_loaded: usize,
    /// Number of `*.json` files that failed validation and were skipped.
    pub files_skipped: usize,
    /// Total modes registered.
    pub modes_registered: usize,
}

/// Thread-safe map from mode id to manifest. Cloned cheaply via
/// `Arc<RwLock<...>>`; reads dominate (every turn looks up the
/// active mode), writes are once per startup.
#[derive(Clone)]
pub struct ModeRegistry {
    inner: Arc<RwLock<HashMap<String, ModeManifest>>>,
    stats: Arc<RwLock<RegistryStats>>,
    /// The on-disk modes directory, when the registry was loaded from one.
    /// `None` for in-memory registries (`empty`/`from_defaults`) — those skip
    /// persistence, so create/delete are in-memory only there.
    modes_dir: Option<PathBuf>,
}

impl ModeRegistry {
    /// Build an empty registry with no defaults loaded. Tests use
    /// this; production uses [`load_with_defaults`].
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(RegistryStats::default())),
            modes_dir: None,
        }
    }

    /// Build a registry pre-populated with the compiled-in defaults
    /// — no disk involved. Useful when there's no on-disk storage
    /// (tests, CI).
    pub fn from_defaults() -> Result<Self, ModeRegistryError> {
        let registry = Self::empty();
        let defaults = defaults::all_defaults()?;
        let count = defaults.len();
        {
            let mut map = registry.inner.write();
            for m in defaults {
                map.insert(m.id.clone(), m);
            }
        }
        {
            let mut stats = registry.stats.write();
            stats.modes_registered = count;
            stats.files_loaded = 0;
        }
        Ok(registry)
    }

    /// Build a registry from a directory + the compiled-in defaults.
    ///
    /// Materializes any default whose `<id>.json` file is missing;
    /// then reads every `*.json` (including the just-written
    /// defaults). Operator-edited files override the compiled-in
    /// values — that's the whole point.
    ///
    /// On directory-create errors this returns `Err` (a runtime
    /// without a modes directory is broken). On per-file errors it
    /// logs and skips, returning the partial registry.
    pub fn load_with_defaults(modes_dir: &Path) -> Result<Self, ModeRegistryError> {
        let mut registry = Self::empty();
        registry.modes_dir = Some(modes_dir.to_path_buf());

        // Create the directory if missing — first-run flow.
        std::fs::create_dir_all(modes_dir).map_err(|err| ModeRegistryError::Io {
            path: modes_dir.to_path_buf(),
            source: err,
        })?;

        // Materialize compiled-in defaults whose file is absent.
        let defaults = defaults::all_defaults()?;
        let mut materialized = 0usize;
        for default in &defaults {
            let target = modes_dir.join(format!("{}.json", default.id));
            if !target.exists() {
                let body = default
                    .to_pretty_json()
                    .map_err(ModeRegistryError::BadDefault)?;
                if let Err(err) = std::fs::write(&target, body) {
                    warn!(
                        target: "ordo_modes",
                        path = %target.display(),
                        error = %err,
                        "failed to materialize default mode; mode will load from compiled-in copy in memory"
                    );
                    // Fall back to in-memory: register the default
                    // even if the disk write failed. Operator can
                    // still use the mode; we just couldn't persist.
                    registry
                        .inner
                        .write()
                        .insert(default.id.clone(), default.clone());
                } else {
                    materialized += 1;
                    info!(
                        target: "ordo_modes",
                        id = %default.id,
                        path = %target.display(),
                        "materialized default mode"
                    );
                }
            }
        }

        // Read every *.json in the directory. Validation errors are
        // logged and skipped — one bad file can't take down the rest.
        let mut loaded = 0usize;
        let mut skipped = 0usize;
        let entries = std::fs::read_dir(modes_dir).map_err(|err| ModeRegistryError::Io {
            path: modes_dir.to_path_buf(),
            source: err,
        })?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    warn!(
                        target: "ordo_modes",
                        error = %err,
                        "skipping unreadable directory entry"
                    );
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            match Self::load_one(&path) {
                Ok(manifest) => {
                    let id = manifest.id.clone();
                    registry.inner.write().insert(id.clone(), manifest);
                    loaded += 1;
                    info!(
                        target: "ordo_modes",
                        id = %id,
                        path = %path.display(),
                        "loaded mode manifest"
                    );
                }
                Err(err) => {
                    skipped += 1;
                    warn!(
                        target: "ordo_modes",
                        path = %path.display(),
                        error = %err,
                        "skipping invalid mode manifest"
                    );
                }
            }
        }

        // If a compiled-in default failed to materialize AND failed
        // to load from disk (because there was nothing there), the
        // earlier in-memory fallback already inserted it. Cross-
        // check: every default id must be registered.
        for default in &defaults {
            let mut map = registry.inner.write();
            if !map.contains_key(&default.id) {
                warn!(
                    target: "ordo_modes",
                    id = %default.id,
                    "default mode wasn't loaded from disk; falling back to compiled-in copy"
                );
                map.insert(default.id.clone(), default.clone());
            }
        }

        let total = registry.inner.read().len();
        {
            let mut stats = registry.stats.write();
            stats.defaults_materialized = materialized;
            stats.files_loaded = loaded;
            stats.files_skipped = skipped;
            stats.modes_registered = total;
        }

        Ok(registry)
    }

    fn load_one(path: &Path) -> Result<ModeManifest, ModeManifestError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|err| ModeManifestError::Json(serde_json::Error::io(err)))?;
        ModeManifest::from_json(&raw)
    }

    /// Look up a manifest by id. Returns a clone so callers don't
    /// hold the lock; manifests are small (<2 KB JSON).
    pub fn get(&self, id: &str) -> Option<ModeManifest> {
        self.inner.read().get(id).cloned()
    }

    /// Sorted list of all registered manifests, for the UXI mode
    /// switcher and the advanced view.
    pub fn list(&self) -> Vec<ModeManifest> {
        let map = self.inner.read();
        let mut out: Vec<ModeManifest> = map.values().cloned().collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn stats(&self) -> RegistryStats {
        self.stats.read().clone()
    }

    /// Insert or replace a mode in the registry. Used by tests and
    /// by an operator-driven "reload single mode" command (future).
    /// Validates the manifest first.
    pub fn upsert(&self, mut manifest: ModeManifest) -> Result<(), ModeManifestError> {
        manifest.normalize_and_validate()?;
        self.inner.write().insert(manifest.id.clone(), manifest);
        Ok(())
    }

    /// Create a NEW mode (operator action). Fails if the id already exists.
    /// Persists to `<modes_dir>/<id>.json` when the registry has a directory,
    /// then registers it. Returns the validated manifest.
    pub fn create(&self, mut manifest: ModeManifest) -> Result<ModeManifest, ModeMutationError> {
        manifest.normalize_and_validate()?;
        if self.inner.read().contains_key(&manifest.id) {
            return Err(ModeMutationError::AlreadyExists(manifest.id.clone()));
        }
        self.persist(&manifest)?;
        self.inner
            .write()
            .insert(manifest.id.clone(), manifest.clone());
        self.refresh_count();
        Ok(manifest)
    }

    /// Update an EXISTING mode's config. The `protected` flag is immutable
    /// through update — you cannot un-protect a core mode and then delete it.
    pub fn update(&self, mut manifest: ModeManifest) -> Result<ModeManifest, ModeMutationError> {
        manifest.normalize_and_validate()?;
        let existing = self
            .inner
            .read()
            .get(&manifest.id)
            .cloned()
            .ok_or_else(|| ModeMutationError::NotFound(manifest.id.clone()))?;
        manifest.protected = existing.protected; // protectedness is not editable
        self.persist(&manifest)?;
        self.inner
            .write()
            .insert(manifest.id.clone(), manifest.clone());
        Ok(manifest)
    }

    /// Delete a mode (operator action). Refuses a `protected` mode unless
    /// `force` is set — the guard behind "you can't casually delete a core
    /// mode." Removes the on-disk file (if any) and the registry entry. Note:
    /// this does NOT purge the mode's scoped memory/RAG partitions; that's a
    /// separate, deliberate cleanup.
    pub fn delete(&self, id: &str, force: bool) -> Result<(), ModeMutationError> {
        let manifest = self
            .inner
            .read()
            .get(id)
            .cloned()
            .ok_or_else(|| ModeMutationError::NotFound(id.to_string()))?;
        if manifest.protected && !force {
            return Err(ModeMutationError::Protected(id.to_string()));
        }
        if let Some(dir) = &self.modes_dir {
            let path = dir.join(format!("{id}.json"));
            if path.exists() {
                std::fs::remove_file(&path).map_err(|err| ModeMutationError::Persist {
                    id: id.to_string(),
                    source: err,
                })?;
            }
        }
        self.inner.write().remove(id);
        self.refresh_count();
        Ok(())
    }

    /// Whether the given mode id is a protected (built-in) mode.
    pub fn is_protected(&self, id: &str) -> bool {
        self.inner.read().get(id).map(|m| m.protected).unwrap_or(false)
    }

    fn persist(&self, manifest: &ModeManifest) -> Result<(), ModeMutationError> {
        if let Some(dir) = &self.modes_dir {
            let body = manifest.to_pretty_json()?;
            let path = dir.join(format!("{}.json", manifest.id));
            std::fs::write(&path, body).map_err(|err| ModeMutationError::Persist {
                id: manifest.id.clone(),
                source: err,
            })?;
        }
        Ok(())
    }

    fn refresh_count(&self) {
        let total = self.inner.read().len();
        self.stats.write().modes_registered = total;
    }

    /// Number of modes registered. Cheap.
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_has_no_modes() {
        let r = ModeRegistry::empty();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn from_defaults_loads_all_modes() {
        let r = ModeRegistry::from_defaults().unwrap();
        assert_eq!(r.len(), 7);
        for id in &[
            "general",
            "rust_vibe_coder",
            "coding",
            "research",
            "security_lab",
            "tech_specialist",
            "diagnostic",
        ] {
            assert!(r.get(id).is_some(), "missing mode: {id}");
        }
    }

    #[test]
    fn list_is_sorted_by_id() {
        let r = ModeRegistry::from_defaults().unwrap();
        let ids: Vec<String> = r.list().into_iter().map(|m| m.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn load_with_defaults_materializes_first_run() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        assert_eq!(r.len(), 7);
        let stats = r.stats();
        assert_eq!(stats.defaults_materialized, 7);
        let all = [
            "general",
            "rust_vibe_coder",
            "coding",
            "research",
            "security_lab",
            "tech_specialist",
            "diagnostic",
        ];
        for id in &all {
            assert!(
                tmp.path().join(format!("{id}.json")).exists(),
                "{id}.json missing"
            );
        }
    }

    #[test]
    fn second_load_does_not_overwrite_disk() {
        let tmp = tempfile::tempdir().unwrap();
        ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        let edited = r#"{
            "id": "general",
            "label": "Edited General",
            "description": "Operator override.",
            "memory_scope": ["global"],
            "rag_domains": [],
            "allowed_tool_lanes": ["knowledge."],
            "blocked_tool_capabilities": [],
            "policies": [],
            "planner_bias": [],
            "persona": []
        }"#;
        std::fs::write(tmp.path().join("general.json"), edited).unwrap();
        let r = ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        assert_eq!(r.get("general").unwrap().label, "Edited General");
        assert_eq!(r.stats().defaults_materialized, 0);
    }

    #[test]
    fn deleted_default_is_remateralized() {
        let tmp = tempfile::tempdir().unwrap();
        ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        std::fs::remove_file(tmp.path().join("coding.json")).unwrap();
        let r = ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        assert!(r.get("coding").is_some());
        assert_eq!(r.stats().defaults_materialized, 1);
    }

    #[test]
    fn invalid_file_skipped_others_load() {
        let tmp = tempfile::tempdir().unwrap();
        ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("corrupt_mode.json"), "{ not valid json").unwrap();
        let r = ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        assert!(r.get("general").is_some());
        assert!(r.get("security_lab").is_some());
        assert!(r.stats().files_skipped >= 1);
    }

    #[test]
    fn unknown_mode_id_returns_none() {
        let r = ModeRegistry::from_defaults().unwrap();
        assert!(r.get("not_a_mode").is_none());
    }

    // ── mode lifecycle (M2) ──────────────────────────────────────────────────

    #[test]
    fn create_persists_and_registers_a_user_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        let m = ModeManifest::new_user_mode("My Cool Mode").unwrap();
        assert_eq!(m.id, "my_cool_mode");
        assert!(!m.protected);

        r.create(m).unwrap();
        assert!(r.get("my_cool_mode").is_some());
        assert!(
            tmp.path().join("my_cool_mode.json").exists(),
            "create should persist to disk"
        );
        // survives a reload from disk
        let r2 = ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        assert!(r2.get("my_cool_mode").is_some());
    }

    #[test]
    fn create_rejects_duplicate_id() {
        let r = ModeRegistry::from_defaults().unwrap();
        let dup = ModeManifest::new_user_mode("general").unwrap();
        assert!(matches!(
            r.create(dup),
            Err(ModeMutationError::AlreadyExists(id)) if id == "general"
        ));
    }

    #[test]
    fn delete_removes_unprotected_mode_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        r.create(ModeManifest::new_user_mode("Scratch").unwrap()).unwrap();
        assert!(tmp.path().join("scratch.json").exists());

        r.delete("scratch", false).unwrap();
        assert!(r.get("scratch").is_none());
        assert!(!tmp.path().join("scratch.json").exists(), "delete should remove the file");
    }

    #[test]
    fn delete_refuses_protected_mode_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ModeRegistry::load_with_defaults(tmp.path()).unwrap();
        assert!(r.is_protected("general"));
        assert!(matches!(
            r.delete("general", false),
            Err(ModeMutationError::Protected(id)) if id == "general"
        ));
        assert!(r.get("general").is_some(), "protected mode must survive a casual delete");
        // force deletes it
        r.delete("general", true).unwrap();
        assert!(r.get("general").is_none());
    }

    #[test]
    fn delete_missing_mode_is_not_found() {
        let r = ModeRegistry::from_defaults().unwrap();
        assert!(matches!(
            r.delete("nope", false),
            Err(ModeMutationError::NotFound(id)) if id == "nope"
        ));
    }

    #[test]
    fn update_cannot_unprotect_a_core_mode() {
        let r = ModeRegistry::from_defaults().unwrap();
        let mut edited = r.get("general").unwrap();
        edited.protected = false; // attempt to un-protect
        edited.label = "Edited".into();
        let saved = r.update(edited).unwrap();
        assert!(saved.protected, "update must preserve protectedness");
        assert_eq!(saved.label, "Edited");
    }

    #[test]
    fn new_user_mode_rejects_empty_name() {
        assert!(ModeManifest::new_user_mode("   ").is_err());
        assert!(ModeManifest::new_user_mode("!!! ???").is_err());
    }

    #[test]
    fn upsert_validates_and_overwrites() {
        let r = ModeRegistry::from_defaults().unwrap();
        let mut new_mode = ModeManifest {
            id: "general".into(),
            label: "Custom General".into(),
            description: "Override.".into(),
            memory_scope: vec!["global".into()],
            rag_domains: vec![],
            allowed_tool_lanes: vec!["knowledge.".into()],
            blocked_tool_capabilities: vec![],
            policies: vec![],
            planner_bias: vec![],
            persona: vec![],
            default_timeout_secs: None,
            default_strictness: None,
            default_credential: None,
            cross_mode_borrow_policy: None,
            cross_mode_consult_policy: None,
            allowed_skill_tags: vec![],
            blocked_skill_tags: vec![],
            blocked_skills: vec![],
            max_skill_risk: None,
            default_skill_admission: None,
            protected: false,
        };
        new_mode.normalize_and_validate().unwrap();
        r.upsert(new_mode).unwrap();
        assert_eq!(r.get("general").unwrap().label, "Custom General");
        assert_eq!(r.len(), 7);
    }

    #[test]
    fn upsert_rejects_invalid_manifest() {
        let r = ModeRegistry::empty();
        let bad = ModeManifest {
            id: "".into(),
            label: "".into(),
            description: "".into(),
            memory_scope: vec![],
            rag_domains: vec![],
            allowed_tool_lanes: vec![],
            blocked_tool_capabilities: vec![],
            policies: vec![],
            planner_bias: vec![],
            persona: vec![],
            default_timeout_secs: None,
            default_strictness: None,
            default_credential: None,
            cross_mode_borrow_policy: None,
            cross_mode_consult_policy: None,
            allowed_skill_tags: vec![],
            blocked_skill_tags: vec![],
            blocked_skills: vec![],
            max_skill_risk: None,
            default_skill_admission: None,
            protected: false,
        };
        assert!(r.upsert(bad).is_err());
        assert!(r.is_empty());
    }
}
