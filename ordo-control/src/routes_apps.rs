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
pub(crate) struct AppsListQuery {
    pub(crate) workspace_id: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) limit: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct CreateAppBody {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) slug: Option<String>,
    #[serde(default)]
    pub(crate) workspace_id: Option<String>,
    #[serde(default)]
    pub(crate) metadata: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) actor: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct UpdateAppBody {
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) metadata_patch: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) actor: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ActorBody {
    #[serde(default)]
    pub(crate) actor: Option<String>,
}

pub(crate) fn apps_service(state: &ControlApiState) -> Result<ordo_apps::AppsService, ControlApiError> {
    state
        .apps
        .clone()
        .ok_or_else(|| ControlApiError::internal("apps service not configured"))
}

pub(crate) fn map_apps_error(err: ordo_apps::AppsError) -> ControlApiError {
    use ordo_apps::AppsError;
    match err {
        AppsError::NotFound(_) => ControlApiError::not_found(err.to_string()),
        AppsError::InvalidArgument(_)
        | AppsError::InvalidTransition { .. }
        | AppsError::SlugConflict { .. } => ControlApiError::bad_request(err.to_string()),
        AppsError::Storage(_) => ControlApiError::internal(err.to_string()),
    }
}

pub(crate) async fn list_apps_route(
    State(state): State<ControlApiState>,
    Query(q): Query<AppsListQuery>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let status = match q.status.as_deref() {
        None => None,
        Some(label) => Some(
            ordo_protocol::AppStatus::from_label(label)
                .ok_or_else(|| ControlApiError::bad_request(format!("unknown status: {label}")))?,
        ),
    };
    let apps = service
        .list(ordo_apps::AppsQuery {
            workspace_id: q.workspace_id,
            status,
            limit: q.limit,
        })
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "apps": apps })))
}

pub(crate) async fn create_app_route(
    State(state): State<ControlApiState>,
    Json(body): Json<CreateAppBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let actor = body.actor.clone().unwrap_or_else(|| "operator".into());
    let app = service
        .create(
            ordo_apps::NewApp {
                name: body.name,
                description: body.description,
                slug: body.slug,
                workspace_id: body.workspace_id,
                metadata: body.metadata,
            },
            &actor,
        )
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

pub(crate) async fn get_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let app = service
        .get(&ordo_apps::AppRef::Id(id))
        .map_err(map_apps_error)?
        .ok_or_else(|| ControlApiError::not_found("app not found"))?;
    Ok(Json(json!({ "app": app })))
}

pub(crate) async fn update_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<UpdateAppBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let app = service
        .update(
            &ordo_apps::AppRef::Id(id),
            ordo_apps::AppUpdate {
                name: body.name,
                description: body.description,
                metadata_patch: body.metadata_patch,
                actor: body.actor,
            },
        )
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

pub(crate) async fn list_app_events_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let events = service.events(id).map_err(map_apps_error)?;
    Ok(Json(json!({ "events": events })))
}

pub(crate) async fn get_app_state_at_version_route(
    State(state): State<ControlApiState>,
    Path((id, seq)): Path<(uuid::Uuid, u64)>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let app = service.state_at_version(id, seq).map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app, "seq": seq })))
}

pub(crate) async fn publish_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<ActorBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let actor = body.actor.unwrap_or_else(|| "operator".into());
    let app = service
        .publish(&ordo_apps::AppRef::Id(id), &actor)
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

pub(crate) async fn unpublish_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<ActorBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let actor = body.actor.unwrap_or_else(|| "operator".into());
    let app = service
        .unpublish(&ordo_apps::AppRef::Id(id), &actor)
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

pub(crate) async fn archive_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let app = service
        .archive(&ordo_apps::AppRef::Id(id), "operator")
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

pub(crate) async fn unarchive_app_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<ActorBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let actor = body.actor.unwrap_or_else(|| "operator".into());
    let app = service
        .unarchive(&ordo_apps::AppRef::Id(id), &actor)
        .await
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "app": app })))
}

// -- deployments HTTP routes (Phase 3.3) ----------------------------

#[derive(Deserialize)]
pub(crate) struct CreateDeploymentBody {
    #[serde(default)]
    pub(crate) preview_path: Option<String>,
    #[serde(default)]
    pub(crate) note: String,
}

pub(crate) async fn list_deployments_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let deployments = service.list_deployments(id).map_err(map_apps_error)?;
    Ok(Json(json!({ "deployments": deployments })))
}

pub(crate) async fn create_deployment_route(
    State(state): State<ControlApiState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<CreateDeploymentBody>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let deployment = service
        .create_deployment(id, body.preview_path, &body.note)
        .map_err(map_apps_error)?;
    Ok(Json(json!({ "deployment": deployment })))
}

pub(crate) async fn promote_deployment_route(
    State(state): State<ControlApiState>,
    Path((_app_id, dep_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let deployment = service.promote_deployment(dep_id).map_err(map_apps_error)?;
    Ok(Json(json!({ "deployment": deployment })))
}

pub(crate) async fn fail_deployment_route(
    State(state): State<ControlApiState>,
    Path((_app_id, dep_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<Json<Value>, ControlApiError> {
    let service = apps_service(&state)?;
    let deployment = service.fail_deployment(dep_id).map_err(map_apps_error)?;
    Ok(Json(json!({ "deployment": deployment })))
}

// -- webhooks HTTP routes (Phase 3.1) -------------------------------

