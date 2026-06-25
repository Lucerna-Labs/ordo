use crate::*;
use crate::helpers::*;

pub struct InterfaceOpsProvider;

impl InterfaceOpsProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for InterfaceOpsProvider {
    fn default() -> Self {
        Self::new()
    }
}

const SSH_DESCRIBE_HOST: &str = "ssh.describe_host";
const SSH_PREPARE_COMMAND: &str = "ssh.prepare_command";
const SSH_SYNC_WORKSPACE: &str = "ssh.sync_workspace";
const API_DESCRIBE_CLIENT: &str = "api.describe_client";
const API_PREPARE_AUTH: &str = "api.prepare_auth";
const API_DISPATCH_WEBHOOK: &str = "api.dispatch_webhook";
const REST_DESCRIBE_ENDPOINT: &str = "rest.describe_endpoint";
const REST_PREPARE_REQUEST: &str = "rest.prepare_request";
const REST_VALIDATE_RESPONSE: &str = "rest.validate_response";
const REST_SYNC_RESOURCE: &str = "rest.sync_resource";

const INTERFACE_OPS_CAPABILITIES: &[&str] = &[
    SSH_DESCRIBE_HOST,
    SSH_PREPARE_COMMAND,
    SSH_SYNC_WORKSPACE,
    API_DESCRIBE_CLIENT,
    API_PREPARE_AUTH,
    API_DISPATCH_WEBHOOK,
    REST_DESCRIBE_ENDPOINT,
    REST_PREPARE_REQUEST,
    REST_VALIDATE_RESPONSE,
    REST_SYNC_RESOURCE,
];

fn interface_ops_description(capability: &str) -> &'static str {
    match capability {
        SSH_DESCRIBE_HOST => {
            "Describes a remote host target: user, host, port, and identity hints."
        }
        SSH_PREPARE_COMMAND => {
            "Prepares a remote command plan for an SSH host without executing it."
        }
        SSH_SYNC_WORKSPACE => "Plans a workspace sync between local and remote paths over SSH.",
        API_DESCRIBE_CLIENT => {
            "Describes an external API client: base URL, auth style, and scopes."
        }
        API_PREPARE_AUTH => "Prepares an auth refresh descriptor for an API client.",
        API_DISPATCH_WEBHOOK => "Prepares a webhook dispatch payload for an API client.",
        REST_DESCRIBE_ENDPOINT => "Describes a REST endpoint: method, path, and resource kind.",
        REST_PREPARE_REQUEST => {
            "Prepares a REST request body/headers against an endpoint description."
        }
        REST_VALIDATE_RESPONSE => {
            "Validates a REST response against a declared status and required fields."
        }
        REST_SYNC_RESOURCE => {
            "Plans a REST resource sync (fetch then write) between two endpoints."
        }
        _ => "Interface Ops capability.",
    }
}

#[async_trait]
impl CapabilityProvider for InterfaceOpsProvider {
    fn name(&self) -> &str {
        "interface-ops"
    }

    fn capabilities(&self) -> Vec<String> {
        INTERFACE_OPS_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        INTERFACE_OPS_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    interface_ops_description(capability),
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
        let result = match capability {
            SSH_DESCRIBE_HOST => describe_ssh_host(arguments),
            SSH_PREPARE_COMMAND => prepare_ssh_command(arguments),
            SSH_SYNC_WORKSPACE => sync_ssh_workspace(arguments),
            API_DESCRIBE_CLIENT => describe_api_client(arguments),
            API_PREPARE_AUTH => prepare_api_auth(arguments),
            API_DISPATCH_WEBHOOK => dispatch_api_webhook(arguments),
            REST_DESCRIBE_ENDPOINT => describe_rest_endpoint(arguments),
            REST_PREPARE_REQUEST => prepare_rest_request(arguments),
            REST_VALIDATE_RESPONSE => validate_rest_response(arguments),
            REST_SYNC_RESOURCE => sync_rest_resource(arguments),
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
}

fn describe_ssh_host(arguments: &Value) -> Result<Value, String> {
    let host = require_string(arguments, "host")?;
    let user = optional_string(arguments, "user").unwrap_or_else(|| "root".to_string());
    let port = arguments
        .get("port")
        .and_then(|value| value.as_u64())
        .unwrap_or(22);
    let identity = optional_string(arguments, "identity");
    Ok(json!({
        "ssh_host": {
            "user": user,
            "host": host,
            "port": port,
            "identity": identity,
            "target": format!("{user}@{host}:{port}"),
        },
    }))
}

fn prepare_ssh_command(arguments: &Value) -> Result<Value, String> {
    let host = require_string(arguments, "host")?;
    let command = require_string(arguments, "command")?;
    let user = optional_string(arguments, "user").unwrap_or_else(|| "root".to_string());
    let port = arguments
        .get("port")
        .and_then(|value| value.as_u64())
        .unwrap_or(22);
    let working_dir = optional_string(arguments, "working_dir");
    let composed = match &working_dir {
        Some(dir) => format!("cd {dir} && {command}"),
        None => command.clone(),
    };
    Ok(json!({
        "ssh_command": {
            "target": format!("{user}@{host}:{port}"),
            "working_dir": working_dir,
            "command": command,
            "composed": composed,
        },
    }))
}

fn sync_ssh_workspace(arguments: &Value) -> Result<Value, String> {
    let host = require_string(arguments, "host")?;
    let local_path = require_string(arguments, "local_path")?;
    let remote_path = require_string(arguments, "remote_path")?;
    let direction = optional_string(arguments, "direction").unwrap_or_else(|| "push".to_string());
    if direction != "push" && direction != "pull" {
        return Err(format!(
            "unknown sync direction '{direction}' (expected 'push' or 'pull')"
        ));
    }
    let user = optional_string(arguments, "user").unwrap_or_else(|| "root".to_string());
    Ok(json!({
        "ssh_sync": {
            "direction": direction,
            "local_path": local_path,
            "remote_path": remote_path,
            "target": format!("{user}@{host}"),
        },
    }))
}

fn describe_api_client(arguments: &Value) -> Result<Value, String> {
    let name = require_string(arguments, "name")?;
    let base_url = require_string(arguments, "base_url")?;
    let auth_style =
        optional_string(arguments, "auth_style").unwrap_or_else(|| "bearer".to_string());
    let scopes = optional_string_array(arguments, "scopes");
    Ok(json!({
        "api_client": {
            "name": name,
            "base_url": base_url,
            "auth_style": auth_style,
            "scopes": scopes,
            "scope_count": scopes.len(),
        },
    }))
}

fn prepare_api_auth(arguments: &Value) -> Result<Value, String> {
    let client = require_string(arguments, "client")?;
    let auth_style =
        optional_string(arguments, "auth_style").unwrap_or_else(|| "bearer".to_string());
    let refresh_url = optional_string(arguments, "refresh_url");
    let steps = match auth_style.as_str() {
        "bearer" => vec![
            "load_refresh_token",
            "exchange_for_access_token",
            "cache_token",
        ],
        "basic" => vec!["load_credentials", "compose_basic_header"],
        "api_key" => vec!["load_api_key", "set_header"],
        "oauth2" => vec![
            "load_refresh_token",
            "post_refresh_request",
            "parse_token_response",
            "cache_token",
        ],
        other => {
            return Err(format!("unknown auth style '{other}'"));
        }
    };
    Ok(json!({
        "api_auth": {
            "client": client,
            "auth_style": auth_style,
            "refresh_url": refresh_url,
            "steps": steps,
        },
    }))
}

fn dispatch_api_webhook(arguments: &Value) -> Result<Value, String> {
    let client = require_string(arguments, "client")?;
    let event = require_string(arguments, "event")?;
    let payload = arguments.get("payload").cloned().unwrap_or(json!({}));
    let target = optional_string(arguments, "target");
    Ok(json!({
        "api_webhook": {
            "client": client,
            "event": event,
            "target": target,
            "payload": payload,
        },
    }))
}

fn describe_rest_endpoint(arguments: &Value) -> Result<Value, String> {
    let method = require_string(arguments, "method")?;
    let path = require_string(arguments, "path")?;
    let resource =
        optional_string(arguments, "resource").unwrap_or_else(|| infer_rest_resource(&path));
    let method_upper = method.to_ascii_uppercase();
    const ALLOWED: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    if !ALLOWED.iter().any(|m| *m == method_upper) {
        return Err(format!("unsupported HTTP method '{method}'"));
    }
    Ok(json!({
        "rest_endpoint": {
            "method": method_upper,
            "path": path,
            "resource": resource,
        },
    }))
}

fn prepare_rest_request(arguments: &Value) -> Result<Value, String> {
    let method = require_string(arguments, "method")?;
    let path = require_string(arguments, "path")?;
    let body = arguments.get("body").cloned().unwrap_or(json!({}));
    let headers = arguments
        .get("headers")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let method_upper = method.to_ascii_uppercase();
    let has_body = matches!(method_upper.as_str(), "POST" | "PUT" | "PATCH");
    Ok(json!({
        "rest_request": {
            "method": method_upper,
            "path": path,
            "headers": headers,
            "body": if has_body { body } else { Value::Null },
            "has_body": has_body,
        },
    }))
}

fn validate_rest_response(arguments: &Value) -> Result<Value, String> {
    let status = arguments
        .get("status")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "missing required numeric field 'status'".to_string())?;
    let expected_status = arguments
        .get("expected_status")
        .and_then(|value| value.as_u64())
        .unwrap_or(200);
    let required_fields = optional_string_array(arguments, "required_fields");
    let body = arguments.get("body").cloned().unwrap_or(json!({}));
    let mut issues = Vec::new();
    if status != expected_status {
        issues.push(format!(
            "status {status} did not match expected {expected_status}"
        ));
    }
    if let Some(object) = body.as_object() {
        for field in &required_fields {
            if !object.contains_key(field) {
                issues.push(format!("missing required field '{field}'"));
            }
        }
    } else if !required_fields.is_empty() {
        issues.push("body is not an object; required fields cannot be checked".to_string());
    }
    Ok(json!({
        "valid": issues.is_empty(),
        "issues": issues,
        "status": status,
        "expected_status": expected_status,
    }))
}

fn sync_rest_resource(arguments: &Value) -> Result<Value, String> {
    let source = require_string(arguments, "source")?;
    let target = require_string(arguments, "target")?;
    let resource = optional_string(arguments, "resource")
        .unwrap_or_else(|| infer_rest_resource(&source.clone()));
    let direction = optional_string(arguments, "direction").unwrap_or_else(|| "pull".to_string());
    if direction != "pull" && direction != "push" && direction != "mirror" {
        return Err(format!(
            "unknown rest sync direction '{direction}' (expected 'pull', 'push', or 'mirror')"
        ));
    }
    let steps = match direction.as_str() {
        "pull" => vec!["GET source", "transform", "PUT target"],
        "push" => vec!["GET target", "compare", "POST source"],
        "mirror" => vec!["GET source", "GET target", "diff", "apply both"],
        _ => unreachable!(),
    };
    Ok(json!({
        "rest_sync": {
            "source": source,
            "target": target,
            "resource": resource,
            "direction": direction,
            "steps": steps,
        },
    }))
}

fn infer_rest_resource(path: &str) -> String {
    path.rsplit('/')
        .find(|segment| {
            !segment.is_empty() && !segment.starts_with(':') && !segment.starts_with('{')
        })
        .unwrap_or("resource")
        .to_string()
}

