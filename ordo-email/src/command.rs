/// Parsed command extracted from an email subject line.
/// The command prefix is stripped; everything after is the command text.
///
/// Example: Subject "ordo: summarize recent blog posts"
///   -> ParsedCommand { raw: "summarize recent blog posts", from: "jesse@example.com" }

#[derive(Debug, Clone)]
pub struct ParsedCommand {
    /// The raw command text (everything after the prefix).
    pub raw: String,
    /// Who sent it.
    pub from_address: String,
    /// Email body for context.
    pub body_plain: String,
    /// Optional HTML body.
    pub body_html: Option<String>,
}

/// Parse a subject line against a prefix. Returns Some if the subject
/// starts with the prefix (case-insensitive). The returned command text
/// is trimmed.
pub fn parse_subject(subject: &str, prefix: &str) -> Option<String> {
    let prefix_lower = prefix.to_lowercase();
    let subject_lower = subject.to_lowercase();
    if subject_lower.starts_with(&prefix_lower) {
        let command = &subject[prefix.len()..].trim().to_string();
        if command.is_empty() {
            None
        } else {
            Some(command.clone())
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixed_subject() {
        assert_eq!(
            parse_subject("ordo: build the thing", "ordo:"),
            Some("build the thing".into())
        );
    }

    #[test]
    fn case_insensitive_prefix() {
        assert_eq!(parse_subject("ORDO: Hello", "ordo:"), Some("Hello".into()));
    }

    #[test]
    fn no_prefix_returns_none() {
        assert_eq!(parse_subject("Re: ordo: old thread", "ordo:"), None);
        assert_eq!(parse_subject("hey check this out", "ordo:"), None);
    }

    #[test]
    fn empty_command_after_prefix() {
        assert_eq!(parse_subject("ordo:   ", "ordo:"), None);
    }
}
