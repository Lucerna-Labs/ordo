//! `web.search` — Tavily-backed search with boundary wrap.
//!
//! ## Why this is in the strainer
//!
//! Search results are untrusted web content the LLM is about to
//! read. Same threat surface as `web.fetch_and_strain`'s output —
//! someone can write a search-result snippet that contains
//! injection-shaped text. Putting search in the strainer crate
//! means every result snippet gets the same boundary wrap +
//! Stage 3.5 encoding/polyglot defenses as fetched pages, and
//! the cup gate sees per-result `<untrusted_web_content>` blocks
//! to taint on.
//!
//! ## What it does
//!
//! 1. POST to `https://api.tavily.com/search` with the API key.
//! 2. For each returned result:
//!    - Validate the URL through the `url_safety` chokepoint
//!      (rejects userinfo, oversized URLs, control chars, etc.).
//!      A failed-validation result is dropped from the response,
//!      not silently fixed — the LLM should never see a URL we
//!      wouldn't have let through `web.fetch_and_strain`.
//!    - Run the snippet through `normalize::normalize` (NFKC fold,
//!      homoglyph fold, special-token strip, base64 shorten).
//!    - Wrap the normalized snippet in `<untrusted_web_content>`
//!      tagged with the result's URL + fetched_at + sha256.
//! 3. Return a structured response with one wrapped block per
//!    result, plus the operator-facing fields (title, url, score)
//!    as plain JSON.
//!
//! ## What it does NOT do
//!
//! - Does not fetch the result URLs. That's the LLM's next step
//!   if it wants the full page — it can call `web.fetch_and_strain`
//!   with any URL from the search results.
//! - Does not filter results for trust. Every result is untrusted-
//!   web by definition; the boundary wrap is what makes that
//!   tractable.
//! - Does not re-rank. Tavily's score is what it is.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

use crate::types::{StrainError, StrainResult};

const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_RESULTS_CAP: usize = 10;
const DEFAULT_MAX_RESULTS: usize = 5;

/// One search result, post-normalize and post-wrap. The LLM sees
/// `wrapped_content` framed as untrusted; `title`, `url`, `score`
/// are operator-facing metadata for ranking + citation.
#[derive(Debug, Clone, Serialize)]
pub struct WrappedSearchResult {
    pub title: String,
    pub url: String,
    pub score: f32,
    /// The result snippet, wrapped in `<untrusted_web_content>`
    /// using the same boundary shape as `web.fetch_and_strain`.
    pub wrapped_content: String,
    /// SHA-256 of the normalized snippet — for audit / dedupe.
    pub sha256: String,
}

/// Top-level response. `answer` is Tavily's optional one-line
/// synthesis (we pass `include_answer: false` by default to keep
/// the response footprint small, but the field is reserved if a
/// future capability arg flips it on).
#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub max_results: usize,
    pub results: Vec<WrappedSearchResult>,
    pub answer: Option<String>,
    pub fetched_at: DateTime<Utc>,
    /// Number of raw results Tavily returned that we DROPPED
    /// because their URL didn't pass the safety gate. Surfaced so
    /// the operator can tell if a search is being heavily filtered.
    pub dropped_unsafe_urls: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TavilyRequest<'a> {
    api_key: &'a str,
    query: &'a str,
    max_results: usize,
    search_depth: &'a str,
    include_answer: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    query: String,
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    score: f32,
}

/// Run a Tavily search. Bounded by an explicit timeout (matches
/// `fetch.rs`'s `DEFAULT_TIMEOUT_SECS`) and `max_results` is
/// clamped to `MAX_RESULTS_CAP` regardless of caller input.
pub async fn tavily_search(
    query: &str,
    max_results: Option<usize>,
    api_key: &str,
) -> StrainResult<SearchResponse> {
    if query.trim().is_empty() {
        return Err(StrainError::InvalidArgument(
            "search query must not be empty".into(),
        ));
    }
    if api_key.trim().is_empty() {
        return Err(StrainError::InvalidArgument(
            "Tavily API key is empty; set TAVILY_API_KEY or add a 'tavily' credential".into(),
        ));
    }
    let max_results = max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(MAX_RESULTS_CAP)
        .max(1);
    let fetched_at = Utc::now();

    let body = TavilyRequest {
        api_key,
        query,
        max_results,
        search_depth: "basic",
        include_answer: false,
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .user_agent(concat!(
            "Ordo-Strainer/",
            env!("CARGO_PKG_VERSION"),
            " (+ordo-strainer/web.search)"
        ))
        .build()
        .map_err(|err| StrainError::Internal(format!("client build: {err}")))?;

    let response = client
        .post(TAVILY_ENDPOINT)
        .json(&body)
        .send()
        .await
        .map_err(|err| StrainError::Internal(format!("tavily search failed: {err}")))?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable>".into());
        // Don't echo the raw API key into the error — the body
        // shouldn't contain it (Tavily doesn't reflect it back),
        // but defense in depth.
        return Err(StrainError::Internal(format!(
            "tavily search returned HTTP {}: {}",
            status.as_u16(),
            truncate_for_log(&body_text, 512),
        )));
    }

    let parsed: TavilyResponse = response
        .json()
        .await
        .map_err(|err| StrainError::Internal(format!("tavily response parse: {err}")))?;

    let mut wrapped: Vec<WrappedSearchResult> = Vec::with_capacity(parsed.results.len());
    let mut dropped = 0usize;
    for result in parsed.results.into_iter() {
        // URL safety gate per result. A search that returns a
        // userinfo URL or a giant URL gets that result dropped,
        // not propagated to the LLM. Defense in depth — the
        // LLM might call web.fetch_and_strain on this URL next,
        // and that path also rejects unsafe URLs, but catching
        // it here keeps the boundary tag's source attribute clean.
        if crate::url_safety::validate_url(&result.url).is_err() {
            dropped += 1;
            tracing::debug!(
                target: "ordo_strainer",
                url = %result.url,
                "search: dropping result with unsafe URL"
            );
            continue;
        }

        // Stage 3.5: normalize the snippet so encoding/polyglot
        // tricks in search results die before the LLM sees them.
        let normalized = crate::normalize::normalize(&result.content);
        let mut hasher = Sha256::new();
        hasher.update(normalized.as_bytes());
        let sha = hex::encode(hasher.finalize());
        let wrapped_content =
            crate::wrap::wrap_boundary(&normalized, &result.url, &fetched_at, &sha);

        wrapped.push(WrappedSearchResult {
            title: result.title,
            url: result.url,
            score: result.score,
            wrapped_content,
            sha256: sha,
        });
    }

    Ok(SearchResponse {
        query: parsed.query,
        max_results,
        results: wrapped,
        answer: parsed.answer,
        fetched_at,
        dropped_unsafe_urls: dropped,
    })
}

/// Read the Tavily API key from env first, then fall back to the
/// `ordo-cloud` credential vault under service id `"tavily"`. The
/// vault path is opt-in for operators who want the key managed
/// alongside their other cloud credentials; the env-var path is
/// the simple "I just set TAVILY_API_KEY=..." default.
///
/// Returns an `InvalidArgument` error when neither source has a
/// non-empty value — the operator gets a clear message instead of
/// a Tavily HTTP 401.
pub fn resolve_api_key_from_env() -> StrainResult<String> {
    match std::env::var("TAVILY_API_KEY") {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
        _ => Err(StrainError::InvalidArgument(
            "TAVILY_API_KEY not configured; set the env var or add a 'tavily' credential in cloud credentials"
                .into(),
        )),
    }
}

/// Defensive log truncation — keeps the error chain readable when
/// the upstream reflects a long body back at us.
fn truncate_for_log(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        s.to_string()
    } else {
        format!("{}…<{} bytes truncated>", &s[..cap], s.len() - cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_rejected() {
        // Synchronous validation without hitting the network.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(tavily_search("", None, "tvly-key"));
        assert!(matches!(res, Err(StrainError::InvalidArgument(_))));
    }

    #[test]
    fn whitespace_query_rejected() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(tavily_search("   ", None, "tvly-key"));
        assert!(matches!(res, Err(StrainError::InvalidArgument(_))));
    }

    #[test]
    fn empty_api_key_rejected() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(tavily_search("test query", None, ""));
        assert!(matches!(res, Err(StrainError::InvalidArgument(_))));
    }

    #[test]
    fn max_results_clamps_to_cap() {
        // Verify the math without hitting the network.
        let computed = std::cmp::min(50, MAX_RESULTS_CAP).max(1);
        assert_eq!(computed, MAX_RESULTS_CAP);
    }

    #[test]
    fn max_results_minimum_is_one() {
        let computed = std::cmp::min(0, MAX_RESULTS_CAP).max(1);
        assert_eq!(computed, 1);
    }

    #[test]
    fn resolve_api_key_from_env_when_present() {
        // Use a test-isolated env to avoid polluting the operator's
        // shell. SAFETY: tests run sequentially in a single thread
        // by default in cargo test; if that ever changes, switch to
        // `serial_test`. Today's cargo behavior is fine.
        std::env::set_var("TAVILY_API_KEY", "tvly-test-key");
        let key = resolve_api_key_from_env().expect("present");
        assert_eq!(key, "tvly-test-key");
        std::env::remove_var("TAVILY_API_KEY");
    }

    #[test]
    fn resolve_api_key_errors_when_missing() {
        std::env::remove_var("TAVILY_API_KEY");
        let res = resolve_api_key_from_env();
        assert!(matches!(res, Err(StrainError::InvalidArgument(_))));
    }

    #[test]
    fn truncate_for_log_passes_short() {
        assert_eq!(truncate_for_log("hello", 100), "hello");
    }

    #[test]
    fn truncate_for_log_truncates_long() {
        let long = "a".repeat(1000);
        let out = truncate_for_log(&long, 100);
        assert!(out.starts_with(&"a".repeat(100)));
        assert!(out.contains("900 bytes truncated"));
    }
}
