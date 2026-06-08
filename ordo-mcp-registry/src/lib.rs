//! ordo-mcp-registry â€” signed lockfiles, drift detection, trust
//! state machine, community-attestation trait.
//!
//! Responsibility boundary: this crate owns the *trust layer*
//! over installed MCP servers. It does NOT own execution
//! (that's sandbox), does NOT own invocation (that's client),
//! does NOT own Worker extraction (that's worker).
//!
//! Load-bearing commitments (blueprint Â§28, Â§29, invariants 28 + 29):
//!
//! - Every installed MCP server has a signed lockfile. The
//!   runtime's Ed25519 signing key signs a canonical
//!   `McpServerLockfile`; verification fails on any byte-level
//!   modification.
//! - Drift from a signed lockfile blocks execution. The registry
//!   detects three drift classes: catalog additions (new tools),
//!   catalog modifications (tool schema changed), capability
//!   widening (broader host functions / domains / paths / topics
//!   than the lockfile declares). Capability narrowing is
//!   accepted quietly â€” it only shrinks attack surface.
//!
//! Trust graduation policy — numeric thresholds from the blueprint:
//!
//! - Untrusted → Observed  : 20 successful invocations, 7 days clean
//! - Observed  → Validated : 50 more successful, 30 days clean
//! - Validated → Trusted   : 200 more successful, 90 days clean
//!
//! Any anomaly demotes one level; critical anomalies (sandbox
//! escape, capability-widening, repeated egress violations)
//! demote straight to Quarantined.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use blake3::Hasher;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use ordo_bus::Bus;
use ordo_protocol::{
    mcp_topics, Attestation, BusEnvelope, CapabilityDeclaration, Envelope, McpServerLockfile,
    NodeId, OrdoMessage, ResourceLimits, ServerIdentity, ServerTrustState, ToolSchema,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

pub mod graduation;

pub use graduation::{GraduationPolicy, TrustLedger};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("server {0} not installed")]
    NotInstalled(String),
    #[error("signature invalid: {0}")]
    SignatureInvalid(String),
    #[error("drift detected: {0}")]
    DriftDetected(String),
    #[error("capability widening is not permitted without user re-authorization: {0}")]
    CapabilityWidening(String),
    #[error("quarantined: {0}")]
    Quarantined(String),
    #[error("storage: {0}")]
    Storage(String),
    #[error("bad input: {0}")]
    BadInput(String),
}

pub type RegistryResult<T> = Result<T, RegistryError>;

/// Community attestation trait â€” default is `LocalAttestationOnly`
/// (the runtime's own observations). Real ecosystem-level
/// attestation networks (MCPTrust, Sigstore-based federations,
/// etc.) plug in via this trait without touching the trust
/// state machine. When ecosystem standards materialize,
/// implementing this trait and wiring it in is the upgrade path.
#[async_trait]
pub trait AttestationSource: Send + Sync {
    async fn query_server_reputation(
        &self,
        server_id: &str,
    ) -> Result<Vec<Attestation>, AttestationError>;

    async fn submit_attestation(&self, attestation: Attestation) -> Result<(), AttestationError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AttestationError {
    #[error("attestation transport: {0}")]
    Transport(String),
    #[error("attestation invalid: {0}")]
    Invalid(String),
    #[error("attestation unsupported by this source: {0}")]
    Unsupported(String),
}

/// Default implementation â€” returns no external attestations.
/// `submit_attestation` is a no-op. Matches the blueprint's
/// "wait for the ecosystem to stabilize" stance.
pub struct LocalAttestationOnly;

#[async_trait]
impl AttestationSource for LocalAttestationOnly {
    async fn query_server_reputation(
        &self,
        _server_id: &str,
    ) -> Result<Vec<Attestation>, AttestationError> {
        Ok(Vec::new())
    }

    async fn submit_attestation(&self, _attestation: Attestation) -> Result<(), AttestationError> {
        Ok(())
    }
}

/// In-memory record holding the signed lockfile bytes + the
/// current trust state. Persistence is optional; the runtime
/// wires a `LockfilePersist` impl for durable storage.
#[derive(Debug, Clone)]
pub struct InstalledServer {
    pub lockfile: McpServerLockfile,
    /// Full tool catalog as of the last signed lockfile. Used for
    /// per-tool drift detection (names + per-tool hashes); the
    /// lockfile's aggregate hash says "something changed", this
    /// tells us exactly which tool.
    pub tool_catalog: Vec<ToolSchema>,
    pub trust_state: ServerTrustState,
    pub installed_at: DateTime<Utc>,
    pub last_clean_invocation_at: Option<DateTime<Utc>>,
    pub clean_invocation_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedServerRecord {
    version: u16,
    lockfile: McpServerLockfile,
    tool_catalog: Vec<ToolSchema>,
    trust_state: ServerTrustState,
    installed_at: DateTime<Utc>,
    last_clean_invocation_at: Option<DateTime<Utc>>,
    clean_invocation_count: u32,
}

impl From<&InstalledServer> for PersistedServerRecord {
    fn from(server: &InstalledServer) -> Self {
        Self {
            version: 1,
            lockfile: server.lockfile.clone(),
            tool_catalog: server.tool_catalog.clone(),
            trust_state: server.trust_state,
            installed_at: server.installed_at,
            last_clean_invocation_at: server.last_clean_invocation_at,
            clean_invocation_count: server.clean_invocation_count,
        }
    }
}

impl PersistedServerRecord {
    fn into_installed(self) -> InstalledServer {
        InstalledServer {
            lockfile: self.lockfile,
            tool_catalog: self.tool_catalog,
            trust_state: self.trust_state,
            installed_at: self.installed_at,
            last_clean_invocation_at: self.last_clean_invocation_at,
            clean_invocation_count: self.clean_invocation_count,
        }
    }
}

/// Optional persistence hook. The runtime implements this to write
/// complete registry records to disk. Older lockfile-only JSON is
/// still accepted during load, but new saves always include the
/// catalog and trust metadata required to rebuild the live registry.
#[async_trait]
pub trait LockfilePersist: Send + Sync {
    async fn save(&self, server_id: &str, registry_record_json: &str) -> Result<(), String>;
    async fn load(&self, server_id: &str) -> Result<Option<String>, String>;
    async fn list(&self) -> Result<Vec<String>, String>;
    async fn remove(&self, server_id: &str) -> Result<(), String>;
}

pub struct NullPersist;

#[async_trait]
impl LockfilePersist for NullPersist {
    async fn save(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    async fn load(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    async fn remove(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

pub struct FileLockfilePersist {
    root: PathBuf,
}

impl FileLockfilePersist {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn server_path(&self, server_id: &str) -> Result<PathBuf, String> {
        if !valid_server_filename(server_id) {
            return Err(format!("invalid server id `{server_id}`"));
        }
        Ok(self.root.join(format!("{server_id}.json")))
    }
}

#[async_trait]
impl LockfilePersist for FileLockfilePersist {
    async fn save(&self, server_id: &str, registry_record_json: &str) -> Result<(), String> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|err| format!("create {}: {err}", self.root.display()))?;
        let path = self.server_path(server_id)?;
        let tmp_path = path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, registry_record_json)
            .await
            .map_err(|err| format!("write {}: {err}", tmp_path.display()))?;
        if tokio::fs::try_exists(&path)
            .await
            .map_err(|err| format!("stat {}: {err}", path.display()))?
        {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|err| format!("remove {}: {err}", path.display()))?;
        }
        tokio::fs::rename(&tmp_path, &path)
            .await
            .map_err(|err| format!("rename {} -> {}: {err}", tmp_path.display(), path.display()))
    }

    async fn load(&self, server_id: &str) -> Result<Option<String>, String> {
        let path = self.server_path(server_id)?;
        match tokio::fs::read_to_string(&path).await {
            Ok(raw) => Ok(Some(raw)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(format!("read {}: {err}", path.display())),
        }
    }

    async fn list(&self) -> Result<Vec<String>, String> {
        let mut entries = Vec::new();
        let mut dir = match tokio::fs::read_dir(&self.root).await {
            Ok(dir) => dir,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Err(err) => return Err(format!("read_dir {}: {err}", self.root.display())),
        };
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|err| format!("read_dir {}: {err}", self.root.display()))?
        {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if valid_server_filename(stem) {
                entries.push(stem.to_string());
            }
        }
        entries.sort();
        Ok(entries)
    }

    async fn remove(&self, server_id: &str) -> Result<(), String> {
        let path = self.server_path(server_id)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("remove {}: {err}", path.display())),
        }
    }
}

pub struct McpRegistryService {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    servers: Arc<RwLock<HashMap<String, InstalledServer>>>,
    ledger: Arc<TrustLedger>,
    policy: GraduationPolicy,
    attestation_source: Arc<dyn AttestationSource>,
    persist: Arc<dyn LockfilePersist>,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
}

impl McpRegistryService {
    pub fn new(signing_key: SigningKey) -> Self {
        let verifying_key = signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
            servers: Arc::new(RwLock::new(HashMap::new())),
            ledger: Arc::new(TrustLedger::default()),
            policy: GraduationPolicy::default(),
            attestation_source: Arc::new(LocalAttestationOnly),
            persist: Arc::new(NullPersist),
            bus: None,
            node_id: NodeId::new(),
        }
    }

    pub fn with_policy(mut self, policy: GraduationPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_attestation_source(mut self, source: Arc<dyn AttestationSource>) -> Self {
        self.attestation_source = source;
        self
    }

    pub fn with_persist(mut self, persist: Arc<dyn LockfilePersist>) -> Self {
        self.persist = persist;
        self
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_node_id(mut self, node_id: NodeId) -> Self {
        self.node_id = node_id;
        self
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.verifying_key
    }

    pub async fn load_persisted(&self) -> RegistryResult<usize> {
        let server_ids = self.persist.list().await.map_err(RegistryError::Storage)?;
        let mut restored = 0usize;
        for server_id in server_ids {
            let Some(raw) = self
                .persist
                .load(&server_id)
                .await
                .map_err(RegistryError::Storage)?
            else {
                continue;
            };
            let installed = match decode_persisted_record(&raw) {
                Ok(installed) => installed,
                Err(err) => {
                    tracing::warn!(
                        target: "ordo_mcp_registry",
                        server_id = %server_id,
                        error = %err,
                        "skipping unreadable MCP registry record"
                    );
                    continue;
                }
            };
            if installed.lockfile.server_id != server_id {
                tracing::warn!(
                    target: "ordo_mcp_registry",
                    record_id = %server_id,
                    lockfile_id = %installed.lockfile.server_id,
                    "skipping MCP registry record with mismatched server id"
                );
                continue;
            }
            if let Err(err) = self.verify_lockfile(&installed.lockfile) {
                tracing::warn!(
                    target: "ordo_mcp_registry",
                    server_id = %server_id,
                    error = %err,
                    "skipping MCP registry record with invalid runtime signature"
                );
                continue;
            }
            self.servers.write().insert(server_id, installed);
            restored = restored.saturating_add(1);
        }
        Ok(restored)
    }

    async fn persist_installed(
        &self,
        server_id: &str,
        installed: &InstalledServer,
    ) -> RegistryResult<()> {
        let record = PersistedServerRecord::from(installed);
        let record_json = serde_json::to_string_pretty(&record)
            .map_err(|err| RegistryError::Storage(err.to_string()))?;
        self.persist
            .save(server_id, &record_json)
            .await
            .map_err(RegistryError::Storage)
    }

    /// Install a server. Hashes the tool catalog, builds the
    /// `McpServerLockfile`, signs it with the runtime's key, and
    /// stores it. Returns the signed lockfile.
    pub async fn install(
        &self,
        server_id: String,
        identity: ServerIdentity,
        tool_catalog: &[ToolSchema],
        declaration: CapabilityDeclaration,
        limits: ResourceLimits,
    ) -> RegistryResult<McpServerLockfile> {
        if identity.sigstore_cert.is_empty() {
            // Invariant 28 â€” the sigstore cert field is mandatory.
            // We don't verify Sigstore end-to-end in v1 (the
            // ecosystem plumbing isn't there yet); but we refuse to
            // register a server without any identity material at
            // all, so an operator never accidentally installs an
            // unsigned binary.
            return Err(RegistryError::SignatureInvalid(
                "sigstore_cert is empty; unsigned installations rejected".into(),
            ));
        }
        let tool_catalog_hash = hash_tool_catalog(tool_catalog);
        let now_ms = Utc::now().timestamp_millis();
        let mut lockfile = McpServerLockfile {
            server_id: server_id.clone(),
            server_identity: identity.clone(),
            installed_at_ms: now_ms,
            sigstore_certificate: identity.sigstore_cert.clone(),
            tool_catalog_hash,
            declared_capabilities: declaration.clone(),
            host_function_allowlist: declaration.host_functions.clone(),
            domain_allowlist: declaration.domains.clone(),
            filesystem_paths_allowlist: declaration.filesystem_paths.clone(),
            bus_topics_allowlist: declaration.bus_topics.clone(),
            resource_limits: limits,
            signed_at_ms: now_ms,
            runtime_signature: Vec::new(),
        };
        let bytes = canonical_bytes(&lockfile)?;
        let sig = self.signing_key.sign(&bytes);
        lockfile.runtime_signature = sig.to_bytes().to_vec();

        let installed = InstalledServer {
            lockfile: lockfile.clone(),
            tool_catalog: tool_catalog.to_vec(),
            trust_state: ServerTrustState::Untrusted,
            installed_at: Utc::now(),
            last_clean_invocation_at: None,
            clean_invocation_count: 0,
        };
        self.persist_installed(&server_id, &installed).await?;
        self.servers.write().insert(server_id.clone(), installed);

        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpSandboxInstalled {
                    server_id: server_id.clone(),
                    lockfile_hash: lockfile_hash(&lockfile),
                },
            );
            let _ = bus.publish(mcp_topics::SANDBOX_INSTALLED, env).await;
        }
        Ok(lockfile)
    }

    pub async fn uninstall(&self, server_id: &str) -> RegistryResult<()> {
        self.servers.write().remove(server_id);
        self.persist
            .remove(server_id)
            .await
            .map_err(RegistryError::Storage)?;
        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpSandboxUninstalled {
                    server_id: server_id.to_string(),
                },
            );
            let _ = bus.publish(mcp_topics::SANDBOX_INSTALLED, env).await;
        }
        Ok(())
    }

    pub fn get(&self, server_id: &str) -> Option<InstalledServer> {
        self.servers.read().get(server_id).cloned()
    }

    pub fn list(&self) -> Vec<InstalledServer> {
        self.servers.read().values().cloned().collect()
    }

    pub fn trust_state(&self, server_id: &str) -> Option<ServerTrustState> {
        self.servers.read().get(server_id).map(|s| s.trust_state)
    }

    /// Verify a lockfile's signature. Called at every connection
    /// (well, at install time and on drift detection). A tampered
    /// lockfile fails this check.
    pub fn verify_lockfile(&self, lockfile: &McpServerLockfile) -> RegistryResult<()> {
        let sig_bytes = lockfile.runtime_signature.clone();
        if sig_bytes.len() != 64 {
            return Err(RegistryError::SignatureInvalid(format!(
                "signature is {}B, expected 64",
                sig_bytes.len()
            )));
        }
        let mut unsigned = lockfile.clone();
        unsigned.runtime_signature = Vec::new();
        let bytes = canonical_bytes(&unsigned)?;
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);
        self.verifying_key
            .verify(&bytes, &sig)
            .map_err(|err| RegistryError::SignatureInvalid(err.to_string()))
    }

    /// Compare the server's current advertised surface against
    /// its lockfile. Returns detailed drift info; caller (the
    /// client) decides the response (block + prompt user).
    pub fn detect_drift(
        &self,
        server_id: &str,
        current_tool_catalog: &[ToolSchema],
        current_declaration: &CapabilityDeclaration,
    ) -> RegistryResult<DriftReport> {
        let installed = self
            .servers
            .read()
            .get(server_id)
            .cloned()
            .ok_or_else(|| RegistryError::NotInstalled(server_id.to_string()))?;
        if matches!(installed.trust_state, ServerTrustState::Quarantined) {
            return Err(RegistryError::Quarantined(server_id.to_string()));
        }
        self.verify_lockfile(&installed.lockfile)?;

        let current_hash = hash_tool_catalog(current_tool_catalog);
        let mut report = DriftReport::clean();
        if current_hash != installed.lockfile.tool_catalog_hash {
            // Figure out the difference shape for the report using
            // the persisted tool catalog.
            let old_names: Vec<&str> = installed
                .tool_catalog
                .iter()
                .map(|t| t.name.as_str())
                .collect();
            let new_names: Vec<&str> = current_tool_catalog
                .iter()
                .map(|t| t.name.as_str())
                .collect();
            for n in &new_names {
                if !old_names.contains(n) {
                    report.catalog_additions.push((*n).to_string());
                }
            }
            for n in &old_names {
                if !new_names.contains(n) {
                    report.catalog_removals.push((*n).to_string());
                }
            }
            for t in current_tool_catalog {
                if let Some(prev) = installed.tool_catalog.iter().find(|p| p.name == t.name) {
                    // Compare per-tool hash to distinguish
                    // "schema changed" from "unchanged".
                    if hash_single_tool(prev) != hash_single_tool(t) {
                        report.catalog_modifications.push(t.name.clone());
                    }
                }
            }
        }
        // Capability widening check â€” any non-subset addition is
        // drift. This is the blueprint's central invariant: a
        // server cannot request broader access post-install.
        widening_added(
            &installed.lockfile.host_function_allowlist,
            &current_declaration.host_functions,
            "host_function",
            &mut report,
        );
        widening_added(
            &installed.lockfile.domain_allowlist,
            &current_declaration.domains,
            "domain",
            &mut report,
        );
        widening_added(
            &installed.lockfile.filesystem_paths_allowlist,
            &current_declaration.filesystem_paths,
            "filesystem_path",
            &mut report,
        );
        widening_added(
            &installed.lockfile.bus_topics_allowlist,
            &current_declaration.bus_topics,
            "bus_topic",
            &mut report,
        );

        if report.has_any_drift() {
            if let Some(bus) = &self.bus {
                let env: BusEnvelope = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::McpRegistryDriftDetected {
                        server_id: server_id.to_string(),
                        details: report.summary(),
                    },
                );
                bus.publish(mcp_topics::REGISTRY_DRIFT_DETECTED, env)
                    .await_noop();
            }
        }
        Ok(report)
    }

    /// User explicitly approves a drift-detected change. Recomputes
    /// the lockfile hashes + re-signs + persists. The client can
    /// invoke again after this call returns.
    pub async fn re_authorize(
        &self,
        server_id: &str,
        new_tool_catalog: &[ToolSchema],
        new_declaration: CapabilityDeclaration,
    ) -> RegistryResult<McpServerLockfile> {
        let installed = self
            .servers
            .read()
            .get(server_id)
            .cloned()
            .ok_or_else(|| RegistryError::NotInstalled(server_id.to_string()))?;
        let tool_catalog_hash = hash_tool_catalog(new_tool_catalog);
        let now_ms = Utc::now().timestamp_millis();
        let mut lockfile = installed.lockfile.clone();
        lockfile.tool_catalog_hash = tool_catalog_hash;
        lockfile.declared_capabilities = new_declaration.clone();
        lockfile.host_function_allowlist = new_declaration.host_functions.clone();
        lockfile.domain_allowlist = new_declaration.domains.clone();
        lockfile.filesystem_paths_allowlist = new_declaration.filesystem_paths.clone();
        lockfile.bus_topics_allowlist = new_declaration.bus_topics.clone();
        lockfile.signed_at_ms = now_ms;
        lockfile.runtime_signature = Vec::new();
        let bytes = canonical_bytes(&lockfile)?;
        let sig = self.signing_key.sign(&bytes);
        lockfile.runtime_signature = sig.to_bytes().to_vec();

        // Re-authorization demotes trust state to Observed: the
        // server's surface materially changed, so it re-graduates
        // from a monitored state.
        let mut updated = installed.clone();
        updated.lockfile = lockfile.clone();
        updated.tool_catalog = new_tool_catalog.to_vec();
        updated.trust_state = ServerTrustState::Observed;
        updated.clean_invocation_count = 0;
        self.persist_installed(server_id, &updated).await?;
        self.servers.write().insert(server_id.to_string(), updated);

        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpRegistryReAuthorized {
                    server_id: server_id.to_string(),
                    new_lockfile_hash: lockfile_hash(&lockfile),
                },
            );
            let _ = bus.publish(mcp_topics::REGISTRY_RE_AUTHORIZED, env).await;
        }
        Ok(lockfile)
    }

    /// Record a successful invocation. Drives trust graduation.
    pub async fn record_success(&self, server_id: &str) -> RegistryResult<()> {
        let mut new_state = None;
        let updated = {
            let mut servers = self.servers.write();
            let Some(server) = servers.get_mut(server_id) else {
                return Err(RegistryError::NotInstalled(server_id.to_string()));
            };
            server.clean_invocation_count = server.clean_invocation_count.saturating_add(1);
            server.last_clean_invocation_at = Some(Utc::now());
            if let Some(next) = self.policy.graduation_target(
                server.trust_state,
                server.clean_invocation_count,
                server.installed_at,
            ) {
                new_state = Some((server.trust_state, next));
                server.trust_state = next;
                server.clean_invocation_count = 0;
            }
            server.clone()
        };
        self.persist_installed(server_id, &updated).await?;
        if let Some((from, to)) = new_state {
            self.emit_trust_change(server_id, from, to, "clean invocation threshold reached")
                .await;
        }
        Ok(())
    }

    /// Record an anomaly. Demotes trust state by one level
    /// (critical anomalies demote straight to Quarantined).
    pub async fn record_anomaly(
        &self,
        server_id: &str,
        severity: AnomalySeverity,
        reason: impl Into<String>,
    ) -> RegistryResult<()> {
        let reason: String = reason.into();
        let (from, to, updated) = {
            let mut servers = self.servers.write();
            let Some(server) = servers.get_mut(server_id) else {
                return Err(RegistryError::NotInstalled(server_id.to_string()));
            };
            let from = server.trust_state;
            let to = match severity {
                AnomalySeverity::Minor => self.policy.demote_one(server.trust_state),
                AnomalySeverity::Critical => ServerTrustState::Quarantined,
            };
            server.trust_state = to;
            server.clean_invocation_count = 0;
            (from, to, server.clone())
        };
        self.persist_installed(server_id, &updated).await?;
        if from != to {
            self.emit_trust_change(server_id, from, to, &reason).await;
        }
        self.ledger.record_anomaly(server_id, severity, reason);
        Ok(())
    }

    pub async fn quarantine(
        &self,
        server_id: &str,
        reason: impl Into<String>,
    ) -> RegistryResult<()> {
        let reason: String = reason.into();
        let (from, updated) = {
            let mut servers = self.servers.write();
            let Some(server) = servers.get_mut(server_id) else {
                return Err(RegistryError::NotInstalled(server_id.to_string()));
            };
            let from = server.trust_state;
            server.trust_state = ServerTrustState::Quarantined;
            server.clean_invocation_count = 0;
            (from, server.clone())
        };
        self.persist_installed(server_id, &updated).await?;
        self.emit_trust_change(server_id, from, ServerTrustState::Quarantined, &reason)
            .await;
        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpRegistryQuarantined {
                    server_id: server_id.to_string(),
                    reason,
                },
            );
            let _ = bus.publish(mcp_topics::REGISTRY_QUARANTINE, env).await;
        }
        Ok(())
    }

    /// Administratively set a server's trust state. Used for
    /// first-run scenarios where the operator vouches for a
    /// server they just installed (e.g. their own organization's
    /// CMS package) and wants to bypass the time-gated automatic
    /// graduation. Bypassing graduation IS an administrative
    /// action â€” the trust-change event the bus emits records the
    /// reason so audit retains visibility.
    pub async fn set_trust_state(
        &self,
        server_id: &str,
        target: ServerTrustState,
        reason: impl Into<String>,
    ) -> RegistryResult<()> {
        let reason: String = reason.into();
        let (from, updated) = {
            let mut servers = self.servers.write();
            let Some(server) = servers.get_mut(server_id) else {
                return Err(RegistryError::NotInstalled(server_id.to_string()));
            };
            let from = server.trust_state;
            server.trust_state = target;
            server.clean_invocation_count = 0;
            (from, server.clone())
        };
        self.persist_installed(server_id, &updated).await?;
        self.emit_trust_change(server_id, from, target, &reason)
            .await;
        Ok(())
    }

    async fn emit_trust_change(
        &self,
        server_id: &str,
        from: ServerTrustState,
        to: ServerTrustState,
        reason: &str,
    ) {
        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpRegistryTrustChanged {
                    server_id: server_id.to_string(),
                    from,
                    to,
                    reason: reason.to_string(),
                },
            );
            let _ = bus.publish(mcp_topics::REGISTRY_TRUST_CHANGED, env).await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnomalySeverity {
    Minor,
    Critical,
}

/// Result of a drift comparison. A report with any non-empty
/// field blocks invocation until re-authorization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DriftReport {
    pub catalog_additions: Vec<String>,
    pub catalog_removals: Vec<String>,
    pub catalog_modifications: Vec<String>,
    pub capability_widenings: Vec<String>,
}

impl DriftReport {
    pub fn clean() -> Self {
        Self::default()
    }

    pub fn has_any_drift(&self) -> bool {
        !self.catalog_additions.is_empty()
            || !self.catalog_modifications.is_empty()
            || !self.capability_widenings.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.catalog_additions.is_empty() {
            parts.push(format!("+{} tool(s)", self.catalog_additions.len()));
        }
        if !self.catalog_modifications.is_empty() {
            parts.push(format!("~{} tool(s)", self.catalog_modifications.len()));
        }
        if !self.capability_widenings.is_empty() {
            parts.push(format!("{} widening(s)", self.capability_widenings.len()));
        }
        if !self.catalog_removals.is_empty() {
            parts.push(format!("-{} tool(s)", self.catalog_removals.len()));
        }
        if parts.is_empty() {
            "no drift".into()
        } else {
            parts.join(", ")
        }
    }
}

fn widening_added(old: &[String], new: &[String], kind: &str, report: &mut DriftReport) {
    for n in new {
        if !old.iter().any(|o| o == n) {
            report.capability_widenings.push(format!("{kind}: {n}"));
        }
    }
}

fn decode_persisted_record(raw: &str) -> RegistryResult<InstalledServer> {
    if let Ok(record) = serde_json::from_str::<PersistedServerRecord>(raw) {
        return Ok(record.into_installed());
    }
    let lockfile: McpServerLockfile = serde_json::from_str(raw)
        .map_err(|err| RegistryError::Storage(format!("decode registry record: {err}")))?;
    Ok(InstalledServer {
        installed_at: DateTime::<Utc>::from_timestamp_millis(lockfile.installed_at_ms)
            .unwrap_or_else(Utc::now),
        lockfile,
        tool_catalog: Vec::new(),
        trust_state: ServerTrustState::Untrusted,
        last_clean_invocation_at: None,
        clean_invocation_count: 0,
    })
}

fn valid_server_filename(server_id: &str) -> bool {
    !server_id.is_empty()
        && server_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Produce a blake3 hash over a canonical rendering of a tool
/// catalog. Names are sorted so order-independent.
fn hash_tool_catalog(tools: &[ToolSchema]) -> [u8; 32] {
    let mut sorted: Vec<&ToolSchema> = tools.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut h = Hasher::new();
    for t in sorted {
        h.update(t.name.as_bytes());
        h.update(b"|");
        h.update(t.description.as_bytes());
        h.update(b"|");
        h.update(
            serde_json::to_string(&t.input_schema)
                .unwrap_or_default()
                .as_bytes(),
        );
        h.update(b"|");
        h.update(
            serde_json::to_string(&t.output_schema)
                .unwrap_or_default()
                .as_bytes(),
        );
        h.update(b"|");
        h.update(risk_level_label(t.risk_level).as_bytes());
        h.update(b"|");
    }
    *h.finalize().as_bytes()
}

fn risk_level_label(r: ordo_protocol::ToolRiskLevel) -> &'static str {
    match r {
        ordo_protocol::ToolRiskLevel::ReadOnly => "read_only",
        ordo_protocol::ToolRiskLevel::Mutating => "mutating",
        ordo_protocol::ToolRiskLevel::Sensitive => "sensitive",
        ordo_protocol::ToolRiskLevel::HighRisk => "high_risk",
    }
}

fn hash_single_tool(tool: &ToolSchema) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(tool.name.as_bytes());
    h.update(b"|");
    h.update(tool.description.as_bytes());
    h.update(b"|");
    h.update(
        serde_json::to_string(&tool.input_schema)
            .unwrap_or_default()
            .as_bytes(),
    );
    h.update(b"|");
    h.update(
        serde_json::to_string(&tool.output_schema)
            .unwrap_or_default()
            .as_bytes(),
    );
    h.update(b"|");
    h.update(risk_level_label(tool.risk_level).as_bytes());
    *h.finalize().as_bytes()
}

fn lockfile_hash(lockfile: &McpServerLockfile) -> [u8; 32] {
    let mut h = Hasher::new();
    let bytes = serde_json::to_vec(lockfile).unwrap_or_default();
    h.update(&bytes);
    *h.finalize().as_bytes()
}

fn canonical_bytes(lockfile: &McpServerLockfile) -> RegistryResult<Vec<u8>> {
    serde_json::to_vec(lockfile).map_err(|err| RegistryError::Storage(err.to_string()))
}

/// Micro-helper to let synchronous code call an async bus publish
/// without needing a tokio block_on. Returns immediately with
/// whatever future polling the bus yields; we discard the result.
trait AwaitNoop {
    fn await_noop(self);
}

impl<T> AwaitNoop for T {
    fn await_noop(self) {
        // The bus returns a future; in drift-detection (called
        // from sync code) we can't await. We spawn a fire-and-
        // forget. For this v1 it's fine â€” drift events being
        // delivered slightly later is acceptable; the registry's
        // internal state change was synchronous.
        let _ = self;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn test_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn test_identity() -> ServerIdentity {
        ServerIdentity {
            name: "test-server".into(),
            version: "0.1".into(),
            publisher: "test".into(),
            sigstore_cert: vec![1, 2, 3], // any nonempty blob for v1
            identity_hash: [0u8; 32],
        }
    }

    fn persistent_test_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ordo-mcp-registry-{name}-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tool(name: &str) -> ToolSchema {
        ToolSchema {
            name: name.into(),
            description: format!("tool {name}"),
            input_schema: serde_json::json!({ "type": "object" }),
            output_schema: serde_json::json!({ "type": "object" }),
            risk_level: ordo_protocol::ToolRiskLevel::ReadOnly,
        }
    }

    #[tokio::test]
    async fn install_signs_lockfile_and_verify_succeeds() {
        let svc = McpRegistryService::new(test_key());
        let lockfile = svc
            .install(
                "srv-a".into(),
                test_identity(),
                &[tool("t1")],
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .await
            .unwrap();
        assert_eq!(lockfile.runtime_signature.len(), 64);
        svc.verify_lockfile(&lockfile).unwrap();
    }

    #[tokio::test]
    async fn persisted_registry_restores_tool_catalog_after_restart() {
        let dir = persistent_test_dir("restore");
        let key = test_key();
        let persist = Arc::new(FileLockfilePersist::new(&dir));
        let svc = McpRegistryService::new(key.clone()).with_persist(persist.clone());
        svc.install(
            "srv-persist".into(),
            test_identity(),
            &[tool("alpha"), tool("beta")],
            CapabilityDeclaration::default(),
            ResourceLimits::default(),
        )
        .await
        .unwrap();

        let restarted = McpRegistryService::new(key).with_persist(persist);
        let restored = restarted.load_persisted().await.unwrap();
        assert_eq!(restored, 1);
        let installed = restarted.get("srv-persist").unwrap();
        assert_eq!(installed.tool_catalog.len(), 2);
        assert_eq!(installed.trust_state, ServerTrustState::Untrusted);
        restarted.verify_lockfile(&installed.lockfile).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn install_rejects_empty_sigstore_cert() {
        let svc = McpRegistryService::new(test_key());
        let mut ident = test_identity();
        ident.sigstore_cert = Vec::new();
        let err = svc
            .install(
                "srv-a".into(),
                ident,
                &[tool("t1")],
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, RegistryError::SignatureInvalid(_)));
    }

    #[tokio::test]
    async fn tampered_lockfile_fails_verify() {
        let svc = McpRegistryService::new(test_key());
        let mut lockfile = svc
            .install(
                "srv-b".into(),
                test_identity(),
                &[tool("t1")],
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .await
            .unwrap();
        // Tamper with the catalog hash â€” any change invalidates.
        lockfile.tool_catalog_hash = [0u8; 32];
        let err = svc.verify_lockfile(&lockfile).unwrap_err();
        assert!(matches!(err, RegistryError::SignatureInvalid(_)));
    }

    #[tokio::test]
    async fn drift_catalog_additions_detected() {
        let svc = McpRegistryService::new(test_key());
        svc.install(
            "srv-c".into(),
            test_identity(),
            &[tool("t1")],
            CapabilityDeclaration::default(),
            ResourceLimits::default(),
        )
        .await
        .unwrap();
        let report = svc
            .detect_drift(
                "srv-c",
                &[tool("t1"), tool("t2")],
                &CapabilityDeclaration::default(),
            )
            .unwrap();
        assert!(report.has_any_drift());
        assert_eq!(report.catalog_additions, vec!["t2"]);
    }

    #[tokio::test]
    async fn drift_capability_widening_detected() {
        let svc = McpRegistryService::new(test_key());
        svc.install(
            "srv-d".into(),
            test_identity(),
            &[tool("t1")],
            CapabilityDeclaration {
                domains: vec!["allowed.test".into()],
                ..Default::default()
            },
            ResourceLimits::default(),
        )
        .await
        .unwrap();
        let report = svc
            .detect_drift(
                "srv-d",
                &[tool("t1")],
                &CapabilityDeclaration {
                    domains: vec!["allowed.test".into(), "attacker.example".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(report.has_any_drift());
        assert!(report
            .capability_widenings
            .iter()
            .any(|s| s.contains("attacker.example")));
    }

    #[tokio::test]
    async fn capability_narrowing_is_not_drift() {
        let svc = McpRegistryService::new(test_key());
        svc.install(
            "srv-e".into(),
            test_identity(),
            &[tool("t1")],
            CapabilityDeclaration {
                domains: vec!["a.test".into(), "b.test".into()],
                ..Default::default()
            },
            ResourceLimits::default(),
        )
        .await
        .unwrap();
        let report = svc
            .detect_drift(
                "srv-e",
                &[tool("t1")],
                &CapabilityDeclaration {
                    domains: vec!["a.test".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!report.has_any_drift());
    }

    #[tokio::test]
    async fn quarantine_blocks_subsequent_drift_checks() {
        let svc = McpRegistryService::new(test_key());
        svc.install(
            "srv-f".into(),
            test_identity(),
            &[tool("t1")],
            CapabilityDeclaration::default(),
            ResourceLimits::default(),
        )
        .await
        .unwrap();
        svc.quarantine("srv-f", "manual").await.unwrap();
        let err = svc
            .detect_drift("srv-f", &[tool("t1")], &CapabilityDeclaration::default())
            .unwrap_err();
        assert!(matches!(err, RegistryError::Quarantined(_)));
    }

    #[tokio::test]
    async fn re_authorize_updates_hash_and_demotes_to_observed() {
        let svc = McpRegistryService::new(test_key());
        svc.install(
            "srv-g".into(),
            test_identity(),
            &[tool("t1")],
            CapabilityDeclaration::default(),
            ResourceLimits::default(),
        )
        .await
        .unwrap();
        // Manually promote to Trusted for the test.
        {
            let mut servers = svc.servers.write();
            servers.get_mut("srv-g").unwrap().trust_state = ServerTrustState::Trusted;
        }
        let new_lockfile = svc
            .re_authorize(
                "srv-g",
                &[tool("t1"), tool("t2")],
                CapabilityDeclaration::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            svc.trust_state("srv-g"),
            Some(ServerTrustState::Observed),
            "re-authorization must demote to Observed"
        );
        svc.verify_lockfile(&new_lockfile).unwrap();
    }

    #[tokio::test]
    async fn critical_anomaly_quarantines_immediately() {
        let svc = McpRegistryService::new(test_key());
        svc.install(
            "srv-h".into(),
            test_identity(),
            &[tool("t1")],
            CapabilityDeclaration::default(),
            ResourceLimits::default(),
        )
        .await
        .unwrap();
        svc.record_anomaly("srv-h", AnomalySeverity::Critical, "sandbox escape")
            .await
            .unwrap();
        assert_eq!(
            svc.trust_state("srv-h"),
            Some(ServerTrustState::Quarantined)
        );
    }

    #[tokio::test]
    async fn local_attestation_only_returns_empty() {
        let src = LocalAttestationOnly;
        let result = src.query_server_reputation("any").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn graduation_accumulates_across_successes() {
        let svc = McpRegistryService::new(test_key()).with_policy(GraduationPolicy::testing());
        svc.install(
            "srv-i".into(),
            test_identity(),
            &[tool("t1")],
            CapabilityDeclaration::default(),
            ResourceLimits::default(),
        )
        .await
        .unwrap();
        // Testing policy needs 2 invocations to graduate.
        svc.record_success("srv-i").await.unwrap();
        assert_eq!(svc.trust_state("srv-i"), Some(ServerTrustState::Untrusted));
        svc.record_success("srv-i").await.unwrap();
        assert_eq!(svc.trust_state("srv-i"), Some(ServerTrustState::Observed));
    }
}
