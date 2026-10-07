//! End-to-end smoke test for the full Ordo runtime.
//!
//! This boots `PlanningOrdoRuntime` with a temp workspace, picks a free
//! TCP port for the control API, and drives a real HTTP client through
//! the key endpoints â€” capability inventory, runtime profile, and the
//! `cloud.credentials.*` round-trip. It is the test that answers "does
//! the whole thing actually work together" rather than any single
//! component in isolation.

use std::{net::TcpListener, path::PathBuf, time::Duration};

use ordo_runtime::{PlanningOrdoRuntime, RuntimeConfig};
use serde_json::json;
use serde_json::Value;

fn pick_free_port() -> u16 {
    // Bind to port 0 to let the OS choose, then drop the listener so the
    // runtime can bind the same port a moment later. There is a tiny race
    // window but for a local smoke test it is acceptable.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

fn temp_workspace() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ordo-runtime lives inside the Ordo workspace")
        .join("target")
        .join("test-workspaces")
        .join(format!("ordo-e2e-{stamp}"));
    std::fs::create_dir_all(&dir).expect("create workspace");
    dir
}

async fn wait_for_health(origin: &str) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("reqwest client");
    for _ in 0..40 {
        match client.get(format!("{origin}/health")).send().await {
            Ok(response) if response.status().is_success() => return,
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    panic!("control API never reported healthy at {origin}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_boots_without_legacy_model_provider_capabilities() {
    // Force the in-memory credential vault so the test never touches the
    // developer's OS keychain.
    std::env::set_var("ORDO_CLOUD_VAULT", "memory");

    let workspace = temp_workspace();
    let port = pick_free_port();
    let mut config = RuntimeConfig::local_default();
    config.resident_cognition_enabled = false;
    config.database_path = workspace.join("ordo.db");
    config.rag_atomdb_path = workspace.join("ordo-rag.atomdb");
    config.legacy_memory_path = workspace.join("memory.jsonl");
    config.legacy_rag_index_path = workspace.join("rag-index.jsonl");
    config.user_files_path = workspace.join("user-files");
    config.plugins_path = workspace.join("plugins");
    config.ui_extensions_path = workspace.join("ui-extensions");
    config.control_api_bind = Some(format!("127.0.0.1:{port}"));
    config.rag_seed_documents.clear();

    let runtime = PlanningOrdoRuntime::boot(config)
        .await
        .expect("runtime boot");
    let origin = format!("http://127.0.0.1:{port}");
    wait_for_health(&origin).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");

    // Capability inventory should cover every lane that was wired during
    // boot, while the retired cloud-model lane stays absent.
    let capabilities: Value = client
        .get(format!("{origin}/api/capabilities"))
        .send()
        .await
        .expect("capabilities request")
        .json()
        .await
        .expect("capabilities json");
    let names: Vec<String> = capabilities["descriptors"]
        .as_array()
        .expect("descriptors array")
        .iter()
        .filter_map(|d| d["capability"].as_str().map(str::to_string))
        .collect();
    assert!(
        !names.iter().any(|c| c == "cloud.openai.chat"),
        "legacy model-provider capability must not be exposed: {names:?}"
    );
    assert!(
        names.iter().any(|c| c == "planning.plan_initiative"),
        "missing planning.plan_initiative"
    );
    assert!(
        names.iter().any(|c| c == "orchestration.advance_stage"),
        "missing orchestration.advance_stage"
    );

    // Runtime profile should report something sensible.
    let profile: Value = client
        .get(format!("{origin}/api/runtime/profile"))
        .send()
        .await
        .expect("profile request")
        .json()
        .await
        .expect("profile json");
    assert!(profile["profile"].as_str().is_some());

    // Model-provider mutation is not backed by a runtime capability in the
    // resident build. The legacy HTTP route fails closed.
    let upsert_response = client
        .post(format!("{origin}/api/cloud/credentials"))
        .json(&json!({
            "service": "e2e-openai",
            "label": "E2E Test",
            "auth_style": "bearer",
            "secret": "sk-e2e-never-leaks",
        }))
        .send()
        .await
        .expect("upsert request");
    assert!(!upsert_response.status().is_success());
    let response_text = upsert_response.text().await.expect("upsert error body");
    assert!(response_text.contains("no provider handled the requested capability"));
    assert!(!response_text.contains("sk-e2e-never-leaks"));

    // Invoke a pure-data orchestration capability through the bus via the
    // brain. This proves the full bus + capability-host pipeline works
    // end-to-end, not just through the HTTP surface.
    let stage = runtime
        .brain()
        .invoke_tool("orchestration.advance_stage", json!({ "stage": "draft" }))
        .await
        .expect("invoke orchestration.advance_stage");
    assert_eq!(
        stage.get("next_stage").and_then(|value| value.as_str()),
        Some("planning-review")
    );

    runtime.shutdown();
    // Best-effort cleanup â€” don't fail the test if files are still locked
    // by a pending background task on Windows.
    let _ = std::fs::remove_dir_all(&workspace);
}
