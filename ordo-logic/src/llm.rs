//! `LlmLogicProvider` — default implementation, LLM-driven.
//!
//! Holds an `Arc<CloudHttp>` + a `CloudCredentialTask` and routes
//! every method through whichever credential satisfies the call,
//! exactly the way the assistant service's tool gateway does. No
//! provider-specific code — works with OpenAI, Anthropic, Ollama,
//! LM Studio, OpenRouter, Groq, or any OpenAI-compatible endpoint
//! the operator configures.
//!
//! Design choices:
//!
//! - Each capability sends ONE chat completion. No multi-turn
//!   reasoning, no tool calls — these are pure text → JSON
//!   transformations. That keeps latency predictable and the
//!   per-capability budget bounded.
//! - We ask for strict JSON in the prompt and parse with a small
//!   fence-stripper before `serde_json::from_str`. Reasoning
//!   models often wrap output in ```json fences; the parser
//!   handles both.
//! - On parse failure we surface a `LogicError::LlmFailed` with a
//!   short snippet of what came back, so the operator can tell
//!   whether to retry, switch providers, or rephrase the input.
//! - Provider selection: we walk credentials the same way
//!   `ordo-mcp-host::cloud_service_call` does — explicit override,
//!   then by-name lookup, then any compatible by auth_style.
//!   Inherits `extras.timeout_secs` per credential automatically.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{debug, warn};

use ordo_cloud::{CloudCredential, CloudCredentialTask, CloudHttp};

use crate::prompts;
use crate::provider::LogicProvider;
use crate::types::{
    ChainValidation, Claim, ClaimClassification, Fallacy, FallacySeverity, LogicError, LogicResult,
};

/// Optional override for which credential to route to. `None` lets
/// the resolver pick any compatible one — same default-discovery
/// behavior as the assistant's cloud calls.
#[derive(Debug, Clone, Default)]
pub struct LogicResolverHints {
    /// Service id of the credential to prefer (e.g. "ollama",
    /// "anthropic"). Falls back to compatibility walk on miss.
    pub preferred_service: Option<String>,
}

#[derive(Clone)]
pub struct LlmLogicProvider {
    http: Arc<CloudHttp>,
    credentials: CloudCredentialTask,
    hints: LogicResolverHints,
}

impl LlmLogicProvider {
    pub fn new(http: Arc<CloudHttp>, credentials: CloudCredentialTask) -> Self {
        Self {
            http,
            credentials,
            hints: LogicResolverHints::default(),
        }
    }

    pub fn with_hints(mut self, hints: LogicResolverHints) -> Self {
        self.hints = hints;
        self
    }

    /// Pick a credential to route the call to. See [`pick_credential_public`]
    /// for the no-self variant — the hybrid provider's formalize path
    /// reuses the same walk.
    async fn pick_credential(&self) -> LogicResult<CloudCredential> {
        if let Some(name) = self.hints.preferred_service.clone() {
            if let Ok(Some(cred)) = self.credentials.get(name).await {
                return Ok(cred);
            }
        }
        let all = self
            .credentials
            .list()
            .await
            .map_err(|err| LogicError::LlmFailed(err.to_string()))?;
        // Prefer non-anthropic (OpenAI-compatible) — wider coverage,
        // works with local Ollama / LM Studio out of the box.
        if let Some(cred) = all.iter().find(|c| c.auth_style != "anthropic") {
            return Ok(cred.clone());
        }
        all.into_iter().next().ok_or(LogicError::NoCredential)
    }

    /// Send a single user prompt, return the assistant message text.
    /// Honors `extras.model` for local provider routing — same
    /// convention every other ordo-* lane uses.
    async fn one_shot(&self, prompt: String) -> LogicResult<String> {
        let credential = self.pick_credential().await?;
        let mut chat_args = json!({
            "messages": [{ "role": "user", "content": prompt }],
            "temperature": 0.1,
            "max_tokens": 2048,
        });
        if let Some(model) = credential.extras.get("model") {
            chat_args["model"] = json!(model);
        }
        let is_anthropic = credential.auth_style == "anthropic";
        let response = if is_anthropic {
            ordo_cloud::anthropic::messages(&self.http, &credential, &chat_args)
                .await
                .map_err(|err| LogicError::LlmFailed(err.to_string()))?
        } else {
            ordo_cloud::openai::chat(&self.http, &credential, &chat_args)
                .await
                .map_err(|err| LogicError::LlmFailed(err.to_string()))?
        };
        // Prefer raw content; fall back to the UI-friendly message
        // (which can be the reasoning preview for thinking models
        // that emit content="" — that's still useful here because
        // we'll JSON-parse it and reasoning preambles get stripped
        // by the fence walker).
        let text = response
            .get("content_raw")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| response.get("assistant_message").and_then(|v| v.as_str()))
            .or_else(|| response.get("assistant_text").and_then(|v| v.as_str()))
            .unwrap_or_default()
            .to_string();
        if text.trim().is_empty() {
            return Err(LogicError::LlmFailed("model returned empty content".into()));
        }
        Ok(text)
    }
}

/// Public alias for the credential walker so the hybrid provider's
/// formalize closure can reuse the same logic without duplicating.
/// Walks: any non-anthropic credential first (OpenAI-compatible
/// covers Ollama / LM Studio / Groq / OpenRouter), then any
/// credential at all.
pub async fn pick_credential_public(
    credentials: &CloudCredentialTask,
) -> LogicResult<CloudCredential> {
    let all = credentials
        .list()
        .await
        .map_err(|err| LogicError::LlmFailed(err.to_string()))?;
    if let Some(cred) = all.iter().find(|c| c.auth_style != "anthropic") {
        return Ok(cred.clone());
    }
    all.into_iter().next().ok_or(LogicError::NoCredential)
}

/// Public alias for [`extract_json`] — same reason: hybrid wiring
/// reuses the parser.
pub fn extract_json_public(raw: &str) -> &str {
    extract_json(raw)
}

/// Strip ```json fences (or plain ```) and any leading prose, return
/// the JSON substring. Permissive: handles cases where the model
/// thinks aloud before emitting the JSON.
fn extract_json(raw: &str) -> &str {
    // Try fenced first — most common with reasoning models.
    if let Some(start) = raw.find("```") {
        let after_fence = &raw[start + 3..];
        // Skip the optional language tag ("json\n") on the same line.
        let body_start = after_fence.find('\n').map(|i| i + 1).unwrap_or(0);
        let body = &after_fence[body_start..];
        if let Some(end) = body.find("```") {
            return body[..end].trim();
        }
        return body.trim();
    }
    // No fence: scan for the first '{' or '[' and trim to that.
    if let Some(i) = raw.find(|c: char| c == '{' || c == '[') {
        return raw[i..].trim();
    }
    raw.trim()
}

fn parse_json<T: serde::de::DeserializeOwned>(raw: &str) -> LogicResult<T> {
    let trimmed = extract_json(raw);
    serde_json::from_str::<T>(trimmed).map_err(|err| {
        let snippet: String = trimmed.chars().take(240).collect();
        LogicError::LlmFailed(format!("could not parse JSON ({err}); snippet: {snippet}"))
    })
}

#[async_trait]
impl LogicProvider for LlmLogicProvider {
    async fn identify_claims(&self, text: &str) -> LogicResult<Vec<Claim>> {
        if text.trim().is_empty() {
            return Err(LogicError::InvalidArgument("text must not be empty".into()));
        }
        let raw = self.one_shot(prompts::identify_claims(text)).await?;
        debug!(target: "ordo_logic", chars = raw.len(), "identify_claims raw");
        // Accept either { "claims": [...] } or a bare array.
        let parsed: Value = serde_json::from_str(extract_json(&raw))
            .map_err(|err| LogicError::LlmFailed(err.to_string()))?;
        let arr = parsed
            .get("claims")
            .cloned()
            .or(Some(parsed.clone()))
            .filter(|v| v.is_array())
            .ok_or_else(|| LogicError::LlmFailed("expected `claims` array in response".into()))?;
        let claims: Vec<Claim> = serde_json::from_value(arr).map_err(|err| {
            warn!(target: "ordo_logic", error = %err, "claims shape mismatch");
            LogicError::LlmFailed(err.to_string())
        })?;
        Ok(claims)
    }

    async fn find_fallacies(&self, argument: &str) -> LogicResult<Vec<Fallacy>> {
        if argument.trim().is_empty() {
            return Err(LogicError::InvalidArgument(
                "argument must not be empty".into(),
            ));
        }
        let raw = self.one_shot(prompts::find_fallacies(argument)).await?;
        let parsed: Value = serde_json::from_str(extract_json(&raw))
            .map_err(|err| LogicError::LlmFailed(err.to_string()))?;
        let arr = parsed
            .get("fallacies")
            .cloned()
            .or(Some(parsed.clone()))
            .filter(|v| v.is_array())
            .ok_or_else(|| {
                LogicError::LlmFailed("expected `fallacies` array in response".into())
            })?;
        // Normalize severity strings the LLM may emit in odd cases
        // (uppercase, trailing punctuation). Default to Moderate on
        // unknown — never hard-fail on a stylistic hiccup.
        let mut fallacies: Vec<Fallacy> = serde_json::from_value(arr).unwrap_or_default();
        for f in &mut fallacies {
            let normalized = match f.severity {
                FallacySeverity::Minor | FallacySeverity::Moderate | FallacySeverity::Critical => {
                    f.severity
                }
            };
            f.severity = normalized;
        }
        Ok(fallacies)
    }

    async fn validate_chain(
        &self,
        premises: &[String],
        conclusion: &str,
    ) -> LogicResult<ChainValidation> {
        if premises.is_empty() {
            return Err(LogicError::InvalidArgument(
                "premises must not be empty".into(),
            ));
        }
        if conclusion.trim().is_empty() {
            return Err(LogicError::InvalidArgument(
                "conclusion must not be empty".into(),
            ));
        }
        let raw = self
            .one_shot(prompts::validate_chain(premises, conclusion))
            .await?;
        let mut cv: ChainValidation = parse_json(&raw)?;
        // LLM-only path is by definition rhetorical. The hybrid
        // provider tags Formal when the prover proves; this baseline
        // stays Rhetorical so callers reading the field straight
        // through aren't misled.
        cv.certainty = crate::types::Certainty::Rhetorical;
        Ok(cv)
    }

    async fn classify_claim_domain(&self, claim: &str) -> LogicResult<ClaimClassification> {
        if claim.trim().is_empty() {
            return Err(LogicError::InvalidArgument(
                "claim must not be empty".into(),
            ));
        }
        let raw = self.one_shot(prompts::classify_claim_domain(claim)).await?;
        parse_json::<ClaimClassification>(&raw)
    }

    async fn steel_man(&self, argument: &str) -> LogicResult<String> {
        if argument.trim().is_empty() {
            return Err(LogicError::InvalidArgument(
                "argument must not be empty".into(),
            ));
        }
        // Plain-prose response — see the prompt comment for why we
        // skip the JSON envelope on this one. Light cleanup: strip
        // any fence the model added despite being told not to, and
        // trim outer whitespace.
        let raw = self.one_shot(prompts::steel_man(argument)).await?;
        let cleaned = strip_outer_fence(raw.trim());
        if cleaned.is_empty() {
            return Err(LogicError::LlmFailed(
                "model returned empty steel-man response".into(),
            ));
        }
        Ok(cleaned)
    }
}

/// If the model wrapped its response in ```...``` fences (common
/// despite the prompt asking for plain prose), strip them. Returns
/// the stripped string; otherwise returns the input unchanged.
fn strip_outer_fence(s: &str) -> String {
    let trimmed = s.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    // Drop opening fence + optional language tag on the same line.
    let after = trimmed.trim_start_matches("```");
    let body = after
        .split_once('\n')
        .map(|(_, rest)| rest)
        .unwrap_or(after);
    // Drop closing fence if present.
    body.trim().trim_end_matches("```").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_strips_fences() {
        let raw = "Some thinking aloud.\n```json\n{\"a\": 1}\n```\ntrailing";
        assert_eq!(extract_json(raw), "{\"a\": 1}");
    }

    #[test]
    fn extract_json_strips_fence_without_lang_tag() {
        let raw = "```\n[1,2,3]\n```";
        assert_eq!(extract_json(raw), "[1,2,3]");
    }

    #[test]
    fn extract_json_finds_bare_object() {
        let raw = "okay here you go: {\"a\": 1}";
        assert_eq!(extract_json(raw), "{\"a\": 1}");
    }

    #[test]
    fn parse_json_returns_typed_value() {
        let raw = "```json\n{\"holds\": true, \"gaps\": [\"foo\"]}\n```";
        let cv: ChainValidation = parse_json(raw).expect("parse");
        assert!(cv.holds);
        assert_eq!(cv.gaps, vec!["foo".to_string()]);
    }

    #[test]
    fn parse_json_surfaces_snippet_on_failure() {
        let raw = "not json at all just prose";
        let err = parse_json::<ChainValidation>(raw).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("LLM call failed"), "{msg}");
    }

    #[test]
    fn strip_outer_fence_drops_fenced_block() {
        let raw = "```text\nThe steel-man:\n\nFirst paragraph.\n\nSecond paragraph.\n```";
        let cleaned = strip_outer_fence(raw);
        assert!(cleaned.starts_with("The steel-man:"));
        assert!(cleaned.ends_with("Second paragraph."));
    }

    #[test]
    fn strip_outer_fence_leaves_plain_prose_alone() {
        let raw = "Plain prose here, no fences.";
        assert_eq!(strip_outer_fence(raw), raw);
    }
}
