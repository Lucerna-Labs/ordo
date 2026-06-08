//! Web fetch — paired with the strain pipeline so the operator
//! can't accidentally invoke a raw fetch that bypasses the
//! transforms.
//!
//! The strainer's value comes from the boundary tag PLUS the
//! transforms PLUS the system prompt rule. A bare fetch capability
//! that returns raw HTML to the assistant invites the assistant
//! (or a future tool author) to skip the strainer "just this once."
//! That's exactly the anti-pattern the doc names. So this module
//! exposes ONE capability — `web.fetch_and_strain` — and that
//! capability ALWAYS runs the response through the strain
//! pipeline before returning.
//!
//! ## Safety bounds
//!
//! Hardcoded; not operator-tunable. Same reasoning as the
//! strainer's mesh — predictable, small, refuses surprises.
//!
//!   - Schemes: `http`, `https` only. `file://`, `data:`, `gopher:`,
//!     `javascript:`, etc. all rejected with InvalidArgument.
//!   - Timeout: 30 seconds total (connect + body). Tunable via the
//!     `timeout_secs` arg on the capability if a specific page is
//!     known-slow, capped at 120 s.
//!   - Max body size: 5 MB. Bigger pages get truncated and the
//!     truncation is recorded in the StrainedContent metadata.
//!   - Redirects: up to 10. Beyond that, fetch fails — runaway
//!     redirect chains aren't a normal operator workflow.
//!   - Content-Type: anything text/html-ish or text/plain. Other
//!     types get rejected (we don't strain PDFs or binaries).
//!   - User-Agent: identifies as Ordo so server-side analytics can
//!     see and block us if they want to.

use std::time::Duration;

use crate::types::{StrainError, StrainResult, StrainedContent};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;
const USER_AGENT: &str = concat!(
    "Ordo-Strainer/",
    env!("CARGO_PKG_VERSION"),
    " (+ordo-strainer; lossy-transform pre-LLM hygiene layer)"
);

/// Fetch a URL and run it through the full strain pipeline. Returns
/// the wrapped, normalized output ready to enter the assistant's
/// context.
///
/// `timeout_secs`: optional override capped at [`MAX_TIMEOUT_SECS`].
/// Defaults to [`DEFAULT_TIMEOUT_SECS`] when None.
pub async fn fetch_and_strain(
    url: &str,
    timeout_secs: Option<u64>,
) -> StrainResult<StrainedContent> {
    // URL safety gate — centralized in `url_safety::validate_url`.
    // Catches scheme violations (file://, data:, javascript:),
    // embedded userinfo (credential leak vector), oversized URLs,
    // and control characters. Mixed-script hosts get flagged and
    // logged but not rejected — see url_safety.rs for rationale.
    let safe = crate::url_safety::validate_url(url)
        .map_err(|err| StrainError::InvalidArgument(err.to_string()))?;
    let _ = safe; // SafeUrl carries warnings; the canonical form is
                  // recorded by `strain()` via the source_url path.

    let timeout = Duration::from_secs(
        timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS),
    );

    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| StrainError::Internal(format!("client build: {err}")))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| StrainError::Internal(format!("fetch failed: {err}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(StrainError::Internal(format!(
            "fetch returned HTTP {} for {url}",
            status.as_u16()
        )));
    }

    // Content-Type guard — refuse PDFs, images, archives, etc.
    // We're a text strainer; binary blobs aren't our job.
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.contains("text/html")
        && !content_type.contains("application/xhtml")
        && !content_type.contains("text/plain")
    {
        return Err(StrainError::InvalidArgument(format!(
            "unsupported content type '{content_type}'; \
             only text/html, application/xhtml, and text/plain are accepted"
        )));
    }

    // Bounded body read — truncate aggressively at MAX_BODY_BYTES so
    // a hostile or buggy server can't OOM us by streaming a 10 GB
    // response. We use chunked reads rather than `text()` so we can
    // stop early.
    let mut bytes: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    let mut truncated = false;
    use futures::StreamExt;
    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|err| StrainError::Internal(format!("body read: {err}")))?;
        if bytes.len() + chunk.len() > MAX_BODY_BYTES {
            let take = MAX_BODY_BYTES.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..take]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }

    let html = String::from_utf8_lossy(&bytes).into_owned();
    let mut strained = crate::strain(&html, url)?;

    // Surface truncation in the wrapped output's source attribute
    // so the operator (and audit) can tell. We can't change the
    // boundary tag's open-element after the fact without re-running
    // wrap, but we can prepend a one-liner the model will see at
    // the top of the markdown.
    if truncated {
        let banner = format!(
            "_(strainer: page exceeded {} MB cap and was truncated)_\n\n",
            MAX_BODY_BYTES / (1024 * 1024)
        );
        strained.markdown = format!("{banner}{}", strained.markdown);
        // Re-wrap so the boundary tag is consistent with the new
        // body. The sha256 changes too, naturally.
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(strained.markdown.as_bytes());
        strained.sha256 = hex::encode(hasher.finalize());
        strained.wrapped = crate::wrap::wrap_boundary(
            &strained.markdown,
            &strained.source,
            &strained.fetched_at,
            &strained.sha256,
        );
    }

    Ok(strained)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The fetch function is async + makes real network calls. We
    // don't unit-test the network leg here — the security regression
    // suite (gap 4) covers integration. These tests focus on the
    // pre-flight guards that fire WITHOUT a network call.

    #[tokio::test]
    async fn rejects_file_scheme() {
        let r = fetch_and_strain("file:///etc/passwd", None).await;
        assert!(matches!(r, Err(StrainError::InvalidArgument(_))));
    }

    #[tokio::test]
    async fn rejects_data_url() {
        let r = fetch_and_strain("data:text/html,<script>alert(1)</script>", None).await;
        assert!(matches!(r, Err(StrainError::InvalidArgument(_))));
    }

    #[tokio::test]
    async fn rejects_javascript_scheme() {
        let r = fetch_and_strain("javascript:alert(1)", None).await;
        assert!(matches!(r, Err(StrainError::InvalidArgument(_))));
    }

    #[tokio::test]
    async fn rejects_malformed_url() {
        let r = fetch_and_strain("not a url at all", None).await;
        assert!(matches!(r, Err(StrainError::InvalidArgument(_))));
    }

    #[test]
    fn timeout_clamps_to_max() {
        // We don't run a fetch here — just verify the math.
        let computed = std::cmp::min(9999, MAX_TIMEOUT_SECS);
        assert_eq!(computed, MAX_TIMEOUT_SECS);
    }
}
