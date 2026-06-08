//! Reference plugin â€” the minimum viable MCP server you can install
//! into Ordo. Advertises a single `example.echo` tool that
//! round-trips its arguments as a text content block. Useful as a
//! template and as the target of the stdio integration test.
//!
//! Plugins don't have to be Rust â€” any binary that speaks JSON-RPC 2.0
//! over newline-delimited stdio works. A Python equivalent is about 30
//! lines and lives in `docs/plugins.md`.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();

        let response = match (method, id) {
            ("initialize", Some(id)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {
                        "name": "example-echo-plugin",
                        "version": "0.1.0",
                    }
                }
            }),
            ("notifications/initialized", _) => continue,
            ("tools/list", Some(id)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "example.echo",
                        "description": "Echo the caller's arguments back as a text block.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" }
                            }
                        }
                    }]
                }
            }),
            ("tools/call", Some(id)) => {
                let args = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                let name = message
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if name != "example.echo" {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!("unknown tool '{name}'")
                        }
                    })
                } else {
                    let text = args
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| args.to_string());
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{
                                "type": "text",
                                "text": format!("echoed: {text}")
                            }],
                            "isError": false
                        }
                    })
                }
            }
            _ => continue,
        };
        let _ = writeln!(out, "{response}");
        let _ = out.flush();
    }
}
