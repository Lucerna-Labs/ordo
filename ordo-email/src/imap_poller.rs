use std::collections::HashSet;
use std::net::TcpStream;

use tracing::debug;

use crate::command;
use crate::config::EmailConfig;

#[derive(Debug, Clone)]
pub struct ReceivedEmail {
    pub seq: u32,
    pub message_id: String,
    pub from: String,
    pub body_plain: String,
    pub body_html: Option<String>,
    pub subject: String,
}

pub struct ImapPoller {
    config: EmailConfig,
    seen_ids: HashSet<String>,
}

impl ImapPoller {
    pub fn new(config: EmailConfig) -> Self {
        Self {
            config,
            seen_ids: HashSet::new(),
        }
    }

    pub fn process_new_messages(&mut self, msg: &ReceivedEmail) -> Option<ReceivedEmail> {
        let dedupe_key = if msg.message_id.is_empty() {
            format!("{}|{}", msg.subject, msg.from)
        } else {
            msg.message_id.clone()
        };

        if self.seen_ids.contains(&dedupe_key) {
            return None;
        }
        self.seen_ids.insert(dedupe_key);

        if self.seen_ids.len() > 10_000 {
            self.seen_ids.clear();
        }

        if !self.config.authorized_senders.is_empty() {
            let from_lower = msg.from.to_lowercase();
            let authorized = self
                .config
                .authorized_senders
                .iter()
                .any(|auth| from_lower == auth.to_lowercase());
            if !authorized {
                debug!("ordo-email: skipped unauthorized: {}", msg.from);
                return None;
            }
        }

        Some(msg.clone())
    }
}

/// Poll IMAP inbox using sync imap via spawn_blocking.
pub async fn poll_inbox(config: &EmailConfig) -> Result<Vec<ReceivedEmail>, String> {
    let host = config.imap_host.clone();
    let port = config.imap_port;
    let username = config.imap_username.clone();
    let password = config.imap_password.clone();
    let mailbox = config.mailbox.clone();

    tokio::task::spawn_blocking(move || poll_sync(&host, port, &username, &password, &mailbox))
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?
}

fn poll_sync(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    mailbox: &str,
) -> Result<Vec<ReceivedEmail>, String> {
    let addr = format!("{host}:{port}");

    let tls = native_tls::TlsConnector::builder()
        .build()
        .map_err(|e| format!("TLS builder: {e}"))?;

    let stream = TcpStream::connect(&addr).map_err(|e| format!("IMAP connect {addr}: {e}"))?;

    let tls_stream = tls
        .connect(host, stream)
        .map_err(|e| format!("IMAP TLS: {e}"))?;

    let client = imap::Client::new(tls_stream);
    let mut session = client
        .login(username, password)
        .map_err(|(e, _)| format!("IMAP login: {e}"))?;

    session
        .select(mailbox)
        .map_err(|e| format!("IMAP select {mailbox}: {e}"))?;

    let uids = session
        .search("UNSEEN")
        .map_err(|e| format!("IMAP search: {e}"))?;

    if uids.is_empty() {
        session.logout().map_err(|e| format!("IMAP logout: {e}"))?;
        return Ok(Vec::new());
    }

    let to_fetch: Vec<u32> = uids.into_iter().take(20).collect();
    let uid_set = to_fetch
        .iter()
        .map(|u: &u32| u.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let fetches = session
        .uid_fetch(&uid_set, "(UID BODY[])")
        .map_err(|e| format!("IMAP fetch: {e}"))?;

    let mut emails = Vec::new();

    for fetch in fetches.iter() {
        let body_raw = match fetch.body() {
            Some(b) => b,
            None => continue,
        };
        if body_raw.is_empty() {
            continue;
        }

        let uid = fetch.uid.unwrap_or(0);

        // Simple header+body extraction — parse RFC2822 headers manually
        let (subject, from, body_text, body_html) = extract_email_fields(body_raw);

        let message_id = extract_header(body_raw, "message-id").unwrap_or_default();

        emails.push(ReceivedEmail {
            seq: uid,
            message_id,
            from,
            body_plain: body_text,
            body_html,
            subject,
        });
    }

    session.logout().map_err(|e| format!("IMAP logout: {e}"))?;
    Ok(emails)
}

/// Extract key fields from raw RFC2822 email bytes without a full MIME parser.
fn extract_email_fields(raw: &[u8]) -> (String, String, String, Option<String>) {
    let subject = extract_header(raw, "subject").unwrap_or_else(|| "(no subject)".to_string());
    let from = extract_header(raw, "from").unwrap_or_else(|| "(unknown)".to_string());

    // Find the end of headers (blank line)
    let header_end = find_header_end(raw);
    let body_bytes = &raw[header_end..];

    // Try to parse body as text
    let raw_body = String::from_utf8_lossy(body_bytes).to_string();
    let cleaned = clean_text_body(&raw_body);

    // Extract HTML if present (multipart/alternative)
    let html = extract_html_part(&raw_body);

    (subject, from, cleaned, html)
}

/// Find the end of RFC2822 headers (first blank line: \r\n\r\n or \n\n)
fn find_header_end(raw: &[u8]) -> usize {
    for i in 0..raw.len().saturating_sub(3) {
        if &raw[i..i + 4] == b"\r\n\r\n" {
            return i + 4;
        }
    }
    for i in 0..raw.len().saturating_sub(1) {
        if &raw[i..i + 2] == b"\n\n" {
            return i + 2;
        }
    }
    raw.len().min(512) // Fallback: skip first 512 bytes
}

/// Extract a header value (case-insensitive, handles folded lines).
fn extract_header(raw: &[u8], name: &str) -> Option<String> {
    // Decode quoted-printable + RFC2047 encoded words
    let raw_text = String::from_utf8_lossy(raw).to_string();
    let name_lower = name.to_lowercase();

    let lines: Vec<&str> = raw_text.lines().collect();
    let mut in_header = false;
    let mut value = String::new();

    for line in &lines {
        let trimmed = line.trim();
        if in_header {
            // Check if this is a continuation line (starts with whitespace)
            if line.starts_with(' ') || line.starts_with('\t') {
                value.push(' ');
                value.push_str(trimmed);
                continue;
            } else {
                // End of this header — found it
                return Some(decode_rfc2047(&value).trim().to_string());
            }
        }
        if let Some(colon_pos) = trimmed.find(':') {
            let key = &trimmed[..colon_pos];
            if key.to_lowercase() == name_lower {
                value = trimmed[colon_pos + 1..].trim().to_string();
                in_header = true;
            }
        }
    }

    if in_header && !value.is_empty() {
        return Some(decode_rfc2047(&value).trim().to_string());
    }
    None
}

/// Decode RFC2047 encoded words (=?charset?encoding?text?=)
fn decode_rfc2047(input: &str) -> String {
    let mut result = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("=?") {
        result.push_str(&rest[..start]);
        let encoded = &rest[start..];
        if let Some(end) = encoded.find("?=") {
            let word = &encoded[..end + 2]; // Include "?="
            if let Some(decoded) = decode_single_rfc2047(word) {
                result.push_str(&decoded);
            } else {
                result.push_str(word);
            }
            rest = &encoded[end + 2..];
        } else {
            result.push_str(rest);
            break;
        }
    }
    result.push_str(rest);
    result
}

fn decode_single_rfc2047(word: &str) -> Option<String> {
    // Format: =?charset?encoding?text?=
    let inner = word.strip_prefix("=?")?.strip_suffix("?=")?;
    let parts: Vec<&str> = inner.splitn(3, '?').collect();
    if parts.len() != 3 {
        return None;
    }
    let (_, encoding, encoded_text) = (parts[0], parts[1], parts[2]);
    match encoding.to_lowercase().as_str() {
        "b" | "b64" => {
            // Base64
            let decoded = base64_decode(encoded_text)?;
            String::from_utf8(decoded).ok()
        }
        "q" => {
            // Q-encoding: =XX for hex, _ for space, everything else literal
            Some(decode_q_encoding(encoded_text))
        }
        _ => Some(encoded_text.to_string()),
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    use std::collections::HashMap;
    let charset: HashMap<char, u8> =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            .chars()
            .enumerate()
            .map(|(i, c)| (c, i as u8))
            .collect();

    let clean: String = input
        .chars()
        .filter(|c| *c != ' ' && *c != '\n' && *c != '\r')
        .collect();
    if !clean.len().is_multiple_of(4) {
        return None;
    }

    let mut buf = Vec::new();
    let bytes: Vec<u8> = clean
        .chars()
        .filter_map(|c| charset.get(&c).copied())
        .collect();
    if bytes.len() != clean.len() {
        return None;
    }

    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let b3 = chunk.get(3).copied().unwrap_or(0) as u32;
        buf.push(((b0 << 2) | (b1 >> 4)) as u8);
        if chunk.len() > 2 {
            buf.push(((b1 << 4) | (b2 >> 2)) as u8);
        }
        if chunk.len() > 3 {
            buf.push(((b2 << 6) | b3) as u8);
        }
    }
    Some(buf)
}

fn decode_q_encoding(input: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '_' {
            result.push(' ');
            i += 1;
        } else if chars[i] == '=' && i + 2 < chars.len() {
            let hex = format!("{}{}", chars[i + 1], chars[i + 2]);
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
            i += 3;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// Clean a raw email body — strip quoted text, signature blocks, normalize whitespace.
fn clean_text_body(raw: &str) -> String {
    let mut lines = Vec::new();
    let mut in_quote = false;
    for line in raw.lines() {
        let trimmed = line.trim();

        // Stop at signature
        if trimmed == "-- " || trimmed == "--" {
            break;
        }

        // Skip quoted reply lines
        if trimmed.starts_with('>') {
            in_quote = true;
            continue;
        }

        // Skip attribution lines after quotes
        if in_quote && (trimmed.starts_with("On ") && trimmed.contains("wrote:")) {
            continue;
        }
        in_quote = false;

        // Skip empty header metadata lines
        if trimmed.is_empty() && lines.is_empty() {
            continue;
        }

        lines.push(trimmed.to_string());
    }

    // Trim trailing empty lines
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

/// Try to extract an HTML part from a multipart MIME message.
fn extract_html_part(raw: &str) -> Option<String> {
    let boundary = find_multipart_boundary(raw)?;
    let mut in_html = false;
    let mut html_lines = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == format!("--{boundary}") || trimmed.starts_with(&format!("--{boundary}--")) {
            if in_html {
                break;
            }
            continue;
        }
        if trimmed.eq_ignore_ascii_case("content-type: text/html") {
            in_html = true;
            continue;
        }
        if in_html {
            // Skip other part headers
            if trimmed.is_empty() {
                // Start of actual HTML content
                continue;
            }
            if line.contains(':') && !line.contains('<') {
                // Still in headers
                continue;
            }
            html_lines.push(line.to_string());
        }
    }

    if html_lines.is_empty() {
        None
    } else {
        Some(html_lines.join("\n"))
    }
}

/// Find the MIME boundary from Content-Type header.
fn find_multipart_boundary(input: &str) -> Option<String> {
    for line in input.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("content-type:") && lower.contains("boundary=") {
            if let Some(pos) = lower.find("boundary=") {
                let rest = &line[pos + 9..];
                let boundary = rest.trim().trim_matches('"').trim_matches('\'').to_string();
                return Some(boundary);
            }
        }
    }
    None
}

/// Filter emails that match the command prefix.
pub fn filter_commands(
    emails: Vec<ReceivedEmail>,
    config: &EmailConfig,
) -> Vec<(ReceivedEmail, command::ParsedCommand)> {
    emails
        .into_iter()
        .filter_map(|email| {
            let cmd_raw = command::parse_subject(&email.subject, &config.command_prefix)?;
            Some((
                email.clone(),
                command::ParsedCommand {
                    raw: cmd_raw,
                    from_address: email.from.clone(),
                    body_plain: email.body_plain.clone(),
                    body_html: email.body_html.clone(),
                },
            ))
        })
        .collect()
}
