use crate::*;
use crate::helpers::*;

/// Capability provider that wires real outbound cloud calls onto the bus.
/// Credentials live in the local SQLite store via `CloudCredentialTask`.
/// When a service has no stored credential, capabilities return a
/// structured `not_configured` error rather than panicking, matching the
/// "local-first, not local-only" contract.
pub struct CloudOpsProvider {
    credentials: ordo_cloud::CloudCredentialTask,
    http: ordo_cloud::CloudHttp,
}

impl CloudOpsProvider {
    pub fn new(credentials: ordo_cloud::CloudCredentialTask) -> Self {
        Self {
            credentials,
            http: ordo_cloud::CloudHttp::new(),
        }
    }

    pub fn with_http(
        credentials: ordo_cloud::CloudCredentialTask,
        http: ordo_cloud::CloudHttp,
    ) -> Self {
        Self { credentials, http }
    }
}

const CLOUD_OPENAI_CHAT: &str = "cloud.openai.chat";
const CLOUD_OPENAI_EMBED: &str = "cloud.openai.embed";
const CLOUD_ANTHROPIC_MESSAGES: &str = "cloud.anthropic.messages";
const CLOUD_REST_REQUEST: &str = "cloud.rest.request";
const CLOUD_CREDENTIALS_LIST: &str = "cloud.credentials.list";
const CLOUD_CREDENTIALS_TEST: &str = "cloud.credentials.test";
const CLOUD_CREDENTIALS_MODELS: &str = "cloud.credentials.models";
const CLOUD_CREDENTIALS_UPSERT: &str = "cloud.credentials.upsert";
const CLOUD_CREDENTIALS_DELETE: &str = "cloud.credentials.delete";
const CLOUD_CREDENTIALS_SET_DEFAULT: &str = "cloud.credentials.set_default";

const CLOUD_OPS_CAPABILITIES: &[&str] = &[
    CLOUD_OPENAI_CHAT,
    CLOUD_OPENAI_EMBED,
    CLOUD_ANTHROPIC_MESSAGES,
    CLOUD_REST_REQUEST,
    CLOUD_CREDENTIALS_LIST,
    CLOUD_CREDENTIALS_TEST,
    CLOUD_CREDENTIALS_MODELS,
    CLOUD_CREDENTIALS_UPSERT,
    CLOUD_CREDENTIALS_DELETE,
    CLOUD_CREDENTIALS_SET_DEFAULT,
];

fn cloud_ops_description(capability: &str) -> &'static str {
    match capability {
        CLOUD_OPENAI_CHAT => {
            "Calls OpenAI chat/completions using a configured `openai` credential."
        }
        CLOUD_OPENAI_EMBED => "Calls OpenAI embeddings using a configured `openai` credential.",
        CLOUD_ANTHROPIC_MESSAGES => {
            "Calls Anthropic /messages using a configured `anthropic` credential."
        }
        CLOUD_REST_REQUEST => {
            "Sends an authenticated REST request against any configured cloud service."
        }
        CLOUD_CREDENTIALS_LIST => "Lists stored cloud credentials with secrets redacted.",
        CLOUD_CREDENTIALS_TEST => {
            "Tests one stored cloud credential and returns a redacted pass/fail status."
        }
        CLOUD_CREDENTIALS_MODELS => {
            "Discovers model identifiers exposed by one stored cloud credential."
        }
        CLOUD_CREDENTIALS_UPSERT => "Creates or updates a stored cloud credential.",
        CLOUD_CREDENTIALS_DELETE => "Deletes a stored cloud credential by service name.",
        CLOUD_CREDENTIALS_SET_DEFAULT => "Sets or clears the active default cloud credential.",
        _ => "Cloud Ops capability.",
    }
}

async fn run_cloud_tool_call(
    provider: &CloudOpsProvider,
    capability: &str,
    arguments: &Value,
) -> Option<ToolCallResult> {
    let result = match capability {
        CLOUD_OPENAI_CHAT => {
            cloud_service_call(provider, "openai", arguments, |http, cred, args| {
                Box::pin(async move { ordo_cloud::openai::chat(http, cred, &args).await })
            })
            .await
        }
        CLOUD_OPENAI_EMBED => {
            cloud_service_call(provider, "openai", arguments, |http, cred, args| {
                Box::pin(async move { ordo_cloud::openai::embed(http, cred, &args).await })
            })
            .await
        }
        CLOUD_ANTHROPIC_MESSAGES => {
            cloud_service_call(provider, "anthropic", arguments, |http, cred, args| {
                Box::pin(async move { ordo_cloud::anthropic::messages(http, cred, &args).await })
            })
            .await
        }
        CLOUD_REST_REQUEST => cloud_rest_request(provider, arguments).await,
        CLOUD_CREDENTIALS_LIST => cloud_credentials_list(provider).await,
        CLOUD_CREDENTIALS_TEST => cloud_credentials_test(provider, arguments).await,
        CLOUD_CREDENTIALS_MODELS => cloud_credentials_models(provider, arguments).await,
        CLOUD_CREDENTIALS_UPSERT => cloud_credentials_upsert(provider, arguments).await,
        CLOUD_CREDENTIALS_DELETE => cloud_credentials_delete(provider, arguments).await,
        CLOUD_CREDENTIALS_SET_DEFAULT => cloud_credentials_set_default(provider, arguments).await,
        _ => return None,
    };
    Some(match result {
        Ok(mut value) => {
            attach_context_to_output(&mut value, arguments);
            ToolCallResult::Completed { result: value }
        }
        Err(error) => ToolCallResult::Failed { error },
    })
}

type CloudFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = ordo_cloud::CloudResult<Value>> + Send + 'a>,
>;

async fn cloud_service_call<F>(
    provider: &CloudOpsProvider,
    kind: &str,
    arguments: &Value,
    call: F,
) -> Result<Value, String>
where
    F: for<'a> FnOnce(
        &'a ordo_cloud::CloudHttp,
        &'a ordo_cloud::CloudCredential,
        Value,
    ) -> CloudFuture<'a>,
{
    // Provider-neutral credential resolution. The `kind` arg ("openai",
    // "anthropic", …) is a HINT, not a service-name lookup: it says
    // which wire shape the caller expects. We walk in this order:
    //   1. an explicit `credential` arg (per-call override)
    //   2. a credential keyed under the kind name (legacy callers + the
    //      common case where someone configured "openai" or "anthropic"
    //      under that exact key)
    //   3. any configured credential whose `auth_style` is compatible
    //      with the kind (OpenAI-shape: anything except "anthropic";
    //      Anthropic-shape: only "anthropic")
    // This means a single Ollama / LM Studio / OpenRouter credential
    // configured under any service name still satisfies cloud.openai.chat.
    let explicit = arguments
        .get("credential")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let mut tried: Vec<String> = Vec::new();
    let mut credential: Option<ordo_cloud::CloudCredential> = None;

    async fn try_named(
        provider: &CloudOpsProvider,
        name: String,
        tried: &mut Vec<String>,
    ) -> Option<ordo_cloud::CloudCredential> {
        if tried.iter().any(|n| n == &name) {
            return None;
        }
        tried.push(name.clone());
        provider.credentials.get(name).await.ok().flatten()
    }

    if let Some(s) = explicit {
        if let Some(named) = try_named(provider, s.clone(), &mut tried).await {
            if !named.enabled() {
                return Err(format!(
                    "credential for service '{s}' is paused; enable it in the Provider tab before use"
                ));
            }
            credential = Some(named);
        }
    }
    if credential.is_none() {
        credential = try_named(provider, kind.to_string(), &mut tried)
            .await
            .filter(ordo_cloud::CloudCredential::enabled);
    }
    if credential.is_none() {
        let kind_is_anthropic = kind == "anthropic";
        if let Ok(all) = provider.credentials.list().await {
            for cred in all {
                if !cred.enabled() {
                    continue;
                }
                let cred_is_anthropic = cred.auth_style == "anthropic";
                if cred_is_anthropic == kind_is_anthropic {
                    credential = Some(cred);
                    break;
                }
            }
        }
    }
    let credential = credential.ok_or_else(|| {
        format!(
            "no compatible credential configured for kind '{kind}'; \
             configure one in the Cloud tab or via cloud.credentials.upsert"
        )
    })?;

    // If the caller didn't specify `model`, surface the credential's
    // extras.model — set in the Cloud tab's Configure modal — so local
    // OpenAI-compatible servers (Ollama, LM Studio) route to whichever
    // model the operator has loaded instead of the cloud-provider
    // default like `gpt-4o-mini`.
    let mut args = arguments.clone();
    if args.get("model").is_none() {
        if let Some(model) = credential.extras.get("model") {
            if let Some(obj) = args.as_object_mut() {
                obj.insert("model".to_string(), json!(model));
            }
        }
    }

    let lifecycle_report = provider
        .credentials
        .enforce_single_local_model(
            &provider.http,
            Some(&credential.service),
            "cloud_service_call",
        )
        .await
        .map_err(|err| err.to_string())?;
    log_cloud_lifecycle_report("cloud service call", &lifecycle_report);
    if let Some(error) = lifecycle_error(&lifecycle_report) {
        return Err(error);
    }

    call(&provider.http, &credential, args)
        .await
        .map_err(|err| err.to_string())
}

async fn cloud_rest_request(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    let service = arguments
        .get("service")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required field 'service'".to_string())?
        .to_string();
    let method = arguments
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let url = arguments
        .get("url")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required field 'url' (absolute or relative path)".to_string())?
        .to_string();
    let credential = match provider.credentials.get(service.clone()).await {
        Ok(Some(credential)) => credential,
        Ok(None) => {
            return Err(format!(
                "credential for service '{service}' is not configured"
            ));
        }
        Err(err) => return Err(err.to_string()),
    };
    if !credential.enabled() {
        return Err(format!(
            "credential for service '{service}' is paused; enable it in the Provider tab before use"
        ));
    }
    let body = arguments.get("body").cloned();
    let headers = ordo_cloud::headers_from_value(arguments.get("headers"));
    let method = match method.as_str() {
        "GET" => ordo_cloud::Method::GET,
        "POST" => ordo_cloud::Method::POST,
        "PUT" => ordo_cloud::Method::PUT,
        "PATCH" => ordo_cloud::Method::PATCH,
        "DELETE" => ordo_cloud::Method::DELETE,
        other => return Err(format!("unsupported HTTP method '{other}'")),
    };
    let response = provider
        .http
        .send_json(&credential, method, &url, body.as_ref(), &headers)
        .await
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "service": service,
        "url": url,
        "response": response,
    }))
}

async fn cloud_credentials_list(provider: &CloudOpsProvider) -> Result<Value, String> {
    let credentials = provider
        .credentials
        .list()
        .await
        .map_err(|err| err.to_string())?;
    let default_service = provider
        .credentials
        .get_default()
        .await
        .map_err(|err| err.to_string())?;
    let redacted: Vec<Value> = credentials
        .iter()
        .map(ordo_cloud::CloudCredential::redacted)
        .collect();
    Ok(json!({
        "count": redacted.len(),
        "credentials": redacted,
        "default_service": default_service,
    }))
}

async fn cloud_credentials_set_default(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    let service = arguments
        .get("service")
        .or_else(|| arguments.get("credential"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let previous_default = provider
        .credentials
        .get_default()
        .await
        .map_err(|err| err.to_string())?;
    let previous = match previous_default.clone() {
        Some(name) => provider
            .credentials
            .get(name)
            .await
            .map_err(|err| err.to_string())?,
        None => None,
    };
    let next = if let Some(service_name) = service.as_ref() {
        let credential = provider
            .credentials
            .get(service_name.clone())
            .await
            .map_err(|err| err.to_string())?;
        if credential.is_none() {
            return Ok(json!({
                "ok": false,
                "default_service": null,
                "error": format!("credential for service '{service_name}' is not configured"),
            }));
        }
        credential
    } else {
        None
    };

    let mut lifecycle_report = ordo_cloud::LocalModelLifecycleReport::default();
    if previous.as_ref().and_then(ordo_cloud::local_model_identity)
        != next.as_ref().and_then(ordo_cloud::local_model_identity)
    {
        if let Some(previous) = previous.as_ref() {
            let report = provider
                .credentials
                .release_local_model(&provider.http, previous, "default_provider_switch")
                .await
                .map_err(|err| err.to_string())?;
            lifecycle_report.merge(report);
            log_cloud_lifecycle_report("default switch release", &lifecycle_report);
            if let Some(error) = lifecycle_error(&lifecycle_report) {
                return Ok(json!({
                    "ok": false,
                    "default_service": previous_default,
                    "error": error,
                    "model_lifecycle": serde_json::to_value(&lifecycle_report).unwrap_or(Value::Null),
                }));
            }
        }
    }

    provider
        .credentials
        .set_default(service.clone())
        .await
        .map_err(|err| err.to_string())?;

    let enforce_report = provider
        .credentials
        .enforce_single_local_model(
            &provider.http,
            service.as_deref(),
            "default_provider_switch",
        )
        .await
        .map_err(|err| err.to_string())?;
    lifecycle_report.merge(enforce_report);
    log_cloud_lifecycle_report("default switch enforce", &lifecycle_report);

    Ok(json!({
        "ok": true,
        "default_service": service,
        "model_lifecycle": serde_json::to_value(&lifecycle_report).unwrap_or(Value::Null),
    }))
}

async fn cloud_credential_for_read(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<ordo_cloud::CloudCredential, String> {
    if let Some(service) = arguments
        .get("service")
        .or_else(|| arguments.get("credential"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return provider
            .credentials
            .get(service.to_string())
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("credential for service '{service}' is not configured"));
    }

    let credentials = provider
        .credentials
        .list()
        .await
        .map_err(|err| err.to_string())?;
    match credentials.as_slice() {
        [credential] => Ok(credential.clone()),
        [] => Err("no cloud credentials are configured".into()),
        _ => Err(
            "service or credential must be provided when multiple credentials are configured"
                .into(),
        ),
    }
}

/// The service name the caller asked about, echoed back into the
/// `{ok:false, ...}` envelopes for the test/models tools so the studio
/// can label which provider failed even when no credential row exists.
fn requested_service(arguments: &Value) -> Value {
    arguments
        .get("service")
        .or_else(|| arguments.get("credential"))
        .cloned()
        .unwrap_or(Value::Null)
}

async fn cloud_credentials_test(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    // A missing / ambiguous credential is an expected operator state,
    // not a server fault — return a clean {ok:false,error} (HTTP 200)
    // so the studio surfaces the reason instead of a bare 500.
    let credential = match cloud_credential_for_read(provider, arguments).await {
        Ok(credential) => credential,
        Err(error) => {
            return Ok(json!({
                "service": requested_service(arguments),
                "ok": false,
                "error": error,
            }));
        }
    };
    let service = credential.service.clone();
    if !credential.enabled() {
        return Ok(json!({
            "service": service,
            "ok": false,
            "error": "credential is paused",
            "credential": credential.redacted(),
        }));
    }

    match ordo_cloud::test_credential(&provider.http, &credential).await {
        Ok(()) => Ok(json!({
            "service": service,
            "ok": true,
            "error": null,
            "credential": credential.redacted(),
        })),
        Err(error) => Ok(json!({
            "service": service,
            "ok": false,
            "error": error,
            "credential": credential.redacted(),
        })),
    }
}

async fn cloud_credentials_models(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    // Same graceful-degradation contract as cloud_credentials_test: an
    // unconfigured / ambiguous service yields {ok:false,error} rather
    // than a 500, so "Discover Models" shows a readable message.
    let credential = match cloud_credential_for_read(provider, arguments).await {
        Ok(credential) => credential,
        Err(error) => {
            return Ok(json!({
                "service": requested_service(arguments),
                "ok": false,
                "error": error,
                "count": 0,
                "models": [],
            }));
        }
    };
    let service = credential.service.clone();
    if !credential.enabled() {
        return Ok(json!({
            "service": service,
            "ok": false,
            "error": "credential is paused",
            "count": 0,
            "models": [],
            "credential": credential.redacted(),
        }));
    }

    match ordo_cloud::list_models(&provider.http, &credential).await {
        Ok(models) => Ok(json!({
            "service": service,
            "ok": true,
            "error": null,
            "count": models.len(),
            "models": models,
            "credential": credential.redacted(),
        })),
        Err(error) => Ok(json!({
            "service": service,
            "ok": false,
            "error": error,
            "count": 0,
            "models": [],
            "credential": credential.redacted(),
        })),
    }
}

pub(crate) async fn cloud_credentials_upsert(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    let service = arguments
        .get("service")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required field 'service'".to_string())?
        .to_string();
    let previous = provider
        .credentials
        .get(service.clone())
        .await
        .map_err(|err| err.to_string())?;
    let update = ordo_cloud::CloudCredentialUpdate {
        service,
        label: arguments
            .get("label")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        auth_style: arguments
            .get("auth_style")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        // An empty `secret` means "preserve the existing one", matching the
        // bus path (`full_into_update`) and the store's `None` semantics. The
        // Studio's Edit modal never carries the (redacted) secret, so editing
        // any other field must NOT wipe the stored key. Delete the credential
        // to remove it.
        secret: arguments
            .get("secret")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        base_url: arguments
            .get("base_url")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        extras: arguments
            .get("extras")
            .and_then(|value| value.as_object())
            .map(|object| {
                object
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            }),
    };
    let credential = provider
        .credentials
        .upsert(update)
        .await
        .map_err(|err| err.to_string())?;
    let mut lifecycle_report = ordo_cloud::LocalModelLifecycleReport::default();
    if previous.as_ref().and_then(ordo_cloud::local_model_identity)
        != ordo_cloud::local_model_identity(&credential)
    {
        if let Some(previous) = previous.as_ref() {
            let report = provider
                .credentials
                .release_local_model(&provider.http, previous, "credential_upsert_model_changed")
                .await
                .map_err(|err| err.to_string())?;
            lifecycle_report.merge(report);
            log_cloud_lifecycle_report("credential upsert release", &lifecycle_report);
        }
    }
    Ok(json!({
        "credential": credential.redacted(),
        "model_lifecycle": serde_json::to_value(&lifecycle_report).unwrap_or(Value::Null),
    }))
}

async fn cloud_credentials_delete(
    provider: &CloudOpsProvider,
    arguments: &Value,
) -> Result<Value, String> {
    let service = arguments
        .get("service")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required field 'service'".to_string())?
        .to_string();
    let previous = provider
        .credentials
        .get(service.clone())
        .await
        .map_err(|err| err.to_string())?;
    let removed = provider
        .credentials
        .delete(service.clone())
        .await
        .map_err(|err| err.to_string())?;
    let mut lifecycle_report = ordo_cloud::LocalModelLifecycleReport::default();
    if removed {
        if let Some(previous) = previous.as_ref() {
            let report = provider
                .credentials
                .release_local_model(&provider.http, previous, "credential_deleted")
                .await
                .map_err(|err| err.to_string())?;
            lifecycle_report.merge(report);
            log_cloud_lifecycle_report("credential delete release", &lifecycle_report);
        }
    }
    Ok(json!({
        "service": service,
        "removed": removed,
        "model_lifecycle": serde_json::to_value(&lifecycle_report).unwrap_or(Value::Null),
    }))
}

#[async_trait]
impl CapabilityProvider for CloudOpsProvider {
    fn name(&self) -> &str {
        "cloud-ops"
    }

    fn capabilities(&self) -> Vec<String> {
        CLOUD_OPS_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        CLOUD_OPS_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    cloud_ops_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        run_cloud_tool_call(self, capability, arguments).await
    }
}
