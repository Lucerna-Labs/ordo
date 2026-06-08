//! Per-type connection testers. Each module exposes an async
//! `test(...)` function returning `Result<String, String>` where
//! `Ok(detail)` is a green-check message and `Err(detail)` is the
//! actual failure surfaced to the operator.
//!
//! Testers run in-process Rust against the live destination's API.
//! They never go through the WASM sandbox â€” the test path is
//! short-lived, the credential is consumed once and dropped, and
//! we want fast operator feedback (sub-second). MCP packages
//! handle the durable on-behalf-of work later.

pub mod anthropic;
pub mod email;
pub mod generic_webhook;
pub mod openai;
pub mod ssh;

use std::time::Duration;

/// Shared HTTP client with a sensible timeout. Created once per
/// test invocation to avoid leaking connection pools across calls.
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("ordo-connections/0.1")
        .build()
        .expect("reqwest client")
}
