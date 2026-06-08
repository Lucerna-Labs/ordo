pub mod atomic_write;
pub mod embedding_store;
pub use atomic_write::atomic_write;
pub use embedding_store::{
    EmbeddingMatch, EmbeddingRecord, EmbeddingStore, EmbeddingStoreError, EmbeddingStoreResult,
    InMemoryEmbeddingStore, SqliteEmbeddingStore,
};

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    thread,
};

use chrono::Utc;
use rusqlite::params;
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use tokio::sync::{mpsc, oneshot};

type DynError = Box<dyn std::error::Error + Send + Sync>;

trait StorageJob<State>: Send {
    fn run(self: Box<Self>, state: &mut State);
}

impl<State, F> StorageJob<State> for F
where
    F: FnOnce(&mut State) + Send + 'static,
{
    fn run(self: Box<Self>, state: &mut State) {
        (*self)(state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageTaskError {
    Closed,
    Operation(String),
}

impl std::fmt::Display for StorageTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "storage task is no longer running"),
            Self::Operation(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for StorageTaskError {}

pub struct StorageTask<State> {
    sender: mpsc::UnboundedSender<Box<dyn StorageJob<State>>>,
}

impl<State> Clone for StorageTask<State> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<State: Send + 'static> StorageTask<State> {
    pub fn start(name: impl Into<String>, state: State) -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Box<dyn StorageJob<State>>>();
        let thread_name = name.into();
        thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let mut state = state;
                while let Some(job) = receiver.blocking_recv() {
                    job.run(&mut state);
                }
            })
            .expect("spawn storage task thread");
        Self { sender }
    }

    pub async fn call<R, F>(&self, f: F) -> Result<R, StorageTaskError>
    where
        R: Send + 'static,
        F: FnOnce(&mut State) -> Result<R, String> + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(Box::new(move |state: &mut State| {
                let _ = reply_tx.send(f(state).map_err(StorageTaskError::Operation));
            }))
            .map_err(|_| StorageTaskError::Closed)?;
        reply_rx.await.map_err(|_| StorageTaskError::Closed)?
    }
}

const MIGRATIONS_SLICE: &[M<'_>] = &[
    M::up(
        "
        CREATE TABLE IF NOT EXISTS memory_records (
            id INTEGER PRIMARY KEY,
            stored_at TEXT NOT NULL,
            content TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_memory_records_stored_at
            ON memory_records (stored_at);
        ",
    ),
    M::up(
        "
        CREATE TABLE IF NOT EXISTS rag_chunks (
            document_id TEXT NOT NULL,
            uri TEXT NOT NULL,
            title TEXT NOT NULL,
            tags_json TEXT NOT NULL,
            chunk_index INTEGER NOT NULL,
            text TEXT NOT NULL,
            PRIMARY KEY (document_id, chunk_index)
        );

        CREATE INDEX IF NOT EXISTS idx_rag_chunks_document_id
            ON rag_chunks (document_id);
        ",
    ),
    M::up(
        "
        ALTER TABLE rag_chunks
        ADD COLUMN embedding_json TEXT NOT NULL DEFAULT '[]';
        ",
    ),
    M::up(
        "
        ALTER TABLE memory_records
        ADD COLUMN tier TEXT NOT NULL DEFAULT 'working';
        ",
    ),
    M::up(
        "
        ALTER TABLE memory_records
        ADD COLUMN size_bytes INTEGER NOT NULL DEFAULT 0;
        ",
    ),
    M::up(
        "
        ALTER TABLE rag_chunks
        ADD COLUMN indexed_at TEXT NOT NULL DEFAULT '';
        ",
    ),
    M::up(
        "
        ALTER TABLE rag_chunks
        ADD COLUMN size_bytes INTEGER NOT NULL DEFAULT 0;
        ",
    ),
    M::up(
        "
        CREATE TABLE IF NOT EXISTS heal_cases (
            fingerprint TEXT PRIMARY KEY,
            component TEXT NOT NULL,
            symptom TEXT NOT NULL,
            summary TEXT NOT NULL,
            why TEXT NOT NULL,
            actions_json TEXT NOT NULL,
            source TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_incident_id TEXT NOT NULL,
            occurrence_count INTEGER NOT NULL DEFAULT 1
        );

        CREATE INDEX IF NOT EXISTS idx_heal_cases_updated_at
            ON heal_cases (updated_at);
        ",
    ),
    M::up(
        "
        CREATE TABLE IF NOT EXISTS heal_attempts (
            id INTEGER PRIMARY KEY,
            incident_id TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            component TEXT NOT NULL,
            symptom TEXT NOT NULL,
            summary TEXT NOT NULL,
            why TEXT NOT NULL,
            actions_json TEXT NOT NULL,
            source TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            size_bytes INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_heal_attempts_fingerprint
            ON heal_attempts (fingerprint, recorded_at);
        ",
    ),
    M::up(
        "
        CREATE TABLE IF NOT EXISTS runtime_settings (
            setting_key TEXT PRIMARY KEY,
            setting_value TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_runtime_settings_updated_at
            ON runtime_settings (updated_at);
        ",
    ),
    M::up(
        "
        ALTER TABLE rag_chunks
        ADD COLUMN collection_name TEXT NOT NULL DEFAULT 'main';
        ",
    ),
    M::up(
        "
        ALTER TABLE rag_chunks
        ADD COLUMN source_document_id TEXT NOT NULL DEFAULT '';
        ",
    ),
    M::up(
        "
        CREATE INDEX IF NOT EXISTS idx_rag_chunks_collection_name
            ON rag_chunks (collection_name);
        ",
    ),
    M::up(
        "
        CREATE TABLE IF NOT EXISTS cloud_credentials (
            service TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            auth_style TEXT NOT NULL,
            secret TEXT NOT NULL,
            base_url TEXT,
            extras_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_cloud_credentials_updated_at
            ON cloud_credentials (updated_at);
        ",
    ),
    M::up(
        "
        CREATE TABLE IF NOT EXISTS review_requests (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            resolved_at TEXT,
            origin_capability TEXT NOT NULL,
            origin_plugin TEXT,
            title TEXT NOT NULL,
            content_type TEXT NOT NULL,
            content TEXT NOT NULL,
            metadata_json TEXT NOT NULL DEFAULT '{}',
            state TEXT NOT NULL,
            edited_content TEXT,
            decision_note TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_review_requests_state
            ON review_requests (state);
        CREATE INDEX IF NOT EXISTS idx_review_requests_created_at
            ON review_requests (created_at);
        ",
    ),
    M::up(
        "
        CREATE TABLE IF NOT EXISTS assistant_sessions (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            title TEXT,
            turn_count INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_assistant_sessions_updated_at
            ON assistant_sessions (updated_at);

        CREATE TABLE IF NOT EXISTS assistant_turns (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            turn_index INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            user_message TEXT NOT NULL,
            assistant_response TEXT NOT NULL,
            context_json TEXT NOT NULL DEFAULT '{}',
            model TEXT,
            credential_service TEXT,
            FOREIGN KEY (session_id) REFERENCES assistant_sessions (id)
        );

        CREATE INDEX IF NOT EXISTS idx_assistant_turns_session
            ON assistant_turns (session_id, turn_index);

        CREATE TABLE IF NOT EXISTS assistant_facts (
            id TEXT PRIMARY KEY,
            subject TEXT NOT NULL,
            predicate TEXT NOT NULL,
            object TEXT NOT NULL,
            source TEXT NOT NULL,
            confidence REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL,
            reinforced_at TEXT NOT NULL,
            embedding BLOB NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_assistant_facts_subject
            ON assistant_facts (subject);
        ",
    ),
    M::up(
        "
        -- Assistant self-knowledge RAG (push 3).
        --
        -- Holds chunked, embedded snippets that describe what the
        -- assistant can do: skill cards, persona guides, capability
        -- notes, and observations about what worked/didn't. The LLM
        -- reaches this layer on demand via `assistant.knowledge_lookup`.
        --
        -- `kind` lets callers filter ('skill', 'persona', 'tool_note',
        --   'observation', 'note'); `domain` optionally tags a chunk
        --   to one of the ten domain slots so routing can scope lookups.
        CREATE TABLE IF NOT EXISTS assistant_knowledge (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            domain TEXT,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'operator',
            confidence REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            reinforced_at TEXT NOT NULL,
            embedding BLOB NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_assistant_knowledge_kind
            ON assistant_knowledge (kind);
        CREATE INDEX IF NOT EXISTS idx_assistant_knowledge_domain
            ON assistant_knowledge (domain);
        ",
    ),
    M::up(
        "
        -- Apps primitive (Phase 1.1). An `app` is a persisted, lifecycle-
        -- managed artifact that bundles a UI extension, plugin config,
        -- RAG corpus and sessions. `app_events` is append-only and is
        -- the source of truth for history/versioning (Phase 1.2) â€” the
        -- `apps` row holds the folded current state for fast reads.
        --
        -- `workspace_id` ships from day one (Rule 6, Phase 4.4 makes
        -- this multi-tenant elsewhere without retrofitting).
        CREATE TABLE IF NOT EXISTS apps (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            slug TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'draft',
            metadata_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            published_at TEXT,
            archived_at TEXT,
            UNIQUE (workspace_id, slug)
        );

        CREATE INDEX IF NOT EXISTS idx_apps_workspace_status
            ON apps (workspace_id, status);
        CREATE INDEX IF NOT EXISTS idx_apps_updated_at
            ON apps (updated_at DESC);

        CREATE TABLE IF NOT EXISTS app_events (
            id TEXT PRIMARY KEY,
            app_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL DEFAULT '{}',
            actor TEXT NOT NULL DEFAULT 'operator',
            created_at TEXT NOT NULL,
            UNIQUE (app_id, seq),
            FOREIGN KEY (app_id) REFERENCES apps(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_app_events_app_seq
            ON app_events (app_id, seq);
        CREATE INDEX IF NOT EXISTS idx_app_events_workspace_created
            ON app_events (workspace_id, created_at DESC);
        ",
    ),
    M::up(
        "
        -- Files primitive (Phase 1.4). Metadata-in-SQLite,
        -- bytes-on-disk hybrid: the `files` row is authoritative for
        -- every stat the platform reads frequently (size, content
        -- type, hash, lifecycle), and the actual bytes live on disk
        -- under the runtime's user_files/ root. `storage_path` is
        -- relative to that root so the DB stays portable.
        -- `app_id` is advisory (not a FK) so files can be created
        -- standalone â€” the operator uploads brand assets before an
        -- app exists, and agent-authored uploads survive the app
        -- being archived. The platform tolerates dangling app_ids
        -- rather than cascading or refusing.
        CREATE TABLE IF NOT EXISTS files (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            original_name TEXT NOT NULL,
            storage_path TEXT NOT NULL,
            content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
            size_bytes INTEGER NOT NULL DEFAULT 0,
            sha256_hex TEXT NOT NULL,
            created_at TEXT NOT NULL,
            created_by TEXT NOT NULL DEFAULT 'operator',
            app_id TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_files_workspace_created
            ON files (workspace_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_files_sha256
            ON files (sha256_hex);
        CREATE INDEX IF NOT EXISTS idx_files_app
            ON files (app_id);
        ",
    ),
    M::up(
        "
        -- Webhook subscriptions (Phase 3.1). External subscribers
        -- receive signed POSTs when matching bus events fire. The
        -- `topics_json` field is a JSON array of bus topic strings
        -- (e.g. `ordo.apps.event`, `ordo.files.event`) the
        -- subscription opts into. `secret` is the HMAC-SHA256 secret
        -- used to sign bodies; callers verify via
        -- `X-Ordo-Signature`.
        CREATE TABLE IF NOT EXISTS webhook_subscriptions (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            target_url TEXT NOT NULL,
            secret TEXT NOT NULL,
            topics_json TEXT NOT NULL DEFAULT '[]',
            description TEXT NOT NULL DEFAULT '',
            active INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            last_delivery_at TEXT,
            last_delivery_status INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_webhook_subscriptions_workspace
            ON webhook_subscriptions (workspace_id, active);
        ",
    ),
    M::up(
        "
        -- App deployments (Phase 3.3). A deployment is a durable
        -- snapshot tag on top of an app's event stream: 'this is the
        -- state we promoted / published / made available for
        -- preview.' The `app_events` log remains the audit of every
        -- change; `app_deployments` marks which of those points are
        -- externally referenceable.
        CREATE TABLE IF NOT EXISTS app_deployments (
            id TEXT PRIMARY KEY,
            app_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            app_event_seq INTEGER NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending',
            preview_path TEXT,
            note TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            promoted_at TEXT,
            FOREIGN KEY (app_id) REFERENCES apps(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_app_deployments_app
            ON app_deployments (app_id, created_at DESC);
        ",
    ),
    M::up(
        "
        -- Phase 4.4: multi-tenant retrofit. Every pre-Phase-1 table
        -- gets a `workspace_id` column with default 'local'. No
        -- current code reads this; adding it now keeps future
        -- workspace-aware queries from requiring a much more fragile
        -- backfill of historical rows.
        --
        -- Rationale: the architecture contract (Rule 6) says new
        -- tables ship with workspace_id from day one. The tables
        -- created below predate that rule; this migration brings
        -- them in line.
        ALTER TABLE memory_records ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE rag_chunks ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE heal_cases ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE heal_attempts ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE runtime_settings ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE cloud_credentials ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE review_requests ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE assistant_sessions ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE assistant_turns ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE assistant_facts ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ALTER TABLE assistant_knowledge ADD COLUMN workspace_id TEXT NOT NULL DEFAULT 'local';
        ",
    ),
    M::up(
        "
        -- Follow-up 2: pluggable vector index (SqliteEmbeddingStore).
        -- Backs the `EmbeddingStore` trait with generic persistence
        -- so new consumers can use the trait today without writing
        -- ad-hoc SQL. Existing (FactStore/KnowledgeStore/RagStore)
        -- continue to use their inline embeddings until migration.
        CREATE TABLE IF NOT EXISTS vector_index (
            namespace TEXT NOT NULL,
            id TEXT NOT NULL,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            vector_json TEXT NOT NULL,
            payload_json TEXT NOT NULL DEFAULT '{}',
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY (namespace, id)
        );

        CREATE INDEX IF NOT EXISTS idx_vector_index_workspace
            ON vector_index (workspace_id, namespace);
        ",
    ),
    M::up(
        "
        -- Hierarchical memory architecture v2 (memory-log crate).
        -- The append-only event log â€” DPM substrate. Rule 6
        -- (workspace_id from day one); Rule 11 wire types in
        -- ordo-protocol::memory.
        --
        -- `id` is a ULID (26-char Crockford base32 string) â€”
        -- sortable + globally unique. `payload_hash` is lowercase
        -- hex blake3 of canonical JSON bytes; the log validates it
        -- on append.
        --
        -- Soft-delete is a FLAG, never a hard DELETE â€” DPM replay
        -- needs the full history to reconstruct past projections.
        CREATE TABLE IF NOT EXISTS memory_events (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            timestamp_ms INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            actor TEXT NOT NULL,
            domain TEXT,
            category TEXT,
            parent_id TEXT,
            payload_json TEXT NOT NULL,
            payload_hash TEXT NOT NULL,
            tier TEXT NOT NULL DEFAULT 'hot',
            pinned INTEGER NOT NULL DEFAULT 0,
            soft_deleted INTEGER NOT NULL DEFAULT 0,
            soft_deleted_at TEXT,
            soft_deleted_reason TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_mem_events_timestamp
            ON memory_events (timestamp_ms);
        CREATE INDEX IF NOT EXISTS idx_mem_events_domain
            ON memory_events (domain, timestamp_ms);
        CREATE INDEX IF NOT EXISTS idx_mem_events_parent
            ON memory_events (parent_id);
        CREATE INDEX IF NOT EXISTS idx_mem_events_type
            ON memory_events (event_type, timestamp_ms);
        -- Hot + pinned composite for the most common retrieval path.
        CREATE INDEX IF NOT EXISTS idx_mem_events_hot_pin
            ON memory_events (tier, pinned)
            WHERE soft_deleted = 0;

        -- Tree structure for the memory router. Tombstoning keeps
        -- historical replays able to see the tree-as-of-then.
        CREATE TABLE IF NOT EXISTS memory_tree_nodes (
            path TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            parent_path TEXT,
            description TEXT NOT NULL,
            retrieval_hint TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            tombstoned INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_mem_tree_parent
            ON memory_tree_nodes (parent_path)
            WHERE tombstoned = 0;
        ",
    ),
    M::up(
        "
        -- Blueprint concern 2: turn_id as a first-class grouping
        -- primitive on every event. Optional for backward
        -- compatibility with events already in the log (their turn_id
        -- stays NULL); REQUIRED at emit time from this point
        -- forward. Partial index so NULL turn_ids don't bloat the
        -- index.
        ALTER TABLE memory_events ADD COLUMN turn_id TEXT;
        CREATE INDEX IF NOT EXISTS idx_mem_turn
            ON memory_events (turn_id)
            WHERE turn_id IS NOT NULL;
        ",
    ),
    M::up(
        "
        -- Secrets blueprint v2 â€” full schema committed on first
        -- add, no phased rollouts. Four tables:
        --
        --   sealed_secrets          the encrypted material + metadata
        --   secrets_audit_chain     append-only hash-chained log
        --   threshold_shares        share metadata (NOT the shares
        --                           themselves â€” those live on their
        --                           holder devices, sealed there)
        --   secrets_rotation_schedule  per-secret rotation policy

        -- Sealed secret material. `ciphertext` is AEAD-wrapped;
        -- `sealing_tier` records which tier protects the DEK that
        -- unwraps it. Rotation invariant 23: when a secret rotates,
        -- the old row's ciphertext is overwritten with zeros and
        -- `retired_at` is set; the row stays for audit continuity.
        CREATE TABLE IF NOT EXISTS sealed_secrets (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            class TEXT NOT NULL,
            protection_kind TEXT NOT NULL,
            protection_threshold_t INTEGER,
            protection_threshold_n INTEGER,
            label TEXT NOT NULL,
            allowed_providers_json TEXT NOT NULL DEFAULT '[]',
            sealing_tier TEXT NOT NULL,
            ciphertext BLOB NOT NULL,
            nonce BLOB NOT NULL,
            aad BLOB NOT NULL,
            dek_generation INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            rotation_due_at TEXT,
            retired_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_sealed_secrets_workspace_class
            ON sealed_secrets (workspace_id, class)
            WHERE retired_at IS NULL;
        CREATE INDEX IF NOT EXISTS idx_sealed_secrets_rotation_due
            ON sealed_secrets (rotation_due_at)
            WHERE retired_at IS NULL AND rotation_due_at IS NOT NULL;

        -- Audit chain. Every row links to the previous via
        -- `prev_hash = blake3(previous entry's canonical bytes)`.
        -- Genesis entry has all-zero prev_hash.
        CREATE TABLE IF NOT EXISTS secrets_audit_chain (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            sequence INTEGER NOT NULL,
            prev_hash BLOB NOT NULL,
            entry_hash BLOB NOT NULL,
            timestamp TEXT NOT NULL,
            event_type TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            UNIQUE (workspace_id, sequence)
        );

        CREATE INDEX IF NOT EXISTS idx_secrets_audit_workspace_seq
            ON secrets_audit_chain (workspace_id, sequence);

        -- Signed anchors over contiguous chain slices. COSE_Sign1
        -- payload stored as BLOB; receipts fetched from a
        -- TransparencyService trait impl live here too.
        CREATE TABLE IF NOT EXISTS secrets_audit_anchors (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            first_sequence INTEGER NOT NULL,
            last_sequence INTEGER NOT NULL,
            chain_root BLOB NOT NULL,
            signed_at TEXT NOT NULL,
            cose_sign1 BLOB NOT NULL,
            signer_tier TEXT NOT NULL,
            service_id TEXT NOT NULL,
            service_attestation BLOB
        );

        CREATE INDEX IF NOT EXISTS idx_secrets_anchors_workspace_range
            ON secrets_audit_anchors (workspace_id, last_sequence DESC);

        -- Threshold share metadata. The actual share material is on
        -- the holder device (laptop vault / YubiKey / paper). This
        -- table records who holds what so the broker can dispatch
        -- a signing round.
        CREATE TABLE IF NOT EXISTS threshold_shares (
            share_id TEXT PRIMARY KEY,
            secret_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            share_index INTEGER NOT NULL,
            total_shares INTEGER NOT NULL,
            holder_fingerprint BLOB NOT NULL,
            holder_label TEXT NOT NULL,
            registered_at TEXT NOT NULL,
            last_signed_at TEXT,
            retired_at TEXT,
            FOREIGN KEY (secret_id) REFERENCES sealed_secrets(id)
        );

        CREATE INDEX IF NOT EXISTS idx_threshold_shares_secret
            ON threshold_shares (secret_id)
            WHERE retired_at IS NULL;

        -- Rotation policy + schedule. Defaults come from
        -- SecretClass::default_rotation_days at insert time;
        -- operator can override.
        CREATE TABLE IF NOT EXISTS secrets_rotation_schedule (
            secret_id TEXT PRIMARY KEY,
            days_until_rotation INTEGER NOT NULL,
            compromise_check INTEGER NOT NULL DEFAULT 1,
            last_rotated_at TEXT,
            FOREIGN KEY (secret_id) REFERENCES sealed_secrets(id)
        );

        -- Vault master-key state. The master DEK is sealed by the
        -- active Sealer tier and persisted as `sealed_dek`. The
        -- `generation` counter increments on each DEK rotation;
        -- all sealed_secrets rows record which generation wrapped
        -- them, and rotation re-seals every active secret under
        -- the new DEK and zeroes the old DEK cipher material.
        CREATE TABLE IF NOT EXISTS vault_state (
            workspace_id TEXT PRIMARY KEY,
            generation INTEGER NOT NULL DEFAULT 0,
            sealing_tier TEXT NOT NULL,
            sealer_label TEXT NOT NULL,
            sealed_dek BLOB NOT NULL,
            created_at TEXT NOT NULL,
            rotated_at TEXT
        );
        ",
    ),
    // Connections â€” operator-facing concept that ties a friendly
    // name + a connection type to credential material in
    // `ordo-secrets-vault` and a per-type test handler. The
    // `vault_secret_id` column references the sealed_secrets row
    // holding the secret material; for types that don't need a
    // secret (e.g. webhook with no auth) it's NULL.
    M::up(
        "
        CREATE TABLE IF NOT EXISTS connections (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL DEFAULT 'local',
            type_id TEXT NOT NULL,
            friendly_name TEXT NOT NULL,
            fields_json TEXT NOT NULL DEFAULT '{}',
            vault_secret_id TEXT,
            status TEXT NOT NULL DEFAULT 'untested',
            status_detail TEXT,
            last_test_at_ms INTEGER,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_connections_workspace_type
            ON connections (workspace_id, type_id);
        CREATE INDEX IF NOT EXISTS idx_connections_workspace_status
            ON connections (workspace_id, status);
        ",
    ),
    M::up(
        "
        -- Mode-scoped workspaces (ordo-modes step 2 + step 3).
        --
        -- Each session is bound to a mode at creation. Mode is a
        -- short string (e.g. 'general', 'vibe_coding') that the
        -- runtime resolves to a manifest at turn time. Existing
        -- sessions backfill to 'general' — the safest default,
        -- matching the conservative-tools / global-memory shape
        -- they were running under before modes existed.
        --
        -- Each fact row carries a scope tag (e.g. 'global' or
        -- 'mode:vibe_coding'). The mode's manifest declares which
        -- scopes it can read; the fact retrieval filters on that.
        -- Existing facts backfill to 'global' so legacy memory
        -- stays visible from every mode.
        --
        -- Keeping the columns plain TEXT (no FK to a modes table)
        -- because modes live on disk as JSON manifests, not in the
        -- database — the registry is the source of truth, not the
        -- DB. The string here is just a join key.
        ALTER TABLE assistant_sessions ADD COLUMN mode TEXT NOT NULL DEFAULT 'general';
        ALTER TABLE assistant_facts ADD COLUMN scope TEXT NOT NULL DEFAULT 'global';

        CREATE INDEX IF NOT EXISTS idx_assistant_sessions_mode
            ON assistant_sessions (mode);
        CREATE INDEX IF NOT EXISTS idx_assistant_facts_scope
            ON assistant_facts (scope);
        ",
    ),
];

const MIGRATIONS: Migrations<'_> = Migrations::from_slice(MIGRATIONS_SLICE);

pub struct OrdoDatabase {
    path: Option<PathBuf>,
    conn: Connection,
}

impl OrdoDatabase {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut conn = Connection::open(&path)?;
        configure_connection(&mut conn)?;
        Ok(Self {
            path: Some(path),
            conn,
        })
    }

    pub fn in_memory() -> Result<Self, DynError> {
        let mut conn = Connection::open_in_memory()?;
        configure_connection(&mut conn)?;
        Ok(Self { path: None, conn })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub profile: Option<String>,
    pub rag_budget_bytes: Option<usize>,
    pub memory_working_budget_bytes: Option<usize>,
    pub memory_pinned_budget_bytes: Option<usize>,
    pub self_heal_history_budget_bytes: Option<usize>,
    pub self_heal_llama_cpp_binary: Option<String>,
    pub self_heal_model_path: Option<String>,
    pub self_heal_model_context_size: Option<usize>,
    pub self_heal_model_max_tokens: Option<usize>,
    pub self_heal_model_temperature: Option<String>,
    pub embedding_llama_cpp_binary: Option<String>,
    pub embedding_model_path: Option<String>,
    pub embedding_dimensions: Option<usize>,
    pub embedding_context_size: Option<usize>,
}

pub type RuntimeSettingsUpdate = RuntimeSettings;

pub struct RuntimeSettingsStore {
    db: OrdoDatabase,
}

impl RuntimeSettingsStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        Ok(Self {
            db: OrdoDatabase::open(path)?,
        })
    }

    pub fn in_memory() -> Result<Self, DynError> {
        Ok(Self {
            db: OrdoDatabase::in_memory()?,
        })
    }

    pub fn load(&self) -> Result<RuntimeSettings, DynError> {
        let rows = self
            .db
            .conn()
            .prepare("SELECT setting_key, setting_value FROM runtime_settings")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;

        Ok(RuntimeSettings {
            profile: parse_optional_nonempty_string(rows.get("profile")),
            rag_budget_bytes: parse_optional_usize(rows.get("rag_budget_bytes")),
            memory_working_budget_bytes: parse_optional_usize(
                rows.get("memory_working_budget_bytes"),
            ),
            memory_pinned_budget_bytes: parse_optional_usize(
                rows.get("memory_pinned_budget_bytes"),
            ),
            self_heal_history_budget_bytes: parse_optional_usize(
                rows.get("self_heal_history_budget_bytes"),
            ),
            self_heal_llama_cpp_binary: parse_optional_nonempty_string(
                rows.get("self_heal_llama_cpp_binary"),
            ),
            self_heal_model_path: parse_optional_nonempty_string(rows.get("self_heal_model_path")),
            self_heal_model_context_size: parse_optional_usize(
                rows.get("self_heal_model_context_size"),
            ),
            self_heal_model_max_tokens: parse_optional_usize(
                rows.get("self_heal_model_max_tokens"),
            ),
            self_heal_model_temperature: parse_optional_nonempty_string(
                rows.get("self_heal_model_temperature"),
            ),
            embedding_llama_cpp_binary: parse_optional_nonempty_string(
                rows.get("embedding_llama_cpp_binary"),
            ),
            embedding_model_path: parse_optional_nonempty_string(rows.get("embedding_model_path")),
            embedding_dimensions: parse_optional_usize(rows.get("embedding_dimensions")),
            embedding_context_size: parse_optional_usize(rows.get("embedding_context_size")),
        })
    }

    pub fn update(&mut self, update: &RuntimeSettingsUpdate) -> Result<RuntimeSettings, DynError> {
        let tx = self.db.conn_mut().transaction()?;
        let updated_at = Utc::now().to_rfc3339();

        if let Some(profile) = &update.profile {
            upsert_runtime_setting(&tx, "profile", profile, &updated_at)?;
        }
        if let Some(value) = update.rag_budget_bytes {
            upsert_runtime_setting(&tx, "rag_budget_bytes", &value.to_string(), &updated_at)?;
        }
        if let Some(value) = update.memory_working_budget_bytes {
            upsert_runtime_setting(
                &tx,
                "memory_working_budget_bytes",
                &value.to_string(),
                &updated_at,
            )?;
        }
        if let Some(value) = update.memory_pinned_budget_bytes {
            upsert_runtime_setting(
                &tx,
                "memory_pinned_budget_bytes",
                &value.to_string(),
                &updated_at,
            )?;
        }
        if let Some(value) = update.self_heal_history_budget_bytes {
            upsert_runtime_setting(
                &tx,
                "self_heal_history_budget_bytes",
                &value.to_string(),
                &updated_at,
            )?;
        }
        if let Some(value) = &update.self_heal_llama_cpp_binary {
            upsert_runtime_setting(&tx, "self_heal_llama_cpp_binary", value, &updated_at)?;
        }
        if let Some(value) = &update.self_heal_model_path {
            upsert_runtime_setting(&tx, "self_heal_model_path", value, &updated_at)?;
        }
        if let Some(value) = update.self_heal_model_context_size {
            upsert_runtime_setting(
                &tx,
                "self_heal_model_context_size",
                &value.to_string(),
                &updated_at,
            )?;
        }
        if let Some(value) = update.self_heal_model_max_tokens {
            upsert_runtime_setting(
                &tx,
                "self_heal_model_max_tokens",
                &value.to_string(),
                &updated_at,
            )?;
        }
        if let Some(value) = &update.self_heal_model_temperature {
            upsert_runtime_setting(&tx, "self_heal_model_temperature", value, &updated_at)?;
        }
        if let Some(value) = &update.embedding_llama_cpp_binary {
            upsert_runtime_setting(&tx, "embedding_llama_cpp_binary", value, &updated_at)?;
        }
        if let Some(value) = &update.embedding_model_path {
            upsert_runtime_setting(&tx, "embedding_model_path", value, &updated_at)?;
        }
        if let Some(value) = update.embedding_dimensions {
            upsert_runtime_setting(&tx, "embedding_dimensions", &value.to_string(), &updated_at)?;
        }
        if let Some(value) = update.embedding_context_size {
            upsert_runtime_setting(
                &tx,
                "embedding_context_size",
                &value.to_string(),
                &updated_at,
            )?;
        }

        tx.commit()?;
        self.load()
    }
}

#[derive(Clone)]
pub struct RuntimeSettingsTask {
    inner: StorageTask<RuntimeSettingsStore>,
}

impl RuntimeSettingsTask {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        Ok(Self::from_store(RuntimeSettingsStore::open(path)?))
    }

    pub fn in_memory() -> Result<Self, DynError> {
        Ok(Self::from_store(RuntimeSettingsStore::in_memory()?))
    }

    pub fn from_store(store: RuntimeSettingsStore) -> Self {
        Self {
            inner: StorageTask::start("runtime-settings-store", store),
        }
    }

    pub async fn load(&self) -> Result<RuntimeSettings, StorageTaskError> {
        self.inner
            .call(|store| store.load().map_err(|err| err.to_string()))
            .await
    }

    pub async fn update(
        &self,
        update: RuntimeSettingsUpdate,
    ) -> Result<RuntimeSettings, StorageTaskError> {
        self.inner
            .call(move |store| store.update(&update).map_err(|err| err.to_string()))
            .await
    }
}

fn configure_connection(conn: &mut Connection) -> Result<(), DynError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // Two independent `OrdoDatabase` connections currently exist
    // on the runtime's database file — one for cloud credentials,
    // one for the memory log. WAL allows readers + a single writer
    // without blocking, but two writers still serialize. Without a
    // busy_timeout, a second writer fails fast with "database is
    // locked"; with one, SQLite retries inside the engine for up
    // to N ms. 5s is generous — longest path is the cloud
    // delete's multi-statement transaction (Cycle-3 atomicity
    // requirement) racing a memory archive insert.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    MIGRATIONS.to_latest(conn)?;
    Ok(())
}

/// Result of a manual WAL checkpoint, surfaced so the shutdown path can
/// log how much of the write-ahead log was folded back into the main
/// database file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpoint {
    /// `1` if SQLite could not acquire the locks needed to finish the
    /// checkpoint (e.g. another connection was mid-write); `0` on success.
    pub busy: i64,
    /// Total number of frames in the write-ahead log.
    pub log_frames: i64,
    /// Number of frames moved back into the main database file.
    pub checkpointed_frames: i64,
}

impl std::fmt::Display for WalCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "busy={} wal_frames={} checkpointed_frames={}",
            self.busy, self.log_frames, self.checkpointed_frames
        )
    }
}

/// Run a `TRUNCATE` WAL checkpoint against the database at `path` on a
/// fresh, short-lived connection, folding the write-ahead log back into
/// the main database file and shrinking the `-wal` file to zero.
///
/// This exists for the graceful-shutdown path (`ordo serve`). The runtime
/// keeps several SQLite connections open in WAL mode with
/// `synchronous=NORMAL`; on a clean exit SQLite only performs a *passive*
/// checkpoint when the last connection closes, and a *hard* kill performs
/// none at all — which is how the 2026-06-07 runtime termination left a
/// 3.6 MB orphaned `ordo.db-wal`. WAL mode keeps that data safe (it is
/// replayed on the next open, so a kill never loses committed work), but
/// an explicit TRUNCATE checkpoint at shutdown keeps the on-disk footprint
/// tidy and makes shutdown deterministic instead of relying on connection
/// drop order across the detached `StorageTask` threads.
///
/// Call this AFTER the runtime's own connections have been torn down so
/// the TRUNCATE is uncontended. A `busy` result is reported in the return
/// value rather than raised as an error: a contended checkpoint is not a
/// failure, because the WAL is still replayed on the next open.
pub fn checkpoint_wal(path: impl AsRef<Path>) -> Result<WalCheckpoint, DynError> {
    let conn = Connection::open(path.as_ref())?;
    // Keep the shutdown path snappy: wait only briefly for a lingering
    // writer rather than blocking process exit for the full 5s used by
    // long-lived connections.
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    let checkpoint = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok(WalCheckpoint {
            busy: row.get(0)?,
            log_frames: row.get(1)?,
            checkpointed_frames: row.get(2)?,
        })
    })?;
    Ok(checkpoint)
}

fn upsert_runtime_setting(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    value: &str,
    updated_at: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "
        INSERT INTO runtime_settings (setting_key, setting_value, updated_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(setting_key) DO UPDATE
        SET setting_value = excluded.setting_value,
            updated_at = excluded.updated_at
        ",
        params![key, value, updated_at],
    )?;
    Ok(())
}

fn parse_optional_usize(value: Option<&String>) -> Option<usize> {
    value.and_then(|value| value.parse::<usize>().ok())
}

fn parse_optional_nonempty_string(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{RuntimeSettingsStore, RuntimeSettingsUpdate};

    #[test]
    fn runtime_settings_round_trip() {
        let mut store = RuntimeSettingsStore::in_memory().expect("settings store");
        let settings = store
            .update(&RuntimeSettingsUpdate {
                profile: Some("full".to_string()),
                rag_budget_bytes: Some(2048),
                memory_working_budget_bytes: Some(4096),
                memory_pinned_budget_bytes: Some(8192),
                self_heal_history_budget_bytes: Some(1024),
                self_heal_llama_cpp_binary: Some("C:/llama/llama-cli.exe".to_string()),
                self_heal_model_path: Some("C:/models/repair.gguf".to_string()),
                self_heal_model_context_size: Some(4096),
                self_heal_model_max_tokens: Some(384),
                self_heal_model_temperature: Some("0.1".to_string()),
                embedding_llama_cpp_binary: Some("C:/llama/llama-embedding.exe".to_string()),
                embedding_model_path: Some("C:/models/embed.gguf".to_string()),
                embedding_dimensions: Some(384),
                embedding_context_size: Some(512),
            })
            .expect("updated settings");

        assert_eq!(settings.profile.as_deref(), Some("full"));
        assert_eq!(settings.rag_budget_bytes, Some(2048));
        assert_eq!(settings.memory_working_budget_bytes, Some(4096));
        assert_eq!(settings.memory_pinned_budget_bytes, Some(8192));
        assert_eq!(settings.self_heal_history_budget_bytes, Some(1024));
        assert_eq!(
            settings.self_heal_llama_cpp_binary.as_deref(),
            Some("C:/llama/llama-cli.exe")
        );
        assert_eq!(
            settings.self_heal_model_path.as_deref(),
            Some("C:/models/repair.gguf")
        );
        assert_eq!(settings.self_heal_model_context_size, Some(4096));
        assert_eq!(settings.self_heal_model_max_tokens, Some(384));
        assert_eq!(settings.self_heal_model_temperature.as_deref(), Some("0.1"));
        assert_eq!(
            settings.embedding_llama_cpp_binary.as_deref(),
            Some("C:/llama/llama-embedding.exe")
        );
        assert_eq!(
            settings.embedding_model_path.as_deref(),
            Some("C:/models/embed.gguf")
        );
        assert_eq!(settings.embedding_dimensions, Some(384));
        assert_eq!(settings.embedding_context_size, Some(512));
        assert_eq!(store.load().expect("loaded settings"), settings);
    }

    #[test]
    fn runtime_settings_updates_are_partial() {
        let mut store = RuntimeSettingsStore::in_memory().expect("settings store");
        store
            .update(&RuntimeSettingsUpdate {
                profile: Some("minimal".to_string()),
                ..RuntimeSettingsUpdate::default()
            })
            .expect("stored profile");

        let settings = store
            .update(&RuntimeSettingsUpdate {
                rag_budget_bytes: Some(777),
                ..RuntimeSettingsUpdate::default()
            })
            .expect("stored budget");

        assert_eq!(settings.profile.as_deref(), Some("minimal"));
        assert_eq!(settings.rag_budget_bytes, Some(777));
        assert_eq!(settings.memory_working_budget_bytes, None);
    }

    #[test]
    fn runtime_settings_allow_clearing_optional_model_paths() {
        let mut store = RuntimeSettingsStore::in_memory().expect("settings store");
        store
            .update(&RuntimeSettingsUpdate {
                self_heal_llama_cpp_binary: Some("C:/llama/llama-cli.exe".to_string()),
                self_heal_model_path: Some("C:/models/repair.gguf".to_string()),
                ..RuntimeSettingsUpdate::default()
            })
            .expect("stored model settings");

        let settings = store
            .update(&RuntimeSettingsUpdate {
                self_heal_llama_cpp_binary: Some(String::new()),
                self_heal_model_path: Some(String::new()),
                ..RuntimeSettingsUpdate::default()
            })
            .expect("cleared model settings");

        assert_eq!(settings.self_heal_llama_cpp_binary, None);
        assert_eq!(settings.self_heal_model_path, None);
    }
}

#[cfg(test)]
mod wal_checkpoint_tests {
    use super::{checkpoint_wal, configure_connection};
    use rusqlite::{params, Connection};

    #[test]
    fn checkpoint_wal_runs_clean_and_preserves_data() {
        // Unique temp dir per test process so parallel runs don't collide.
        let dir = std::env::temp_dir().join(format!("ordo-wal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db = dir.join("checkpoint.db");
        let _ = std::fs::remove_file(&db);

        // Populate in WAL mode. `configure_connection` sets journal_mode=WAL
        // and runs the migrations, which create `memory_records`.
        {
            let mut conn = Connection::open(&db).expect("open writer");
            configure_connection(&mut conn).expect("configure connection");
            for i in 0..200 {
                conn.execute(
                    "INSERT INTO memory_records (stored_at, content) VALUES (?1, ?2)",
                    params![format!("2026-01-01T00:00:{:02}Z", i % 60), "x".repeat(128)],
                )
                .expect("insert row");
            }
        } // writer connection dropped here, mirroring post-shutdown state

        // The shutdown path runs the checkpoint once the runtime's own
        // connections are gone, so an idle TRUNCATE must succeed cleanly.
        let stats = checkpoint_wal(&db).expect("checkpoint");
        assert_eq!(stats.busy, 0, "idle checkpoint must not be busy: {stats}");

        // If a WAL file remains it must be folded to zero bytes.
        let wal = db.with_file_name("checkpoint.db-wal");
        if wal.exists() {
            let len = std::fs::metadata(&wal).expect("wal metadata").len();
            assert_eq!(len, 0, "wal should be truncated to zero, was {len}");
        }

        // Data survives the checkpoint intact.
        let conn = Connection::open(&db).expect("reopen");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_records", [], |row| row.get(0))
            .expect("count rows");
        assert_eq!(count, 200);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
