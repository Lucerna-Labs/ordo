//! External plugin loader for Ordo.
//!
//! Plugins are subprocesses that speak the Model Context Protocol
//! (MCP) over stdio. Each plugin ships a `plugin.json` manifest that
//! declares how to spawn it and what capability lanes it expects to
//! contribute. On boot the runtime scans its plugin directory, spawns
//! every enabled plugin, runs the MCP `initialize` + `tools/list`
//! handshake, and registers each advertised tool as a first-class
//! capability on the shared bus.
//!
//! This is the "browser extension" story for Ordo â€” users can
//! drop a plugin into `user-files/plugins/` (or install one via the
//! CLI/dashboard) and get new capabilities without rebuilding the
//! core.

pub mod client;
pub mod host;
pub mod manifest;
pub mod protocol;
pub mod provider;
pub mod transport;

pub use client::{McpClient, McpClientError};
pub use host::{PluginHost, PluginLoadStatus, PluginState};
pub use manifest::{
    discover_plugins, DiscoveryError, DiscoveryReport, LoadedManifest, ManifestError,
    PluginManifest, RESERVED_CORE_LANES,
};
pub use protocol::{McpContentBlock, McpToolDescriptor, McpToolResult, PROTOCOL_VERSION};
pub use provider::PluginProvider;
pub use transport::{McpTransport, StdioTransport, TransportError};

#[cfg(test)]
mod tests {
    //! Unit tests using the in-memory channel transport â€” no
    //! subprocesses involved. The real-subprocess path is exercised by
    //! `tests/stdio_plugin.rs`.

    use super::*;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use transport::test_support::{ChannelTransport, TestServerHandle};

    async fn dummy_server(handle: TestServerHandle) {
        // Minimal MCP server: responds to initialize, tools/list,
        // tools/call. Lives only for the duration of the test.
        loop {
            let line = match handle.recv_line().await {
                Some(line) => line,
                None => return,
            };
            let msg: Value = match serde_json::from_str(&line) {
                Ok(msg) => msg,
                Err(_) => continue,
            };
            let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
            let id = msg.get("id").and_then(|v| v.as_u64());
            match (method, id) {
                ("initialize", Some(id)) => {
                    handle
                        .send_line(
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "protocolVersion": PROTOCOL_VERSION,
                                    "capabilities": {"tools": {}},
                                    "serverInfo": {"name": "dummy", "version": "0.1.0"},
                                }
                            })
                            .to_string(),
                        )
                        .await;
                }
                ("notifications/initialized", _) => {
                    // No response for notifications.
                }
                ("tools/list", Some(id)) => {
                    handle
                        .send_line(
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "tools": [{
                                        "name": "example.echo",
                                        "description": "Echo input back",
                                        "inputSchema": {"type": "object"}
                                    }]
                                }
                            })
                            .to_string(),
                        )
                        .await;
                }
                ("tools/call", Some(id)) => {
                    let args = msg
                        .pointer("/params/arguments")
                        .cloned()
                        .unwrap_or(Value::Null);
                    handle
                        .send_line(
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "content": [{
                                        "type": "text",
                                        "text": format!("echo: {args}")
                                    }],
                                    "isError": false
                                }
                            })
                            .to_string(),
                        )
                        .await;
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn client_initializes_lists_tools_and_calls_tool_in_memory() {
        let (client_transport, handle) = ChannelTransport::pair();
        tokio::spawn(async move { dummy_server(handle).await });
        let transport: Arc<dyn McpTransport> = Arc::new(client_transport);
        let client = Arc::new(McpClient::new(transport));
        client.start_dispatcher().await;

        let init = client
            .initialize("test-runner", "0.1.0")
            .await
            .expect("initialize");
        assert_eq!(init["serverInfo"]["name"].as_str(), Some("dummy"));

        let tools = client.list_tools().await.expect("tools/list");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "example.echo");

        let result = client
            .call_tool("example.echo", json!({"ping": "pong"}))
            .await
            .expect("call_tool");
        assert!(!result.is_error);
        let text = result.content[0].as_text().unwrap_or_default();
        assert!(text.contains("echo: "), "got: {text}");
        assert!(text.contains("ping"), "got: {text}");
    }

    #[tokio::test]
    async fn tool_call_times_out_cleanly() {
        let (client_transport, _handle) = ChannelTransport::pair();
        let transport: Arc<dyn McpTransport> = Arc::new(client_transport);
        let client = Arc::new(
            McpClient::new(transport).with_call_timeout(std::time::Duration::from_millis(150)),
        );
        client.start_dispatcher().await;
        // Server never responds â€” the send works but no response arrives.
        let err = client
            .list_tools()
            .await
            .expect_err("expected timeout error");
        assert!(matches!(err, McpClientError::Timeout), "got: {err:?}");
    }
}
