//! OpenAI tester. GETs `/v1/models` to verify the API key is
//! accepted. Returns the count of accessible models on success.

use serde_json::Value;

use super::http_client;

pub async fn test(secret: Option<&str>) -> Result<String, String> {
    let key = secret.ok_or("missing API key")?;
    let response = http_client()
        .get("https://api.openai.com/v1/models")
        .bearer_auth(key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("network error: {err}"))?;

    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        let count = body
            .get("data")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(format!("authenticated; {count} models accessible"))
    } else {
        let message = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| body.to_string());
        Err(format!("OpenAI returned {}: {message}", status.as_u16()))
    }
}
