use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

const UI_BRIDGE_JS: &str = include_str!("ui_bridge.js");
pub(crate) async fn list_ui_extensions(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let path = match &state.ui_extensions_path {
        Some(path) => path.clone(),
        None => {
            return Ok(Json(json!({
                "extensions_dir": null,
                "extensions": [],
                "errors": [],
            })));
        }
    };
    let report = ordo_ui_extensions::discover_ui_extensions(&path);
    let extensions: Vec<Value> = report
        .loaded
        .iter()
        .map(|loaded| {
            let surfaces: Vec<Value> = loaded
                .manifest
                .surfaces
                .iter()
                .map(|surface| match surface {
                    ordo_ui_extensions::Surface::Tab(tab) => json!({
                        "kind": "tab",
                        "id": tab.id,
                        "label": tab.label,
                        "icon": tab.icon,
                        "description": tab.description,
                        "entry_url": format!(
                            "/api/ui-extensions/{}/files/{}",
                            loaded.manifest.name, tab.entry
                        ),
                    }),
                })
                .collect();
            json!({
                "name": loaded.manifest.name,
                "version": loaded.manifest.version,
                "description": loaded.manifest.description,
                "author": loaded.manifest.author,
                "enabled": loaded.manifest.enabled,
                "surfaces": surfaces,
                "permissions": loaded.manifest.permissions,
                "manifest_path": loaded.manifest_path.display().to_string(),
            })
        })
        .collect();
    let errors: Vec<Value> = report
        .errors
        .iter()
        .map(|err| {
            json!({
                "manifest_path": err.path.display().to_string(),
                "error": err.error,
            })
        })
        .collect();
    Ok(Json(json!({
        "extensions_dir": path.display().to_string(),
        "extensions": extensions,
        "errors": errors,
    })))
}

pub(crate) async fn serve_ui_bridge() -> Response {
    (
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        UI_BRIDGE_JS,
    )
        .into_response()
}

pub(crate) async fn serve_ui_extension_file(
    State(state): State<ControlApiState>,
    Path((name, request_path)): Path<(String, String)>,
) -> Response {
    let root = match &state.ui_extensions_path {
        Some(path) => path.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "ui extensions not configured" })),
            )
                .into_response();
        }
    };
    let report = ordo_ui_extensions::discover_ui_extensions(&root);
    let extension = match report
        .loaded
        .into_iter()
        .find(|ext| ext.manifest.name == name)
    {
        Some(ext) => ext,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("ui extension '{name}' not found") })),
            )
                .into_response();
        }
    };
    if !extension.manifest.enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("ui extension '{name}' is disabled") })),
        )
            .into_response();
    }
    let resolved = match extension.resolve_static(&request_path) {
        Ok(path) => path,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response();
        }
    };
    if !resolved.exists() || !resolved.is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("file not found: {request_path}") })),
        )
            .into_response();
    }
    match std::fs::read(&resolved) {
        Ok(body) => (
            StatusCode::OK,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    ordo_ui_extensions::content_type_for(&resolved),
                ),
                // Extensions load small static assets; a short cache is
                // a reasonable default. Development reloads still work
                // because the manifest list is always fresh.
                (axum::http::header::CACHE_CONTROL, "no-cache"),
            ],
            body,
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

// ---------- review -------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ReviewRecentQuery {
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ReviewDecisionBody {
    #[serde(default)]
    pub(crate) note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewEditBody {
    pub(crate) content: String,
    #[serde(default)]
    pub(crate) note: Option<String>,
}

