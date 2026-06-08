//! Ordo MCP bridge â€” a standalone binary that speaks the
//! Model Context Protocol (JSON-RPC 2.0 over stdio) and translates
//! every call into HTTP requests against a running Ordo
//! instance.
//!
//! Architecturally a thin translator. The runtime is the source of
//! truth for every piece of state this crate exposes; we never
//! cache, never reinvent, never add our own authorization or review
//! gating (that lives inside the runtime). The rule: if behavior
//! diverges from `cargo run --bin ordo-cli`, that's a bug in
//! this crate, not a feature.
//!
//! See also:
//!   - `docs/architecture-contract.md` (Rule 2: this is an external
//!     capability consumer, not a new extension point)
//!   - MCP spec: https://modelcontextprotocol.io

pub mod config;
pub mod mcp;
pub mod rpc;
pub mod runtime;
pub mod server;
pub mod tools;
pub mod transport;

pub use config::Config;
pub use runtime::{RuntimeClient, RuntimeError};
pub use server::Server;
