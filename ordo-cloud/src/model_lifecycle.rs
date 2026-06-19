use serde::Serialize;
use serde_json::{json, Value};

use crate::{CloudCredential, CloudError, CloudHttp, CloudResult, Method};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalModelProvider {
    Ollama,
    LmStudio,
}

impl LocalModelProvider {
    fn label(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::LmStudio => "lmstudio",
        }
    }

    fn default_origin(self) -> &'static str {
        match self {
            Self::Ollama => "http://127.0.0.1:11434",
            Self::LmStudio => "http://127.0.0.1:1234",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalModelIdentity {
    pub provider: LocalModelProvider,
    pub service: String,
    pub model: Option<String>,
    pub origin: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalModelUnloadEvent {
    pub provider: LocalModelProvider,
    pub service: String,
    pub model: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LocalModelLifecycleReport {
    pub active: Option<LocalModelIdentity>,
    pub unloaded: Vec<LocalModelUnloadEvent>,
    pub errors: Vec<String>,
}

impl LocalModelLifecycleReport {
    pub fn merge(&mut self, mut other: LocalModelLifecycleReport) {
        if other.active.is_some() {
            self.active = other.active.take();
        }
        self.unloaded.append(&mut other.unloaded);
        self.errors.append(&mut other.errors);
    }

    pub fn has_work(&self) -> bool {
        self.active.is_some() || !self.unloaded.is_empty() || !self.errors.is_empty()
    }
}

pub fn local_model_identity(credential: &CloudCredential) -> Option<LocalModelIdentity> {
    let provider = local_model_provider(credential)?;
    Some(LocalModelIdentity {
        provider,
        service: credential.service.clone(),
        model: local_model_name(credential),
        origin: local_provider_origin(credential, provider).ok()?,
    })
}

pub fn local_model_provider(credential: &CloudCredential) -> Option<LocalModelProvider> {
    let service = credential.service.to_ascii_lowercase();
    let provider_kind = credential
        .extras
        .get("provider_kind")
        .map(|value| value.to_ascii_lowercase());
    let base_url = credential
        .base_url
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();

    if service == "ollama-cloud-api"
        || service == "ollama-cloud"
        || base_url.contains("ollama.com")
        || matches!(
            provider_kind.as_deref(),
            Some("cloud_model" | "cloud" | "remote_model")
        )
    {
        return None;
    }

    let is_ollama_service = matches!(service.as_str(), "ollama" | "ollama-local" | "ollama_local");
    let is_lmstudio_service = matches!(
        service.as_str(),
        "lmstudio" | "lm-studio" | "lm_studio" | "lmstudio-local" | "lmstudio_local"
    );
    let is_local_model_flag = provider_kind.as_deref() == Some("local_model");

    let is_ollama_base = base_url.contains("localhost:11434")
        || base_url.contains("127.0.0.1:11434")
        || base_url.contains("[::1]:11434")
        || base_url.contains("0.0.0.0:11434");
    let is_lmstudio_base = base_url.contains("localhost:1234")
        || base_url.contains("127.0.0.1:1234")
        || base_url.contains("[::1]:1234")
        || base_url.contains("0.0.0.0:1234");

    if is_ollama_service || (is_local_model_flag && is_ollama_base) {
        return Some(LocalModelProvider::Ollama);
    }
    if is_lmstudio_service || (is_local_model_flag && is_lmstudio_base) {
        return Some(LocalModelProvider::LmStudio);
    }

    None
}

pub fn local_model_name(credential: &CloudCredential) -> Option<String> {
    credential
        .extras
        .get("model")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn local_provider_origin(
    credential: &CloudCredential,
    provider: LocalModelProvider,
) -> CloudResult<String> {
    let raw = credential
        .base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| provider.default_origin());
    let mut url = reqwest::Url::parse(raw)
        .map_err(|err| CloudError::InvalidArgument(format!("invalid local provider URL: {err}")))?;
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub fn synthetic_local_credential(provider: LocalModelProvider) -> CloudCredential {
    let service = provider.label().to_string();
    CloudCredential {
        service: service.clone(),
        label: service,
        auth_style: "none".to_string(),
        secret: String::new(),
        base_url: Some(format!("{}/v1", provider.default_origin())),
        extras: Default::default(),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

pub async fn release_local_model(
    http: &CloudHttp,
    credential: &CloudCredential,
    reason: impl Into<String>,
) -> LocalModelLifecycleReport {
    let reason = reason.into();
    let mut report = LocalModelLifecycleReport::default();
    let Some(identity) = local_model_identity(credential) else {
        return report;
    };
    report.active = None;
    unload_identity(http, credential, &identity, &reason, None, &mut report).await;
    report
}

pub async fn enforce_single_local_model(
    http: &CloudHttp,
    configured: &[CloudCredential],
    active: Option<&CloudCredential>,
    reason: impl Into<String>,
) -> LocalModelLifecycleReport {
    let reason = reason.into();
    let active_identity = active.and_then(local_model_identity);
    let mut report = LocalModelLifecycleReport {
        active: active_identity.clone(),
        ..Default::default()
    };

    let mut candidates: Vec<CloudCredential> = configured
        .iter()
        .filter(|credential| local_model_provider(credential).is_some())
        .cloned()
        .collect();
    if let Some(active) = active {
        if local_model_provider(active).is_some()
            && !candidates.iter().any(|candidate| {
                candidate.service == active.service && candidate.base_url == active.base_url
            })
        {
            candidates.push(active.clone());
        }
    }
    let should_probe_default_local_ports = active_identity.is_some() || !candidates.is_empty();
    if should_probe_default_local_ports {
        for provider in [LocalModelProvider::Ollama, LocalModelProvider::LmStudio] {
            if !candidates
                .iter()
                .any(|credential| local_model_provider(credential) == Some(provider))
            {
                candidates.push(synthetic_local_credential(provider));
            }
        }
    }

    for credential in candidates {
        let Some(identity) = local_model_identity(&credential) else {
            continue;
        };
        let keep_model = active_identity.as_ref().and_then(|active| {
            (active.provider == identity.provider
                && active.origin == identity.origin
                && active.service == identity.service)
                .then(|| active.model.clone())
                .flatten()
        });
        unload_identity(
            http,
            &credential,
            &identity,
            &reason,
            keep_model.as_deref(),
            &mut report,
        )
        .await;
    }

    report
}

async fn unload_identity(
    http: &CloudHttp,
    credential: &CloudCredential,
    identity: &LocalModelIdentity,
    reason: &str,
    keep_model: Option<&str>,
    report: &mut LocalModelLifecycleReport,
) {
    let mut listing_failed = false;
    let loaded = match loaded_models(http, credential, identity).await {
        Ok(models) => models,
        Err(err) => {
            listing_failed = true;
            let message = err.to_string();
            if !looks_like_provider_offline(&message) {
                report.errors.push(format!(
                    "{} list loaded models failed: {message}",
                    identity.provider.label()
                ));
            }
            Vec::new()
        }
    };
    let mut targets = loaded
        .into_iter()
        .filter(|model| keep_model.map(|keep| keep != model).unwrap_or(true))
        .collect::<Vec<_>>();
    if targets.is_empty() && listing_failed {
        if let Some(model) = identity.model.as_ref() {
            if keep_model.map(|keep| keep != model).unwrap_or(true) {
                targets.push(model.clone());
            }
        }
    }
    targets.sort();
    targets.dedup();

    for model in targets {
        match unload_one(http, credential, identity, &model).await {
            Ok(()) => report.unloaded.push(LocalModelUnloadEvent {
                provider: identity.provider,
                service: identity.service.clone(),
                model,
                reason: reason.to_string(),
            }),
            Err(err) => {
                let message = err.to_string();
                if !looks_like_provider_offline(&message) {
                    report.errors.push(format!(
                        "{} unload '{}' failed: {message}",
                        identity.provider.label(),
                        model
                    ));
                }
            }
        }
    }
}

async fn loaded_models(
    http: &CloudHttp,
    credential: &CloudCredential,
    identity: &LocalModelIdentity,
) -> CloudResult<Vec<String>> {
    let url = match identity.provider {
        LocalModelProvider::Ollama => format!("{}/api/ps", identity.origin),
        LocalModelProvider::LmStudio => format!("{}/api/v1/models", identity.origin),
    };
    let payload = http
        .send_json(credential, Method::GET, &url, None, &[])
        .await?;
    let mut models = match identity.provider {
        LocalModelProvider::Ollama => parse_ollama_loaded(&payload),
        LocalModelProvider::LmStudio => parse_lmstudio_loaded(&payload),
    };
    models.sort();
    models.dedup();
    Ok(models)
}

async fn unload_one(
    http: &CloudHttp,
    credential: &CloudCredential,
    identity: &LocalModelIdentity,
    model: &str,
) -> CloudResult<()> {
    let (url, body) = match identity.provider {
        LocalModelProvider::Ollama => (
            format!("{}/api/generate", identity.origin),
            json!({
                "model": model,
                "prompt": "",
                "stream": false,
                "keep_alive": 0,
            }),
        ),
        LocalModelProvider::LmStudio => (
            format!("{}/api/v1/models/unload", identity.origin),
            json!({ "instance_id": model }),
        ),
    };
    http.send_json(credential, Method::POST, &url, Some(&body), &[])
        .await
        .map(|_| ())
}

fn parse_ollama_loaded(payload: &Value) -> Vec<String> {
    payload
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            field_string(row, &["name", "model", "id"]).or_else(|| row.as_str().map(str::to_string))
        })
        .collect()
}

fn parse_lmstudio_loaded(payload: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for row in payload
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            payload
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
    {
        if let Some(instances) = row.get("instances").and_then(Value::as_array) {
            for instance in instances {
                if let Some(id) = field_string(
                    instance,
                    &["instance_id", "instanceId", "id", "model", "name"],
                ) {
                    out.push(id);
                }
            }
        }
        if row_looks_loaded(row) {
            if let Some(id) =
                field_string(row, &["instance_id", "instanceId", "id", "model", "name"])
            {
                out.push(id);
            }
        }
    }
    out
}

fn row_looks_loaded(row: &Value) -> bool {
    if row.get("loaded").and_then(Value::as_bool) == Some(true)
        || row.get("is_loaded").and_then(Value::as_bool) == Some(true)
        || row.get("isLoaded").and_then(Value::as_bool) == Some(true)
    {
        return true;
    }
    field_string(row, &["status", "state", "load_state", "loadState"])
        .map(|status| {
            let status = status.to_ascii_lowercase();
            status.contains("loaded")
                || status.contains("running")
                || status.contains("active")
                || status.contains("ready")
        })
        .unwrap_or(false)
}

fn field_string(row: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = row.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn looks_like_provider_offline(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("connection refused")
        || message.contains("error trying to connect")
        || message.contains("error sending request for url")
        || message.contains("connection reset")
        || message.contains("failed to lookup address")
        || message.contains("operation timed out")
        || message.contains("deadline has elapsed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn local_credential(
        service: &str,
        base_url: String,
        model: &str,
        provider_kind: &str,
    ) -> CloudCredential {
        let mut extras = std::collections::HashMap::new();
        extras.insert("model".to_string(), model.to_string());
        extras.insert("provider_kind".to_string(), provider_kind.to_string());
        CloudCredential {
            service: service.to_string(),
            label: service.to_string(),
            auth_style: "bearer".to_string(),
            secret: "local".to_string(),
            base_url: Some(base_url),
            extras,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn local_provider_classification_keeps_ollama_cloud_api_remote() {
        let local = local_credential(
            "ollama",
            "http://127.0.0.1:11434/v1".to_string(),
            "qwen",
            "local_model",
        );
        assert_eq!(
            local_model_provider(&local),
            Some(LocalModelProvider::Ollama)
        );

        let remote = local_credential(
            "ollama-cloud-api",
            "https://ollama.com/v1".to_string(),
            "gpt-oss:cloud",
            "cloud_model",
        );
        assert_eq!(local_model_provider(&remote), None);
    }

    #[tokio::test]
    async fn enforce_single_local_model_unloads_other_ollama_models() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/ps"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models": [
                    { "name": "old-model" },
                    { "name": "active-model" }
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/generate"))
            .and(body_json(json!({
                "model": "old-model",
                "prompt": "",
                "stream": false,
                "keep_alive": 0,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"done": true})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
            .mount(&server)
            .await;

        let active = local_credential(
            "ollama",
            format!("{}/v1", server.uri()),
            "active-model",
            "local_model",
        );
        let inactive_lmstudio = local_credential(
            "lmstudio",
            format!("{}/v1", server.uri()),
            "gemma",
            "local_model",
        );
        let report = enforce_single_local_model(
            &CloudHttp::new(),
            &[active.clone(), inactive_lmstudio],
            Some(&active),
            "test",
        )
        .await;

        assert_eq!(report.errors, Vec::<String>::new());
        assert_eq!(report.unloaded.len(), 1);
        assert_eq!(report.unloaded[0].model, "old-model");
    }

    #[tokio::test]
    async fn release_lmstudio_model_uses_native_unload_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "id": "gemma", "loaded": true }
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/models/unload"))
            .and(body_json(json!({ "instance_id": "gemma" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"instance_id": "gemma"})))
            .expect(1)
            .mount(&server)
            .await;

        let credential = local_credential(
            "lmstudio",
            format!("{}/v1", server.uri()),
            "gemma",
            "local_model",
        );
        let report = release_local_model(&CloudHttp::new(), &credential, "test").await;

        assert_eq!(report.errors, Vec::<String>::new());
        assert_eq!(report.unloaded.len(), 1);
        assert_eq!(report.unloaded[0].model, "gemma");
    }
}
