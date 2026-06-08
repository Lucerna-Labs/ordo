//! `FilesProvider` â€” exposes `FilesService` as a `CapabilityProvider`.
//!
//! Capabilities:
//!   - `files.list` â€” list file metadata rows for a workspace/app
//!   - `files.get` â€” fetch one metadata row
//!   - `files.download` â€” return bytes base64-encoded in the response
//!     (the HTTP mirror in `ordo-control` exposes the raw bytes
//!     stream; the provider channel is JSON-only per Rule 9, so we
//!     base64)
//!   - `files.upload` â€” accept base64 bytes + metadata, persist
//!   - `files.delete` â€” remove bytes + metadata
//!
//! Upload and delete are destructive; they go through
//! `SecurityStack.gate(provider, "files")` at runtime wiring time so
//! classification and audit happen consistently.

use ordo_protocol::{CapabilityActivation, CapabilityDescriptor, CapabilityTier};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::service::FilesService;
use crate::types::{FilesQuery, NewUpload};

const PROVIDER_NAME: &str = "ordo-files";

pub struct FilesProvider {
    service: FilesService,
}

impl FilesProvider {
    pub fn new(service: FilesService) -> Self {
        Self { service }
    }

    fn describe(cap: &str, description: &str, input_schema: Value) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            cap,
            PROVIDER_NAME,
            description,
            CapabilityTier::Optional,
            CapabilityActivation::Lazy,
        )
        .with_input_schema(input_schema)
    }

    pub fn capabilities_list() -> Vec<&'static str> {
        vec![
            "files.list",
            "files.get",
            "files.download",
            "files.upload",
            "files.delete",
        ]
    }

    pub fn descriptors() -> Vec<CapabilityDescriptor> {
        let id_schema = json!({
            "type": "object",
            "required": ["id"],
            "properties": {"id": {"type": "string", "format": "uuid"}}
        });
        vec![
            Self::describe(
                "files.list",
                "List files in a workspace, optionally scoped to an app.",
                json!({
                    "type": "object",
                    "properties": {
                        "workspace_id": {"type": "string", "default": "local"},
                        "app_id": {"type": "string", "format": "uuid"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 500}
                    }
                }),
            ),
            Self::describe(
                "files.get",
                "Fetch metadata for a single file by UUID.",
                id_schema.clone(),
            ),
            Self::describe(
                "files.download",
                "Return a file's bytes, base64-encoded. Prefer the HTTP mirror (GET /api/files/:id/content) for large assets.",
                id_schema.clone(),
            ),
            Self::describe(
                "files.upload",
                "Upload bytes + metadata. Bytes are base64-encoded over this channel; destructive â€” review-gated.",
                json!({
                    "type": "object",
                    "required": ["original_name", "data_base64"],
                    "properties": {
                        "original_name": {"type": "string", "minLength": 1},
                        "content_type": {"type": "string"},
                        "workspace_id": {"type": "string", "default": "local"},
                        "app_id": {"type": "string", "format": "uuid"},
                        "created_by": {"type": "string", "default": "operator"},
                        "data_base64": {"type": "string", "description": "Base64-encoded bytes."}
                    }
                }),
            ),
            Self::describe(
                "files.delete",
                "Delete a file's bytes and metadata. Destructive â€” review-gated.",
                id_schema,
            ),
        ]
    }

    pub async fn invoke(&self, capability: &str, arguments: &Value) -> Result<Value, String> {
        match capability {
            "files.list" => {
                let query: FilesQuery =
                    serde_json::from_value(arguments.clone()).unwrap_or_default();
                let files = self.service.list(query).map_err(|e| e.to_string())?;
                Ok(json!({ "files": files }))
            }
            "files.get" => {
                let id = parse_uuid(arguments, "id")?;
                let entry = self
                    .service
                    .get_metadata(id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "file not found".to_string())?;
                Ok(json!({ "file": entry }))
            }
            "files.download" => {
                let id = parse_uuid(arguments, "id")?;
                let (entry, bytes) = self.service.download(id).await.map_err(|e| e.to_string())?;
                let data = base64_encode(&bytes);
                Ok(json!({
                    "file": entry,
                    "data_base64": data,
                }))
            }
            "files.upload" => {
                let original_name = arguments
                    .get("original_name")
                    .and_then(|v| v.as_str())
                    .ok_or("missing original_name")?
                    .to_string();
                let data_b64 = arguments
                    .get("data_base64")
                    .and_then(|v| v.as_str())
                    .ok_or("missing data_base64")?;
                let bytes =
                    base64_decode(data_b64).map_err(|e| format!("invalid data_base64: {e}"))?;
                let upload = NewUpload {
                    original_name,
                    content_type: arguments
                        .get("content_type")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    workspace_id: arguments
                        .get("workspace_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    created_by: arguments
                        .get("created_by")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    app_id: arguments
                        .get("app_id")
                        .and_then(|v| v.as_str())
                        .and_then(|s| Uuid::parse_str(s).ok()),
                };
                let entry = self
                    .service
                    .upload(upload, bytes)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "file": entry }))
            }
            "files.delete" => {
                let id = parse_uuid(arguments, "id")?;
                let removed = self.service.delete(id).await.map_err(|e| e.to_string())?;
                Ok(json!({ "deleted": removed.is_some(), "file": removed }))
            }
            other => Err(format!("unknown files capability: {other}")),
        }
    }
}

fn parse_uuid(args: &Value, key: &str) -> Result<Uuid, String> {
    let s = args
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing `{key}`"))?;
    Uuid::parse_str(s).map_err(|e| format!("invalid {key}: {e}"))
}

/// Minimal base64 to avoid pulling in a crate just for this â€” the
/// std-ish implementation is adequate for upload/download bridging.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHA[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHA[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHA[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in input.chars() {
        if c == '=' {
            break;
        }
        if c.is_whitespace() {
            continue;
        }
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - b'A' as u32,
            'a'..='z' => (c as u32) - b'a' as u32 + 26,
            '0'..='9' => (c as u32) - b'0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid character '{c}'")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        let cases: &[&[u8]] = &[b"", b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"];
        for bytes in cases {
            let encoded = base64_encode(bytes);
            let decoded = base64_decode(&encoded).expect("decode");
            assert_eq!(&decoded, bytes, "round trip for {bytes:?}");
        }
    }
}
