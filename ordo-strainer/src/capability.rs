//! Bus surface — capability descriptors + invoke dispatch.
//!
//! The runtime adapter `ordo-mcp-host::StrainerCapabilityAdapter`
//! wraps these helpers and registers them on the bus so the
//! assistant can call `web.strain` like any other tool.

use ordo_protocol::{CapabilityActivation, CapabilityDescriptor, CapabilityTier};
use serde_json::{json, Value};

use crate::types::StrainError;

pub const PROVIDER_NAME: &str = "ordo-strainer";

pub const WEB_STRAIN: &str = "web.strain";
pub const WEB_FETCH_AND_STRAIN: &str = "web.fetch_and_strain";
pub const WEB_SEARCH: &str = "web.search";

fn describe(cap: &str, description: &str, schema: Value) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        cap,
        PROVIDER_NAME,
        description,
        CapabilityTier::Optional,
        CapabilityActivation::Lazy,
    )
    .with_input_schema(schema)
}

pub fn capability_descriptors() -> Vec<CapabilityDescriptor> {
    vec![
        describe(
            WEB_STRAIN,
            "Run the Strainer on raw HTML — extract main content, strip invisible / hidden \
             elements, normalize to markdown, wrap in untrusted_web_content tags. The result \
             is what enters the assistant's context, never the raw page.",
            json!({
                "type": "object",
                "required": ["html", "source_url"],
                "properties": {
                    "html": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Raw HTML fetched from the URL."
                    },
                    "source_url": {
                        "type": "string",
                        "minLength": 1,
                        "description": "URL the HTML came from. Recorded in the boundary wrapper for audit + Stage 5 taint propagation."
                    },
                    "fetched_at": {
                        "type": "string",
                        "format": "date-time",
                        "description": "RFC 3339 timestamp of the fetch. Defaults to now if omitted."
                    }
                }
            }),
        ),
        describe(
            WEB_FETCH_AND_STRAIN,
            "Fetch a URL over http/https and run the response through the full Strainer \
             pipeline. The strain is non-skippable — there is no raw-fetch capability the \
             assistant can call to bypass it. Bounded by hardcoded safety limits (5 MB body \
             cap, 30s default timeout, 10 redirects max, http/https only, text/* responses \
             only). Use this any time the assistant needs to read a URL the operator hasn't \
             already pre-strained.",
            json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Absolute http or https URL to fetch."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 120,
                        "description": "Per-call timeout override. Defaults to 30s, capped at 120s."
                    }
                }
            }),
        ),
        describe(
            WEB_SEARCH,
            "Run a Tavily web search and return per-result snippets wrapped in \
             `<untrusted_web_content>` tags — same boundary shape as `web.fetch_and_strain` so \
             the cup gate sees them as untrusted ancestry. Each result carries title, url, and \
             score for citation/ranking; the snippet itself goes through the strainer's \
             encoding/polyglot defense (NFKC fold, homoglyph fold, special-token strip) before \
             wrapping. Results whose URLs fail the strainer's safety gate (userinfo, oversized, \
             control chars) are dropped from the response and counted in `dropped_unsafe_urls`. \
             Use when the assistant needs to find URLs to fetch; a typical pattern is search → \
             pick relevant result → call `web.fetch_and_strain` on that result's url.",
            json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Natural-language search query."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 10,
                        "description": "Maximum results to return. Defaults to 5; capped at 10."
                    }
                }
            }),
        ),
    ]
}

/// Dispatch a `web.*` tool call into the strainer. Returns
/// `Ok(Some(value))` on match, `Ok(None)` on prefix-miss (so the bus
/// host falls through cleanly), `Err(message)` on semantic failure.
///
/// Async because `web.fetch_and_strain` makes a network call. The
/// pure-strain path (`web.strain`) is sync internally but still
/// awaited by the adapter — matches the shape `ordo_logic` uses.
pub async fn invoke_capability(
    capability: &str,
    arguments: &Value,
) -> Result<Option<Value>, String> {
    match capability {
        WEB_STRAIN => Ok(Some(invoke_web_strain(arguments)?)),
        WEB_FETCH_AND_STRAIN => Ok(Some(invoke_web_fetch_and_strain(arguments).await?)),
        WEB_SEARCH => Ok(Some(invoke_web_search(arguments).await?)),
        _ => Ok(None),
    }
}

fn invoke_web_strain(arguments: &Value) -> Result<Value, String> {
    let html = arguments
        .get("html")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing `html`".to_string())?;
    let source_url = arguments
        .get("source_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing `source_url`".to_string())?;

    let fetched_at = arguments
        .get("fetched_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    let strained = match fetched_at {
        Some(ts) => crate::strain_with_timestamp(html, source_url, ts),
        None => crate::strain(html, source_url),
    }
    .map_err(|err: StrainError| err.to_string())?;

    Ok(strained_to_json(&strained))
}

async fn invoke_web_fetch_and_strain(arguments: &Value) -> Result<Value, String> {
    let url = arguments
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing `url`".to_string())?;

    // `timeout_secs` is bounds-checked here AND clamped again inside
    // `fetch_and_strain`. Belt-and-braces — schemas can lie.
    let timeout_secs = arguments.get("timeout_secs").and_then(|v| v.as_u64());

    let strained = crate::fetch::fetch_and_strain(url, timeout_secs)
        .await
        .map_err(|err: StrainError| err.to_string())?;

    Ok(strained_to_json(&strained))
}

fn strained_to_json(strained: &crate::types::StrainedContent) -> Value {
    json!({
        "wrapped": strained.wrapped,
        "markdown": strained.markdown,
        "source": strained.source,
        "fetched_at": strained.fetched_at.to_rfc3339(),
        "sha256": strained.sha256,
    })
}

async fn invoke_web_search(arguments: &Value) -> Result<Value, String> {
    let query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing `query`".to_string())?;
    let max_results = arguments
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let api_key =
        crate::search::resolve_api_key_from_env().map_err(|err: StrainError| err.to_string())?;

    let response = crate::search::tavily_search(query, max_results, &api_key)
        .await
        .map_err(|err: StrainError| err.to_string())?;

    serde_json::to_value(&response).map_err(|err| err.to_string())
}
