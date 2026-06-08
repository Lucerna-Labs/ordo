//! Classifier abstraction — the unit of pluggable scanning.
//!
//! A `Classifier` takes a `ScanInput` (text + context about what phase
//! of a tool call this is, which plugin, which capability, which field)
//! and returns zero or more `Finding`s. Findings carry a severity and a
//! `rule_id` that lets the policy engine decide what to do.
//!
//! Today every built-in classifier is regex-based. Future classifiers
//! can be ML models, LLM-judges, embedding similarity checks, or
//! anything else that produces `Finding`s — the trait stays the same.

use serde::{Deserialize, Serialize};

/// When during the tool-call lifecycle a scan is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Scanning the caller's arguments before the tool runs.
    PreCall,
    /// Scanning the tool's return value.
    PostCall,
}

/// Where in the JSON payload a finding was located. Helps operators
/// spot the actual offending field in a large blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingLocation {
    /// JSON pointer (RFC 6901) into the arguments or result, e.g.
    /// `/messages/0/content` for a nested chat message.
    pub pointer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warn,
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Error => "error",
        }
    }
}

/// Everything a classifier receives about the scan target. Wrapped in
/// its own struct so future classifiers can key off plugin/capability
/// context without reworking the trait.
pub struct ScanInput<'a> {
    pub text: &'a str,
    pub phase: Phase,
    pub plugin: &'a str,
    pub capability: &'a str,
    /// JSON pointer to the location `text` came from. Pass `""` for
    /// top-level scans.
    pub pointer: &'a str,
}

/// A single classification result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    /// Already-redacted preview of what matched (never the raw match).
    pub match_preview: String,
    pub location: FindingLocation,
}

impl Finding {
    pub fn new(
        rule_id: impl Into<String>,
        severity: Severity,
        message: impl Into<String>,
        match_preview: impl Into<String>,
        pointer: impl Into<String>,
    ) -> Self {
        Self {
            rule_id: rule_id.into(),
            severity,
            message: message.into(),
            match_preview: match_preview.into(),
            location: FindingLocation {
                pointer: pointer.into(),
            },
        }
    }
}

/// Pluggable scanner. `scan` returns the full list of findings; the
/// policy engine later decides which ones become actionable.
pub trait Classifier: Send + Sync {
    /// Stable id (e.g. `secret.openai_key`). Used by policy overrides.
    fn id(&self) -> &str;
    /// One-sentence human description for the rules inventory UI.
    fn description(&self) -> &str;
    /// Default severity for matches (can be overridden by policy).
    fn default_severity(&self) -> Severity;
    /// Whether this classifier is active at this phase. Most rules
    /// run on both sides; some (e.g. "response too large") only run
    /// post-call.
    fn applies_to(&self, phase: Phase) -> bool {
        let _ = phase;
        true
    }
    /// Return every finding in the scan input. Empty vec = clean.
    fn scan(&self, input: &ScanInput<'_>) -> Vec<Finding>;
}

/// Redact a matched substring so it can be shown in an audit log
/// without leaking the value. Keeps the first 4 and last 4 chars,
/// blanks the middle. Very short matches collapse to `***`.
pub fn redact_preview(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    if chars.len() <= 10 {
        return "***".to_string();
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}
