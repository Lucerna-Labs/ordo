//! OpenAI-specific helpers: chat completions, embeddings, and speech.

use futures::Stream;
use reqwest::header::CONTENT_TYPE;
use reqwest::Method;
use serde_json::{json, Value};

use crate::{CloudCredential, CloudError, CloudHttp, CloudResult};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_CHAT_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const DEFAULT_TTS_MODEL: &str = "gpt-4o-mini-tts";
pub const DEFAULT_TTS_VOICE: &str = "alloy";
pub const DEFAULT_TTS_FORMAT: &str = "mp3";

#[derive(Debug, Clone)]
pub struct SpeechAudio {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub format: String,
}

/// Call `POST /chat/completions` and return the raw JSON response plus the
/// first assistant message as a string.
pub async fn chat(
    http: &CloudHttp,
    credential: &CloudCredential,
    arguments: &Value,
) -> CloudResult<Value> {
    let model = arguments
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or(DEFAULT_CHAT_MODEL)
        .to_string();
    let messages = arguments
        .get("messages")
        .cloned()
        .ok_or_else(|| CloudError::InvalidArgument("missing required field 'messages'".into()))?;
    if !messages.is_array() {
        return Err(CloudError::InvalidArgument(
            "'messages' must be an array of {role, content} objects".into(),
        ));
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
    });
    if let Some(temperature) = arguments
        .get("temperature")
        .and_then(|value| value.as_f64())
    {
        body["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = arguments.get("max_tokens").and_then(|value| value.as_u64()) {
        body["max_tokens"] = json!(max_tokens);
    }
    // Tool-use fields — forwarded verbatim when the caller supplies them.
    if let Some(tools) = arguments.get("tools") {
        body["tools"] = tools.clone();
    }
    if let Some(tool_choice) = arguments.get("tool_choice") {
        body["tool_choice"] = tool_choice.clone();
    }

    let response = http
        .send_json(
            credential,
            Method::POST,
            "/chat/completions",
            Some(&body),
            &[],
        )
        .await?;

    let content = response
        .pointer("/choices/0/message/content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    // Thinking-style providers (Ollama with qwen3.x / deepseek-r1, …)
    // place the model's stream-of-thought in a non-standard `reasoning`
    // field and may emit an empty `content` if the token budget runs
    // out before the visible answer.
    //
    // We expose three fields downstream so callers can pick the right
    // one for the right job:
    //   - `content_raw`: the literal `content` string from the model.
    //     Empty when the turn is a pure tool-call response or when a
    //     reasoning model burned its budget on thinking. This is what
    //     belongs in the conversation history (OpenAI spec allows
    //     content="" alongside tool_calls); putting a UI fallback in
    //     that slot would confuse the next iteration.
    //   - `reasoning`: the model's pre-answer thinking trace, if any.
    //     Useful as a fallback for the operator when content is empty,
    //     but should NOT be fed back to the model as if it were the
    //     model's own previous content — that loops the model on
    //     meta-commentary.
    //   - `assistant_message`: a UI-friendly view that prefers content
    //     and falls back to reasoning so a silent tool-only turn
    //     surfaces *something* in the chat bubble. Operator-facing
    //     only — never rejoin into the prompt context.
    let reasoning = response
        .pointer("/choices/0/message/reasoning")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let assistant = if content.is_empty() && !reasoning.is_empty() {
        format!("(no content emitted; reasoning preview)\n{reasoning}")
    } else {
        content.to_string()
    };
    let tool_calls = response
        .pointer("/choices/0/message/tool_calls")
        .cloned()
        .unwrap_or(Value::Null);
    let finish_reason = response
        .pointer("/choices/0/finish_reason")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    Ok(json!({
        "model": model,
        "assistant_message": assistant,
        "content_raw": content,
        "reasoning": reasoning,
        "tool_calls": tool_calls,
        "finish_reason": finish_reason,
        "raw": response,
    }))
}

/// Call `POST /audio/speech` and return encoded audio bytes.
///
/// This uses the OpenAI-compatible speech endpoint. Providers that
/// implement the same `/v1/audio/speech` contract can reuse the same
/// credential path; providers with different TTS APIs should get their
/// own wrapper rather than being forced through this shape.
pub async fn speech(
    http: &CloudHttp,
    credential: &CloudCredential,
    arguments: &Value,
) -> CloudResult<SpeechAudio> {
    let input = arguments
        .get("input")
        .or_else(|| arguments.get("text"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| CloudError::InvalidArgument("missing required field 'input'".into()))?;
    if input.trim().is_empty() {
        return Err(CloudError::InvalidArgument(
            "speech input must not be empty".into(),
        ));
    }
    if input.chars().count() > 4096 {
        return Err(CloudError::InvalidArgument(
            "speech input must be 4096 characters or fewer".into(),
        ));
    }

    let model = arguments
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or(DEFAULT_TTS_MODEL);
    let voice = arguments
        .get("voice")
        .and_then(|value| value.as_str())
        .unwrap_or(DEFAULT_TTS_VOICE);
    let format = arguments
        .get("response_format")
        .or_else(|| arguments.get("format"))
        .and_then(|value| value.as_str())
        .unwrap_or(DEFAULT_TTS_FORMAT);

    let mut body = json!({
        "model": model,
        "voice": voice,
        "input": input,
        "response_format": format,
    });
    if let Some(instructions) = arguments.get("instructions").and_then(|value| value.as_str()) {
        if !instructions.trim().is_empty() {
            body["instructions"] = json!(instructions);
        }
    }
    if let Some(speed) = arguments.get("speed").and_then(|value| value.as_f64()) {
        body["speed"] = json!(speed.clamp(0.25, 4.0));
    }

    let response = http
        .send_request(
            credential,
            Method::POST,
            "/audio/speech",
            Some(&body),
            &[],
        )
        .await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| content_type_for_format(format))
        .to_string();
    let bytes = response.bytes().await.map_err(|err| CloudError::Request {
        service: credential.service.clone(),
        message: err.to_string(),
    })?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).to_string();
        return Err(CloudError::BadStatus {
            service: credential.service.clone(),
            status: status.as_u16(),
            body,
        });
    }
    Ok(SpeechAudio {
        bytes: bytes.to_vec(),
        content_type,
        format: format.to_string(),
    })
}

pub(crate) fn content_type_for_format(format: &str) -> &'static str {
    match format {
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "opus" => "audio/opus",
        "pcm" => "audio/L16",
        "wav" => "audio/wav",
        _ => "audio/mpeg",
    }
}

/// Events surfaced by `chat_stream`. Matches the bits of OpenAI's
/// SSE protocol the assistant loop cares about — token deltas,
/// (optional) tool-call deltas, and the final \"done\" marker.
#[derive(Debug, Clone)]
pub enum ChatStreamEvent {
    /// Chunk of assistant text. Concatenating every `TokenDelta` gives
    /// the final assistant_message.
    TokenDelta { delta: String },
    /// OpenAI reports tool-call chunks in streaming mode; we surface
    /// them as raw JSON deltas in case a caller wants to reconstruct.
    /// The current assistant loop does not use streaming when tools
    /// are enabled, so this is primarily informational.
    ToolCallDelta { raw: Value },
    /// Stream finished. `finish_reason` mirrors the non-stream path.
    Done { finish_reason: Option<String> },
    /// Transport / parse error. The caller should treat the stream
    /// as terminated.
    Error { message: String },
}

/// Streaming chat completion. Returns a `Stream<Item = ChatStreamEvent>`;
/// each `TokenDelta` is a chunk of the assistant's reply as it arrives.
/// Use this for UX-facing \"typing\" indicators. Tool-use streaming
/// adds a lot of state-machine complexity; callers are expected to
/// drop back to the non-streaming `chat()` when tools are in play.
pub async fn chat_stream(
    http: &CloudHttp,
    credential: &CloudCredential,
    arguments: &Value,
) -> CloudResult<impl Stream<Item = ChatStreamEvent> + Send> {
    let model = arguments
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or(DEFAULT_CHAT_MODEL)
        .to_string();
    let messages = arguments
        .get("messages")
        .cloned()
        .ok_or_else(|| CloudError::InvalidArgument("missing required field 'messages'".into()))?;

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
    });
    if let Some(temperature) = arguments
        .get("temperature")
        .and_then(|value| value.as_f64())
    {
        body["temperature"] = json!(temperature);
    }

    let response = http
        .send_request(
            credential,
            Method::POST,
            "/chat/completions",
            Some(&body),
            &[],
        )
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(CloudError::Request {
            service: credential.service.clone(),
            message: format!("status {status}: {text}"),
        });
    }

    let bytes_stream = response.bytes_stream();
    Ok(sse_to_events(bytes_stream))
}

/// Convert a raw byte stream of `data: {...}\n\n` SSE events into
/// `ChatStreamEvent`s. Splits on newlines, strips the `data: ` prefix,
/// and translates `[DONE]` into the `Done` variant.
fn sse_to_events<S>(stream: S) -> impl Stream<Item = ChatStreamEvent> + Send
where
    S: Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static,
{
    use futures::StreamExt;
    let state = std::cell::RefCell::new((String::new(), Option::<String>::None));
    let _ = &state; // suppress unused warning in macro paths
    async_stream::stream! {
        let mut buffer = String::new();
        let mut finish_reason: Option<String> = None;
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(err) => {
                    yield ChatStreamEvent::Error { message: err.to_string() };
                    return;
                }
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            buffer.push_str(text);
            // Events are separated by blank lines. Pull out completed
            // frames and leave the rest in the buffer.
            while let Some(idx) = buffer.find("\n\n") {
                let frame = buffer[..idx].to_string();
                buffer.drain(..idx + 2);
                for line in frame.lines() {
                    let line = line.trim_start();
                    let payload = if let Some(rest) = line.strip_prefix("data:") {
                        rest.trim_start()
                    } else {
                        continue;
                    };
                    if payload == "[DONE]" {
                        yield ChatStreamEvent::Done { finish_reason: finish_reason.clone() };
                        return;
                    }
                    let json: Value = match serde_json::from_str(payload) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if let Some(reason) = json
                        .pointer("/choices/0/finish_reason")
                        .and_then(|v| v.as_str())
                    {
                        finish_reason = Some(reason.to_string());
                    }
                    if let Some(content) = json
                        .pointer("/choices/0/delta/content")
                        .and_then(|v| v.as_str())
                    {
                        if !content.is_empty() {
                            yield ChatStreamEvent::TokenDelta { delta: content.to_string() };
                        }
                    }
                    if let Some(tool) = json.pointer("/choices/0/delta/tool_calls") {
                        yield ChatStreamEvent::ToolCallDelta { raw: tool.clone() };
                    }
                }
            }
        }
        // Upstream closed without a [DONE] — surface whatever
        // finish_reason we saw.
        yield ChatStreamEvent::Done { finish_reason };
    }
}

/// Call `POST /embeddings` for one or more input strings.
pub async fn embed(
    http: &CloudHttp,
    credential: &CloudCredential,
    arguments: &Value,
) -> CloudResult<Value> {
    let model = arguments
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or(DEFAULT_EMBEDDING_MODEL)
        .to_string();
    let input = arguments.get("input").cloned().ok_or_else(|| {
        CloudError::InvalidArgument("missing required field 'input' (string or [string])".into())
    })?;
    let body = json!({ "model": model, "input": input });
    let response = http
        .send_json(credential, Method::POST, "/embeddings", Some(&body), &[])
        .await?;
    let vector_count = response
        .pointer("/data")
        .and_then(|value| value.as_array())
        .map(|array| array.len())
        .unwrap_or(0);
    Ok(json!({
        "model": model,
        "vector_count": vector_count,
        "raw": response,
    }))
}
