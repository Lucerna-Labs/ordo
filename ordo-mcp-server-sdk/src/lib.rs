//! ordo-mcp-server-sdk â€” reusable SDK for building stdio-based
//! MCP server binaries.
//!
//! ## Pattern
//!
//! Every destination MCP (wordpress / bluesky / mastodon / future
//! ones) is its own binary that:
//!
//! 1. Constructs an `McpServer` via `McpServerBuilder`
//! 2. Registers tools with `.tool(name, description, input_schema, handler)`
//! 3. Registers an optional `--test` handler
//! 4. Calls `.run().await` to start the JSON-RPC stdio loop
//!
//! Example:
//!
//! ```ignore
//! use ordo_mcp_server_sdk::{McpServerBuilder, ToolResult};
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let server = McpServerBuilder::new("wordpress-mcp")
//!         .version("0.1.0")
//!         .tool("wordpress.test_connection",
//!               "Verify WP credentials",
//!               json!({ "type": "object" }),
//!               |_args| async move {
//!                   ToolResult::text_ok("connection ok")
//!               })
//!         .test_handler(|| async {
//!             // returns {"status": "ok"} or {"status": "error", "message": ...}
//!             Ok(json!({"status": "ok"}))
//!         })
//!         .build();
//!     server.run().await
//! }
//! ```
//!
//! ## What the SDK handles for you
//!
//! - JSON-RPC 2.0 framing (line-delimited JSON over stdin/stdout)
//! - MCP `initialize` handshake â€” returns server info + capabilities
//! - `notifications/initialized` (no-op response)
//! - `ping` (returns `{}`)
//! - `tools/list` â€” assembles descriptors from registered tools
//! - `tools/call` â€” dispatches to your handler, wraps result in
//!   MCP content blocks
//! - `resources/list` and `prompts/list` â€” empty (signals
//!   "this server has no resources/prompts surface")
//! - Unknown method â†’ JSON-RPC `-32601` Method Not Found
//! - Parse error â†’ JSON-RPC `-32700` Parse Error
//! - Internal handler panic â†’ caught and returned as `isError: true`
//!   tool result (not a JSON-RPC error â€” matches the reference
//!   `ordo-mcp/src/tools.rs` pattern)
//! - `--test` CLI convention: when first arg is `--test`, calls
//!   the registered test handler, prints JSON to stdout, exits

pub mod credentials;
pub mod rpc;
pub mod server;
pub mod tool;

pub use credentials::{CredentialError, VaultCredentialStore};
pub use rpc::{codes, ErrorObject, Request, Response};
pub use server::{McpServer, McpServerBuilder, McpServerError};
pub use tool::{
    Tool, ToolHandler, ToolHandlerFn, ToolResult, ToolsCallParams, ToolsCallResult,
    ToolsListResult, ContentBlock,
};
