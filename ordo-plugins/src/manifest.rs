//! Plugin manifest â€” the `plugin.json` that describes how to spawn a
//! plugin and what it expects to provide.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Lane prefixes reserved for the core runtime. Plugins that advertise
/// tools matching these prefixes are rejected unless the manifest
/// explicitly sets `core_override: true` (an escape hatch for future
/// trusted first-party extensions).
pub const RESERVED_CORE_LANES: &[&str] = &[
    "cloud.",
    "runtime.",
    "filesystem.",
    "self_heal.",
    "memory.",
    "knowledge.",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginManifest {
    /// Unique plugin identifier. Lowercase alphanumerics + hyphens.
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Executable to spawn. Resolved relative to the plugin directory
    /// when it is not an absolute path.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Lane prefixes the plugin is allowed to contribute to
    /// (e.g. `["seo.", "workflow.", "cms.", "example."]`). Must not include
    /// any reserved prefixes without `core_override`.
    #[serde(default)]
    pub expected_lanes: Vec<String>,
    /// Environment variables to forward from the parent process. All
    /// other env vars are scrubbed.
    #[serde(default)]
    pub required_env: Vec<String>,
    /// Extra key/value env vars set on the child (e.g. for API base
    /// URLs the plugin needs). Never used for secrets; use the cloud
    /// credential vault for those.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Bypass the reserved-lane guard. Only meaningful for first-party
    /// plugins bundled with Ordo itself.
    #[serde(default)]
    pub core_override: bool,
    /// Whether the plugin is enabled. Operators flip this via the CLI
    /// or dashboard. Disabled plugins are parsed but never spawned.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_version() -> String {
    "0.0.0".to_string()
}

fn default_enabled() -> bool {
    true
}

impl PluginManifest {
    /// Returns `Err(_)` if the manifest advertises a reserved lane
    /// without the override flag.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.name.trim().is_empty() {
            return Err(ManifestError::MissingName);
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ManifestError::InvalidName(self.name.clone()));
        }
        if self.command.trim().is_empty() {
            return Err(ManifestError::MissingCommand);
        }
        if !self.core_override {
            for lane in &self.expected_lanes {
                if RESERVED_CORE_LANES
                    .iter()
                    .any(|prefix| lane.starts_with(prefix))
                {
                    return Err(ManifestError::ReservedLane(lane.clone()));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest has no `name`")]
    MissingName,
    #[error("invalid plugin name '{0}' (use lowercase letters, digits, hyphens)")]
    InvalidName(String),
    #[error("manifest has no `command`")]
    MissingCommand,
    #[error("manifest lane '{0}' is reserved for the core runtime")]
    ReservedLane(String),
    #[error("manifest IO: {0}")]
    Io(String),
    #[error("manifest parse: {0}")]
    Parse(String),
}

/// A manifest loaded from disk along with the directory it lives in â€”
/// needed to resolve relative `command` paths at spawn time.
#[derive(Debug, Clone)]
pub struct LoadedManifest {
    pub manifest: PluginManifest,
    pub directory: PathBuf,
    pub manifest_path: PathBuf,
}

impl LoadedManifest {
    pub fn from_path(manifest_path: &Path) -> Result<Self, ManifestError> {
        let raw = std::fs::read_to_string(manifest_path)
            .map_err(|err| ManifestError::Io(err.to_string()))?;
        let manifest: PluginManifest =
            serde_json::from_str(&raw).map_err(|err| ManifestError::Parse(err.to_string()))?;
        manifest.validate()?;
        let directory = manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Ok(Self {
            manifest,
            directory,
            manifest_path: manifest_path.to_path_buf(),
        })
    }

    /// Absolute path to the binary the plugin should spawn.
    pub fn resolved_command(&self) -> PathBuf {
        let raw = Path::new(&self.manifest.command);
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.directory.join(raw)
        }
    }
}

/// Scan a directory for `plugin.json` manifests (one per sub-directory).
/// Returns the successfully parsed manifests; manifests that fail to
/// load are reported in `errors` so the operator can see them in the UI
/// without blocking the rest of the runtime.
pub fn discover_plugins(root: &Path) -> DiscoveryReport {
    let mut loaded = Vec::new();
    let mut errors = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => {
            return DiscoveryReport { loaded, errors };
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("plugin.json");
        if !manifest_path.exists() {
            continue;
        }
        match LoadedManifest::from_path(&manifest_path) {
            Ok(manifest) => loaded.push(manifest),
            Err(err) => errors.push(DiscoveryError {
                path: manifest_path,
                error: err.to_string(),
            }),
        }
    }
    DiscoveryReport { loaded, errors }
}

#[derive(Debug)]
pub struct DiscoveryReport {
    pub loaded: Vec<LoadedManifest>,
    pub errors: Vec<DiscoveryError>,
}

#[derive(Debug, Clone)]
pub struct DiscoveryError {
    pub path: PathBuf,
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_lanes_rejected_without_override() {
        let m = PluginManifest {
            name: "evil".into(),
            version: "0.1.0".into(),
            description: String::new(),
            command: "evil.exe".into(),
            args: vec![],
            expected_lanes: vec!["cloud.openai.exfiltrate".into()],
            required_env: vec![],
            env: HashMap::new(),
            core_override: false,
            enabled: true,
        };
        assert!(matches!(m.validate(), Err(ManifestError::ReservedLane(_))));
    }

    #[test]
    fn reserved_lanes_allowed_with_override() {
        let m = PluginManifest {
            name: "first-party".into(),
            version: "0.1.0".into(),
            description: String::new(),
            command: "plugin.exe".into(),
            args: vec![],
            expected_lanes: vec!["cloud.openai.special".into()],
            required_env: vec![],
            env: HashMap::new(),
            core_override: true,
            enabled: true,
        };
        assert!(m.validate().is_ok());
    }

    #[test]
    fn invalid_names_rejected() {
        let m = PluginManifest {
            name: "Bad Name!".into(),
            version: "0.1.0".into(),
            description: String::new(),
            command: "x".into(),
            args: vec![],
            expected_lanes: vec![],
            required_env: vec![],
            env: HashMap::new(),
            core_override: false,
            enabled: true,
        };
        assert!(matches!(m.validate(), Err(ManifestError::InvalidName(_))));
    }
}
