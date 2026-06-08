//! Connection type registry — the catalog of named integrations
//! the operator picks from when adding a new connection.
//!
//! Each entry is data the studio renders into a tile + a form
//! schema. The studio loops over `catalog()` and renders
//! whatever's there.
//!
//! Adding a new type = add a new `ConnectionType` to `catalog()`
//! and (optionally) a new tester in `crate::testers`. No UI
//! change required.

use serde::{Deserialize, Serialize};

use crate::testers;

pub type ConnectionTypeId = &'static str;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionType {
    pub id: ConnectionTypeId,
    pub display_name: &'static str,
    pub description: &'static str,
    pub icon: &'static str, // emoji or named icon — studio renders it
    pub category: ConnectionCategory,
    /// Form fields for the non-secret config (handle, site URL,
    /// instance URL, etc.). Each is rendered by the studio.
    pub fields: Vec<FieldSchema>,
    /// When true, the form includes a password-style field whose
    /// value is sealed in the vault. The exact label / placeholder
    /// for that field comes from `secret_label` / `secret_placeholder`.
    pub requires_secret: bool,
    pub secret_label: &'static str,
    pub secret_placeholder: &'static str,
    pub secret_help: &'static str,
    /// When true the studio shows a Test Connection button + runs
    /// the type's tester on save.
    pub has_test: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionCategory {
    AiProvider,
    Infrastructure,
    Generic,
}

impl ConnectionCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::AiProvider => "AI Provider",
            Self::Infrastructure => "Infrastructure",
            Self::Generic => "Generic",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSchema {
    pub name: &'static str,
    pub label: &'static str,
    pub field_type: FieldType,
    pub required: bool,
    pub placeholder: &'static str,
    pub help: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Url,
    Email,
    Number,
    /// Multi-line text (PEM keys, JSON snippets, etc.).
    LongText,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestReport {
    pub status: TestStatus,
    pub detail: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Ok,
    Error,
    NotApplicable,
}

/// First-party catalog. Order matters — this is the order tiles
/// render in the studio.
pub fn catalog() -> Vec<ConnectionType> {
    vec![
        // ----- AI providers -----
        ConnectionType {
            id: "openai",
            display_name: "OpenAI",
            description: "OpenAI API for chat, embeddings, and more.",
            icon: "🤖",
            category: ConnectionCategory::AiProvider,
            fields: vec![FieldSchema {
                name: "organization",
                label: "Organization (optional)",
                field_type: FieldType::Text,
                required: false,
                placeholder: "org-...",
                help: "Optional organization id; leave blank for personal accounts.",
            }],
            requires_secret: true,
            secret_label: "API key",
            secret_placeholder: "sk-...",
            secret_help: "Generate at platform.openai.com → API keys.",
            has_test: true,
        },
        ConnectionType {
            id: "anthropic",
            display_name: "Anthropic",
            description: "Anthropic Claude API for messages.",
            icon: "✨",
            category: ConnectionCategory::AiProvider,
            fields: vec![],
            requires_secret: true,
            secret_label: "API key",
            secret_placeholder: "sk-ant-...",
            secret_help: "Generate at console.anthropic.com → Settings → API Keys.",
            has_test: true,
        },
        // ----- Infrastructure -----
        ConnectionType {
            id: "ssh",
            display_name: "SSH Server",
            description: "Remote shell access for deploys, builds, and admin scripts.",
            icon: "💻",
            category: ConnectionCategory::Infrastructure,
            fields: vec![
                FieldSchema {
                    name: "host",
                    label: "Host",
                    field_type: FieldType::Text,
                    required: true,
                    placeholder: "build-01.example.com",
                    help: "Hostname or IP address of the remote server.",
                },
                FieldSchema {
                    name: "port",
                    label: "Port",
                    field_type: FieldType::Number,
                    required: false,
                    placeholder: "22",
                    help: "SSH port. Leave blank for the default 22.",
                },
                FieldSchema {
                    name: "username",
                    label: "Username",
                    field_type: FieldType::Text,
                    required: true,
                    placeholder: "deploy",
                    help: "Remote username.",
                },
            ],
            requires_secret: true,
            secret_label: "Password or private key (PEM)",
            secret_placeholder: "password OR -----BEGIN OPENSSH PRIVATE KEY-----",
            secret_help:
                "Either an SSH password or a PEM-formatted private key. Stored encrypted in the vault.",
            has_test: true,
        },
        // ----- Email -----
        ConnectionType {
            id: "email",
            display_name: "Email",
            description:
                "IMAP inbox polling + SMTP sending. Ordo reads commands from your inbox and sends replies.",
            icon: "📧",
            category: ConnectionCategory::Infrastructure,
            fields: vec![
                FieldSchema {
                    name: "email_address",
                    label: "Email address",
                    field_type: FieldType::Email,
                    required: true,
                    placeholder: "ordo@lucernamedia.com",
                    help: "The email address to poll and send from.",
                },
                FieldSchema {
                    name: "display_name",
                    label: "Display name",
                    field_type: FieldType::Text,
                    required: false,
                    placeholder: "Ordo",
                    help: "Name shown on outgoing messages.",
                },
                FieldSchema {
                    name: "imap_host",
                    label: "IMAP host",
                    field_type: FieldType::Text,
                    required: true,
                    placeholder: "imap.gmail.com",
                    help: "IMAP server for receiving email.",
                },
                FieldSchema {
                    name: "imap_port",
                    label: "IMAP port",
                    field_type: FieldType::Number,
                    required: false,
                    placeholder: "993",
                    help: "IMAP port. Defaults to 993 (TLS).",
                },
                FieldSchema {
                    name: "smtp_host",
                    label: "SMTP host",
                    field_type: FieldType::Text,
                    required: true,
                    placeholder: "smtp.gmail.com",
                    help: "SMTP server for sending email.",
                },
                FieldSchema {
                    name: "smtp_port",
                    label: "SMTP port",
                    field_type: FieldType::Number,
                    required: false,
                    placeholder: "587",
                    help: "SMTP port. Defaults to 587 (STARTTLS).",
                },
                FieldSchema {
                    name: "imap_username",
                    label: "Username",
                    field_type: FieldType::Text,
                    required: true,
                    placeholder: "ordo@lucernamedia.com",
                    help: "IMAP login username (usually same as email address).",
                },
                FieldSchema {
                    name: "authorized_senders",
                    label: "Authorized senders (one per line)",
                    field_type: FieldType::LongText,
                    required: false,
                    placeholder: "jesse@lucernamedia.com",
                    help: "Only accept commands from these email addresses. Leave blank to accept all.",
                },
                FieldSchema {
                    name: "command_prefix",
                    label: "Command prefix",
                    field_type: FieldType::Text,
                    required: false,
                    placeholder: "ordo:",
                    help: "Subject prefix for commands. Default: ordo:",
                },
            ],
            requires_secret: true,
            secret_label: "IMAP password or app password",
            secret_placeholder: "abcd efgh ijkl mnop",
            secret_help:
                "For Gmail: generate an App Password under Google Account -> Security -> 2-Step Verification -> App Passwords. Never use your login password.",
            has_test: true,
        },
        // ----- Generic catch-alls -----
        ConnectionType {
            id: "generic_api_key",
            display_name: "Generic API Key",
            description: "Any service whose auth is just a single secret value.",
            icon: "🔑",
            category: ConnectionCategory::Generic,
            fields: vec![FieldSchema {
                name: "service_url",
                label: "Service URL (optional)",
                field_type: FieldType::Url,
                required: false,
                placeholder: "https://api.example.com",
                help: "Optional — display / reference only.",
            }],
            requires_secret: true,
            secret_label: "Secret value",
            secret_placeholder: "...",
            secret_help: "The raw API key, token, or password the service expects.",
            has_test: false,
        },
        ConnectionType {
            id: "generic_webhook",
            display_name: "Generic Webhook",
            description:
                "Outbound HTTP endpoint to POST to. Optional bearer or HMAC secret.",
            icon: "📡",
            category: ConnectionCategory::Generic,
            fields: vec![
                FieldSchema {
                    name: "url",
                    label: "Webhook URL",
                    field_type: FieldType::Url,
                    required: true,
                    placeholder: "https://hooks.example.com/...",
                    help: "Where to POST the payload.",
                },
                FieldSchema {
                    name: "method",
                    label: "Method",
                    field_type: FieldType::Text,
                    required: false,
                    placeholder: "POST",
                    help: "HTTP method. Defaults to POST.",
                },
            ],
            requires_secret: false,
            secret_label: "Secret (optional)",
            secret_placeholder: "",
            secret_help:
                "Optional — included as a Bearer token if provided.",
            has_test: true,
        },
        ConnectionType {
            id: "custom_mcp",
            display_name: "Custom MCP Server",
            description:
                "Add a custom MCP server by command path. Advanced — for developers.",
            icon: "🧩",
            category: ConnectionCategory::Generic,
            fields: vec![
                FieldSchema {
                    name: "command",
                    label: "Command",
                    field_type: FieldType::Text,
                    required: true,
                    placeholder: "/usr/local/bin/my-mcp",
                    help: "Absolute path to the MCP server binary.",
                },
                FieldSchema {
                    name: "args",
                    label: "Arguments (one per line)",
                    field_type: FieldType::LongText,
                    required: false,
                    placeholder: "--config\n/path/to/config",
                    help: "Optional command-line arguments, one per line.",
                },
            ],
            requires_secret: false,
            secret_label: "Secret (optional)",
            secret_placeholder: "",
            secret_help:
                "Optional — passed as an environment variable named MCP_SECRET when launching the server.",
            has_test: false,
        },
    ]
}

/// Look up a connection type by id. Returns `None` if not in the
/// catalog (the studio uses this to guard against stale rows
/// pointing at removed types).
pub fn find(id: &str) -> Option<ConnectionType> {
    catalog().into_iter().find(|t| t.id == id)
}

/// Run the appropriate tester for a given type. Returns a
/// `TestReport` even on type-without-test (with `NotApplicable`).
pub async fn run_test(
    type_id: &str,
    fields: &serde_json::Value,
    secret: Option<&str>,
) -> TestReport {
    let started = std::time::Instant::now();
    let result = match type_id {
        "openai" => testers::openai::test(secret).await,
        "anthropic" => testers::anthropic::test(secret).await,
        "ssh" => testers::ssh::test(fields).await,
        "generic_webhook" => testers::generic_webhook::test(fields, secret).await,
        "email" => testers::email::test(fields, secret).await,
        _ => {
            return TestReport {
                status: TestStatus::NotApplicable,
                detail: "no test handler for this connection type".into(),
                duration_ms: started.elapsed().as_millis() as u64,
            }
        }
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(detail) => TestReport {
            status: TestStatus::Ok,
            detail,
            duration_ms,
        },
        Err(detail) => TestReport {
            status: TestStatus::Error,
            detail,
            duration_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_no_duplicate_ids() {
        let cat = catalog();
        let mut seen = std::collections::HashSet::new();
        for t in &cat {
            assert!(seen.insert(t.id), "duplicate type id: {}", t.id);
        }
    }

    #[test]
    fn catalog_excludes_removed_channel_workflows() {
        let cat = catalog();
        for entry in cat {
            assert!(
                matches!(
                    entry.category,
                    ConnectionCategory::AiProvider
                        | ConnectionCategory::Infrastructure
                        | ConnectionCategory::Generic
                ),
                "unsupported connection category still present"
            );
        }
    }

    #[test]
    fn find_returns_none_for_unknown() {
        assert!(find("totally-not-a-type").is_none());
    }
}
