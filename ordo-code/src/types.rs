//! Configuration + policy for the code-execution capability.

/// Runtime policy for the code runners. Set at wiring time in
/// `ordo-runtime` from `RuntimeConfig`.
#[derive(Debug, Clone)]
pub struct CodePolicy {
    /// Allowlist of languages permitted on the native runner. Empty =
    /// all supported languages (rust / python / node / shell).
    pub enabled_languages: Vec<String>,
    /// Default wall-clock cap per run, in milliseconds. A request may
    /// lower it via `timeout_ms` / `max_duration_ms`.
    pub default_timeout_ms: u64,
    /// Master switch for the native subprocess runner. When false,
    /// `code.run_native` refuses with an actionable error regardless of
    /// which backend was compiled in.
    pub allow_native: bool,
}

impl Default for CodePolicy {
    fn default() -> Self {
        Self {
            enabled_languages: Vec::new(),
            default_timeout_ms: 30_000,
            allow_native: false,
        }
    }
}
