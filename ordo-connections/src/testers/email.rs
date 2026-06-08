//! Email connection tester.
//!
//! Validates that required fields are present and the IMAP/SMTP
//! host config looks sane. A full IMAP login test requires the
//! bridge to be running -- this is a lightweight pre-flight check.

use serde_json::Value;

pub async fn test(fields: &Value, secret: Option<&str>) -> Result<String, String> {
    let address = fields
        .get("email_address")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("missing field: email_address")?;

    let imap_host = fields
        .get("imap_host")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("missing field: imap_host")?;

    let smtp_host = fields
        .get("smtp_host")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("missing field: smtp_host")?;

    let _username = fields
        .get("imap_username")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("missing field: imap_username")?;

    // Secret is required for actual sending, but may not be set yet
    if secret.is_none() || secret.unwrap().is_empty() {
        return Err("password is required".into());
    }

    let authorized = fields
        .get("authorized_senders")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let prefix = fields
        .get("command_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("ordo:");

    Ok(format!(
        "config valid (addr={address}, imap={imap_host}, smtp={smtp_host}, prefix={prefix}, auth_senders={})",
        if authorized.is_empty() { "any" } else { authorized }
    ))
}
