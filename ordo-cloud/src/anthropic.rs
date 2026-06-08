//! Anthropic-specific helpers for the `messages` API.
//!
//! The assistant service uses OpenAI-style messages arrays (role +
//! content, plus `tool_calls` / `tool` role for agentic loops). This
//! module translates that shape into Anthropic's format on the way
//! in and normalizes the response back into the same OpenAI-ish shape
//! on the way out, so `AssistantService`'s turn loop doesn't need to
//! branch on the provider beyond choosing which function to call.
//!
//! Translation summary:
//!   - All `role: "system"` messages are concatenated into Anthropic's
//!     top-level `system` string.
//!   - `role: "user"` / `role: "assistant"` string-content messages
//!     pass through.
//!   - `role: "assistant"` with `tool_calls` becomes an assistant
//!     message whose `content` is an array of `tool_use` blocks.
//!   - `role: "tool"` becomes `role: "user"` with a `tool_result`
//!     block referencing the originating `tool_use` id.
//!   - OpenAI's `tools` schema (`[{type,function:{name,description,parameters}}]`)
//!     is unwrapped into Anthropic's `[{name,description,input_schema}]`.
//!
//! Response normalization:
//!   - Text content blocks concatenate into `assistant_message`.
//!   - `tool_use` blocks surface as OpenAI-style `tool_calls` entries
//!     (id, `type: "function"`, `function.name`, `function.arguments`
//!     stringified).
//!   - `stop_reason: "tool_use"` maps to `finish_reason: "tool_calls"`.

use reqwest::Method;
use serde_json::{json, Value};

use crate::{CloudCredential, CloudError, CloudHttp, CloudResult};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
pub const DEFAULT_MODEL: &str = "claude-3-5-haiku-latest";
pub const DEFAULT_MAX_TOKENS: u64 = 1024;

/// Call `POST /messages` with a list of user/assistant messages.
pub async fn messages(
    http: &CloudHttp,
    credential: &CloudCredential,
    arguments: &Value,
) -> CloudResult<Value> {
    let model = arguments
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or(DEFAULT_MODEL)
        .to_string();
    let incoming = arguments
        .get("messages")
        .cloned()
        .ok_or_else(|| CloudError::InvalidArgument("missing required field 'messages'".into()))?;
    if !incoming.is_array() {
        return Err(CloudError::InvalidArgument(
            "'messages' must be an array of {role, content} objects".into(),
        ));
    }

    let max_tokens = arguments
        .get("max_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(DEFAULT_MAX_TOKENS);

    // --- Translate OpenAI-style messages into Anthropic's shape ----
    let (system_prompt, translated_messages) = translate_messages(incoming.as_array().unwrap());

    // Prompt caching is on by default; callers can disable with `"cache": false`.
    let cache_enabled = arguments
        .get("cache")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": translated_messages,
    });
    // Explicit `system` argument wins over anything extracted from
    // the messages array; otherwise stitch together all system
    // messages we extracted. When caching is on, system goes in as
    // a content block carrying `cache_control: {type: "ephemeral"}` so
    // Anthropic caches the prefix (~80% cost reduction on cache hits).
    let system_text = arguments
        .get("system")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| system_prompt.clone());
    if !system_text.is_empty() {
        body["system"] = if cache_enabled {
            json!([{
                "type": "text",
                "text": system_text,
                "cache_control": {"type": "ephemeral"},
            }])
        } else {
            json!(system_text)
        };
    }
    if let Some(temperature) = arguments
        .get("temperature")
        .and_then(|value| value.as_f64())
    {
        body["temperature"] = json!(temperature);
    }
    // Translate tool schemas if present. When caching is on, mark the
    // last tool with `cache_control` â€” Anthropic caches the whole tool
    // array up to and including the marked block, so one marker at the
    // tail covers the full toolbox.
    if let Some(tools) = arguments.get("tools").and_then(|v| v.as_array()) {
        let mut translated = translate_tool_schemas(tools);
        if cache_enabled {
            if let Some(last) = translated.last_mut() {
                if let Some(obj) = last.as_object_mut() {
                    obj.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
                }
            }
        }
        body["tools"] = json!(translated);
        // Anthropic's `tool_choice` takes a different shape; default
        // to `{type: "auto"}` when the caller asked for `"auto"`.
        if let Some(choice) = arguments.get("tool_choice") {
            body["tool_choice"] = translate_tool_choice(choice);
        }
    }

    let response = http
        .send_json(credential, Method::POST, "/messages", Some(&body), &[])
        .await?;

    Ok(normalize_response(&model, response))
}

fn translate_messages(messages: &[Value]) -> (String, Vec<Value>) {
    let mut system_chunks = Vec::new();
    let mut translated = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" => {
                if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                    system_chunks.push(text.to_string());
                }
            }
            "user" => {
                // Phase 1.3: when user.content is an array (multimodal),
                // convert OpenAI-native `image_url` blocks into
                // Anthropic's `image` block shape. Text blocks and any
                // already-Anthropic blocks (e.g. existing tool_result
                // arrays) pass through unchanged.
                if let Some(content_array) = message.get("content").and_then(|v| v.as_array()) {
                    let mut translated_content: Vec<Value> =
                        Vec::with_capacity(content_array.len());
                    for block in content_array {
                        let kind = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match kind {
                            "image_url" => {
                                let url = block
                                    .pointer("/image_url/url")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if let Some(img) = image_url_to_anthropic_block(url) {
                                    translated_content.push(img);
                                }
                                // URL-only (non-data) images are skipped
                                // for Anthropic â€” the provider requires
                                // base64 for vision input. The OpenAI
                                // path still gets the url block via the
                                // original message, this conversion is
                                // Anthropic-specific.
                            }
                            _ => translated_content.push(block.clone()),
                        }
                    }
                    let mut converted = message.clone();
                    if let Some(obj) = converted.as_object_mut() {
                        obj.insert("content".into(), json!(translated_content));
                    }
                    translated.push(converted);
                } else {
                    translated.push(message.clone());
                }
            }
            "assistant" => {
                // Assistant messages may carry tool_calls; if so,
                // convert them into Anthropic `tool_use` blocks. Mix
                // with any free text in `content`.
                let text = message
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tool_calls = message
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                if tool_calls.is_empty() {
                    // Plain assistant reply â€” pass through as-is.
                    translated.push(message.clone());
                    continue;
                }
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(json!({"type": "text", "text": text}));
                }
                for call in tool_calls {
                    let id = call
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = call
                        .pointer("/function/name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let arguments_raw = call
                        .pointer("/function/arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    let input: Value = serde_json::from_str(arguments_raw)
                        .unwrap_or(Value::Object(Default::default()));
                    content.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input,
                    }));
                }
                translated.push(json!({"role": "assistant", "content": content}));
            }
            "tool" => {
                let tool_call_id = message
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let content_text = message
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                translated.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content_text,
                    }],
                }));
            }
            _ => {
                // Unknown role â€” pass through and let Anthropic reject it.
                translated.push(message.clone());
            }
        }
    }
    (system_chunks.join("\n\n"), translated)
}

/// Convert an OpenAI-style image url into Anthropic's image block.
/// Only `data:` URLs are supported because Anthropic requires inline
/// base64 for vision input; remote URLs are dropped with a warning
/// trace rather than failing the turn.
fn image_url_to_anthropic_block(url: &str) -> Option<Value> {
    let prefix = "data:";
    if !url.starts_with(prefix) {
        tracing::warn!(
            target: "ordo_cloud",
            url = url,
            "anthropic translator: dropping non-data:// image url (Anthropic requires base64)"
        );
        return None;
    }
    // data:<media>;base64,<data>
    let tail = &url[prefix.len()..];
    let (meta, data) = tail.split_once(',')?;
    let (media_type, encoding) = meta.split_once(';').unwrap_or((meta, ""));
    if !encoding.contains("base64") {
        tracing::warn!(
            target: "ordo_cloud",
            "anthropic translator: data url not base64-encoded, dropping"
        );
        return None;
    }
    Some(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": data,
        }
    }))
}

fn translate_tool_schemas(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| {
            let function = tool.get("function")?;
            let name = function.get("name")?.as_str()?.to_string();
            let description = function
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = function
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object"}));
            Some(json!({
                "name": name,
                "description": description,
                "input_schema": input_schema,
            }))
        })
        .collect()
}

fn translate_tool_choice(choice: &Value) -> Value {
    match choice {
        Value::String(s) if s == "auto" => json!({"type": "auto"}),
        Value::String(s) if s == "required" => json!({"type": "any"}),
        Value::String(s) if s == "none" => json!({"type": "none"}),
        _ => json!({"type": "auto"}),
    }
}

fn normalize_response(model: &str, response: Value) -> Value {
    let content = response
        .get("content")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut assistant_text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for block in &content {
        let ty = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "text" => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    assistant_text_parts.push(text.to_string());
                }
            }
            "tool_use" => {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments,
                    }
                }));
            }
            _ => {}
        }
    }

    let stop_reason = response
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // Map "tool_use" â†’ OpenAI's "tool_calls" finish_reason for
    // loop-parity.
    let finish_reason = if stop_reason == "tool_use" {
        "tool_calls".to_string()
    } else {
        stop_reason.clone()
    };

    // Surface cache usage from Anthropic's `usage` block so the
    // assistant can emit hit/miss metrics. Values are zero when the
    // request didn't use caching.
    let usage = response.get("usage").cloned().unwrap_or(Value::Null);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let joined = assistant_text_parts.join("");
    json!({
        "model": model,
        "assistant_message": joined.clone(),
        // Anthropic doesn't have a separate reasoning channel like
        // qwen3/deepseek-r1 — its text blocks ARE the answer. Surface
        // the same raw content as `content_raw` for parity with the
        // OpenAI path so callers can use the same field uniformly.
        "content_raw": joined.clone(),
        "reasoning": "",
        // Alias kept for compatibility with the original thin wrapper.
        "assistant_text": joined,
        "stop_reason": stop_reason,
        "finish_reason": finish_reason,
        "tool_calls": tool_calls,
        "usage": usage,
        "cache": {
            "read_tokens": cache_read,
            "creation_tokens": cache_creation,
            "hit": cache_read > 0,
        },
        "raw": response,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_extracts_system_and_preserves_user() {
        let messages = vec![
            json!({"role": "system", "content": "You are helpful."}),
            json!({"role": "user", "content": "hi"}),
        ];
        let (system, translated) = translate_messages(&messages);
        assert_eq!(system, "You are helpful.");
        assert_eq!(translated.len(), 1);
        assert_eq!(translated[0]["role"], "user");
    }

    #[test]
    fn translate_assistant_tool_calls_to_tool_use_blocks() {
        let messages = vec![json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call_123",
                "type": "function",
                "function": {"name": "assistant.recall_memory", "arguments": "{\"query\":\"brand\"}"}
            }]
        })];
        let (_, translated) = translate_messages(&messages);
        assert_eq!(translated.len(), 1);
        let content = translated[0]["content"].as_array().expect("content array");
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "assistant.recall_memory");
        assert_eq!(content[0]["id"], "call_123");
        assert_eq!(content[0]["input"]["query"], "brand");
    }

    #[test]
    fn translate_tool_role_to_user_with_tool_result() {
        let messages = vec![json!({
            "role": "tool",
            "tool_call_id": "call_123",
            "content": "{\"hits\":[]}"
        })];
        let (_, translated) = translate_messages(&messages);
        assert_eq!(translated[0]["role"], "user");
        let content = translated[0]["content"].as_array().expect("content array");
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_123");
    }

    #[test]
    fn translate_tool_schemas_unwraps_function_envelope() {
        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "t",
                "description": "d",
                "parameters": {"type": "object"}
            }
        })];
        let translated = translate_tool_schemas(&tools);
        assert_eq!(translated[0]["name"], "t");
        assert_eq!(translated[0]["input_schema"]["type"], "object");
    }

    #[test]
    fn user_content_array_converts_image_url_data_to_anthropic_block() {
        let messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "what's in this screenshot?"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
            ]
        })];
        let (_, translated) = translate_messages(&messages);
        let content = translated[0]["content"].as_array().expect("array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "AAAA");
    }

    #[test]
    fn user_content_array_drops_remote_image_url_for_anthropic() {
        let messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "look"},
                {"type": "image_url", "image_url": {"url": "https://example.com/pic.png"}}
            ]
        })];
        let (_, translated) = translate_messages(&messages);
        let content = translated[0]["content"].as_array().expect("array");
        // Remote URL dropped; text preserved.
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[test]
    fn normalize_response_surfaces_cache_usage() {
        let raw = json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "ok"}],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_read_input_tokens": 1234,
                "cache_creation_input_tokens": 0
            }
        });
        let normalized = normalize_response("claude-test", raw);
        assert_eq!(normalized["cache"]["read_tokens"], 1234);
        assert_eq!(normalized["cache"]["hit"], true);
    }

    #[test]
    fn normalize_response_extracts_text_and_tool_calls() {
        let raw = json!({
            "stop_reason": "tool_use",
            "content": [
                {"type": "text", "text": "thinking..."},
                {"type": "tool_use", "id": "tu_1", "name": "assistant.recall_memory", "input": {"query": "brand"}}
            ]
        });
        let normalized = normalize_response("claude-test", raw);
        assert_eq!(normalized["assistant_message"], "thinking...");
        assert_eq!(normalized["finish_reason"], "tool_calls");
        let calls = normalized["tool_calls"].as_array().expect("array");
        assert_eq!(calls[0]["function"]["name"], "assistant.recall_memory");
        assert!(calls[0]["function"]["arguments"]
            .as_str()
            .unwrap()
            .contains("brand"));
    }
}
