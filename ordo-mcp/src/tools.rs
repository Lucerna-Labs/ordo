//! Tool registry â€” the bridge from MCP `tools/call` to Ordo
//! HTTP endpoints.
//!
//! The set is curated, not auto-discovered: we want sharp, well-named
//! tools for agents (one concept per tool, non-overlapping scopes)
//! rather than dumping the whole capability inventory. Plugin and
//! domain-specific capabilities are still reachable via the
//! `cc_invoke_tool` escape hatch.
//!
//! Keep this list disciplined. Every tool added here increases the
//! LLM's attention budget.

use serde_json::{json, Value};

use crate::mcp::{ToolDescriptor, ToolsCallResult};
use crate::runtime::{RuntimeClient, RuntimeError};

pub fn describe_tools() -> Vec<ToolDescriptor> {
    vec![
        // ---- Apps lifecycle -----------------------------------------
        ToolDescriptor {
            name: "cc_apps_list".into(),
            description: "List Ordo apps in a workspace, optionally filtered by status.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "default": "local"},
                    "status": {"type": "string", "enum": ["draft", "published", "archived"]},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500}
                }
            }),
        },
        ToolDescriptor {
            name: "cc_apps_get".into(),
            description: "Fetch one Ordo app by UUID.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {"id": {"type": "string", "format": "uuid"}}
            }),
        },
        ToolDescriptor {
            name: "cc_apps_create".into(),
            description: "Create a new Ordo app. Returns the created app with its UUID.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {"type": "string", "minLength": 1},
                    "description": {"type": "string"},
                    "slug": {"type": "string", "description": "URL-safe slug; derived from name when omitted."},
                    "workspace_id": {"type": "string", "default": "local"},
                    "metadata": {"type": "object", "additionalProperties": true},
                    "actor": {"type": "string", "default": "operator"}
                }
            }),
        },
        ToolDescriptor {
            name: "cc_apps_update".into(),
            description: "Patch an app's name, description, and metadata. Metadata keys set to null are removed.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": {"type": "string", "format": "uuid"},
                    "name": {"type": "string"},
                    "description": {"type": "string"},
                    "metadata_patch": {"type": "object", "additionalProperties": true},
                    "actor": {"type": "string"}
                }
            }),
        },
        ToolDescriptor {
            name: "cc_apps_publish".into(),
            description: "Transition a draft app to published. Review-gated when configured.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": {"type": "string", "format": "uuid"},
                    "actor": {"type": "string"}
                }
            }),
        },
        ToolDescriptor {
            name: "cc_apps_archive".into(),
            description: "Archive an app. Destructive â€” review-gated when configured.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {"id": {"type": "string", "format": "uuid"}}
            }),
        },
        ToolDescriptor {
            name: "cc_apps_events".into(),
            description: "Return an app's append-only event log in sequence order.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {"id": {"type": "string", "format": "uuid"}}
            }),
        },
        // ---- Files --------------------------------------------------
        ToolDescriptor {
            name: "cc_files_list".into(),
            description: "List uploaded files for a workspace or app.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "default": "local"},
                    "app_id": {"type": "string", "format": "uuid"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500}
                }
            }),
        },
        ToolDescriptor {
            name: "cc_files_get_metadata".into(),
            description: "Fetch metadata (without bytes) for one file.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {"id": {"type": "string", "format": "uuid"}}
            }),
        },
        ToolDescriptor {
            name: "cc_files_upload".into(),
            description: "Upload a file. Bytes are carried as base64 in `data_base64`.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["original_name", "data_base64"],
                "properties": {
                    "original_name": {"type": "string"},
                    "content_type": {"type": "string"},
                    "workspace_id": {"type": "string", "default": "local"},
                    "app_id": {"type": "string", "format": "uuid"},
                    "created_by": {"type": "string"},
                    "data_base64": {"type": "string"}
                }
            }),
        },
        ToolDescriptor {
            name: "cc_files_delete".into(),
            description: "Delete a file. Destructive â€” review-gated when configured.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {"id": {"type": "string", "format": "uuid"}}
            }),
        },
        // ---- Assistant ----------------------------------------------
        ToolDescriptor {
            name: "cc_assistant_turn".into(),
            description: "Send a turn to the Ordo Assistant. The Assistant handles memory, RAG, and platform tool use autonomously â€” prefer this over invoking lower-level tools directly.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["user_message"],
                "properties": {
                    "user_message": {"type": "string", "minLength": 1},
                    "session_id": {"type": "string", "format": "uuid", "description": "Continue an existing conversation."},
                    "use_rag": {"type": "boolean", "default": true},
                    "use_memory": {"type": "boolean", "default": true},
                    "use_tools": {"type": "boolean", "default": true},
                    "review": {"type": "boolean", "default": false},
                    "attachments": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                {"type": "object", "required": ["type", "url"], "properties": {"type": {"const": "image_url"}, "url": {"type": "string"}}},
                                {"type": "object", "required": ["type", "data", "media_type"], "properties": {"type": {"const": "image_base64"}, "data": {"type": "string"}, "media_type": {"type": "string"}}}
                            ]
                        }
                    }
                }
            }),
        },
        ToolDescriptor {
            name: "cc_assistant_recall".into(),
            description: "Semantic search over operator facts (brand, preferences, client info, etc.). Returns top-k matches.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {"type": "string"},
                    "top_k": {"type": "integer", "default": 5}
                }
            }),
        },
        // ---- Meta / inventory ---------------------------------------
        ToolDescriptor {
            name: "cc_capabilities_list".into(),
            description: "List every capability advertised by the running Ordo instance. Useful for discovering what plugins and providers are loaded.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDescriptor {
            name: "cc_invoke_tool".into(),
            description: "Escape hatch â€” invoke any Ordo capability by its canonical name (e.g. `brand.extract_voice`, `self_heal.list_cases`). Prefer the specific `cc_*` tools when they exist.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["capability"],
                "properties": {
                    "capability": {"type": "string"},
                    "arguments": {"type": "object"}
                }
            }),
        },
    ]
}

/// Dispatch a `tools/call` to the runtime. All unknown tool names
/// produce a clean MCP error content block rather than a JSON-RPC
/// error â€” that's the convention: tool errors are normal results
/// with `isError: true`.
pub async fn dispatch_call(
    client: &RuntimeClient,
    name: &str,
    arguments: &Value,
) -> ToolsCallResult {
    match do_dispatch(client, name, arguments).await {
        Ok(value) => ToolsCallResult::json_ok(&value),
        Err(err) => ToolsCallResult::text_err(format!("{name} failed: {err}")),
    }
}

async fn do_dispatch(
    client: &RuntimeClient,
    name: &str,
    args: &Value,
) -> Result<Value, RuntimeError> {
    let default_args = Value::Object(Default::default());
    let args = if args.is_null() { &default_args } else { args };

    match name {
        "cc_apps_list" => client.list_apps(args).await,
        "cc_apps_get" => {
            let id = require_string(args, "id")?;
            client.get_app(&id).await
        }
        "cc_apps_create" => client.create_app(args).await,
        "cc_apps_update" => {
            let id = require_string(args, "id")?;
            let mut body = args.clone();
            // HTTP endpoint accepts a patch body without the id
            // (id is in the path), so strip it.
            if let Some(obj) = body.as_object_mut() {
                obj.remove("id");
            }
            client.update_app(&id, &body).await
        }
        "cc_apps_publish" => {
            let id = require_string(args, "id")?;
            let actor = args.get("actor").cloned().unwrap_or(Value::Null);
            client.publish_app(&id, &json!({"actor": actor})).await
        }
        "cc_apps_archive" => {
            let id = require_string(args, "id")?;
            client.archive_app(&id).await
        }
        "cc_apps_events" => {
            let id = require_string(args, "id")?;
            client.list_app_events(&id).await
        }
        "cc_files_list" => client.list_files(args).await,
        "cc_files_get_metadata" => {
            let id = require_string(args, "id")?;
            client.get_file_metadata(&id).await
        }
        "cc_files_upload" => client.upload_file(args).await,
        "cc_files_delete" => {
            let id = require_string(args, "id")?;
            client.delete_file(&id).await
        }
        "cc_assistant_turn" => client.assistant_turn(args).await,
        "cc_assistant_recall" => client.assistant_recall(args).await,
        "cc_capabilities_list" => client.list_capabilities().await,
        "cc_invoke_tool" => {
            let capability = require_string(args, "capability")?;
            let arguments = args
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            client.invoke_tool(&capability, &arguments).await
        }
        other => Err(RuntimeError::Transport(format!("unknown tool: {other}"))),
    }
}

fn require_string(args: &Value, key: &str) -> Result<String, RuntimeError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| RuntimeError::Transport(format!("missing or non-string `{key}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_described_tool_has_a_matching_dispatch_arm() {
        // Guard rail: descriptor and dispatch diverge frequently
        // under feature churn. Keep them in sync.
        let descriptors = describe_tools();
        let described: std::collections::HashSet<&str> =
            descriptors.iter().map(|t| t.name.as_str()).collect();
        let dispatched = [
            "cc_apps_list",
            "cc_apps_get",
            "cc_apps_create",
            "cc_apps_update",
            "cc_apps_publish",
            "cc_apps_archive",
            "cc_apps_events",
            "cc_files_list",
            "cc_files_get_metadata",
            "cc_files_upload",
            "cc_files_delete",
            "cc_assistant_turn",
            "cc_assistant_recall",
            "cc_capabilities_list",
            "cc_invoke_tool",
        ];
        for name in dispatched {
            assert!(
                described.contains(name),
                "dispatched tool `{name}` is missing from describe_tools()"
            );
        }
        assert_eq!(
            described.len(),
            dispatched.len(),
            "describe_tools() has a tool without a dispatch arm"
        );
    }
}
