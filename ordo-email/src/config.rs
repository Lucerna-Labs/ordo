use serde::{Deserialize, Serialize};

/// Full email account configuration. Stored encrypted in the vault;
/// only decrypted at startup when the bridge spins up.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmailConfig {
    /// Email address to poll and send from (e.g. "alex@lucernamedia.com")
    pub address: String,

    /// Display name on outgoing messages
    #[serde(default)]
    pub display_name: Option<String>,

    // --- IMAP ---
    /// IMAP server hostname (e.g. "imap.gmail.com")
    pub imap_host: String,
    /// IMAP port (default 993 for TLS)
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    /// IMAP username (usually same as email address)
    pub imap_username: String,
    /// IMAP password or app-specific password
    pub imap_password: String,

    // --- SMTP ---
    /// SMTP server hostname (e.g. "smtp.gmail.com")
    pub smtp_host: String,
    /// SMTP port (default 587 for STARTTLS)
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,

    // --- Behaviour ---
    /// Only process emails from these addresses. Empty = accept all.
    #[serde(default)]
    pub authorized_senders: Vec<String>,

    /// Poll interval in seconds
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: u64,

    /// Subject prefix that triggers command parsing (e.g. "ordo:")
    #[serde(default = "default_command_prefix")]
    pub command_prefix: String,

    /// IMAP mailbox to poll
    #[serde(default = "default_mailbox")]
    pub mailbox: String,
}

fn default_imap_port() -> u16 {
    993
}
fn default_smtp_port() -> u16 {
    587
}
fn default_poll_seconds() -> u64 {
    30
}
fn default_command_prefix() -> String {
    "ordo:".to_string()
}
fn default_mailbox() -> String {
    "INBOX".to_string()
}

impl EmailConfig {
    /// Quick sanity checks. Returns a list of problems, empty = ok.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.address.is_empty() {
            issues.push("address is empty".into());
        }
        if self.imap_host.is_empty() {
            issues.push("imap_host is empty".into());
        }
        if self.imap_username.is_empty() {
            issues.push("imap_username is empty".into());
        }
        if self.imap_password.is_empty() {
            issues.push("imap_password is empty".into());
        }
        if self.smtp_host.is_empty() {
            issues.push("smtp_host is empty".into());
        }
        issues
    }
}
