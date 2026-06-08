//! Sandboxed execution for Ordo-generated code (Phase 3.2).
//!
//! The surface is a small `Sandbox` trait with two impls:
//!
//!   - `NullSandbox` (always compiled) â€” returns a clear
//!     `SandboxError::Unavailable` on every call. Lets the rest of
//!     the platform wire a sandbox slot without bringing in wasmtime
//!     on every build.
//!
//!   - `WasmtimeSandbox` (behind the `wasmtime` cargo feature) â€”
//!     real execution with fuel-limited compute, bounded memory,
//!     and JSON-in / JSON-out I/O. No filesystem, no network, no
//!     clock by default.
//!
//! The trait is the contract â€” future impls (Firecracker, Deno
//! isolates, a subprocess with `unshare`, etc.) can slot in without
//! changing callers.

pub mod types;

#[cfg(feature = "wasmtime")]
pub mod wasmtime_impl;

#[cfg(feature = "wasmtime")]
pub use wasmtime_impl::WasmtimeSandbox;

#[cfg(feature = "subprocess")]
pub mod subprocess_impl;

#[cfg(feature = "subprocess")]
pub use subprocess_impl::{SubprocessConfig, SubprocessSandbox};

use async_trait::async_trait;

pub use types::{
    NullSandbox, SandboxError, SandboxExecution, SandboxLimits, SandboxRequest, SandboxResult,
};

/// Executes sandboxed code. One call = one execution.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Human-readable backend name (`"null"`, `"wasmtime"`, etc.) â€”
    /// surfaced in logs and in the capability descriptor so the
    /// operator can see which implementation is active.
    fn name(&self) -> &'static str;

    /// Execute `request`. Returns on completion, fuel exhaustion,
    /// memory limit, or timeout.
    async fn execute(&self, request: SandboxRequest) -> SandboxResult;
}
