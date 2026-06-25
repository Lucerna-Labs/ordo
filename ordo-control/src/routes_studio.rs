use std::path::Path as FsPath;
use std::path::PathBuf;
use std::sync::Arc;
use axum::body::Body;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};
use crate::{ControlApiError, STUDIO_DIST_DIR, STUDIO_INDEX};
use crate::routes_system::dashboard;

pub(crate) async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
pub(crate) fn studio_dist_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("ORDO_STUDIO_DIST") {
        let candidate = PathBuf::from(path);
        if candidate.join(STUDIO_INDEX).is_file() {
            return Some(candidate);
        }
    }

    let cwd_candidate = std::env::current_dir().ok()?.join(STUDIO_DIST_DIR);
    if cwd_candidate.join(STUDIO_INDEX).is_file() {
        return Some(cwd_candidate);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let exe_candidate = exe_dir.join(STUDIO_DIST_DIR);
            if exe_candidate.join(STUDIO_INDEX).is_file() {
                return Some(exe_candidate);
            }

            // Packaged layout (.deb / AppImage / portable): the binary lives in
            // <root>/bin while the bundle lives in <root>/ordo-studio/dist, so
            // also look one directory up from the executable.
            if let Some(exe_root) = exe_dir.parent() {
                let exe_root_candidate = exe_root.join(STUDIO_DIST_DIR);
                if exe_root_candidate.join(STUDIO_INDEX).is_file() {
                    return Some(exe_root_candidate);
                }
            }
        }
    }

    let manifest_candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(STUDIO_DIST_DIR);
    if manifest_candidate.join(STUDIO_INDEX).is_file() {
        return Some(manifest_candidate);
    }

    None
}

pub(crate) fn studio_content_type(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

pub(crate) fn static_path_is_reserved(path: &str) -> bool {
    path == "/dashboard"
        || path == "/metrics"
        || path == "/health"
        || path.starts_with("/api")
        || path.starts_with("/ws")
        || path.starts_with("/sse")
        || path.starts_with("/avatar")
        || path.starts_with("/proxy")
}

pub(crate) fn safe_studio_candidate(root: &FsPath, request_path: &str) -> Option<PathBuf> {
    let relative = request_path.trim_start_matches('/');
    if relative.is_empty() || relative == STUDIO_INDEX {
        return Some(root.join(STUDIO_INDEX));
    }

    let relative_path = FsPath::new(relative);
    if relative_path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }

    Some(root.join(relative_path))
}

pub(crate) fn plain_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "not found",
    )
        .into_response()
}

pub(crate) async fn serve_studio_path(path: PathBuf) -> Response {
    let content_type = studio_content_type(&path);
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => plain_not_found(),
    }
}

pub(crate) async fn serve_studio_request(request_path: &str) -> Response {
    let Some(root) = studio_dist_dir() else {
        return plain_not_found();
    };

    let Some(candidate) = safe_studio_candidate(&root, request_path) else {
        return plain_not_found();
    };

    if candidate.is_file() {
        return serve_studio_path(candidate).await;
    }

    if request_path.starts_with("/assets/") || FsPath::new(request_path).extension().is_some() {
        return plain_not_found();
    }

    serve_studio_path(root.join(STUDIO_INDEX)).await
}

pub(crate) async fn studio_index_or_dashboard() -> Response {
    if studio_dist_dir().is_some() {
        serve_studio_request("/index.html").await
    } else {
        dashboard().await.into_response()
    }
}

pub(crate) async fn studio_asset_fallback(OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path();
    if static_path_is_reserved(path) {
        return plain_not_found();
    }

    serve_studio_request(path).await
}

pub(crate) async fn proxy_ollama_route(Path(path): Path<String>) -> Result<Response, ControlApiError> {
    proxy_local_provider("http://127.0.0.1:11434", path).await
}

pub(crate) async fn proxy_lmstudio_route(Path(path): Path<String>) -> Result<Response, ControlApiError> {
    proxy_local_provider("http://127.0.0.1:1234", path).await
}

pub(crate) async fn proxy_local_provider(base: &str, path: String) -> Result<Response, ControlApiError> {
    if path.split('/').any(|segment| segment == "..") {
        return Err(ControlApiError::bad_request("invalid proxy path"));
    }

    let url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let upstream = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|err| {
            ControlApiError::bad_request(format!("local provider unreachable: {err}"))
        })?;
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let bytes = upstream
        .bytes()
        .await
        .map_err(|err| ControlApiError::internal(format!("local provider read failed: {err}")))?;

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    Ok(response)
}

