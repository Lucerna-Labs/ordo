//! Anthropic tester. POSTs a 1-token messages call against the
//! cheapest model. The Anthropic API doesn't have a free
//! verify-credentials endpoint; this is the standard "I'm alive"
//! probe. Even on 401 we get a clean structured error to surface.

use serde_json::{json, Value};

use super::http_client;

pub async fn test(secret: Option<&str>) -> Result<String, String> {
    let key = secret.ok_or("missing API key")?;
    let body = json!({
        "model": "claude-haiku-4-5",
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "ping"}]
    });
    let response = http_client()
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|err| format!("network error: {err}"))?;

    let status = response.status();
    let payload: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        let model = payload
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        Ok(format!("authenticated; round-trip OK against {model}"))
    } else {
        let message = payload
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| payload.to_string());
        Err(format!("Anthropic returned {}: {message}", status.as_u16()))
    }
}
