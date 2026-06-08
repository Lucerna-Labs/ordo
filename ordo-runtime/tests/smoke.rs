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
    let dir = std::env::temp_dir().join(format!("ordo-e2e-{stamp}"));
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
async fn runtime_boots_and_round_trips_cloud_credentials() {
    // Force the in-memory credential vault so the test never touches the
    // developer's OS keychain.
    std::env::set_var("ORDO_CLOUD_VAULT", "memory");

    let workspace = temp_workspace();
    let port = pick_free_port();
    let mut config = RuntimeConfig::local_default();
    config.database_path = workspace.join("ordo.db");
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
    // boot â€” including the new `cloud.*` and Ordo operations surfaces.
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
        names.iter().any(|c| c == "cloud.openai.chat"),
        "missing cloud.openai.chat in {names:?}"
    );
    assert!(
        names.iter().any(|c| c == "cloud.credentials.list"),
        "missing cloud.credentials.list"
    );
    assert!(
        names.iter().any(|c| c == "planning.plan_initiative"),
        "missing planning.plan_initiative"
    );
    assert!(
        names.iter().any(|c| c == "planning.capture_brief"),
        "missing planning.capture_brief"
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

    // Round-trip a cloud credential through the control API.
    let upsert: Value = client
        .post(format!("{origin}/api/cloud/credentials"))
        .json(&json!({
            "service": "e2e-openai",
            "label": "E2E Test",
            "auth_style": "bearer",
            "secret": "sk-e2e-never-leaks",
        }))
        .send()
        .await
        .expect("upsert request")
        .json()
        .await
        .expect("upsert json");
    assert_eq!(upsert["credential"]["service"].as_str(), Some("e2e-openai"));
    assert!(upsert["credential"]["has_secret"]
        .as_bool()
        .unwrap_or(false));
    assert!(
        upsert["credential"]
            .get("secret")
            .is_none_or(|v| v.is_null()),
        "secret field must not appear in the response"
    );
    let response_text = serde_json::to_string(&upsert).expect("serialize upsert");
    assert!(
        !response_text.contains("sk-e2e-never-leaks"),
        "secret leaked in response: {response_text}"
    );

    let list: Value = client
        .get(format!("{origin}/api/cloud/credentials"))
        .send()
        .await
        .expect("list request")
        .json()
        .await
        .expect("list json");
    let credentials = list["credentials"].as_array().expect("credentials array");
    assert!(
        credentials
            .iter()
            .any(|c| c["service"].as_str() == Some("e2e-openai")),
        "expected e2e-openai in credentials: {credentials:?}"
    );

    let delete: Value = client
        .delete(format!("{origin}/api/cloud/credentials"))
        .json(&json!({ "service": "e2e-openai" }))
        .send()
        .await
        .expect("delete request")
        .json()
        .await
        .expect("delete json");
    assert_eq!(delete["removed"].as_bool(), Some(true));

    // Invoke the pure-data planning capability through the bus via the
    // brain. This proves the full bus + capability-host pipeline works
    // end-to-end, not just through the HTTP surface.
    let brief = runtime
        .brain()
        .invoke_tool(
            "planning.capture_brief",
            json!({
                "title": "Spring colorway",
                "goal": "launch in March",
                "audience": "trail runners",
                "deliverables": ["hero video", "landing page"],
            }),
        )
        .await
        .expect("invoke planning.capture_brief");
    assert!(
        brief.get("brief").is_some(),
        "expected brief in output: {brief}"
    );

    runtime.shutdown();
    // Best-effort cleanup â€” don't fail the test if files are still locked
    // by a pending background task on Windows.
    let _ = std::fs::remove_dir_all(&workspace);
}
