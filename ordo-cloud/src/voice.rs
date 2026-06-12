//! Provider-agnostic voice (text-to-speech) dispatch.
//!
//! The avatar — and the assistant "speak responses" feature — should be
//! able to use *any* configured voice provider, not just OpenAI's
//! `/audio/speech`. Different vendors expose different TTS HTTP
//! contracts; this module picks the right one per credential and
//! normalizes the result back to [`SpeechAudio`] so callers stay
//! provider-blind.
//!
//! Resolution order for which API shape a credential speaks:
//!   1. Explicit `extras["voice_api"]` on the credential
//!      (`"openai"` / `"openai_compatible"` / `"minimax"`).
//!   2. Inference from the service name or base_url — anything
//!      containing `"minimax"` is treated as MiniMax.
//!   3. Default: OpenAI-compatible (`POST {base_url}/audio/speech`),
//!      which covers OpenAI itself plus the many gateways that clone
//!      its contract.
//!
//! Adding a provider is a two-line change: a new [`VoiceApi`] variant
//! and one async wrapper. Callers never change — [`synthesize`] is the
//! single entry point, and [`defaults_for`] gives the right model/voice
//! fallbacks per API so an OpenAI default (`alloy`) never leaks into a
//! MiniMax request.

use serde_json::{json, Value};

use reqwest::Method;

use crate::openai::{self, SpeechAudio};
use crate::{
    apply_auth_only, resolve_url, timeout_for, CloudCredential, CloudError, CloudHttp, CloudResult,
};

/// Default OpenAI-compatible speech-to-text model.
const DEFAULT_STT_MODEL: &str = "whisper-1";

/// Result of a speech-to-text transcription.
#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    pub model: String,
}

/// Transcribe `audio` (raw bytes of the given container `format`, e.g. "webm",
/// "wav", "mp3") via the credential's OpenAI-compatible
/// `POST {base_url}/audio/transcriptions` endpoint (multipart). Works against
/// OpenAI Whisper and any local server that implements the same contract
/// (whisper.cpp / faster-whisper / LocalAI) — the credential's `base_url`
/// selects which. Recognized `arguments`: `model`, `language`.
///
/// Beta scope: OpenAI-compatible (Whisper) shape only. A MiniMax-style ASR
/// adapter can be added later the same way the MiniMax TTS adapter was.
pub async fn transcribe(
    http: &CloudHttp,
    credential: &CloudCredential,
    audio: Vec<u8>,
    format: &str,
    arguments: &Value,
) -> CloudResult<Transcript> {
    if audio.is_empty() {
        return Err(CloudError::InvalidArgument(
            "transcription audio must not be empty".into(),
        ));
    }
    let model = arguments
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| credential.extras.get("stt_model").cloned())
        .unwrap_or_else(|| DEFAULT_STT_MODEL.to_string());

    let ext = sanitize_audio_ext(format);
    let part = reqwest::multipart::Part::bytes(audio)
        .file_name(format!("audio.{ext}"))
        .mime_str(audio_mime_for_ext(&ext))
        .map_err(|err| CloudError::InvalidArgument(err.to_string()))?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model.clone())
        .text("response_format", "json");
    if let Some(language) = arguments.get("language").and_then(Value::as_str) {
        if !language.trim().is_empty() {
            form = form.text("language", language.to_string());
        }
    }

    // Build the request directly so reqwest sets the multipart content-type
    // (with boundary); apply only the credential's auth header, never a JSON
    // content-type.
    let url = resolve_url(credential, "/audio/transcriptions")?;
    let builder = http
        .client
        .request(Method::POST, url)
        .timeout(timeout_for(credential));
    let builder = apply_auth_only(builder, credential)?;

    let response = builder
        .multipart(form)
        .send()
        .await
        .map_err(|err| CloudError::Request {
            service: credential.service.clone(),
            message: err.to_string(),
        })?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|err| CloudError::Request {
        service: credential.service.clone(),
        message: err.to_string(),
    })?;
    if !status.is_success() {
        return Err(CloudError::BadStatus {
            service: credential.service.clone(),
            status: status.as_u16(),
            body: String::from_utf8_lossy(&bytes).to_string(),
        });
    }
    // OpenAI / Whisper return { "text": "..." } for response_format=json.
    let payload: Value = serde_json::from_slice(&bytes).map_err(|err| CloudError::Request {
        service: credential.service.clone(),
        message: format!("transcription response was not JSON: {err}"),
    })?;
    let text = payload
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::Request {
            service: credential.service.clone(),
            message: "transcription response missing 'text'".into(),
        })?
        .to_string();

    Ok(Transcript { text, model })
}

/// Restrict an audio container hint to a small safe set for the upload
/// filename extension; unknown values fall back to a generic container.
fn sanitize_audio_ext(format: &str) -> String {
    let f = format.trim().trim_start_matches('.').to_ascii_lowercase();
    match f.as_str() {
        "webm" | "ogg" | "oga" | "wav" | "mp3" | "mp4" | "m4a" | "mpeg" | "mpga" | "flac" => f,
        _ => "webm".to_string(),
    }
}

fn audio_mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "wav" => "audio/wav",
        "mp3" | "mpga" | "mpeg" => "audio/mpeg",
        "mp4" | "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        _ => "audio/webm",
    }
}

/// MiniMax T2A v2 defaults. `speech-02-hd` is the current
/// high-definition model; `male-qn-qingse` is a stock preset voice.
const MINIMAX_DEFAULT_MODEL: &str = "speech-02-hd";
const MINIMAX_DEFAULT_VOICE: &str = "male-qn-qingse";
const MINIMAX_DEFAULT_FORMAT: &str = "mp3";

/// Which TTS HTTP contract a credential speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceApi {
    /// `POST {base_url}/audio/speech` — OpenAI and compatible gateways.
    OpenAiCompatible,
    /// MiniMax T2A v2 — `POST {base_url}/t2a_v2`; audio is returned as
    /// a hex string under `data.audio`, and a logical `base_resp`
    /// status rides along even on HTTP 200.
    MiniMax,
}

/// Resolve which voice API a credential speaks. See module docs.
pub fn voice_api_for(credential: &CloudCredential) -> VoiceApi {
    if let Some(explicit) = credential.extras.get("voice_api") {
        match explicit.trim().to_ascii_lowercase().as_str() {
            "minimax" => return VoiceApi::MiniMax,
            "openai" | "openai_compatible" | "openai-compatible" => {
                return VoiceApi::OpenAiCompatible
            }
            // Unknown value → fall through to inference rather than
            // hard-failing; the operator likely meant a real provider.
            _ => {}
        }
    }
    let haystack = format!(
        "{} {}",
        credential.service.to_ascii_lowercase(),
        credential
            .base_url
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
    );
    if haystack.contains("minimax") {
        return VoiceApi::MiniMax;
    }
    VoiceApi::OpenAiCompatible
}

/// Per-API default `(model, voice, format)`. Callers resolve a
/// request's model/voice/format against these so an OpenAI default
/// never reaches a MiniMax endpoint (and vice versa).
pub fn defaults_for(api: VoiceApi) -> (&'static str, &'static str, &'static str) {
    match api {
        VoiceApi::OpenAiCompatible => (
            openai::DEFAULT_TTS_MODEL,
            openai::DEFAULT_TTS_VOICE,
            openai::DEFAULT_TTS_FORMAT,
        ),
        VoiceApi::MiniMax => (
            MINIMAX_DEFAULT_MODEL,
            MINIMAX_DEFAULT_VOICE,
            MINIMAX_DEFAULT_FORMAT,
        ),
    }
}

/// Synthesize speech for `arguments` using whichever API the
/// credential speaks. Recognized argument keys: `input`/`text`,
/// `model`, `voice`, `response_format`/`format`, `instructions`
/// (OpenAI only), `speed`.
pub async fn synthesize(
    http: &CloudHttp,
    credential: &CloudCredential,
    arguments: &Value,
) -> CloudResult<SpeechAudio> {
    match voice_api_for(credential) {
        VoiceApi::OpenAiCompatible => openai::speech(http, credential, arguments).await,
        VoiceApi::MiniMax => minimax_speech(http, credential, arguments).await,
    }
}

/// MiniMax T2A v2 adapter.
///
/// NOTE: the request/response shape here follows MiniMax's documented
/// T2A v2 contract. It is structurally complete but has not been
/// exercised against a live MiniMax account in this build — verify
/// with a real key + `GroupId` (set `extras.group_id`) before relying
/// on it. The OpenAI-compatible path is the tested default.
async fn minimax_speech(
    http: &CloudHttp,
    credential: &CloudCredential,
    arguments: &Value,
) -> CloudResult<SpeechAudio> {
    let input = arguments
        .get("input")
        .or_else(|| arguments.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::InvalidArgument("missing required field 'input'".into()))?;
    if input.trim().is_empty() {
        return Err(CloudError::InvalidArgument(
            "speech input must not be empty".into(),
        ));
    }

    let model = arguments
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| credential.extras.get("tts_model").cloned())
        .unwrap_or_else(|| MINIMAX_DEFAULT_MODEL.to_string());
    let voice = arguments
        .get("voice")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| credential.extras.get("tts_voice").cloned())
        .unwrap_or_else(|| MINIMAX_DEFAULT_VOICE.to_string());
    let format = arguments
        .get("response_format")
        .or_else(|| arguments.get("format"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| credential.extras.get("tts_format").cloned())
        .unwrap_or_else(|| MINIMAX_DEFAULT_FORMAT.to_string());
    let speed = arguments
        .get("speed")
        .and_then(Value::as_f64)
        .unwrap_or(1.0)
        .clamp(0.5, 2.0);

    let body = json!({
        "model": model,
        "text": input,
        "stream": false,
        "voice_setting": { "voice_id": voice, "speed": speed, "vol": 1.0, "pitch": 0 },
        "audio_setting": { "format": format, "sample_rate": 32000, "channel": 1 },
    });

    // MiniMax scopes requests by account GroupId, passed as a query
    // parameter. Sourced from the credential's extras. Percent-encode the
    // value so a GroupId containing reserved characters (`&`, `=`, space,
    // non-ASCII) can't corrupt the query string.
    let path = match credential
        .extras
        .get("group_id")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        Some(group) => format!("/t2a_v2?GroupId={}", percent_encode_query(group)),
        None => "/t2a_v2".to_string(),
    };

    let response = http
        .send_request(credential, Method::POST, &path, Some(&body), &[])
        .await?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|err| CloudError::Request {
        service: credential.service.clone(),
        message: err.to_string(),
    })?;
    let payload: Value = serde_json::from_slice(&bytes).map_err(|err| CloudError::Request {
        service: credential.service.clone(),
        message: format!("minimax response was not JSON: {err}"),
    })?;
    if !status.is_success() {
        return Err(CloudError::BadStatus {
            service: credential.service.clone(),
            status: status.as_u16(),
            body: payload.to_string(),
        });
    }
    // MiniMax reports logical failures in `base_resp` even on HTTP 200.
    if let Some(code) = payload
        .pointer("/base_resp/status_code")
        .and_then(Value::as_i64)
    {
        if code != 0 {
            let msg = payload
                .pointer("/base_resp/status_msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(CloudError::BadStatus {
                service: credential.service.clone(),
                status: 502,
                body: format!("minimax status {code}: {msg}"),
            });
        }
    }
    let hex_audio = payload
        .pointer("/data/audio")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::Request {
            service: credential.service.clone(),
            message: "minimax response missing data.audio".into(),
        })?;
    let audio = decode_hex(hex_audio).map_err(|err| CloudError::Request {
        service: credential.service.clone(),
        message: format!("minimax audio hex decode: {err}"),
    })?;

    Ok(SpeechAudio {
        bytes: audio,
        content_type: openai::content_type_for_format(&format).to_string(),
        format,
    })
}

/// Percent-encode a query-parameter value (RFC 3986 unreserved set kept
/// verbatim; everything else `%`-escaped). Dependency-free — GroupIds are
/// normally alphanumeric, so this is defensive against the pathological
/// case where a value carries `&`/`=`/space/non-ASCII.
fn percent_encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Decode a hex string (as MiniMax returns audio) into raw bytes.
/// Tolerates surrounding whitespace; rejects odd lengths / non-hex.
fn decode_hex(input: &str) -> Result<Vec<u8>, String> {
    let trimmed = input.trim();
    if !trimmed.len().is_multiple_of(2) {
        return Err("odd-length hex string".into());
    }
    (0..trimmed.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&trimmed[i..i + 2], 16).map_err(|err| err.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cred(service: &str, base_url: Option<&str>, voice_api: Option<&str>) -> CloudCredential {
        let mut extras = HashMap::new();
        if let Some(api) = voice_api {
            extras.insert("voice_api".to_string(), api.to_string());
        }
        CloudCredential {
            service: service.to_string(),
            label: service.to_string(),
            auth_style: "bearer".to_string(),
            secret: "sk-test".to_string(),
            base_url: base_url.map(str::to_string),
            extras,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn explicit_voice_api_wins() {
        let c = cred("custom", Some("https://example.com/v1"), Some("minimax"));
        assert_eq!(voice_api_for(&c), VoiceApi::MiniMax);
        let c = cred("minimax", None, Some("openai"));
        assert_eq!(voice_api_for(&c), VoiceApi::OpenAiCompatible);
    }

    #[test]
    fn infers_minimax_from_name_or_url() {
        assert_eq!(
            voice_api_for(&cred("minimax", None, None)),
            VoiceApi::MiniMax
        );
        assert_eq!(
            voice_api_for(&cred("voice", Some("https://api.minimax.io/v1"), None)),
            VoiceApi::MiniMax
        );
    }

    #[test]
    fn defaults_to_openai_compatible() {
        assert_eq!(
            voice_api_for(&cred("openai", Some("https://api.openai.com/v1"), None)),
            VoiceApi::OpenAiCompatible
        );
        assert_eq!(
            voice_api_for(&cred("some-gateway", None, None)),
            VoiceApi::OpenAiCompatible
        );
    }

    #[test]
    fn unknown_voice_api_falls_through_to_inference() {
        // bogus explicit value + minimax name → still MiniMax
        let c = cred("minimax", None, Some("totally-bogus"));
        assert_eq!(voice_api_for(&c), VoiceApi::MiniMax);
    }

    #[test]
    fn per_api_defaults_differ() {
        let (m, v, _f) = defaults_for(VoiceApi::OpenAiCompatible);
        assert_eq!(v, "alloy");
        assert_eq!(m, "gpt-4o-mini-tts");
        let (m, v, _f) = defaults_for(VoiceApi::MiniMax);
        assert_eq!(v, "male-qn-qingse");
        assert_eq!(m, "speech-02-hd");
    }

    #[test]
    fn percent_encode_query_keeps_unreserved_escapes_rest() {
        assert_eq!(percent_encode_query("grp-123_ok.~"), "grp-123_ok.~");
        assert_eq!(percent_encode_query("a b"), "a%20b");
        assert_eq!(percent_encode_query("x&y=z"), "x%26y%3Dz");
        // non-ASCII byte (é = 0xC3 0xA9 in UTF-8)
        assert_eq!(percent_encode_query("é"), "%C3%A9");
    }

    #[test]
    fn hex_decode_roundtrip() {
        assert_eq!(decode_hex("48656c6c6f").unwrap(), b"Hello");
        assert_eq!(decode_hex("  ff00 ").unwrap(), vec![0xff, 0x00]); // outer whitespace trimmed
        assert!(decode_hex("ff 00").is_err()); // inner space → odd length / non-hex
        assert!(decode_hex("abc").is_err()); // odd length
        assert!(decode_hex("zz").is_err()); // non-hex digits
        assert_eq!(decode_hex("FFee").unwrap(), vec![0xff, 0xee]);
    }
}
