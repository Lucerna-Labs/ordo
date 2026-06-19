//! Frontend extensions for the Ordo studio.
//!
//! A UI extension is a sandboxed iframe surface â€” a new tab, panel, or
//! overlay in the studio â€” plus the set of capabilities its JavaScript
//! is allowed to call. The extension lives in
//! `user-files/ui-extensions/<name>/` with a `ui.json` manifest and
//! static HTML/JS/CSS bundle. The studio loads each declared surface
//! as an iframe with `sandbox="allow-scripts"` (no same-origin) and
//! mediates *every* runtime call through a postMessage bridge.
//!
//! The whole point of this crate is a trust boundary: extension code
//! is foreign HTML/JS, so it never gets direct access to the bus.
//! Permissions declared in `ui.json` are enforced in the parent
//! studio before any request leaves for the control API.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Lane prefixes that are reserved for the core runtime. Matching
/// declarations in the manifest's `permissions.mcp_tools` require the
/// `core_override` escape hatch (mirrors ordo-plugins).
pub const RESERVED_CORE_LANES: &[&str] = &[
    "cloud.",
    "runtime.",
    "filesystem.",
    "self_heal.",
    "memory.",
    "knowledge.",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UiExtensionManifest {
    /// Unique extension identifier. Lowercase alphanumerics + hyphens.
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// Declared surfaces the extension contributes. Most extensions
    /// declare exactly one tab.
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    /// Capability permission list. Supports `lane.*` wildcards.
    #[serde(default)]
    pub permissions: Permissions,
    /// Allow the manifest to name reserved core lanes
    /// (`cloud.*`, `runtime.*`, etc.). Only meaningful for first-party
    /// extensions.
    #[serde(default)]
    pub core_override: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_version() -> String {
    "0.0.0".into()
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Surface {
    /// A new top-level tab in the studio nav.
    Tab(TabSurface),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TabSurface {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// HTML file within the extension directory to load as the iframe
    /// entry. Resolved as a relative path; escapes are rejected.
    pub entry: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Permissions {
    /// Capability ids the extension may call. Supports `lane.*`
    /// wildcards. An empty list means "read-only extension: no tool
    /// calls allowed."
    #[serde(default)]
    pub mcp_tools: Vec<String>,
    /// Topics the extension may subscribe to via the bridge. Today
    /// the only recognised topic prefix is `review.*` â€” others are
    /// forwarded only when the bridge grows explicit support.
    #[serde(default)]
    pub subscribe_events: Vec<String>,
}

impl UiExtensionManifest {
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
        if self.surfaces.is_empty() {
            return Err(ManifestError::NoSurfaces);
        }
        for surface in &self.surfaces {
            let Surface::Tab(tab) = surface;
            if tab.id.trim().is_empty() {
                return Err(ManifestError::InvalidSurface("missing id".into()));
            }
            if tab.entry.trim().is_empty() {
                return Err(ManifestError::InvalidSurface("missing entry".into()));
            }
            // Reject absolute and escaping entries.
            let entry_path = Path::new(&tab.entry);
            if entry_path.is_absolute() {
                return Err(ManifestError::InvalidSurface(format!(
                    "entry '{}' must be relative",
                    tab.entry
                )));
            }
            for component in entry_path.components() {
                if matches!(component, Component::ParentDir | Component::Prefix(_)) {
                    return Err(ManifestError::InvalidSurface(format!(
                        "entry '{}' escapes the extension directory",
                        tab.entry
                    )));
                }
            }
        }
        if !self.core_override {
            for pattern in &self.permissions.mcp_tools {
                if RESERVED_CORE_LANES
                    .iter()
                    .any(|prefix| pattern.starts_with(prefix))
                {
                    return Err(ManifestError::ReservedLane(pattern.clone()));
                }
            }
        }
        Ok(())
    }

    /// Returns `true` when `capability` is covered by one of the
    /// manifest's declared permissions.
    pub fn permits_tool(&self, capability: &str) -> bool {
        self.permissions
            .mcp_tools
            .iter()
            .any(|pattern| glob_matches(pattern, capability))
    }

    /// Returns `true` when `topic` matches a declared subscription
    /// pattern.
    pub fn permits_event(&self, topic: &str) -> bool {
        self.permissions
            .subscribe_events
            .iter()
            .any(|pattern| glob_matches(pattern, topic))
    }
}

/// `lane.*` / `lane.specific` matcher. Only supports a trailing `*`
/// wildcard (no segment wildcards, no regex). Everything else is a
/// literal comparison.
pub fn glob_matches(pattern: &str, input: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        input.starts_with(prefix)
    } else {
        pattern == input
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest has no `name`")]
    MissingName,
    #[error("invalid extension name '{0}' (use lowercase letters, digits, hyphens)")]
    InvalidName(String),
    #[error("manifest must declare at least one surface")]
    NoSurfaces,
    #[error("invalid surface: {0}")]
    InvalidSurface(String),
    #[error("permission '{0}' references a reserved core lane")]
    ReservedLane(String),
    #[error("manifest IO: {0}")]
    Io(String),
    #[error("manifest parse: {0}")]
    Parse(String),
}

#[derive(Debug, Clone)]
pub struct LoadedUiExtension {
    pub manifest: UiExtensionManifest,
    pub directory: PathBuf,
    pub manifest_path: PathBuf,
}

impl LoadedUiExtension {
    pub fn from_path(manifest_path: &Path) -> Result<Self, ManifestError> {
        let raw = std::fs::read_to_string(manifest_path)
            .map_err(|err| ManifestError::Io(err.to_string()))?;
        let manifest: UiExtensionManifest =
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

    /// Resolve a request path (the `*path` portion of
    /// `/api/ui-extensions/:name/files/*path`) against this
    /// extension's directory, rejecting traversal.
    pub fn resolve_static(&self, request_path: &str) -> Result<PathBuf, ManifestError> {
        let rel = Path::new(request_path);
        if rel.is_absolute() {
            return Err(ManifestError::InvalidSurface(format!(
                "static path '{request_path}' must be relative"
            )));
        }
        let mut resolved = self.directory.clone();
        for component in rel.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    return Err(ManifestError::InvalidSurface(format!(
                        "static path '{request_path}' must be relative"
                    )));
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(ManifestError::InvalidSurface(format!(
                        "static path '{request_path}' escapes the extension directory"
                    )));
                }
                Component::Normal(segment) => resolved.push(segment),
            }
        }
        if !resolved.starts_with(&self.directory) {
            return Err(ManifestError::InvalidSurface(format!(
                "static path '{request_path}' escapes the extension directory"
            )));
        }
        Ok(resolved)
    }
}

#[derive(Debug)]
pub struct DiscoveryReport {
    pub loaded: Vec<LoadedUiExtension>,
    pub errors: Vec<DiscoveryError>,
}

#[derive(Debug, Clone)]
pub struct DiscoveryError {
    pub path: PathBuf,
    pub error: String,
}

pub fn discover_ui_extensions(root: &Path) -> DiscoveryReport {
    let mut loaded = Vec::new();
    let mut errors = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return DiscoveryReport { loaded, errors },
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("ui.json");
        if !manifest_path.exists() {
            continue;
        }
        match LoadedUiExtension::from_path(&manifest_path) {
            Ok(extension) => loaded.push(extension),
            Err(err) => errors.push(DiscoveryError {
                path: manifest_path,
                error: err.to_string(),
            }),
        }
    }
    DiscoveryReport { loaded, errors }
}

/// Determine a reasonable `Content-Type` for a served static file
/// based on extension. Deliberately minimal â€” we only support the
/// handful of types a UI extension realistically uses.
pub fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        Some("md") => "text/markdown; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> UiExtensionManifest {
        UiExtensionManifest {
            name: "word".into(),
            version: "0.1.0".into(),
            description: "".into(),
            author: "".into(),
            surfaces: vec![Surface::Tab(TabSurface {
                id: "word".into(),
                label: "Word".into(),
                icon: None,
                entry: "index.html".into(),
                description: None,
            })],
            permissions: Permissions {
                mcp_tools: vec!["automation.*".into(), "filesystem.write_file".into()],
                subscribe_events: vec![],
            },
            core_override: true, // permits filesystem.write_file
            enabled: true,
        }
    }

    #[test]
    fn reserved_lane_rejected_without_override() {
        let mut manifest = sample_manifest();
        manifest.core_override = false;
        assert!(matches!(
            manifest.validate(),
            Err(ManifestError::ReservedLane(_))
        ));
    }

    #[test]
    fn permits_tool_glob_and_literal() {
        let manifest = sample_manifest();
        assert!(manifest.permits_tool("automation.list"));
        assert!(manifest.permits_tool("automation.inspect"));
        assert!(manifest.permits_tool("filesystem.write_file"));
        assert!(!manifest.permits_tool("filesystem.read_file"));
        assert!(!manifest.permits_tool("cloud.openai.chat"));
    }

    #[test]
    fn entry_with_parent_traversal_rejected() {
        let mut manifest = sample_manifest();
        let Surface::Tab(tab) = &mut manifest.surfaces[0];
        tab.entry = "../outside.html".into();
        assert!(matches!(
            manifest.validate(),
            Err(ManifestError::InvalidSurface(_))
        ));
    }

    #[test]
    fn resolve_static_rejects_parent_traversal() {
        let extension = LoadedUiExtension {
            manifest: sample_manifest(),
            directory: PathBuf::from("/tmp/ext"),
            manifest_path: PathBuf::from("/tmp/ext/ui.json"),
        };
        assert!(extension.resolve_static("../etc/passwd").is_err());
        assert!(extension.resolve_static("/absolute").is_err());
        assert!(extension.resolve_static("nested/path.js").is_ok());
    }

    #[test]
    fn content_types_cover_expected_files() {
        assert_eq!(
            content_type_for(Path::new("x.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type_for(Path::new("x.mjs")),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for(Path::new("x.svg")), "image/svg+xml");
        assert_eq!(
            content_type_for(Path::new("x.bin")),
            "application/octet-stream"
        );
    }
}
