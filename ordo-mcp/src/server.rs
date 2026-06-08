//! Request router â€” takes a parsed JSON-RPC request, returns a
//! response. Transport-agnostic (stdio + HTTP both drive this).

use serde_json::{json, Value};

use crate::mcp::{
    self, InitializeParams, InitializeResult, ServerCapabilities, ToolsCallParams, ToolsListResult,
};
use crate::rpc::{codes, Request, Response};
use crate::runtime::RuntimeClient;
use crate::tools;

#[derive(Clone)]
pub struct Server {
    client: RuntimeClient,
}

impl Server {
    pub fn new(client: RuntimeClient) -> Self {
        Self { client }
    }

    /// Handle a single incoming request. Notifications (no `id`) are
    /// acknowledged silently â€” the caller should check
    /// `response.is_some()` before writing.
    pub async fn handle(&self, req: Request) -> Option<Response> {
        let id = req.id.clone();
        let is_notification = id.is_none();

        let result = self.dispatch(&req).await;

        if is_notification {
            return None;
        }
        let id = id.unwrap_or(Value::Null);
        Some(match result {
            Ok(value) => Response::ok(id, value),
            Err((code, message)) => Response::error(id, code, message),
        })
    }

    async fn dispatch(&self, req: &Request) -> Result<Value, (i32, String)> {
        match req.method.as_str() {
            "initialize" => {
                let params: InitializeParams = match &req.params {
                    Some(value) => serde_json::from_value(value.clone()).map_err(|err| {
                        (codes::INVALID_PARAMS, format!("initialize params: {err}"))
                    })?,
                    None => InitializeParams {
                        protocol_version: None,
                        client_info: None,
                        capabilities: None,
                    },
                };
                if let Some(pv) = &params.protocol_version {
                    tracing::info!(
                        target: "ordo_mcp",
                        client_protocol = %pv,
                        "initialize"
                    );
                }
                let result = InitializeResult {
                    protocol_version: mcp::PROTOCOL_VERSION,
                    server_info: mcp::SERVER_INFO,
                    capabilities: ServerCapabilities::default(),
                    instructions: Some(
                        "Ordo MCP bridge. Call `cc_assistant_turn` to talk to the \
                         Assistant; use the `cc_apps_*` and `cc_files_*` tools for direct app / \
                         file management. `cc_invoke_tool` is the escape hatch for plugin \
                         capabilities."
                            .into(),
                    ),
                };
                serde_json::to_value(&result)
                    .map_err(|err| (codes::INTERNAL_ERROR, err.to_string()))
            }
            // Notifications from the client after initialize. We just
            // acknowledge (the handler returns None because id is
            // absent).
            "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
            "tools/list" => {
                let result = ToolsListResult {
                    tools: tools::describe_tools(),
                };
                serde_json::to_value(&result)
                    .map_err(|err| (codes::INTERNAL_ERROR, err.to_string()))
            }
            "tools/call" => {
                let params: ToolsCallParams = match &req.params {
                    Some(value) => serde_json::from_value(value.clone())
                        .map_err(|err| (codes::INVALID_PARAMS, format!("tools/call: {err}")))?,
                    None => {
                        return Err((codes::INVALID_PARAMS, "tools/call requires params".into()))
                    }
                };
                let args = params.arguments.unwrap_or_else(|| json!({}));
                let result = tools::dispatch_call(&self.client, &params.name, &args).await;
                serde_json::to_value(&result)
                    .map_err(|err| (codes::INTERNAL_ERROR, err.to_string()))
            }
            // Empty-list responses for optional capabilities. Many
            // clients probe these regardless of capability
            // advertisement; being polite avoids spurious error logs.
            "resources/list" => Ok(json!({"resources": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
            "ping" => Ok(json!({})),
            other => Err((codes::METHOD_NOT_FOUND, format!("unknown method: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn server() -> Server {
        let client = RuntimeClient::new(Config::default()).expect("client");
        Server::new(client)
    }

    #[tokio::test]
    async fn initialize_returns_server_info_and_tools_capability() {
        let srv = server();
        let req = Request {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "initialize".into(),
            params: Some(json!({"protocolVersion": "2024-11-05"})),
        };
        let resp = srv.handle(req).await.expect("response");
        let result = resp.result.expect("result");
        assert_eq!(result["serverInfo"]["name"], "ordo-mcp");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["protocolVersion"], mcp::PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn tools_list_includes_assistant_turn() {
        let srv = server();
        let req = Request {
            jsonrpc: "2.0".into(),
            id: Some(json!(2)),
            method: "tools/list".into(),
            params: None,
        };
        let resp = srv.handle(req).await.expect("response");
        let result = resp.result.expect("result");
        let tools = result["tools"].as_array().expect("tools array");
        assert!(tools.iter().any(|t| t["name"] == "cc_assistant_turn"));
        assert!(tools.iter().any(|t| t["name"] == "cc_apps_list"));
    }

    #[tokio::test]
    async fn notification_produces_no_response() {
        let srv = server();
        let req = Request {
            jsonrpc: "2.0".into(),
            id: None,
            method: "notifications/initialized".into(),
            params: None,
        };
        let resp = srv.handle(req).await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let srv = server();
        let req = Request {
            jsonrpc: "2.0".into(),
            id: Some(json!(3)),
            method: "something/unknown".into(),
            params: None,
        };
        let resp = srv.handle(req).await.expect("response");
        let error = resp.error.expect("error");
        assert_eq!(error.code, codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn resources_and_prompts_list_return_empty() {
        let srv = server();
        for method in ["resources/list", "prompts/list"] {
            let req = Request {
                jsonrpc: "2.0".into(),
                id: Some(json!(9)),
                method: method.into(),
                params: None,
            };
            let resp = srv.handle(req).await.expect("response");
            assert!(resp.result.is_some(), "{method} should return a result");
        }
    }
}
