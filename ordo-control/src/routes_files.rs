use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub(crate) struct FilesListQuery {
    pub(crate) workspace_id: Option<String>,
    pub(crate) app_id: Option<uuid::Uuid>,
    pub(crate) limit: Option<u32>,
}

pub(crate) async fn list_files_route(
    State(state): State<ControlApiState>,
    Query(q): Query<FilesListQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = state
        .files
        .clone()
        .ok_or_else(|| ControlApiError::internal("files service not configured"))?;
    let files = service
        .list(ordo_files::FilesQuery {
            workspace_id: q.workspace_id,
            app_id: q.app_id,
            limit: q.limit,
        })
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(json!({ "files": files })))
}

pub(crate) async fn get_file_metadata_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = state
        .files
        .clone()
        .ok_or_else(|| ControlApiError::internal("files service not configured"))?;
    let entry = service
        .get_metadata(id)
        .map_err(|err| ControlApiError::internal(err.to_string()))?
        .ok_or_else(|| ControlApiError::not_found("file not found"))?;
    Ok(Json(json!({ "file": entry })))
}

pub(crate) async fn download_file_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let Some(service) = state.files.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "files service not configured" })),
        )
            .into_response();
    };
    match service.download(id).await {
        Ok((entry, bytes)) => {
            use axum::http::header;
            let mut response = bytes.into_response();
            let headers = response.headers_mut();
            if let Ok(value) = header::HeaderValue::from_str(&entry.content_type) {
                headers.insert(header::CONTENT_TYPE, value);
            }
            let disposition = format!(
                "inline; filename=\"{}\"",
                sanitize_header(&entry.original_name)
            );
            if let Ok(value) = header::HeaderValue::from_str(&disposition) {
                headers.insert(header::CONTENT_DISPOSITION, value);
            }
            response
        }
        Err(ordo_files::FilesError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file not found" })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

pub(crate) async fn delete_file_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = state
        .files
        .clone()
        .ok_or_else(|| ControlApiError::internal("files service not configured"))?;
    let removed = service
        .delete(id)
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(
        json!({ "deleted": removed.is_some(), "file": removed }),
    ))
}

#[derive(Deserialize)]
pub(crate) struct UploadJsonBody {
    pub(crate) original_name: String,
    #[serde(default)]
    pub(crate) content_type: Option<String>,
    #[serde(default)]
    pub(crate) workspace_id: Option<String>,
    #[serde(default)]
    pub(crate) app_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub(crate) created_by: Option<String>,
    /// Base64-encoded file bytes. Using a JSON body keeps the endpoint
    /// consistent with the MCP provider's `files.upload` tool â€” both
    /// carry bytes as base64. Raw multipart can be added as a second
    /// endpoint when a streaming use case emerges.
    pub(crate) data_base64: String,
}

pub(crate) async fn upload_file_json(
    State(state): State<ControlApiState>,
    Json(body): Json<UploadJsonBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = state
        .files
        .clone()
        .ok_or_else(|| ControlApiError::internal("files service not configured"))?;
    let bytes = base64_decode_minimal(&body.data_base64).map_err(ControlApiError::bad_request)?;
    let entry = service
        .upload(
            ordo_files::NewUpload {
                original_name: body.original_name,
                content_type: body.content_type,
                workspace_id: body.workspace_id,
                created_by: body.created_by,
                app_id: body.app_id,
            },
            bytes,
        )
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    Ok(Json(json!({ "file": entry })))
}

/// Minimal base64 decoder â€” same algorithm as
/// `ordo-files/src/provider.rs` so the two stay in lockstep. Local
/// helper keeps the control crate's dep graph unchanged.
pub(crate) fn base64_decode_minimal(input: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in input.chars() {
        if c == '=' {
            break;
        }
        if c.is_whitespace() {
            continue;
        }
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - b'A' as u32,
            'a'..='z' => (c as u32) - b'a' as u32 + 26,
            '0'..='9' => (c as u32) - b'0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid base64 character '{c}'")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Ok(buf)
}

/// Strip anything that would break a `Content-Disposition` value â€”
/// namely double quotes and CRLF. Non-ASCII survives as-is since
/// modern browsers accept UTF-8 filenames in the simple form.
pub(crate) fn sanitize_header(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '"' | '\r' | '\n'))
        .collect()
}

// -- apps HTTP routes (Phase 1.5) -----------------------------------
//
// Mirrors `AppsService`. Status transitions get dedicated POST
// endpoints rather than status-in-PATCH so review gating can be
// applied uniformly (publish + archive are destructive per Rule 5).

