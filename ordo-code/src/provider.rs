//! `CodeProvider` — exposes `CodeService` as a set of capabilities.
//!
//! Capabilities:
//!   - `workspace.write_file` / `workspace.read_file` / `workspace.list`
//!     — read/write files in the confined code workspace
//!   - `code.run` — run a compiled WASM module in the in-process
//!     sandbox (pure compute, no fs/network)
//!   - `code.run_native` — run a native command (cargo/rustc/python/
//!     node/shell) in the workspace; higher privilege, opt-in + gated.
//!
//! Headless on purpose (no `ordo-mcp-host` dep): the
//! `CodeCapabilityAdapter` in `ordo-mcp-host` bridges this into the
//! `CapabilityProvider` surface, mirroring `ordo-files`.

use ordo_protocol::{CapabilityActivation, CapabilityDescriptor, CapabilityTier};
use serde_json::{json, Value};

use crate::service::CodeService;

const PROVIDER_NAME: &str = "ordo-code";

pub struct CodeProvider {
    service: CodeService,
}

impl CodeProvider {
    pub fn new(service: CodeService) -> Self {
        Self { service }
    }

    fn describe(
        cap: &str,
        description: &str,
        tier: CapabilityTier,
        activation: CapabilityActivation,
        input_schema: Value,
    ) -> CapabilityDescriptor {
        CapabilityDescriptor::new(cap, PROVIDER_NAME, description, tier, activation)
            .with_input_schema(input_schema)
    }

    pub fn capabilities_list() -> Vec<&'static str> {
        vec![
            "workspace.write_file",
            "workspace.read_file",
            "workspace.list",
            "code.run",
            "code.run_native",
        ]
    }

    pub fn descriptors() -> Vec<CapabilityDescriptor> {
        vec![
            Self::describe(
                "workspace.write_file",
                "Write a UTF-8 text file into the sandbox code workspace (a confined directory). Creates parent directories. Use this to author code before running it.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
                json!({
                    "type": "object",
                    "required": ["path", "content"],
                    "properties": {
                        "path": {"type": "string", "description": "path relative to the workspace root"},
                        "content": {"type": "string"}
                    }
                }),
            ),
            Self::describe(
                "workspace.read_file",
                "Read a UTF-8 text file from the sandbox code workspace.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
                json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {"path": {"type": "string"}}
                }),
            ),
            Self::describe(
                "workspace.list",
                "List entries in the workspace, or a subdirectory of it.",
                CapabilityTier::Optional,
                CapabilityActivation::Lazy,
                json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "subdirectory relative to the workspace root; omit for the root"}
                    }
                }),
            ),
            Self::describe(
                "code.run",
                "Run a compiled WebAssembly module in the in-process sandbox (fuel + memory + wall-clock limited; NO filesystem or network). For pure-compute WASM only — to run cargo/python/node use code.run_native.",
                CapabilityTier::Optional,
                CapabilityActivation::Lazy,
                json!({
                    "type": "object",
                    "properties": {
                        "wasm_base64": {"type": "string", "description": "base64-encoded compiled WASM module"},
                        "input": {"type": "object", "description": "JSON input passed to the module"},
                        "entry": {"type": "string", "default": "ordo_entry"},
                        "max_duration_ms": {"type": "integer"},
                        "max_instructions": {"type": "integer"},
                        "max_memory_bytes": {"type": "integer"}
                    }
                }),
            ),
            Self::describe(
                "code.run_native",
                "Run a native command (cargo/rustc/python/node/shell) in the confined workspace. Network is allowed (deps can be fetched); higher privilege, opt-in + gated. Provide `language` (+ `source` for a quick snippet, or `args`), or an explicit `program` + `args`. Combine with workspace.write_file to author a project first.",
                CapabilityTier::Heavy,
                CapabilityActivation::Lazy,
                json!({
                    "type": "object",
                    "properties": {
                        "language": {"type": "string", "enum": ["rust", "python", "node", "shell"]},
                        "program": {"type": "string", "description": "explicit program (cargo/rustc/python/node/pwsh/cmd); overrides `language`"},
                        "args": {"type": "array", "items": {"type": "string"}},
                        "source": {"type": "string", "description": "inline snippet (python/node/shell); written into the workspace and executed"},
                        "cwd": {"type": "string", "description": "subdirectory (relative to workspace) to run in"},
                        "stdin": {"type": "string"},
                        "timeout_ms": {"type": "integer"}
                    }
                }),
            ),
        ]
    }

    pub async fn invoke(&self, capability: &str, arguments: &Value) -> Result<Value, String> {
        match capability {
            "workspace.write_file" => self.service.write_file(arguments).await,
            "workspace.read_file" => self.service.read_file(arguments).await,
            "workspace.list" => self.service.list(arguments).await,
            "code.run" => self.service.run_wasm(arguments).await,
            "code.run_native" => self.service.run_native(arguments).await,
            other => Err(format!("unknown code capability: {other}")),
        }
    }
}
