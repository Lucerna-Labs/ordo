//! Subset of the Model Context Protocol (MCP) messages that the Ordo
//! runtime needs to host external plugins.
//!
//! MCP is JSON-RPC 2.0 over stdio with newline-delimited messages. We
//! intentionally implement only the slice required for:
//!
//! 1. `initialize` / `notifications/initialized`
//! 2. `tools/list`
//! 3. `tools/call`
//!
//! Everything else (prompts, resources, sampling) can be added later.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Request envelope â€” JSON-RPC 2.0 with an always-present integer id.
#[derive(Debug, Clone, Serialize)]
pub struct McpRequest<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl<'a> McpRequest<'a> {
    pub fn new(id: u64, method: &'a str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params: if params.is_null() { None } else { Some(params) },
        }
    }
}

/// Client â†’ server notification (no id, no response expected).
#[derive(Debug, Clone, Serialize)]
pub struct McpNotification<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl<'a> McpNotification<'a> {
    pub fn new(method: &'a str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params: if params.is_null() { None } else { Some(params) },
        }
    }
}

/// Any message received from a plugin. JSON-RPC responses and
/// server-originated notifications share the wire; we hand both back
/// to the caller and let the dispatcher disambiguate.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum McpIncoming {
    Response(McpResponse),
    Notification(McpIncomingNotification),
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<McpError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpIncomingNotification {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// Tool descriptor returned by `tools/list`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Option<Value>,
}

/// Payload returned by `tools/call`. Plugins can either return
/// structured content blocks (text, images, resources) or an error
/// flag.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpToolResult {
    #[serde(default)]
    pub content: Vec<McpContentBlock>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContentBlock {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

impl McpContentBlock {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            McpContentBlock::Text { text } => Some(text),
            McpContentBlock::Other => None,
        }
    }
}
