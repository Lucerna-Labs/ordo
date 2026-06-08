//! Integration test: spawns the `example-echo-plugin` binary as a real
//! subprocess and drives the full MCP handshake through the stdio
//! transport. This is the test that proves the plumbing works end-to-
//! end, not just at the trait level.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ordo_plugins::{host::PluginHost, manifest::PluginManifest, McpTransport};
use ordo_plugins::{McpClient, StdioTransport};
use tokio::process::Command;

fn example_binary() -> PathBuf {
    // Cargo exports CARGO_BIN_EXE_<name> when running integration tests
    // against a binary in the same crate.
    PathBuf::from(env!("CARGO_BIN_EXE_example-echo-plugin"))
}

#[tokio::test]
async fn stdio_plugin_handshake_list_and_call() {
    let binary = example_binary();
    assert!(
        binary.exists(),
        "expected example plugin binary at {}",
        binary.display()
    );

    let mut child = Command::new(&binary)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn example-echo-plugin");

    // Take handles before constructing the transport wrapper.
    let _stderr = child.stderr.take();
    let transport_inner = {
        let temp_child = child;
        StdioTransport::new(temp_child).expect("build stdio transport")
    };
    let transport: Arc<dyn McpTransport> = Arc::new(transport_inner);

    let client = Arc::new(McpClient::new(transport).with_call_timeout(Duration::from_secs(10)));
    client.start_dispatcher().await;

    let init = client
        .initialize("stdio-integration-test", "0.1.0")
        .await
        .expect("initialize");
    assert_eq!(
        init["serverInfo"]["name"].as_str(),
        Some("example-echo-plugin")
    );

    let tools = client.list_tools().await.expect("tools/list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "example.echo");

    let result = client
        .call_tool(
            "example.echo",
            serde_json::json!({ "text": "hello plugins" }),
        )
        .await
        .expect("tool call");
    assert!(!result.is_error);
    let text = result.content[0].as_text().unwrap_or_default();
    assert_eq!(text, "echoed: hello plugins");
}

#[tokio::test]
async fn plugin_host_loads_manifest_from_disk_and_exposes_capability() {
    // Build a temporary plugin directory: one manifest pointing at the
    // compiled example binary.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let root = std::env::temp_dir().join(format!("ordo-plugin-test-{stamp}"));
    let plugin_dir = root.join("echo");
    std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin dir");

    let manifest = PluginManifest {
        name: "echo".into(),
        version: "0.1.0".into(),
        description: "Reference echo plugin".into(),
        command: example_binary().to_string_lossy().to_string(),
        args: Vec::new(),
        expected_lanes: vec!["example.".into()],
        required_env: Vec::new(),
        env: std::collections::HashMap::new(),
        core_override: false,
        enabled: true,
    };
    let manifest_path = plugin_dir.join("plugin.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");

    let report = ordo_plugins::discover_plugins(&root);
    assert_eq!(report.loaded.len(), 1, "errors: {:?}", report.errors);

    let host = PluginHost::from_discovery(report).await;
    assert_eq!(host.statuses.len(), 1);
    assert!(
        matches!(host.statuses[0].state, ordo_plugins::PluginState::Active),
        "state: {:?}",
        host.statuses[0].state
    );
    assert_eq!(host.statuses[0].tool_count, 1);
    assert_eq!(host.statuses[0].capabilities, vec!["example.echo"]);

    // Drive the provider end-to-end: the host's PluginProvider should
    // forward `example.echo` through the MCP subprocess and return the
    // echoed text.
    use ordo_mcp_host::CapabilityProvider;
    let provider = host.plugins[0].provider.clone();
    let outcome = provider
        .handle_tool_call(
            "example.echo",
            &serde_json::json!({ "text": "via provider" }),
        )
        .await
        .expect("provider handled call");
    match outcome {
        ordo_mcp_host::ToolCallResult::Completed { result } => {
            let text = result.get("text").and_then(|v| v.as_str()).unwrap_or("");
            assert_eq!(text, "echoed: via provider");
        }
        ordo_mcp_host::ToolCallResult::Failed { error } => {
            panic!("expected Completed, got Failed: {error}");
        }
    }

    host.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn plugin_host_rejects_plugin_advertising_reserved_lane() {
    // Manifest declares the reserved `cloud.` prefix without
    // `core_override` â€” should fail validation before spawn.
    let manifest = ordo_plugins::PluginManifest {
        name: "sneaky".into(),
        version: "0.1.0".into(),
        description: String::new(),
        command: "irrelevant".into(),
        args: Vec::new(),
        expected_lanes: vec!["cloud.".into()],
        required_env: Vec::new(),
        env: std::collections::HashMap::new(),
        core_override: false,
        enabled: true,
    };
    let err = manifest.validate().expect_err("should reject");
    let msg = err.to_string();
    assert!(msg.contains("reserved"), "got: {msg}");
}
