//! Config loading.
//!
//! Resolution order (first hit wins):
//!   1. `ORDO_MCP_CONFIG` env var â†’ path to TOML
//!   2. `~/.ordo/mcp.json` (JSON — we avoid pulling a TOML
//!      parser for this single case; JSON is fine for a 3-field config)
//!   3. Env vars: `ORDO_URL`, `ORDO_API_TOKEN`,
//!      `ORDO_WORKSPACE`
//!   4. Defaults: localhost:4141, no token, workspace "local"
//!
//! The binary refuses to start if the runtime URL is unreachable on
//! probe â€” fail-fast beats "silently failing tools" for Claude Desktop
//! users.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Base URL of the Ordo control API
    /// (e.g. `http://127.0.0.1:4141`). No trailing slash.
    #[serde(default = "default_runtime_url")]
    pub runtime_url: String,

    /// Optional bearer token. When set, every request to the runtime
    /// includes `Authorization: Bearer <token>`. Phase 2.5 wires the
    /// server side of this.
    #[serde(default)]
    pub api_token: Option<String>,

    /// Default workspace_id to inject into tool calls when the client
    /// doesn't specify one.
    #[serde(default = "default_workspace_id")]
    pub workspace_id: String,

    /// Request timeout in seconds. Defaults to 120 since some
    /// assistant turns wait on LLM completions.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            runtime_url: default_runtime_url(),
            api_token: None,
            workspace_id: default_workspace_id(),
            timeout_secs: default_timeout_secs(),
        }
    }
}

fn default_runtime_url() -> String {
    "http://127.0.0.1:4141".to_string()
}

fn default_workspace_id() -> String {
    "local".to_string()
}

fn default_timeout_secs() -> u64 {
    120
}

impl Config {
    pub fn load() -> Self {
        if let Some(path) = std::env::var_os("ORDO_MCP_CONFIG") {
            if let Some(config) = Self::load_file(PathBuf::from(path)) {
                return config.merge_env();
            }
        }
        if let Some(home) = home_dir() {
            let default_path = home.join(".ordo").join("mcp.json");
            if default_path.exists() {
                if let Some(config) = Self::load_file(default_path) {
                    return config.merge_env();
                }
            }
        }
        Self::default().merge_env()
    }

    fn load_file(path: PathBuf) -> Option<Self> {
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(config) => Some(config),
            Err(err) => {
                tracing::warn!(
                    target: "ordo_mcp",
                    path = %path.display(),
                    error = %err,
                    "config file failed to parse, falling back to env + defaults"
                );
                None
            }
        }
    }

    /// Env vars override file values so operators can flip tokens
    /// without editing the config file.
    fn merge_env(mut self) -> Self {
        if let Ok(url) = std::env::var("ORDO_URL") {
            self.runtime_url = url;
        }
        if let Ok(token) = std::env::var("ORDO_API_TOKEN") {
            if !token.is_empty() {
                self.api_token = Some(token);
            }
        }
        if let Ok(ws) = std::env::var("ORDO_WORKSPACE") {
            if !ws.is_empty() {
                self.workspace_id = ws;
            }
        }
        if let Ok(timeout) = std::env::var("ORDO_TIMEOUT_SECS") {
            if let Ok(v) = timeout.parse::<u64>() {
                self.timeout_secs = v;
            }
        }
        // Strip any trailing slash on the URL â€” the client joins paths
        // that start with `/`, so double slashes break hosts that are
        // pedantic about routing.
        while self.runtime_url.ends_with('/') {
            self.runtime_url.pop();
        }
        self
    }
}

fn home_dir() -> Option<PathBuf> {
    // Minimal home-dir lookup â€” avoid pulling `dirs` for a single
    // non-critical path.
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_env_strips_trailing_slashes() {
        let original_url = std::env::var("ORDO_URL").ok();
        std::env::set_var("ORDO_URL", "http://host:9/////");
        let config = Config::default().merge_env();
        assert_eq!(config.runtime_url, "http://host:9");
        match original_url {
            Some(v) => std::env::set_var("ORDO_URL", v),
            None => std::env::remove_var("ORDO_URL"),
        }
    }

    #[test]
    fn defaults_populate_when_no_config() {
        let config = Config::default();
        assert!(config.runtime_url.starts_with("http://"));
        assert_eq!(config.workspace_id, "local");
        assert_eq!(config.timeout_secs, 120);
        assert!(config.api_token.is_none());
    }
}
