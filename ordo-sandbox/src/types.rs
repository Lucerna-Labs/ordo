use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Sandbox;

/// Resource limits applied to a single sandbox invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxLimits {
    /// Approximate compute cap â€” translates to wasmtime fuel units.
    /// Higher = more work permitted. The mapping is rough on purpose:
    /// fuel per instruction isn't constant.
    pub max_instructions: u64,
    /// Memory cap in bytes. Sandboxes that can't honor this reject
    /// the request.
    pub max_memory_bytes: u64,
    /// Wall-clock timeout in milliseconds. Kills in-flight
    /// invocations that exceed it regardless of instruction count.
    pub max_duration_ms: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            max_instructions: 100_000_000,
            max_memory_bytes: 64 * 1024 * 1024, // 64 MiB
            max_duration_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRequest {
    /// WebAssembly module bytes. Implementations that don't execute
    /// WASM ignore this (they use `script` instead).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wasm_bytes: Vec<u8>,
    /// Optional non-WASM script body (e.g. JavaScript when a future
    /// isolate-backed impl lands). Kept on the request so callers
    /// can submit either, and the sandbox picks based on what it
    /// understands.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub script: String,
    /// JSON input passed as the module's argument.
    #[serde(default)]
    pub input: Value,
    /// Entry point function name for WASM modules. Defaults to
    /// `"ordo_entry"` â€” modules can expose that function, take a
    /// JSON-encoded input pointer, return a JSON-encoded output.
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub limits: SandboxLimits,
}

fn default_entry() -> String {
    "ordo_entry".to_string()
}

#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
pub enum SandboxError {
    #[error("sandbox unavailable: {0}")]
    Unavailable(String),
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("invalid module: {0}")]
    InvalidModule(String),
    #[error("execution trap: {0}")]
    Trap(String),
    #[error("internal: {0}")]
    Internal(String),
}

pub type SandboxResult = Result<SandboxExecution, SandboxError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxExecution {
    /// JSON output returned by the module.
    pub output: Value,
    /// Instructions consumed (approximate â€” fuel units spent).
    pub instructions_used: u64,
    /// Wall-clock duration of the execution in milliseconds.
    pub duration_ms: u64,
    /// Optional stdout/logs captured from the module (WASI impls).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
}

/// No-op sandbox. Always compiled; returns `Unavailable` on every
/// call. The runtime wires this when the `wasmtime` feature is off
/// so the rest of the platform can still advertise the sandbox lane
/// and fail cleanly instead of panicking.
pub struct NullSandbox;

#[async_trait]
impl Sandbox for NullSandbox {
    fn name(&self) -> &'static str {
        "null"
    }

    async fn execute(&self, _request: SandboxRequest) -> SandboxResult {
        Err(SandboxError::Unavailable(
            "this build has no sandbox backend; rebuild ordo-sandbox with --features wasmtime"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn null_sandbox_reports_unavailable_with_actionable_message() {
        let sandbox = NullSandbox;
        let err = sandbox
            .execute(SandboxRequest {
                wasm_bytes: vec![],
                script: String::new(),
                input: serde_json::Value::Null,
                entry: "ordo_entry".into(),
                limits: SandboxLimits::default(),
            })
            .await
            .expect_err("should fail");
        match err {
            SandboxError::Unavailable(msg) => {
                assert!(msg.contains("--features wasmtime"), "got: {msg}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn default_limits_are_sane() {
        let l = SandboxLimits::default();
        assert!(l.max_instructions >= 1_000_000);
        assert!(l.max_memory_bytes >= 1 << 20);
        assert!(l.max_duration_ms >= 100);
    }
}
