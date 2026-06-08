//! JSON-RPC 2.0 message shapes.
//!
//! This file is MCP-agnostic — MCP is a thin vocabulary layered on
//! top of JSON-RPC 2.0. Keeping the two separate lets us use the same
//! types in future if we ever bridge to another JSON-RPC-flavored
//! protocol.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC 2.0 request. `id` is absent for notifications (we only
/// emit those, we never receive them in MCP's request direction).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response. Either `result` xor `error` is set.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(ErrorObject {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Standard JSON-RPC 2.0 error codes. MCP reuses these.
pub mod codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_roundtrip_without_id() {
        let wire = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let parsed: Request = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed.method, "notifications/initialized");
        assert!(parsed.id.is_none());
    }

    #[test]
    fn response_ok_serializes_result_only() {
        let resp = Response::ok(json!(1), json!({"answer": 42}));
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"result\""));
        assert!(!s.contains("\"error\""));
    }

    #[test]
    fn response_error_serializes_error_only() {
        let resp = Response::error(json!(1), codes::METHOD_NOT_FOUND, "nope");
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"error\""));
        assert!(!s.contains("\"result\""));
    }
}
