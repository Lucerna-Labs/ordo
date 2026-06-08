use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::Utc;
use futures::StreamExt;
use ordo_bus::Bus;
use ordo_models::{ModelClient, ModelRequest, StaticModelClient};
use ordo_protocol::{
    topics, Envelope, NodeId, OrdoMessage, SelfHealIncident, SelfHealPlan, SelfHealSource,
};
use ordo_store::{OrdoDatabase, StorageTask, StorageTaskError};
use rusqlite::{params, OptionalExtension};

type DynError = Box<dyn std::error::Error + Send + Sync>;

const SELF_HEAL_SKILL: &str = include_str!("../../docs/self-heal-skill.md");

#[derive(Debug, Clone, Copy)]
pub struct SelfHealHistoryBudget {
    pub max_bytes: usize,
}

impl Default for SelfHealHistoryBudget {
    fn default() -> Self {
        Self {
            max_bytes: 512 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
struct StoredHealCase {
    component: String,
    symptom: String,
    summary: String,
    why: String,
    actions: Vec<String>,
    occurrence_count: usize,
}

#[derive(Debug, Clone)]
pub struct SelfHealCaseSummary {
    pub fingerprint: String,
    pub component: String,
    pub symptom: String,
    pub summary: String,
    pub why: String,
    pub actions: Vec<String>,
    pub source: String,
    pub occurrence_count: usize,
    pub updated_at: String,
}

pub struct SelfHealStore {
    db: OrdoDatabase,
    budget: SelfHealHistoryBudget,
}

#[derive(Clone)]
pub struct SelfHealStorageTask {
    path: Option<PathBuf>,
    inner: StorageTask<SelfHealStore>,
}

impl SelfHealStore {
    pub fn in_memory() -> Self {
        Self::in_memory_with_budget(SelfHealHistoryBudget::default())
    }

    pub fn in_memory_with_budget(budget: SelfHealHistoryBudget) -> Self {
        Self {
            db: OrdoDatabase::in_memory().expect("open in-memory self-heal database"),
            budget,
        }
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        Self::open_with_budget(path, SelfHealHistoryBudget::default())
    }

    pub fn open_with_budget(
        path: impl Into<PathBuf>,
        budget: SelfHealHistoryBudget,
    ) -> Result<Self, DynError> {
        Ok(Self {
            db: OrdoDatabase::open(path)?,
            budget,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.db.path()
    }

    fn lookup_case(&self, fingerprint: &str) -> Result<Option<StoredHealCase>, DynError> {
        self.db
            .conn()
            .query_row(
                "
                SELECT component, symptom, summary, why, actions_json, occurrence_count
                FROM heal_cases
                WHERE fingerprint = ?1
                ",
                params![fingerprint],
                |row| {
                    let actions_json: String = row.get(4)?;
                    Ok(StoredHealCase {
                        component: row.get(0)?,
                        symptom: row.get(1)?,
                        summary: row.get(2)?,
                        why: row.get(3)?,
                        actions: serde_json::from_str(&actions_json).unwrap_or_default(),
                        occurrence_count: row.get::<_, i64>(5)? as usize,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn recent_component_cases(
        &self,
        component: &str,
        limit: usize,
    ) -> Result<Vec<StoredHealCase>, DynError> {
        let mut stmt = self.db.conn().prepare(
            "
            SELECT component, symptom, summary, why, actions_json, occurrence_count
            FROM heal_cases
            WHERE component = ?1
            ORDER BY updated_at DESC
            LIMIT ?2
            ",
        )?;
        let rows = stmt.query_map(params![component, limit as i64], |row| {
            let actions_json: String = row.get(4)?;
            Ok(StoredHealCase {
                component: row.get(0)?,
                symptom: row.get(1)?,
                summary: row.get(2)?,
                why: row.get(3)?,
                actions: serde_json::from_str(&actions_json).unwrap_or_default(),
                occurrence_count: row.get::<_, i64>(5)? as usize,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn list_cases(&self, limit: usize) -> Result<Vec<SelfHealCaseSummary>, DynError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut stmt = self.db.conn().prepare(
            "
            SELECT fingerprint, component, symptom, summary, why, actions_json, source, occurrence_count, updated_at
            FROM heal_cases
            ORDER BY updated_at DESC
            LIMIT ?1
            ",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let actions_json: String = row.get(5)?;
            Ok(SelfHealCaseSummary {
                fingerprint: row.get(0)?,
                component: row.get(1)?,
                symptom: row.get(2)?,
                summary: row.get(3)?,
                why: row.get(4)?,
                actions: serde_json::from_str(&actions_json).unwrap_or_default(),
                source: row.get(6)?,
                occurrence_count: row.get::<_, i64>(7)? as usize,
                updated_at: row.get(8)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn get_case(&self, fingerprint: &str) -> Result<Option<SelfHealCaseSummary>, DynError> {
        let mut stmt = self.db.conn().prepare(
            "
            SELECT fingerprint, component, symptom, summary, why, actions_json, source, occurrence_count, updated_at
            FROM heal_cases
            WHERE fingerprint = ?1
            LIMIT 1
            ",
        )?;
        let mut rows = stmt.query(params![fingerprint])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let actions_json: String = row.get(5)?;
        Ok(Some(SelfHealCaseSummary {
            fingerprint: row.get(0)?,
            component: row.get(1)?,
            symptom: row.get(2)?,
            summary: row.get(3)?,
            why: row.get(4)?,
            actions: serde_json::from_str(&actions_json).unwrap_or_default(),
            source: row.get(6)?,
            occurrence_count: row.get::<_, i64>(7)? as usize,
            updated_at: row.get(8)?,
        }))
    }

    pub fn forget_case(&mut self, fingerprint: &str) -> Result<bool, DynError> {
        let tx = self.db.conn_mut().transaction()?;
        tx.execute(
            "DELETE FROM heal_attempts WHERE fingerprint = ?1",
            params![fingerprint],
        )?;
        let deleted_cases = tx.execute(
            "DELETE FROM heal_cases WHERE fingerprint = ?1",
            params![fingerprint],
        )?;
        tx.commit()?;
        Ok(deleted_cases > 0)
    }

    pub fn record_plan(
        &mut self,
        incident: &SelfHealIncident,
        plan: &SelfHealPlan,
    ) -> Result<(), DynError> {
        let recorded_at = Utc::now().to_rfc3339();
        let actions_json = serde_json::to_string(&plan.actions)?;
        let size_bytes = estimate_attempt_size(incident, plan) as i64;

        self.db.conn_mut().execute(
            "
            INSERT INTO heal_cases (
                fingerprint,
                component,
                symptom,
                summary,
                why,
                actions_json,
                source,
                created_at,
                updated_at,
                last_incident_id,
                occurrence_count
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, 1)
            ON CONFLICT(fingerprint) DO UPDATE SET
                component = excluded.component,
                symptom = excluded.symptom,
                summary = excluded.summary,
                why = excluded.why,
                actions_json = excluded.actions_json,
                source = excluded.source,
                updated_at = excluded.updated_at,
                last_incident_id = excluded.last_incident_id,
                occurrence_count = heal_cases.occurrence_count + 1
            ",
            params![
                incident.fingerprint,
                incident.component,
                incident.symptom,
                plan.summary,
                plan.why,
                actions_json,
                source_to_str(plan.source),
                recorded_at,
                incident.incident_id.to_string(),
            ],
        )?;

        self.db.conn_mut().execute(
            "
            INSERT INTO heal_attempts (
                incident_id,
                fingerprint,
                component,
                symptom,
                summary,
                why,
                actions_json,
                source,
                recorded_at,
                size_bytes
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                incident.incident_id.to_string(),
                incident.fingerprint,
                incident.component,
                incident.symptom,
                plan.summary,
                plan.why,
                serde_json::to_string(&plan.actions)?,
                source_to_str(plan.source),
                recorded_at,
                size_bytes,
            ],
        )?;

        self.prune_history()?;
        Ok(())
    }

    fn history_bytes(&self) -> Result<i64, DynError> {
        let bytes = self.db.conn().query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM heal_attempts",
            [],
            |row| row.get(0),
        )?;
        Ok(bytes)
    }

    fn history_count(&self) -> Result<usize, DynError> {
        let count = self
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM heal_attempts", [], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(count as usize)
    }

    fn prune_history(&mut self) -> Result<(), DynError> {
        let budget = self.budget.max_bytes as i64;
        loop {
            let current = self.history_bytes()?;
            if current <= budget {
                break;
            }

            let deleted = self.db.conn_mut().execute(
                "
                DELETE FROM heal_attempts
                WHERE id IN (
                    SELECT attempts.id
                    FROM heal_attempts AS attempts
                    JOIN (
                        SELECT fingerprint, MAX(id) AS keep_id
                        FROM heal_attempts
                        GROUP BY fingerprint
                    ) AS keepers
                      ON attempts.fingerprint = keepers.fingerprint
                    WHERE attempts.id <> keepers.keep_id
                    ORDER BY attempts.recorded_at ASC, attempts.id ASC
                    LIMIT 1
                )
                ",
                [],
            )?;

            if deleted == 0 {
                break;
            }
        }

        Ok(())
    }
}

impl SelfHealStorageTask {
    pub fn from_store(store: SelfHealStore) -> Self {
        Self {
            path: store.path().map(PathBuf::from),
            inner: StorageTask::start("self-heal-store", store),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub async fn history_count(&self) -> Result<usize, StorageTaskError> {
        self.inner
            .call(|store| store.history_count().map_err(|err| err.to_string()))
            .await
    }

    pub(crate) async fn lookup_case(
        &self,
        fingerprint: String,
    ) -> Result<Option<StoredHealCase>, StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .lookup_case(&fingerprint)
                    .map_err(|err| err.to_string())
            })
            .await
    }

    pub(crate) async fn recent_component_cases(
        &self,
        component: String,
        limit: usize,
    ) -> Result<Vec<StoredHealCase>, StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .recent_component_cases(&component, limit)
                    .map_err(|err| err.to_string())
            })
            .await
    }

    pub async fn record_plan(
        &self,
        incident: SelfHealIncident,
        plan: SelfHealPlan,
    ) -> Result<(), StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .record_plan(&incident, &plan)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })
            .await
    }

    pub async fn list_cases(
        &self,
        limit: usize,
    ) -> Result<Vec<SelfHealCaseSummary>, StorageTaskError> {
        self.inner
            .call(move |store| store.list_cases(limit).map_err(|err| err.to_string()))
            .await
    }

    pub async fn get_case(
        &self,
        fingerprint: String,
    ) -> Result<Option<SelfHealCaseSummary>, StorageTaskError> {
        self.inner
            .call(move |store| store.get_case(&fingerprint).map_err(|err| err.to_string()))
            .await
    }

    pub async fn forget_case(&self, fingerprint: String) -> Result<bool, StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .forget_case(&fingerprint)
                    .map_err(|err| err.to_string())
            })
            .await
    }
}

pub struct SelfHealPeer {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
    store: SelfHealStorageTask,
    model: Option<Arc<dyn ModelClient>>,
}

impl SelfHealPeer {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self::with_store_and_model(bus, SelfHealStore::in_memory(), None)
    }

    pub fn with_store(bus: Arc<dyn Bus>, store: SelfHealStore) -> Self {
        Self::with_store_and_model(bus, store, None)
    }

    pub fn with_store_and_model(
        bus: Arc<dyn Bus>,
        store: SelfHealStore,
        model: Option<Arc<dyn ModelClient>>,
    ) -> Self {
        Self::with_storage_and_model(bus, SelfHealStorageTask::from_store(store), model)
    }

    pub fn with_storage_and_model(
        bus: Arc<dyn Bus>,
        store: SelfHealStorageTask,
        model: Option<Arc<dyn ModelClient>>,
    ) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
            store,
            model,
        }
    }

    pub async fn run(&mut self) -> Result<(), DynError> {
        let mut sub = self.bus.subscribe(topics::SELF_HEAL_REQUEST).await?;
        let node_id = self.node_id.clone();
        let bus = self.bus.clone();
        let backend = if self.model.is_some() {
            "llama.cpp"
        } else {
            "deterministic fallback"
        };
        let history_count = self.store.history_count().await.map_err(storage_error)?;

        match self.store.path() {
            Some(path) => println!(
                "[SelfHeal] Peer online with {} history attempt(s) at {} using {}",
                history_count,
                path.display(),
                backend
            ),
            None => println!(
                "[SelfHeal] Peer online with {} in-memory history attempt(s) using {}",
                history_count, backend
            ),
        }

        while let Some(envelope) = sub.next().await {
            let correlation_id = envelope.correlation_id.clone();
            if let OrdoMessage::SelfHealRequested { incident } = envelope.payload {
                let previous = self
                    .store
                    .lookup_case(incident.fingerprint.clone())
                    .await
                    .map_err(storage_error)?;
                let similar_cases = if previous.is_none() {
                    self.store
                        .recent_component_cases(incident.component.clone(), 3)
                        .await
                        .map_err(storage_error)?
                } else {
                    Vec::new()
                };
                let plan =
                    plan_incident(self.model.clone(), &incident, previous, similar_cases).await?;
                self.store
                    .record_plan(incident.clone(), plan.clone())
                    .await
                    .map_err(storage_error)?;

                let response = Envelope::new(
                    node_id.clone(),
                    OrdoMessage::SelfHealPlanned {
                        incident_id: incident.incident_id,
                        fingerprint: incident.fingerprint.clone(),
                        plan,
                    },
                );
                let response = match correlation_id {
                    Some(cid) => response.with_correlation(cid),
                    None => response,
                };
                let _ = bus.publish(topics::SELF_HEAL_RESPONSE, response).await;
            }
        }

        Ok(())
    }
}

fn storage_error(error: StorageTaskError) -> DynError {
    Box::new(std::io::Error::other(error.to_string()))
}

async fn plan_incident(
    model: Option<Arc<dyn ModelClient>>,
    incident: &SelfHealIncident,
    previous: Option<StoredHealCase>,
    similar_cases: Vec<StoredHealCase>,
) -> Result<SelfHealPlan, DynError> {
    if let Some(previous) = previous {
        return Ok(SelfHealPlan {
            summary: format!("Reapply known fix for {}", incident.component),
            why: format!(
                "This fingerprint has already been repaired {} time(s). Reusing the last successful fix instead of re-planning from scratch.",
                previous.occurrence_count
            ),
            actions: previous.actions,
            source: SelfHealSource::MemoryReuse,
            reused_previous_fix: true,
            memory_hits: 1,
        });
    }

    if let Some(model) = model {
        let prompt = build_model_prompt(incident, &similar_cases);
        match model
            .complete(ModelRequest { prompt })
            .await
            .and_then(|response| {
                parse_model_plan(&response.text, similar_cases.len())
                    .ok_or_else(|| "self-heal model returned an unparsable response".into())
            }) {
            Ok(plan) => return Ok(plan),
            Err(err) => {
                eprintln!(
                    "[SelfHeal] model planning failed for {}: {}",
                    incident.fingerprint, err
                );
            }
        }
    }

    Ok(fallback_plan(incident, similar_cases.len()))
}

fn build_model_prompt(incident: &SelfHealIncident, similar_cases: &[StoredHealCase]) -> String {
    let similar_summary = if similar_cases.is_empty() {
        "No similar repair cases were stored yet.".to_string()
    } else {
        similar_cases
            .iter()
            .map(|case| {
                format!(
                    "- component={} symptom={} summary={} why={} actions={}",
                    case.component,
                    case.symptom,
                    case.summary,
                    case.why,
                    case.actions.join(" | ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "{SELF_HEAL_SKILL}\n\nReturn only lines in this exact format:\nSUMMARY: <single-line summary>\nWHY: <single-line reason>\nACTION: <first action>\nACTION: <second action>\nACTION: <third action>\n\nIncident:\n- component: {}\n- symptom: {}\n- fingerprint: {}\n- urgency: {:?}\n- logs: {}\n\nSimilar remembered repairs:\n{}",
        incident.component,
        incident.symptom,
        incident.fingerprint,
        incident.urgency,
        incident.logs.join(" | "),
        similar_summary
    )
}

fn parse_model_plan(response: &str, memory_hits: usize) -> Option<SelfHealPlan> {
    let mut summary = None;
    let mut why = None;
    let mut actions = Vec::new();

    for line in response.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("SUMMARY:") {
            summary = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("WHY:") {
            why = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("ACTION:") {
            let action = value.trim();
            if !action.is_empty() {
                actions.push(action.to_string());
            }
        }
    }

    let summary = summary?;
    let why = why?;
    if actions.is_empty() {
        return None;
    }

    Some(SelfHealPlan {
        summary,
        why,
        actions,
        source: SelfHealSource::LlamaCpp,
        reused_previous_fix: false,
        memory_hits,
    })
}

fn fallback_plan(incident: &SelfHealIncident, memory_hits: usize) -> SelfHealPlan {
    let lowered = format!(
        "{} {} {}",
        incident.component,
        incident.symptom,
        incident.logs.join(" ")
    )
    .to_ascii_lowercase();

    let (summary, why, actions) = if lowered.contains("path")
        || lowered.contains("root")
        || lowered.contains("filesystem")
        || lowered.contains("escape")
    {
        (
            format!("Repair filesystem root handling in {}", incident.component),
            "The platform keeps runtime state and user files separated, so self-heal should verify path normalization and rooted access before asking the user to debug it manually.".to_string(),
            vec![
                "Confirm the configured user-files root exists and is mounted where the runtime expects it.".to_string(),
                "Normalize the requested path and reject attempts that escape the configured root.".to_string(),
                "Replay the failing read or write against a known-good path under user-files to verify the repair.".to_string(),
            ],
        )
    } else if lowered.contains("sqlite") || lowered.contains("database") || lowered.contains("wal")
    {
        (
            "Repair local databank connectivity".to_string(),
            "Ordo is local-first, so self-heal should prefer restoring the embedded datastore path and journal mode before suggesting a larger migration.".to_string(),
            vec![
                "Verify the SQLite path is writable and the parent directory exists.".to_string(),
                "Check for stale WAL or SHM artifacts only after confirming the runtime is stopped cleanly.".to_string(),
                "Retry the last failing store operation and record the result for future recurrence matching.".to_string(),
            ],
        )
    } else if lowered.contains("llama")
        || lowered.contains("model")
        || lowered.contains("embedding")
    {
        (
            "Repair local model adapter configuration".to_string(),
            "The self-heal lane depends on a user-supplied local model, so the safest first move is to verify the binary path, model path, and fallback behavior rather than assuming the model is broken.".to_string(),
            vec![
                "Check that the configured llama.cpp binary exists and is executable.".to_string(),
                "Check that the selected model file exists and matches the expected format for the local engine.".to_string(),
                "If the local model is unavailable, fall back to deterministic repair guidance and keep the incident fingerprint for later reuse.".to_string(),
            ],
        )
    } else if lowered.contains("transport") || lowered.contains("relay") || lowered.contains("quic")
    {
        (
            "Repair transport fallback path".to_string(),
            "The routing model prefers direct links and then relay fallback, so self-heal should confirm that the selected transport and fallback policy still match the peer's capabilities.".to_string(),
            vec![
                "Inspect the last route directive and verify the chosen peer still advertises the required capability.".to_string(),
                "Retry the session using relay fallback if direct transport no longer matches the NAT or handshake constraints.".to_string(),
                "Record the successful transport path so repeated incidents can skip rediscovery next time.".to_string(),
            ],
        )
    } else {
        (
            format!("Stabilize {}", incident.component),
            "Self-heal should start from the last visible failure, preserve the local-first runtime shape, and record the successful fix so the next identical incident can be handled faster.".to_string(),
            vec![
                "Capture the smallest reproducible symptom for the failing component.".to_string(),
                "Apply the least disruptive repair that keeps the current runtime contracts intact.".to_string(),
                "Record the outcome under this incident fingerprint for future automatic reuse.".to_string(),
            ],
        )
    };

    SelfHealPlan {
        summary,
        why,
        actions,
        source: SelfHealSource::DeterministicFallback,
        reused_previous_fix: false,
        memory_hits,
    }
}

fn estimate_attempt_size(incident: &SelfHealIncident, plan: &SelfHealPlan) -> usize {
    let actions_bytes = plan
        .actions
        .iter()
        .map(|action| action.len())
        .sum::<usize>();
    let logs_bytes = incident.logs.iter().map(|line| line.len()).sum::<usize>();
    incident.component.len()
        + incident.symptom.len()
        + incident.fingerprint.len()
        + plan.summary.len()
        + plan.why.len()
        + actions_bytes
        + logs_bytes
        + 64
}

fn source_to_str(source: SelfHealSource) -> &'static str {
    match source {
        SelfHealSource::MemoryReuse => "memory_reuse",
        SelfHealSource::LlamaCpp => "llama_cpp",
        SelfHealSource::DeterministicFallback => "deterministic_fallback",
    }
}

pub fn default_self_heal_model() -> Arc<dyn ModelClient> {
    Arc::new(StaticModelClient::new(
        "SUMMARY: Reconcile runtime state paths\nWHY: Local-first services should repair mounts and configuration before escalating to a reinstall.\nACTION: Verify the configured paths exist.\nACTION: Retry the failing operation against the local runtime.\nACTION: Record the verified fix under the incident fingerprint.",
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ordo_bus::InProcessBus;
    use ordo_models::StaticModelClient;
    use uuid::Uuid;

    use super::{
        parse_model_plan, plan_incident, SelfHealHistoryBudget, SelfHealPeer, SelfHealStore,
    };
    use crate::fallback_plan;
    use ordo_protocol::{SelfHealIncident, SelfHealSource, SelfHealUrgency};

    fn sample_incident() -> SelfHealIncident {
        SelfHealIncident {
            incident_id: Uuid::new_v4(),
            component: "ordo-mcp-host/filesystem".to_string(),
            symptom: "filesystem.read_file failed because the requested path escaped the root"
                .to_string(),
            fingerprint: "filesystem-root-escape".to_string(),
            urgency: SelfHealUrgency::Medium,
            logs: vec!["path '../secret.txt' escapes configured root".to_string()],
        }
    }

    #[test]
    fn parses_structured_model_output() {
        let plan = parse_model_plan(
            "SUMMARY: Fix root\nWHY: Rooted access failed\nACTION: Check root\nACTION: Retry read",
            2,
        )
        .expect("parsed plan");

        assert_eq!(plan.source, SelfHealSource::LlamaCpp);
        assert_eq!(plan.memory_hits, 2);
        assert_eq!(plan.actions.len(), 2);
    }

    #[test]
    fn fallback_plan_matches_filesystem_incident() {
        let plan = fallback_plan(&sample_incident(), 0);
        assert_eq!(plan.source, SelfHealSource::DeterministicFallback);
        assert!(plan.summary.contains("filesystem"));
        assert_eq!(plan.actions.len(), 3);
    }

    #[tokio::test]
    async fn repeated_incident_reuses_previous_fix() {
        let bus = Arc::new(InProcessBus::new());
        let store = SelfHealStore::in_memory();
        let model = Arc::new(StaticModelClient::new(
            "SUMMARY: Repair root\nWHY: Repeat path validation\nACTION: Check root\nACTION: Retry read\nACTION: Save fix",
        ));
        let peer = SelfHealPeer::with_store_and_model(bus, store, Some(model.clone()));
        let first_incident = sample_incident();

        let first_plan = plan_incident(
            Some(model),
            &first_incident,
            None::<super::StoredHealCase>,
            Vec::new(),
        )
        .await
        .expect("first plan");
        peer.store
            .record_plan(first_incident.clone(), first_plan.clone())
            .await
            .expect("record first plan");

        let second_plan = plan_incident(
            peer.model.clone(),
            &SelfHealIncident {
                incident_id: Uuid::new_v4(),
                ..first_incident.clone()
            },
            peer.store
                .lookup_case("filesystem-root-escape".to_string())
                .await
                .expect("lookup case"),
            Vec::new(),
        )
        .await
        .expect("second plan");

        assert_eq!(first_plan.source, SelfHealSource::LlamaCpp);
        assert_eq!(second_plan.source, SelfHealSource::MemoryReuse);
        assert!(second_plan.reused_previous_fix);
        assert!(second_plan.why.contains("Reusing the last successful fix"));
        assert_eq!(
            second_plan
                .why
                .matches("This fingerprint has already been repaired")
                .count(),
            1
        );
    }

    #[test]
    fn history_budget_prunes_old_attempts_but_keeps_latest_case() {
        let mut store =
            SelfHealStore::in_memory_with_budget(SelfHealHistoryBudget { max_bytes: 350 });
        let base_plan = parse_model_plan(
            "SUMMARY: Repair root\nWHY: Root mismatch\nACTION: Check root\nACTION: Retry read\nACTION: Save fix",
            0,
        )
        .expect("base plan");

        for index in 0..4 {
            let incident = SelfHealIncident {
                incident_id: Uuid::new_v4(),
                component: "ordo-mcp-host/filesystem".to_string(),
                symptom: format!("path escape occurrence {}", index),
                fingerprint: "filesystem-root-escape".to_string(),
                urgency: SelfHealUrgency::Medium,
                logs: vec![format!("log {}", index)],
            };
            store
                .record_plan(&incident, &base_plan)
                .expect("record plan");
        }

        assert!(store.history_count().expect("history count") < 4);
        let case = store
            .lookup_case("filesystem-root-escape")
            .expect("lookup case")
            .expect("stored case");
        assert!(case.occurrence_count >= 4);
    }

    #[test]
    fn list_cases_and_forget_case_manage_history() {
        let mut store = SelfHealStore::in_memory();
        let incident = sample_incident();
        let plan = fallback_plan(&incident, 0);
        store.record_plan(&incident, &plan).expect("record plan");

        let cases = store.list_cases(5).expect("list cases");
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].fingerprint, "filesystem-root-escape");
        assert_eq!(cases[0].source, "deterministic_fallback");
        assert_eq!(cases[0].actions.len(), 3);

        let removed = store
            .forget_case("filesystem-root-escape")
            .expect("forget case");
        assert!(removed);
        assert!(store.list_cases(5).expect("list after forget").is_empty());
    }

    #[test]
    fn get_case_returns_public_summary() {
        let mut store = SelfHealStore::in_memory();
        let incident = sample_incident();
        let plan = fallback_plan(&incident, 0);
        store.record_plan(&incident, &plan).expect("record plan");

        let case = store
            .get_case("filesystem-root-escape")
            .expect("get case")
            .expect("stored case");

        assert_eq!(case.fingerprint, "filesystem-root-escape");
        assert_eq!(case.summary, plan.summary);
        assert_eq!(case.actions, plan.actions);
    }
}
