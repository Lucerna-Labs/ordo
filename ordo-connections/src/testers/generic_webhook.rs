//! Generic webhook tester. Sends a small `{"_test": true}`
//! payload to the configured URL using the configured method
//! (default POST). Any 2xx counts as success.

use serde_json::{json, Value};

use super::http_client;

pub async fn test(fields: &Value, secret: Option<&str>) -> Result<String, String> {
    let url = fields
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("missing field: url")?;
    let method = fields
        .get("method")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("POST")
        .to_uppercase();

    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|err| format!("invalid method: {err}"))?;
    let mut builder = http_client().request(method.clone(), url);
    if let Some(token) = secret.filter(|s| !s.is_empty()) {
        builder = builder.bearer_auth(token);
    }
    if matches!(method, reqwest::Method::POST | reqwest::Method::PUT) {
        builder = builder.json(&json!({ "_test": true }));
    }
    let response = builder
        .send()
        .await
        .map_err(|err| format!("network error: {err}"))?;
    let status = response.status();
    if status.is_success() {
        Ok(format!(
            "webhook accepted {} (HTTP {})",
            url,
            status.as_u16()
        ))
    } else {
        let body = response.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        Err(format!("webhook returned {}: {snippet}", status.as_u16()))
    }
}
