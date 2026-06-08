//! HTTP client wrapper for the Ordo control API.
//!
//! The MCP bridge intentionally knows nothing about the runtime's
//! internals â€” it only talks to the public endpoints documented in
//! `ordo-control/src/lib.rs`. When those endpoints evolve, this file
//! is the single place that changes.

use std::time::Duration;

use reqwest::Method;
use serde_json::Value;

use crate::config::Config;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("http transport: {0}")]
    Transport(String),
    #[error("runtime returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("response was not valid JSON: {0}")]
    InvalidJson(String),
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Clone)]
pub struct RuntimeClient {
    http: reqwest::Client,
    config: Config,
}

impl RuntimeClient {
    pub fn new(config: Config) -> RuntimeResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        Ok(Self { http, config })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// GET /health â€” used to fail-fast at startup so the MCP server
    /// doesn't silently hand "empty toolbox" errors to Claude Desktop.
    pub async fn probe(&self) -> RuntimeResult<Value> {
        self.request(Method::GET, "/health", None).await
    }

    /// GET /api/capabilities â€” the master tool inventory.
    pub async fn list_capabilities(&self) -> RuntimeResult<Value> {
        self.request(Method::GET, "/api/capabilities", None).await
    }

    /// POST /api/tools/:capability â€” generic tool invocation. Most
    /// MCP tool calls go through this.
    pub async fn invoke_tool(&self, capability: &str, arguments: &Value) -> RuntimeResult<Value> {
        let path = format!("/api/tools/{}", capability);
        self.request(Method::POST, &path, Some(arguments.clone()))
            .await
    }

    // ---- Apps convenience wrappers -------------------------------------
    pub async fn list_apps(&self, query: &Value) -> RuntimeResult<Value> {
        let mut url = format!("{}/api/apps", self.config.runtime_url);
        let qs = query_string_from_json(query);
        if !qs.is_empty() {
            url.push('?');
            url.push_str(&qs);
        }
        self.raw_request(Method::GET, &url, None).await
    }

    pub async fn create_app(&self, body: &Value) -> RuntimeResult<Value> {
        self.request(Method::POST, "/api/apps", Some(body.clone()))
            .await
    }

    pub async fn get_app(&self, id: &str) -> RuntimeResult<Value> {
        self.request(Method::GET, &format!("/api/apps/{id}"), None)
            .await
    }

    pub async fn update_app(&self, id: &str, body: &Value) -> RuntimeResult<Value> {
        self.request(
            Method::PATCH,
            &format!("/api/apps/{id}"),
            Some(body.clone()),
        )
        .await
    }

    pub async fn publish_app(&self, id: &str, body: &Value) -> RuntimeResult<Value> {
        self.request(
            Method::POST,
            &format!("/api/apps/{id}/publish"),
            Some(body.clone()),
        )
        .await
    }

    pub async fn archive_app(&self, id: &str) -> RuntimeResult<Value> {
        self.request(Method::DELETE, &format!("/api/apps/{id}"), None)
            .await
    }

    pub async fn list_app_events(&self, id: &str) -> RuntimeResult<Value> {
        self.request(Method::GET, &format!("/api/apps/{id}/events"), None)
            .await
    }

    // ---- Files convenience wrappers ------------------------------------
    pub async fn list_files(&self, query: &Value) -> RuntimeResult<Value> {
        let mut url = format!("{}/api/files", self.config.runtime_url);
        let qs = query_string_from_json(query);
        if !qs.is_empty() {
            url.push('?');
            url.push_str(&qs);
        }
        self.raw_request(Method::GET, &url, None).await
    }

    pub async fn get_file_metadata(&self, id: &str) -> RuntimeResult<Value> {
        self.request(Method::GET, &format!("/api/files/{id}"), None)
            .await
    }

    pub async fn upload_file(&self, body: &Value) -> RuntimeResult<Value> {
        self.request(Method::POST, "/api/files", Some(body.clone()))
            .await
    }

    pub async fn delete_file(&self, id: &str) -> RuntimeResult<Value> {
        self.request(Method::DELETE, &format!("/api/files/{id}"), None)
            .await
    }

    // ---- Assistant convenience wrappers --------------------------------
    pub async fn assistant_turn(&self, body: &Value) -> RuntimeResult<Value> {
        self.request(Method::POST, "/api/assistant/turn", Some(body.clone()))
            .await
    }

    pub async fn assistant_recall(&self, body: &Value) -> RuntimeResult<Value> {
        self.request(Method::POST, "/api/assistant/recall", Some(body.clone()))
            .await
    }

    // ---- Low-level ------------------------------------------------------
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> RuntimeResult<Value> {
        let url = format!("{}{}", self.config.runtime_url, path);
        self.raw_request(method, &url, body).await
    }

    async fn raw_request(
        &self,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> RuntimeResult<Value> {
        let mut req = self.http.request(method, url);
        if let Some(token) = &self.config.api_token {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req
            .send()
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        if !status.is_success() {
            return Err(RuntimeError::Status {
                status: status.as_u16(),
                body: text,
            });
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str::<Value>(&text)
            .map_err(|err| RuntimeError::InvalidJson(format!("{err}: {text}")))
    }
}

/// Flatten a JSON object into a URL query string. Non-object inputs
/// produce an empty string (caller inspects `is_empty`).
fn query_string_from_json(value: &Value) -> String {
    let Some(obj) = value.as_object() else {
        return String::new();
    };
    let mut out = String::new();
    for (key, value) in obj {
        if value.is_null() {
            continue;
        }
        if !out.is_empty() {
            out.push('&');
        }
        let v = match value {
            Value::String(s) => s.clone(),
            other => other.to_string().trim_matches('"').to_string(),
        };
        out.push_str(&urlencode(key));
        out.push('=');
        out.push_str(&urlencode(&v));
    }
    out
}

/// Minimal URL-encoder â€” enough for scalar query values. Avoids
/// pulling `urlencoding` crate.
fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match *byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_string_from_json_skips_nulls_and_encodes() {
        let value = json!({
            "workspace_id": "local",
            "status": "draft",
            "limit": null,
            "needs encoding": "a b&c"
        });
        let qs = query_string_from_json(&value);
        assert!(qs.contains("workspace_id=local"));
        assert!(qs.contains("status=draft"));
        assert!(!qs.contains("limit="));
        assert!(qs.contains("needs%20encoding=a%20b%26c"));
    }

    #[test]
    fn query_string_empty_for_non_object() {
        assert_eq!(query_string_from_json(&json!("scalar")), "");
        assert_eq!(query_string_from_json(&json!(null)), "");
    }
}
