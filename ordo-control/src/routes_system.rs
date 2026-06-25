use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

/// `/metrics` endpoint. Reads the shared `MetricsHandle` from the
/// request's extensions â€” populated by the traffic layer at
/// `with_traffic` time. When the layer isn't installed (e.g. in unit
/// tests that build a bare router), the handler returns a minimal
/// body with the static build info only.
pub(crate) async fn metrics_endpoint(
    handle: Option<axum::extract::Extension<MetricsHandle>>,
) -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    String,
) {
    let body = match handle {
        Some(axum::extract::Extension(h)) => h.render(),
        None => MetricsHandle::new().render(),
    };
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
}

pub(crate) async fn list_capabilities(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let descriptors = state
        .brain
        .query_capability_descriptors()
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let lanes = summarize_capability_lanes(&descriptors);
    Ok(Json(json!({
        "count": descriptors.len(),
        "lane_count": lanes.len(),
        "lanes": lanes,
        "descriptors": descriptors,
    })))
}

pub(crate) async fn list_rag_collections(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let collections = match state.brain.query_rag_collections().await {
        Ok(collections) => collections,
        Err(err) => {
            return Ok(Json(json!({
                "available": false,
                "count": 0,
                "results": [],
                "error": err.to_string(),
            })));
        }
    };
    Ok(Json(json!({
        "available": true,
        "count": collections.len(),
        "results": collections,
    })))
}

pub(crate) async fn preview_rag(
    State(state): State<ControlApiState>,
    Query(query): Query<RagPreviewQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let raw_query = query.query.unwrap_or_default();
    let trimmed_query = raw_query.trim();
    if trimmed_query.is_empty() {
        return Err(ControlApiError::bad_request("preview query is required"));
    }

    let requested_collections = parse_collection_query(query.collections.as_deref());
    let using_inferred_collections = requested_collections.is_empty();
    let effective_collections = if using_inferred_collections {
        infer_rag_collections(trimmed_query)
    } else {
        requested_collections.clone()
    };
    let top_k = query.top_k.unwrap_or(5).clamp(1, 8);
    let hits = state
        .brain
        .query_rag_in_collections(trimmed_query, &effective_collections, top_k)
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let effective_collection_labels = effective_collections
        .iter()
        .map(|collection| rag_collection_label(collection))
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "query": trimmed_query,
        "top_k": top_k,
        "using_inferred_collections": using_inferred_collections,
        "requested_collections": requested_collections,
        "effective_collections": effective_collections,
        "effective_collection_labels": effective_collection_labels,
        "hit_count": hits.len(),
        "hits": hits,
    })))
}

pub(crate) async fn describe_profile(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    invoke_tool(&state.brain, "runtime.describe_profile", json!({})).await
}

#[derive(Deserialize)]
pub(crate) struct FindBinaryQuery {
    pub(crate) name: String,
}

/// GET `/api/system/find_binary?name=<exe_name>` — walks a small set
/// of candidate paths anchored on the running runtime's location and
/// returns the first one that exists. The studio's MCP tab uses this
/// to auto-detect `ordo-mcp.exe` so the operator doesn't have to type
/// or browse for a path that's almost always sitting next to the
/// runtime binary it's already talking to.
///
/// Response:
///   { "name": "ordo-mcp.exe", "found": "<abs path or null>",
///     "candidates": ["<path>", "<path>", …] }
///
/// The candidates list is returned even on miss so the studio can
/// surface a "we looked here" hint if the operator has to fix it
/// manually.
pub(crate) async fn find_binary(Query(query): Query<FindBinaryQuery>) -> Result<Json<Value>, ControlApiError> {
    use std::path::PathBuf;

    let raw = query.name.trim();
    if raw.is_empty() {
        return Err(ControlApiError::bad_request(
            "missing required query 'name'".to_string(),
        ));
    }
    // Reject path-traversal: caller specifies a basename, not a path.
    // The whole point of this endpoint is to LOCATE a binary; letting
    // the caller pass `../../etc/passwd` would invert that.
    if raw.contains('/') || raw.contains('\\') || raw.contains("..") {
        return Err(ControlApiError::bad_request(
            "'name' must be a basename (no path separators)".to_string(),
        ));
    }
    // On Windows, normalize to .exe if the caller didn't include it.
    let name = if cfg!(windows) && !raw.to_ascii_lowercase().ends_with(".exe") {
        format!("{raw}.exe")
    } else {
        raw.to_string()
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            // 1. Sibling of the runtime binary (most common — both
            //    built into the same `target/{profile}/` dir).
            candidates.push(dir.join(&name));
            // 2. Walk up looking for sibling `target/release` /
            //    `target/debug` directories. Handles cases where the
            //    runtime is in `target/release` and the caller wants
            //    a binary that only built into `target/debug`.
            let mut walker = dir.parent();
            for _ in 0..4 {
                let Some(up) = walker else { break };
                candidates.push(up.join("release").join(&name));
                candidates.push(up.join("debug").join(&name));
                walker = up.parent();
            }
        }
    }
    // 3. Anything on PATH. `which` would be cleaner but pulling a
    //    new dep for one lookup isn't worth it; walk PATH manually.
    if let Some(path_var) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&path_var) {
            candidates.push(entry.join(&name));
        }
    }

    // De-duplicate while preserving order (operators see candidates
    // in priority order if the search misses).
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let candidates: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect();

    let found = candidates
        .iter()
        .find(|p| p.is_file())
        .map(|p| p.display().to_string());
    let candidate_strs: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();

    Ok(Json(json!({
        "name": name,
        "found": found,
        "candidates": candidate_strs,
    })))
}

