use axum::{
    extract::{Path, State},
    Json,
};
use ordo_protocol::BuildGateResult;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{ControlApiError, ControlApiState};

#[derive(Debug, Deserialize)]
pub struct StartBuildRequest {
    pub project_id: String,
}

pub async fn list_builds_route(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let peer = state.build_planner.lock().await;
    Ok(Json(json!({
        "builds": peer.ledgers_snapshot(),
        "active_builds": peer.active_builds(),
    })))
}

pub async fn start_build_route(
    State(state): State<ControlApiState>,
    Json(body): Json<StartBuildRequest>,
) -> Result<Json<Value>, ControlApiError> {
    let project_id = body.project_id.trim();
    if project_id.is_empty() {
        return Err(ControlApiError::bad_request("project_id is required"));
    }

    let mut peer = state.build_planner.lock().await;
    let build_id = peer
        .start_build(project_id.to_string())
        .await
        .map_err(|err| ControlApiError::internal(err.to_string()))?;
    let ledger = peer
        .get(&build_id)
        .map(|planner| planner.ledger().clone())
        .ok_or_else(|| ControlApiError::internal("build planner did not retain started build"))?;

    Ok(Json(json!({
        "build_id": build_id,
        "ledger": ledger,
    })))
}

pub async fn get_build_route(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ControlApiError> {
    let build_id = parse_build_id(&id)?;
    let peer = state.build_planner.lock().await;
    let ledger = peer
        .get(&build_id)
        .map(|planner| planner.ledger().clone())
        .ok_or_else(|| ControlApiError::not_found("build not found"))?;

    Ok(Json(json!({
        "ledger": ledger,
    })))
}

pub async fn submit_gate_result_route(
    State(state): State<ControlApiState>,
    Path(id): Path<String>,
    Json(result): Json<BuildGateResult>,
) -> Result<Json<Value>, ControlApiError> {
    let build_id = parse_build_id(&id)?;
    if result.build_id != build_id {
        return Err(ControlApiError::bad_request(
            "gate result build_id does not match route build id",
        ));
    }

    let mut peer = state.build_planner.lock().await;
    let decision = peer
        .handle_gate_result(result)
        .await
        .map_err(|err| ControlApiError::bad_request(err.to_string()))?;
    let ledger = peer
        .get(&build_id)
        .map(|planner| planner.ledger().clone())
        .ok_or_else(|| ControlApiError::internal("build ledger missing after gate result"))?;

    Ok(Json(json!({
        "decision": decision_label(&decision),
        "ledger": ledger,
    })))
}

fn parse_build_id(value: &str) -> Result<Uuid, ControlApiError> {
    Uuid::parse_str(value).map_err(|_| ControlApiError::bad_request("invalid build id"))
}

fn decision_label(decision: &ordo_build_planner::BuildPlannerDecision) -> &'static str {
    match decision {
        ordo_build_planner::BuildPlannerDecision::Advance(_) => "advance",
        ordo_build_planner::BuildPlannerDecision::HardHalt(_) => "hard_halt",
        ordo_build_planner::BuildPlannerDecision::Deferred(_) => "deferred",
        ordo_build_planner::BuildPlannerDecision::AutonomousRetryEligible { .. } => {
            "autonomous_retry_requested"
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request, http::StatusCode};
    use ordo_bus::{Bus, InProcessBus};
    use ordo_protocol::{BuildGateEvidence, BuildGateResult, BuildStep, GateOutcome};
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::build_router;

    #[tokio::test]
    async fn build_routes_start_list_get_and_advance() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let app = build_router(bus);

        let start_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/builds")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"project_id":"demo"}"#))
                    .expect("start build request"),
            )
            .await
            .expect("start build response");
        assert_eq!(start_response.status(), StatusCode::OK);
        let start_body = to_bytes(start_response.into_body(), usize::MAX)
            .await
            .expect("start body");
        let start_json: Value = serde_json::from_slice(&start_body).expect("start json");
        let build_id = start_json["build_id"]
            .as_str()
            .expect("build id")
            .to_string();
        assert_eq!(
            start_json["ledger"]["current_step"].as_str(),
            Some("intake")
        );

        let list_response = app
            .clone()
            .oneshot(
                Request::get("/api/builds")
                    .body(axum::body::Body::empty())
                    .expect("list request"),
            )
            .await
            .expect("list response");
        assert_eq!(list_response.status(), StatusCode::OK);
        let list_body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .expect("list body");
        let list_json: Value = serde_json::from_slice(&list_body).expect("list json");
        assert_eq!(list_json["builds"].as_array().expect("builds").len(), 1);

        let gate = BuildGateResult {
            build_id: build_id.parse().expect("uuid"),
            project_id: "demo".to_string(),
            step: BuildStep::Intake,
            outcome: GateOutcome::Pass {
                evidence: BuildGateEvidence::new("intake passed"),
            },
        };
        let gate_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/builds/{build_id}/gate"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&gate).expect("gate json"),
                    ))
                    .expect("gate request"),
            )
            .await
            .expect("gate response");
        assert_eq!(gate_response.status(), StatusCode::OK);
        let gate_body = to_bytes(gate_response.into_body(), usize::MAX)
            .await
            .expect("gate body");
        let gate_json: Value = serde_json::from_slice(&gate_body).expect("gate json");
        assert_eq!(gate_json["decision"].as_str(), Some("advance"));
        assert_eq!(
            gate_json["ledger"]["current_step"].as_str(),
            Some("blueprint")
        );

        let get_response = app
            .oneshot(
                Request::get(format!("/api/builds/{build_id}"))
                    .body(axum::body::Body::empty())
                    .expect("get request"),
            )
            .await
            .expect("get response");
        assert_eq!(get_response.status(), StatusCode::OK);
    }
}
