use crate::*;
use std::path::PathBuf;
use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Json as JsonResponse;
use serde::Deserialize;
use serde_json::{json, Value};

use std::path::Path as FsPath;
pub(crate) fn map_automation_error(err: AutomationError) -> ControlApiError {
    match err {
        AutomationError::Validation(err) => ControlApiError::bad_request(err.to_string()),
        AutomationError::AlreadyExists => ControlApiError::bad_request(err.to_string()),
        AutomationError::NotFound => ControlApiError::not_found(err.to_string()),
        AutomationError::ApprovalRequired => ControlApiError::bad_request(err.to_string()),
    }
}

pub(crate) fn seeded_automation_orchestrator() -> AutomationOrchestrator {
    let mut automation = AutomationOrchestrator::new();
    let _ = automation.register(default_diagnostic_automation());
    let _ = automation.register(default_dreaming_automation());
    automation
}

pub(crate) fn build_planner_from_plugins(
    bus: Arc<dyn Bus>,
    plugins_path: &Option<PathBuf>,
) -> ordo_build_planner::BuildPlannerPeer {
    let Some(path) = build_planner_path_from_plugins(plugins_path) else {
        return ordo_build_planner::BuildPlannerPeer::new(bus);
    };

    let ledgers =
        match ordo_build_planner::BuildLedgerStore::open(&path).and_then(|store| store.list()) {
            Ok(ledgers) => ledgers,
            Err(err) => {
                tracing::warn!(
                    target: "ordo_control::builds",
                    error = %err,
                    path = %path.display(),
                    "failed to load build ledgers; using in-memory build planner"
                );
                return ordo_build_planner::BuildPlannerPeer::new(bus);
            }
        };

    match ordo_build_planner::BuildLedgerTask::open(&path) {
        Ok(task) => ordo_build_planner::BuildPlannerPeer::with_store(bus, task, ledgers),
        Err(err) => {
            tracing::warn!(
                target: "ordo_control::builds",
                error = %err,
                path = %path.display(),
                "failed to open build ledger task; using in-memory build planner"
            );
            ordo_build_planner::BuildPlannerPeer::with_ledgers(bus, ledgers)
        }
    }
}

pub(crate) fn build_planner_path_from_plugins(plugins_path: &Option<PathBuf>) -> Option<PathBuf> {
    let plugins_path = plugins_path.as_ref()?;
    let user_files = plugins_path.parent()?;
    Some(user_files.join("build-ledgers"))
}

pub(crate) fn automation_path_from_plugins(plugins_path: &Option<PathBuf>) -> Option<PathBuf> {
    let plugins_path = plugins_path.as_ref()?;
    let user_files = plugins_path.parent()?;
    Some(user_files.join("automations.json"))
}

pub(crate) fn agent_teams_path_from_plugins(plugins_path: &Option<PathBuf>) -> Option<PathBuf> {
    let plugins_path = plugins_path.as_ref()?;
    let user_files = plugins_path.parent()?;
    Some(user_files.join("agent-teams.json"))
}

pub(crate) fn empty_agent_teams_state() -> Value {
    json!({
        "teams": [],
        "active_team_id": ""
    })
}

pub(crate) fn normalize_agent_teams_state(state: Value) -> Result<Value, ControlApiError> {
    let teams = state
        .get("teams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    for team in &teams {
        let id = team
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                ControlApiError::bad_request("each Agent Team requires a non-empty id")
            })?;
        if !seen.insert(id.to_string()) {
            return Err(ControlApiError::bad_request(format!(
                "duplicate Agent Team id '{id}'"
            )));
        }
        if team
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            return Err(ControlApiError::bad_request(format!(
                "Agent Team '{id}' requires a non-empty name"
            )));
        }
        if !team
            .get("members")
            .and_then(Value::as_array)
            .map(|members| !members.is_empty())
            .unwrap_or(false)
        {
            return Err(ControlApiError::bad_request(format!(
                "Agent Team '{id}' requires at least one member"
            )));
        }
    }
    let requested_active = state
        .get("active_team_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let active_team_id = if requested_active.is_empty()
        || teams.iter().any(|team| {
            team.get("id")
                .and_then(Value::as_str)
                .map(|id| id == requested_active)
                .unwrap_or(false)
        }) {
        requested_active
    } else {
        ""
    };
    Ok(json!({
        "teams": teams,
        "active_team_id": active_team_id
    }))
}

pub(crate) fn load_agent_teams_state(path: &FsPath) -> Result<Value, ControlApiError> {
    if !path.exists() {
        return Ok(empty_agent_teams_state());
    }
    let raw =
        std::fs::read_to_string(path).map_err(|err| ControlApiError::internal(err.to_string()))?;
    let state: Value =
        serde_json::from_str(&raw).map_err(|err| ControlApiError::internal(err.to_string()))?;
    normalize_agent_teams_state(state)
}

pub(crate) fn write_agent_teams_state(path: &FsPath, state: &Value) -> Result<(), ControlApiError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| ControlApiError::internal(err.to_string()))?;
    }
    let encoded = serde_json::to_vec_pretty(state)
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    ordo_store::atomic_write(path, encoded)
        .map_err(|err| ControlApiError::internal(err.to_string()))
}

pub(crate) fn agent_teams_path(state: &ControlApiState) -> Result<PathBuf, ControlApiError> {
    agent_teams_path_from_plugins(&state.plugins_path)
        .ok_or_else(|| ControlApiError::internal("agent teams path not configured"))
}

pub(crate) async fn list_agent_teams_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let path = agent_teams_path(&state)?;
    let snapshot = load_agent_teams_state(&path)?;
    Ok(Json(json!({
        "teams": snapshot.get("teams").cloned().unwrap_or_else(|| json!([])),
        "active_team_id": snapshot.get("active_team_id").and_then(Value::as_str).unwrap_or(""),
        "path": path.display().to_string(),
    })))
}

pub(crate) async fn save_agent_teams_route(
    State(state): State<ControlApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    let path = agent_teams_path(&state)?;
    let snapshot = normalize_agent_teams_state(body)?;
    write_agent_teams_state(&path, &snapshot)?;
    Ok(Json(json!({
        "teams": snapshot.get("teams").cloned().unwrap_or_else(|| json!([])),
        "active_team_id": snapshot.get("active_team_id").and_then(Value::as_str).unwrap_or(""),
        "path": path.display().to_string(),
    })))
}

pub(crate) async fn set_active_agent_team_route(
    State(state): State<ControlApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ControlApiError> {
    let path = agent_teams_path(&state)?;
    let current = load_agent_teams_state(&path)?;
    let team_id = body
        .get("team_id")
        .or_else(|| body.get("active_team_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let teams = current
        .get("teams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !team_id.is_empty()
        && !teams
            .iter()
            .any(|team| team.get("id").and_then(Value::as_str) == Some(team_id))
    {
        return Err(ControlApiError::not_found(format!(
            "Agent Team '{team_id}' not found"
        )));
    }
    let snapshot = normalize_agent_teams_state(json!({
        "teams": teams,
        "active_team_id": team_id,
    }))?;
    write_agent_teams_state(&path, &snapshot)?;
    Ok(Json(json!({
        "teams": snapshot.get("teams").cloned().unwrap_or_else(|| json!([])),
        "active_team_id": snapshot.get("active_team_id").and_then(Value::as_str).unwrap_or(""),
        "path": path.display().to_string(),
    })))
}

pub(crate) fn persist_automations(state: &ControlApiState) -> Result<(), ControlApiError> {
    let Some(path) = &state.automation_path else {
        return Ok(());
    };
    state
        .automation
        .lock()
        .save_to_path(path)
        .map_err(|err| ControlApiError::internal(err.to_string()))
}

pub(crate) async fn list_automations_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let automation = state.automation.lock();
    let automations: Vec<AutomationSpec> = automation.list().into_iter().cloned().collect();
    Ok(Json(json!({
        "automations": automations,
        "events": automation.event_log(),
    })))
}

pub(crate) async fn create_automation_route(
    State(state): State<ControlApiState>,
    Json(spec): Json<AutomationSpec>,
) -> Result<Json<Value>, ControlApiError> {
    let events = {
        let mut automation = state.automation.lock();
        automation.register(spec).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    let automation = state.automation.lock();
    Ok(Json(json!({
        "events": events,
        "automations": automation.list().into_iter().cloned().collect::<Vec<AutomationSpec>>(),
    })))
}

pub(crate) async fn get_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let automation = state.automation.lock();
    let spec = automation
        .get(id)
        .cloned()
        .ok_or_else(|| ControlApiError::not_found("automation not found"))?;
    Ok(Json(json!({ "automation": spec })))
}

pub(crate) async fn approve_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let events = {
        let mut automation = state.automation.lock();
        automation.approve(id).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    Ok(Json(json!({ "events": events })))
}

pub(crate) async fn enable_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let event = {
        let mut automation = state.automation.lock();
        automation.enable(id).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    Ok(Json(json!({ "event": event })))
}

pub(crate) async fn disable_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let event = {
        let mut automation = state.automation.lock();
        automation.disable(id).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    Ok(Json(json!({ "event": event })))
}

pub(crate) async fn delete_automation_route(
    State(state): State<ControlApiState>,
    Path(id): Path<AutomationId>,
) -> Result<Json<Value>, ControlApiError> {
    let event = {
        let mut automation = state.automation.lock();
        automation.delete(id).map_err(map_automation_error)?
    };
    persist_automations(&state)?;
    Ok(Json(json!({ "event": event })))
}

pub(crate) async fn tick_automations_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let mut automation = state.automation.lock();
    let events = automation.tick(chrono::Utc::now());
    Ok(Json(json!({ "events": events })))
}

