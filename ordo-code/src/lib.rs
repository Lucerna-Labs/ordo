//! Ordo code-execution primitive.
//!
//! Exposes `workspace.*` (write/read/list files in a confined
//! workspace) and `code.*` (run code) capabilities. Two `Sandbox`
//! backends back the runners: the low-risk WASM runner (`code.run`) and
//! the higher-privilege native subprocess runner (`code.run_native`,
//! opt-in + gated).
//!
//! Modeled on `ordo-files`: a headless service + provider live here; the
//! `CapabilityProvider` bridge (`CodeCapabilityAdapter`) lives in
//! `ordo-mcp-host` so leaf crates don't depend on it (keeps the
//! dependency graph shallow).

pub mod provider;
pub mod service;
pub mod types;

pub use provider::CodeProvider;
pub use service::CodeService;
pub use types::CodePolicy;
