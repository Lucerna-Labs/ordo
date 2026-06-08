//! MCP protocol vocabulary layered on top of JSON-RPC 2.0.
//!
//! We implement the subset a tool-calling client actually needs:
//!   - `initialize` + `notifications/initialized`
//!   - `tools/list`
//!   - `tools/call`
//!
//! Resources and prompts are advertised as absent capabilities â€” if a
//! client asks, we respond with empty lists rather than erroring, so
//! we pass compatibility probes cleanly.
//!
//! Spec reference: https://modelcontextprotocol.io (2024-11-05 draft).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP protocol version we speak. Clients that don't match negotiate
/// via the `initialize` handshake.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server identity returned during initialize.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: &'static str,
}

pub const SERVER_INFO: ServerInfo = ServerInfo {
    name: "ordo-mcp",
    version: env!("CARGO_PKG_VERSION"),
};

/// Capability bundle. We enable `tools` and declare everything else
/// as `None` so clients know what not to ask for.
#[derive(Debug, Clone, Serialize)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<Value>,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            tools: ToolsCapability {
                list_changed: false,
            },
            resources: None,
            prompts: None,
            logging: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsCapability {
    /// True when the server will emit `notifications/tools/list_changed`.
    /// We advertise `false` because the Ordo tool surface is
    /// stable within a session â€” clients re-list on reconnect.
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InitializeParams {
    /// Client-proposed protocol version. We log it for diagnostics
    /// and echo our supported version back.
    #[serde(rename = "protocolVersion", default)]
    pub protocol_version: Option<String>,
    #[serde(rename = "clientInfo", default)]
    pub client_info: Option<Value>,
    #[serde(default)]
    pub capabilities: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
    pub capabilities: ServerCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// `tools/list` result.
#[derive(Debug, Clone, Serialize)]
pub struct ToolsListResult {
    pub tools: Vec<ToolDescriptor>,
}

/// Shape of a single tool advertised to the client. `input_schema` is
/// a JSON Schema object describing the arguments.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// `tools/call` params.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolsCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Option<Value>,
}

/// `tools/call` result. MCP's content model is a list of content
/// blocks; we emit a single text block containing JSON whenever the
/// result isn't inherently text/image/etc.
#[derive(Debug, Clone, Serialize)]
pub struct ToolsCallResult {
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError", default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

impl ToolsCallResult {
    pub fn text_ok(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            is_error: false,
        }
    }

    pub fn text_err(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            is_error: true,
        }
    }

    /// Serialize a JSON value into a text content block. MCP content
    /// is human-shaped; JSON-in-text is the common pattern for tools
    /// whose result is structured.
    pub fn json_ok(value: &Value) -> Self {
        Self::text_ok(serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initialize_result_serializes_capabilities() {
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            server_info: SERVER_INFO,
            capabilities: ServerCapabilities::default(),
            instructions: None,
        };
        let s = serde_json::to_value(&result).unwrap();
        assert_eq!(s["serverInfo"]["name"], "ordo-mcp");
        assert_eq!(s["capabilities"]["tools"]["listChanged"], false);
        assert!(s["capabilities"].get("resources").is_none());
    }

    #[test]
    fn tools_list_result_round_trip() {
        let list = ToolsListResult {
            tools: vec![ToolDescriptor {
                name: "apps.list".into(),
                description: "List apps".into(),
                input_schema: json!({"type": "object"}),
            }],
        };
        let s = serde_json::to_value(&list).unwrap();
        assert_eq!(s["tools"][0]["name"], "apps.list");
        assert_eq!(s["tools"][0]["inputSchema"]["type"], "object");
    }
}
