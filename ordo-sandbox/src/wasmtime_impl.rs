//! Wasmtime-backed sandbox. Compiled only when the `wasmtime` feature
//! is enabled â€” the dep tree is heavy enough to warrant gating.
//!
//! Contract:
//!   - Each invocation gets a fresh `Store` and `Engine` instance â€” no
//!     state leaks between calls.
//!   - Fuel-based instruction cap (`max_instructions`). Running out
//!     is a clean `SandboxError::LimitExceeded`, not a panic.
//!   - Memory cap via the store's memory limiter.
//!   - Wall-clock timeout via `tokio::time::timeout`.
//!   - No WASI imports exposed â€” modules are pure compute. When a
//!     future module genuinely needs syscalls, that becomes a new
//!     explicit capability on `SandboxRequest`.
//!
//! Module ABI (v1):
//!   - Export a function named `request.entry` (default `"ordo_entry"`)
//!     with signature `(i32, i32) -> (i32, i32)`:
//!       input_ptr, input_len â†’ output_ptr, output_len
//!   - Export `alloc: (i32) -> i32` so the host can allocate a buffer
//!     inside the module for the input bytes.
//!   - Export `memory` (linear memory).
//!
//! Modules that don't match the ABI get a clean
//! `SandboxError::InvalidModule`.

use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;
use wasmtime::{Caller, Config, Engine, Linker, Module, Store, StoreLimitsBuilder};

use crate::types::{SandboxError, SandboxExecution, SandboxRequest, SandboxResult};
use crate::Sandbox;

pub struct WasmtimeSandbox {
    engine: Engine,
}

impl WasmtimeSandbox {
    /// Build a fresh sandbox. Cheap â€” the engine caches compiled
    /// modules internally, so constructing one per process is fine.
    pub fn new() -> Result<Self, SandboxError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|err| SandboxError::Internal(err.to_string()))?;
        Ok(Self { engine })
    }
}

#[async_trait]
impl Sandbox for WasmtimeSandbox {
    fn name(&self) -> &'static str {
        "wasmtime"
    }

    async fn execute(&self, request: SandboxRequest) -> SandboxResult {
        let engine = self.engine.clone();
        let duration_cap = std::time::Duration::from_millis(request.limits.max_duration_ms);
        // wasmtime runs on the OS thread; wall-clock timeout via
        // tokio::time::timeout protects against tight compute loops
        // where the fuel check hasn't tripped yet (epoch
        // interruption adds belt + suspenders but requires a ticker
        // thread we don't set up for the MVP).
        let run = tokio::task::spawn_blocking(move || execute_sync(&engine, request));
        match tokio::time::timeout(duration_cap, run).await {
            Ok(Ok(result)) => result,
            Ok(Err(join_err)) => Err(SandboxError::Internal(join_err.to_string())),
            Err(_elapsed) => Err(SandboxError::LimitExceeded(format!(
                "wall-clock timeout after {}ms",
                duration_cap.as_millis()
            ))),
        }
    }
}

fn execute_sync(engine: &Engine, request: SandboxRequest) -> SandboxResult {
    if request.wasm_bytes.is_empty() {
        return Err(SandboxError::InvalidModule(
            "wasm_bytes is empty â€” wasmtime sandbox needs a compiled module".into(),
        ));
    }

    let module = Module::new(engine, &request.wasm_bytes)
        .map_err(|err| SandboxError::InvalidModule(format!("failed to compile module: {err}")))?;

    let limits = StoreLimitsBuilder::new()
        .memory_size(request.limits.max_memory_bytes as usize)
        .instances(1)
        .tables(4)
        .memories(1)
        .build();
    struct StoreState {
        limits: wasmtime::StoreLimits,
        stdout: String,
    }
    let mut store = Store::new(
        engine,
        StoreState {
            limits,
            stdout: String::new(),
        },
    );
    store.limiter(|s: &mut StoreState| &mut s.limits);
    store
        .set_fuel(request.limits.max_instructions)
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    let mut linker: Linker<StoreState> = Linker::new(engine);
    // Minimal host import: `ordo::log(ptr, len)` appends to stdout.
    // Modules that don't need it are fine â€” we only define, we don't
    // require.
    linker
        .func_wrap(
            "ordo",
            "log",
            |mut caller: Caller<'_, StoreState>, ptr: i32, len: i32| -> i32 {
                let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(mem) => mem,
                    None => return -1,
                };
                let data = memory.data(&caller);
                let start = ptr as usize;
                let end = start.saturating_add(len as usize);
                if end > data.len() {
                    return -1;
                }
                let text = String::from_utf8_lossy(&data[start..end]).to_string();
                caller.data_mut().stdout.push_str(&text);
                caller.data_mut().stdout.push('\n');
                0
            },
        )
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|err| SandboxError::InvalidModule(format!("instantiate: {err}")))?;

    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| SandboxError::InvalidModule("module does not export `memory`".into()))?;

    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "alloc")
        .map_err(|err| {
            SandboxError::InvalidModule(format!(
                "module does not export `alloc(i32) -> i32`: {err}"
            ))
        })?;
    let entry = instance
        .get_typed_func::<(i32, i32), (i32, i32)>(&mut store, &request.entry)
        .map_err(|err| {
            SandboxError::InvalidModule(format!(
                "entry `{}` missing or has wrong signature: {err}",
                request.entry
            ))
        })?;

    let input_bytes = serde_json::to_vec(&request.input)
        .map_err(|err| SandboxError::Internal(format!("encode input: {err}")))?;
    let input_len = input_bytes.len() as i32;
    let input_ptr = alloc
        .call(&mut store, input_len)
        .map_err(|err| map_trap("alloc", err))?;
    memory
        .write(&mut store, input_ptr as usize, &input_bytes)
        .map_err(|err| SandboxError::Internal(format!("write input: {err}")))?;

    let started = Instant::now();
    let (out_ptr, out_len) = entry
        .call(&mut store, (input_ptr, input_len))
        .map_err(|err| map_trap(&request.entry, err))?;

    let duration_ms = started.elapsed().as_millis() as u64;

    let mut output_bytes = vec![0u8; out_len as usize];
    memory
        .read(&store, out_ptr as usize, &mut output_bytes)
        .map_err(|err| SandboxError::Internal(format!("read output: {err}")))?;
    let output: Value = serde_json::from_slice(&output_bytes)
        .map_err(|err| SandboxError::Internal(format!("decode output: {err}")))?;

    let remaining = store.get_fuel().unwrap_or(0);
    let consumed = request.limits.max_instructions.saturating_sub(remaining);
    let stdout = std::mem::take(&mut store.data_mut().stdout);

    Ok(SandboxExecution {
        output,
        instructions_used: consumed,
        duration_ms,
        stdout,
    })
}

fn map_trap(context: &str, err: impl std::fmt::Display) -> SandboxError {
    let msg = err.to_string();
    if msg.contains("fuel") {
        SandboxError::LimitExceeded(format!("{context}: {msg}"))
    } else if msg.contains("memory") && msg.contains("limit") {
        SandboxError::LimitExceeded(format!("{context}: {msg}"))
    } else {
        SandboxError::Trap(format!("{context}: {msg}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_wasm_bytes_return_invalid_module() {
        let sandbox = WasmtimeSandbox::new().expect("engine");
        let err = sandbox
            .execute(SandboxRequest {
                wasm_bytes: vec![],
                script: String::new(),
                input: Value::Null,
                entry: "ordo_entry".into(),
                limits: Default::default(),
            })
            .await
            .expect_err("should fail");
        assert!(matches!(err, SandboxError::InvalidModule(_)));
    }

    #[tokio::test]
    async fn malformed_bytes_are_rejected_cleanly() {
        let sandbox = WasmtimeSandbox::new().expect("engine");
        let err = sandbox
            .execute(SandboxRequest {
                wasm_bytes: b"not-a-wasm-module".to_vec(),
                script: String::new(),
                input: Value::Null,
                entry: "ordo_entry".into(),
                limits: Default::default(),
            })
            .await
            .expect_err("should fail");
        assert!(matches!(err, SandboxError::InvalidModule(_)));
    }
}
