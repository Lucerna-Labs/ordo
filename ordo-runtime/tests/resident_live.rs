//! Opt-in live proof for the permanent resident Ordo model.
//!
//! This test loads the real 27B weights, so it is ignored during ordinary
//! `cargo test` runs. Run it explicitly with:
//! `cargo test -p ordo-runtime --test resident_live -- --ignored --nocapture`.

use std::path::PathBuf;

use ordo_assistant::TurnRequest;
use ordo_runtime::{PlanningOrdoRuntime, RuntimeConfig, RuntimeProfile};

const RESIDENT_SERVICE: &str = "ordo-resident";
const RESIDENT_MODEL: &str = "ordo-bonsai-2-27b-uncensored-lightcone-direct";

fn workspace_state() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ordo-runtime lives inside the Ordo workspace")
        .join("target")
        .join("resident-live")
        .join(stamp.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "loads the real resident Bonsai 27B model"]
async fn fake_provider_override_cannot_redirect_the_live_runtime() {
    std::env::set_var("ORDO_CLOUD_VAULT", "memory");

    let state = workspace_state();
    std::fs::create_dir_all(&state).expect("create workspace-local live-test state");

    let mut config = RuntimeConfig::local_default();
    config.profile = RuntimeProfile::Minimal;
    config.database_path = state.join("ordo.db");
    config.rag_atomdb_path = state.join("ordo-rag.atomdb");
    config.legacy_memory_path = state.join("memory.jsonl");
    config.legacy_rag_index_path = state.join("rag-index.jsonl");
    config.official_memory_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ordo-runtime lives inside the Ordo workspace")
        .join("docs")
        .join("official-memory.md");
    config.user_files_path = state.join("user-files");
    config.plugins_path = state.join("plugins");
    config.ui_extensions_path = state.join("ui-extensions");
    config.modes_path = state.join("user-files").join("modes");
    config.code_workspace_path = state.join("user-files").join("workspace");
    config.control_api_bind = None;
    config.rag_seed_documents.clear();

    let runtime = PlanningOrdoRuntime::boot(config)
        .await
        .expect("boot runtime with resident cognition");
    let tool_status = runtime
        .assistant()
        .tool_lightcone_status()
        .await
        .expect("exclusive Tool-Lightcone gateway");
    assert_eq!(tool_status.world_id, "computer-use-stability-v1");
    assert_eq!(tool_status.rule_count, 24);
    assert_eq!(tool_status.edge_count, 96);
    assert_eq!(tool_status.engine_observations, 1_152);
    let result = runtime
        .assistant()
        .turn(TurnRequest {
            user_message: "In one sentence, explain why binary search requires ordered data."
                .to_string(),
            credential: Some("forbidden-provider-override".to_string()),
            use_rag: false,
            use_memory: false,
            use_tools: false,
            ..TurnRequest::default()
        })
        .await
        .expect("resident assistant turn");

    assert_eq!(
        result.turn.credential_service.as_deref(),
        Some(RESIDENT_SERVICE)
    );
    assert_eq!(result.turn.model.as_deref(), Some(RESIDENT_MODEL));
    assert!(!result.turn.assistant_response.trim().is_empty());
    assert!(!result.turn.assistant_response.contains("<think>"));

    println!("{}", result.turn.assistant_response);
    runtime.shutdown();
    let _ = std::fs::remove_dir_all(state);
}
