//! Sandbox policy â€” single-purpose allowlist checks mapped from
//! the installed `CapabilityDeclaration`.
//!
//! The policy is the bright line between "a sandboxed module
//! called a host function" and "the host function did the work".
//! Every check returns a boolean; the caller decides the response
//! shape (deny + emit event for HTTP, deny + truncate for
//! filesystem, etc.).

use ordo_protocol::CapabilityDeclaration;

#[derive(Debug, Clone, Default)]
pub struct SandboxPolicy {
    pub host_functions: Vec<String>,
    pub domains: Vec<String>,
    pub filesystem_paths: Vec<String>,
    pub bus_topics: Vec<String>,
}

impl SandboxPolicy {
    pub fn from_declaration(decl: &CapabilityDeclaration) -> Self {
        Self {
            host_functions: decl.host_functions.clone(),
            domains: decl.domains.clone(),
            filesystem_paths: decl.filesystem_paths.clone(),
            bus_topics: decl.bus_topics.clone(),
        }
    }

    pub fn function_allowed(&self, function: &str) -> bool {
        self.host_functions.iter().any(|h| h == function)
    }

    pub fn domain_allowed(&self, domain: &str) -> bool {
        self.domains.iter().any(|d| {
            // Exact match or suffix match (e.g. declare
            // "allowed.test" to cover "api.allowed.test").
            d == domain || domain.ends_with(&format!(".{d}"))
        })
    }

    pub fn filesystem_read_allowed(&self, path: &str) -> bool {
        self.filesystem_paths
            .iter()
            .any(|p| path == p || path.starts_with(&format!("{p}/")))
    }

    pub fn topic_allowed(&self, topic: &str) -> bool {
        self.bus_topics.iter().any(|t| {
            if let Some(prefix) = t.strip_suffix(".*") {
                topic.starts_with(prefix)
            } else {
                t == topic
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyViolation {
    UnknownHostFunction(String),
    EgressBlocked { domain: String },
    FilesystemBlocked { path: String },
    TopicBlocked { topic: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_exact_and_suffix_match() {
        let policy = SandboxPolicy::from_declaration(&CapabilityDeclaration {
            domains: vec!["allowed.test".into()],
            ..Default::default()
        });
        assert!(policy.domain_allowed("allowed.test"));
        assert!(policy.domain_allowed("api.allowed.test"));
        assert!(!policy.domain_allowed("allowed.test.attacker.example"));
        assert!(!policy.domain_allowed("blocked.example"));
    }

    #[test]
    fn filesystem_read_accepts_declared_prefix() {
        let policy = SandboxPolicy::from_declaration(&CapabilityDeclaration {
            filesystem_paths: vec!["/data/server-x".into()],
            ..Default::default()
        });
        assert!(policy.filesystem_read_allowed("/data/server-x"));
        assert!(policy.filesystem_read_allowed("/data/server-x/config.toml"));
        assert!(!policy.filesystem_read_allowed("/data/server-y"));
        assert!(!policy.filesystem_read_allowed("/etc/passwd"));
    }

    #[test]
    fn topic_supports_wildcard_suffix() {
        let policy = SandboxPolicy::from_declaration(&CapabilityDeclaration {
            bus_topics: vec!["ordo.mcp.sandbox.*".into(), "ordo.health.probe".into()],
            ..Default::default()
        });
        assert!(policy.topic_allowed("ordo.mcp.sandbox.status"));
        assert!(policy.topic_allowed("ordo.mcp.sandbox.anything"));
        assert!(policy.topic_allowed("ordo.health.probe"));
        assert!(!policy.topic_allowed("ordo.secrets.vault.dereference.request"));
    }
}
