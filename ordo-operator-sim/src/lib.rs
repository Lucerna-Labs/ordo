use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Passed,
    Warning,
    Failed,
    Skipped,
}

impl StepStatus {
    fn icon(self) -> &'static str {
        match self {
            StepStatus::Passed => "OK",
            StepStatus::Warning => "WARN",
            StepStatus::Failed => "FAIL",
            StepStatus::Skipped => "SKIP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportVerdict {
    Healthy,
    Warning,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimConfig {
    pub origin: String,
    pub output_dir: PathBuf,
    pub profile: String,
    pub include_voice: bool,
    pub strict: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            origin: "http://127.0.0.1:4141".to_string(),
            output_dir: PathBuf::from("target/operator-sim"),
            profile: "baseline".to_string(),
            include_voice: false,
            strict: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReport {
    pub name: String,
    pub status: StepStatus,
    pub duration_ms: u128,
    pub summary: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationReport {
    pub origin: String,
    pub profile: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub verdict: ReportVerdict,
    pub steps: Vec<StepReport>,
}

impl SimulationReport {
    pub fn from_steps(
        origin: String,
        profile: String,
        started_at: DateTime<Utc>,
        steps: Vec<StepReport>,
        strict: bool,
    ) -> Self {
        let finished_at = Utc::now();
        let verdict = verdict_for(&steps, strict);
        Self {
            origin,
            profile,
            started_at,
            finished_at,
            verdict,
            steps,
        }
    }

    pub fn write_json(&self, output_dir: &Path) -> Result<PathBuf, SimError> {
        fs::create_dir_all(output_dir)?;
        let path = output_dir.join("operator-sim-report.json");
        let body = serde_json::to_string_pretty(self)?;
        fs::write(&path, body)?;
        Ok(path)
    }

    pub fn write_markdown(&self, output_dir: &Path) -> Result<PathBuf, SimError> {
        fs::create_dir_all(output_dir)?;
        let path = output_dir.join("operator-sim-report.md");
        fs::write(&path, self.to_markdown())?;
        Ok(path)
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Ordo Operator Simulator Report\n\n");
        out.push_str(&format!("**Verdict:** {:?}\n\n", self.verdict));
        out.push_str(&format!("**Origin:** `{}`\n\n", self.origin));
        out.push_str(&format!("**Profile:** `{}`\n\n", self.profile));
        out.push_str(&format!("**Started:** `{}`\n\n", self.started_at));
        out.push_str(&format!("**Finished:** `{}`\n\n", self.finished_at));
        out.push_str("| Step | Status | Duration | Summary |\n");
        out.push_str("|---|---:|---:|---|\n");
        for step in &self.steps {
            out.push_str(&format!(
                "| {} | {} | {} ms | {} |\n",
                escape_cell(&step.name),
                step.status.icon(),
                step.duration_ms,
                escape_cell(&step.summary)
            ));
        }
        out.push_str("\n## Details\n\n");
        for step in &self.steps {
            out.push_str(&format!("### {} ({})\n\n", step.name, step.status.icon()));
            out.push_str(&format!("{}\n\n", step.summary));
            if !step.detail.is_null() {
                out.push_str("```json\n");
                out.push_str(
                    &serde_json::to_string_pretty(&step.detail)
                        .unwrap_or_else(|_| "{}".to_string()),
                );
                out.push_str("\n```\n\n");
            }
        }
        out
    }
}

pub fn verdict_for(steps: &[StepReport], strict: bool) -> ReportVerdict {
    if steps.iter().any(|step| step.status == StepStatus::Failed) {
        return ReportVerdict::Failed;
    }
    if steps.iter().any(|step| step.status == StepStatus::Warning) {
        if strict {
            ReportVerdict::Failed
        } else {
            ReportVerdict::Warning
        }
    } else {
        ReportVerdict::Healthy
    }
}

pub async fn run_operator_sim(config: SimConfig) -> Result<SimulationReport, SimError> {
    let client = ControlClient::new(config.origin.clone())?;
    let started_at = Utc::now();
    let mut runner = ScenarioRunner::new(client);

    runner.health().await;
    runner.runtime_profile().await;
    runner.runtime_storage().await;
    runner.capabilities().await;
    runner.modes().await;
    runner.sessions().await;
    runner.maintenance_surfaces().await;
    runner.file_and_app_surfaces().await;
    runner.create_session_and_turn().await;
    if config.include_voice {
        runner.voice_speech().await;
    } else {
        runner.skip("Voice speech", "Skipped because --voice was not supplied");
    }

    Ok(SimulationReport::from_steps(
        config.origin,
        config.profile,
        started_at,
        runner.steps,
        config.strict,
    ))
}

struct ScenarioRunner {
    client: ControlClient,
    steps: Vec<StepReport>,
    session_id: Option<String>,
}

impl ScenarioRunner {
    fn new(client: ControlClient) -> Self {
        Self {
            client,
            steps: Vec::new(),
            session_id: None,
        }
    }

    async fn health(&mut self) {
        self.get_required("Health", "/health", |value| {
            let ok = value
                .get("ok")
                .and_then(Value::as_bool)
                .or_else(|| value.get("healthy").and_then(Value::as_bool))
                .unwrap_or(true);
            if ok {
                StepOutcome::passed("Control API is healthy", value)
            } else {
                StepOutcome::failed("Control API responded but did not report healthy", value)
            }
        })
        .await;
    }

    async fn runtime_profile(&mut self) {
        self.get_required("Runtime profile", "/api/runtime/profile", |value| {
            let profile = value
                .get("profile")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            StepOutcome::passed(format!("Runtime profile is {profile}"), value)
        })
        .await;
    }

    async fn runtime_storage(&mut self) {
        self.get_required("Runtime storage", "/api/runtime/storage", |value| {
            StepOutcome::passed("Storage budgets are readable", value)
        })
        .await;
    }

    async fn capabilities(&mut self) {
        self.get_required("Capability inventory", "/api/capabilities", |value| {
            let count = count_array(&value, "descriptors").unwrap_or(0);
            if count == 0 {
                StepOutcome::warning("Capability inventory is reachable but empty", value)
            } else {
                StepOutcome::passed(format!("{count} capabilities visible"), value)
            }
        })
        .await;
    }

    async fn modes(&mut self) {
        self.get_required("Mode inventory", "/api/assistant/modes", |value| {
            let count = count_array(&value, "modes").unwrap_or(0);
            if count == 0 {
                StepOutcome::failed("No assistant modes are registered", value)
            } else {
                StepOutcome::passed(format!("{count} modes visible"), value)
            }
        })
        .await;
    }

    async fn sessions(&mut self) {
        self.get_required(
            "Existing sessions",
            "/api/assistant/sessions?limit=20",
            |value| {
                let count = value.get("count").and_then(Value::as_u64).unwrap_or(0);
                StepOutcome::passed(format!("{count} recent sessions readable"), value)
            },
        )
        .await;
    }

    async fn maintenance_surfaces(&mut self) {
        let surfaces = [
            ("MCP servers", "/api/mcp/servers", "servers"),
            ("Plugins", "/api/plugins", "plugins"),
            ("Skills", "/api/tools/skills.list", ""),
            ("Automations", "/api/automations", "automations"),
            ("Security audit", "/api/security/audit?limit=10", "entries"),
            ("Review queue", "/api/review/pending", "pending"),
        ];
        for (name, path, array_key) in surfaces {
            if path.starts_with("/api/tools/") {
                self.post_optional(name, path, json!({}), array_key).await;
            } else {
                self.get_optional(name, path, array_key).await;
            }
        }
    }

    async fn file_and_app_surfaces(&mut self) {
        self.get_optional("Files", "/api/files?workspace_id=default", "files")
            .await;
        self.get_optional("Apps", "/api/apps?workspace_id=default", "apps")
            .await;
        self.get_optional("Connections", "/api/connections/types", "types")
            .await;
    }

    async fn create_session_and_turn(&mut self) {
        let title = format!(
            "Operator simulator {}",
            Utc::now().format("%Y-%m-%d %H:%M:%S")
        );
        let start = Instant::now();
        let created = self
            .client
            .post_json(
                "/api/tools/assistant.new_session",
                &json!({
                    "title": title,
                    "mode": "general",
                }),
            )
            .await;
        match created {
            Ok(value) => {
                let session_id = value.get("id").and_then(Value::as_str).map(str::to_string);
                if let Some(id) = session_id {
                    self.session_id = Some(id.clone());
                    self.push(
                        "Create chat session",
                        start,
                        StepOutcome::passed(format!("Created session {id}"), value),
                    );
                    self.assistant_turn(id).await;
                } else {
                    self.push(
                        "Create chat session",
                        start,
                        StepOutcome::failed(
                            "Session creation response did not include an id",
                            value,
                        ),
                    );
                }
            }
            Err(err) => {
                self.push(
                    "Create chat session",
                    start,
                    StepOutcome::failed(
                        format!("Could not create assistant session: {err}"),
                        Value::Null,
                    ),
                );
            }
        }
    }

    async fn assistant_turn(&mut self, session_id: String) {
        let start = Instant::now();
        let result = self
            .client
            .post_json(
                "/api/assistant/turn",
                &json!({
                    "session_id": session_id,
                    "user_message": "Operator simulator smoke check. Reply with one short sentence.",
                    "use_rag": false,
                    "use_memory": false,
                    "use_tools": false,
                    "stream": false,
                    "history_window": 4,
                    "metadata": {
                        "source": "ordo-operator-sim",
                        "scenario": "baseline"
                    }
                }),
            )
            .await;
        match result {
            Ok(value) => {
                let response = value
                    .pointer("/turn/assistant_response")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if response.trim().is_empty() {
                    self.push(
                        "Assistant turn",
                        start,
                        StepOutcome::warning(
                            "Turn completed but produced no visible response",
                            value,
                        ),
                    );
                } else {
                    self.push(
                        "Assistant turn",
                        start,
                        StepOutcome::passed("Assistant returned visible content", value),
                    );
                }
            }
            Err(err) if err.is_expected_provider_gap() => self.push(
                "Assistant turn",
                start,
                StepOutcome::warning(
                    format!("Provider is not ready for chat turns: {err}"),
                    Value::Null,
                ),
            ),
            Err(err) => self.push(
                "Assistant turn",
                start,
                StepOutcome::failed(format!("Assistant turn failed: {err}"), Value::Null),
            ),
        }
    }

    async fn voice_speech(&mut self) {
        let start = Instant::now();
        let result = self
            .client
            .post_bytes(
                "/api/voice/speech",
                &json!({
                    "input": "Ordo operator simulator voice check.",
                    "format": "mp3",
                }),
            )
            .await;
        match result {
            Ok(bytes) if bytes.is_empty() => self.push(
                "Voice speech",
                start,
                StepOutcome::warning("Voice endpoint returned an empty audio body", Value::Null),
            ),
            Ok(bytes) => self.push(
                "Voice speech",
                start,
                StepOutcome::passed(
                    format!("Voice endpoint returned {} bytes of audio", bytes.len()),
                    json!({ "bytes": bytes.len() }),
                ),
            ),
            Err(err) if err.is_expected_provider_gap() => self.push(
                "Voice speech",
                start,
                StepOutcome::warning(format!("Voice provider is not ready: {err}"), Value::Null),
            ),
            Err(err) => self.push(
                "Voice speech",
                start,
                StepOutcome::failed(format!("Voice endpoint failed: {err}"), Value::Null),
            ),
        }
    }

    async fn get_required<F>(&mut self, name: &str, path: &str, classify: F)
    where
        F: FnOnce(Value) -> StepOutcome,
    {
        let start = Instant::now();
        let outcome = match self.client.get_json(path).await {
            Ok(value) => classify(value),
            Err(err) => StepOutcome::failed(format!("{name} failed: {err}"), Value::Null),
        };
        self.push(name, start, outcome);
    }

    async fn get_optional(&mut self, name: &str, path: &str, array_key: &str) {
        let start = Instant::now();
        let outcome = match self.client.get_json(path).await {
            Ok(value) => summarize_optional_inventory(name, value, array_key),
            Err(err) if err.status == Some(StatusCode::NOT_FOUND) => StepOutcome::warning(
                format!("{name} endpoint is not available in this build"),
                Value::Null,
            ),
            Err(err) => {
                StepOutcome::warning(format!("{name} could not be read: {err}"), Value::Null)
            }
        };
        self.push(name, start, outcome);
    }

    async fn post_optional(&mut self, name: &str, path: &str, body: Value, array_key: &str) {
        let start = Instant::now();
        let outcome = match self.client.post_json(path, &body).await {
            Ok(value) => summarize_optional_inventory(name, value, array_key),
            Err(err) if err.status == Some(StatusCode::NOT_FOUND) => StepOutcome::warning(
                format!("{name} tool is not exposed in this build"),
                Value::Null,
            ),
            Err(err) => {
                StepOutcome::warning(format!("{name} could not be read: {err}"), Value::Null)
            }
        };
        self.push(name, start, outcome);
    }

    fn skip(&mut self, name: &str, summary: &str) {
        self.steps.push(StepReport {
            name: name.to_string(),
            status: StepStatus::Skipped,
            duration_ms: 0,
            summary: summary.to_string(),
            detail: Value::Null,
        });
    }

    fn push(&mut self, name: &str, start: Instant, outcome: StepOutcome) {
        self.steps.push(StepReport {
            name: name.to_string(),
            status: outcome.status,
            duration_ms: start.elapsed().as_millis(),
            summary: outcome.summary,
            detail: outcome.detail,
        });
    }
}

#[derive(Debug)]
struct StepOutcome {
    status: StepStatus,
    summary: String,
    detail: Value,
}

impl StepOutcome {
    fn passed(summary: impl Into<String>, detail: Value) -> Self {
        Self {
            status: StepStatus::Passed,
            summary: summary.into(),
            detail,
        }
    }

    fn warning(summary: impl Into<String>, detail: Value) -> Self {
        Self {
            status: StepStatus::Warning,
            summary: summary.into(),
            detail,
        }
    }

    fn failed(summary: impl Into<String>, detail: Value) -> Self {
        Self {
            status: StepStatus::Failed,
            summary: summary.into(),
            detail,
        }
    }
}

#[derive(Clone)]
struct ControlClient {
    origin: String,
    client: reqwest::Client,
}

impl ControlClient {
    fn new(origin: String) -> Result<Self, SimError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            origin: origin.trim_end_matches('/').to_string(),
            client,
        })
    }

    async fn get_json(&self, path: &str) -> Result<Value, SimHttpError> {
        let response = self.client.get(self.url(path)).send().await?;
        Self::json_response(response).await
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value, SimHttpError> {
        let response = self.client.post(self.url(path)).json(body).send().await?;
        Self::json_response(response).await
    }

    async fn post_bytes(&self, path: &str, body: &Value) -> Result<Vec<u8>, SimHttpError> {
        let response = self.client.post(self.url(path)).json(body).send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if status.is_success() {
            Ok(bytes.to_vec())
        } else {
            Err(SimHttpError::from_body(
                status,
                String::from_utf8_lossy(&bytes).into_owned(),
            ))
        }
    }

    async fn json_response(response: reqwest::Response) -> Result<Value, SimHttpError> {
        let status = response.status();
        let text = response.text().await?;
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
        };
        if status.is_success() {
            Ok(value)
        } else {
            Err(SimHttpError::from_value(status, value))
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.origin, path)
    }
}

#[derive(Debug)]
pub enum SimError {
    Http(reqwest::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Args(String),
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::Http(err) => write!(f, "{err}"),
            SimError::Io(err) => write!(f, "{err}"),
            SimError::Json(err) => write!(f, "{err}"),
            SimError::Args(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SimError {}

impl From<reqwest::Error> for SimError {
    fn from(value: reqwest::Error) -> Self {
        SimError::Http(value)
    }
}

impl From<std::io::Error> for SimError {
    fn from(value: std::io::Error) -> Self {
        SimError::Io(value)
    }
}

impl From<serde_json::Error> for SimError {
    fn from(value: serde_json::Error) -> Self {
        SimError::Json(value)
    }
}

#[derive(Debug)]
struct SimHttpError {
    status: Option<StatusCode>,
    message: String,
}

impl SimHttpError {
    fn from_value(status: StatusCode, value: Value) -> Self {
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .or_else(|| value.get("message").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        Self {
            status: Some(status),
            message,
        }
    }

    fn from_body(status: StatusCode, body: String) -> Self {
        Self {
            status: Some(status),
            message: body,
        }
    }

    fn is_expected_provider_gap(&self) -> bool {
        let lower = self.message.to_ascii_lowercase();
        lower.contains("credential")
            || lower.contains("provider")
            || lower.contains("llm")
            || lower.contains("model")
            || lower.contains("api key")
            || lower.contains("connection refused")
    }
}

impl fmt::Display for SimHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(f, "{status}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl From<reqwest::Error> for SimHttpError {
    fn from(value: reqwest::Error) -> Self {
        Self {
            status: value.status(),
            message: value.to_string(),
        }
    }
}

fn summarize_optional_inventory(name: &str, value: Value, array_key: &str) -> StepOutcome {
    if array_key.is_empty() {
        return StepOutcome::passed(format!("{name} surface responded"), value);
    }
    let count = count_array(&value, array_key)
        .or_else(|| {
            value
                .get("count")
                .and_then(Value::as_u64)
                .map(|v| v as usize)
        })
        .unwrap_or(0);
    StepOutcome::passed(
        format!("{name} surface responded with {count} item(s)"),
        value,
    )
}

fn count_array(value: &Value, key: &str) -> Option<usize> {
    value.get(key).and_then(Value::as_array).map(Vec::len)
}

fn escape_cell(input: &str) -> String {
    input.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(status: StepStatus) -> StepReport {
        StepReport {
            name: "sample".to_string(),
            status,
            duration_ms: 1,
            summary: "summary".to_string(),
            detail: Value::Null,
        }
    }

    #[test]
    fn warning_verdict_becomes_failure_only_in_strict_mode() {
        let steps = vec![step(StepStatus::Passed), step(StepStatus::Warning)];
        assert_eq!(verdict_for(&steps, false), ReportVerdict::Warning);
        assert_eq!(verdict_for(&steps, true), ReportVerdict::Failed);
    }

    #[test]
    fn markdown_includes_step_rows_and_details() {
        let report = SimulationReport::from_steps(
            "http://127.0.0.1:4141".to_string(),
            "baseline".to_string(),
            Utc::now(),
            vec![step(StepStatus::Passed)],
            false,
        );
        let markdown = report.to_markdown();
        assert!(markdown.contains("Ordo Operator Simulator Report"));
        assert!(markdown.contains("| sample | OK | 1 ms | summary |"));
    }
}
