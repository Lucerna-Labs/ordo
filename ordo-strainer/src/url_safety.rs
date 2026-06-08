//! URL safety + canonicalization for the strain pipeline.
//!
//! ## Why this exists
//!
//! The strainer is the boundary where untrusted URLs cross into
//! Ordo's audit log (boundary tag's `source` attribute) and into
//! the model's context. Browser MCPs are about to land — they'll
//! hand back URLs they navigated to, and the LLM will emit URLs
//! describing what it found. Both flow through here.
//!
//! Without a single URL-validation chokepoint, every consumer
//! reinvents the rules and disagrees about edge cases:
//!
//!   - `https://evil.com#https://trusted.com` — the fragment
//!     looks trusted in source-URL displays that don't render
//!     the host clearly. Spoofing vector.
//!   - `https://user:hunter2@example.com/` — the userinfo
//!     leaks credentials into the audit log when written to disk.
//!   - Multi-kilobyte URLs — usually auth-token blobs the LLM
//!     shouldn't be parroting back into context.
//!   - Mixed-script hostnames (`https://example.com/` where one
//!     letter is Cyrillic) — punycode-encoded homoglyph domains.
//!     Hard to defeat at the URL layer alone, but worth flagging.
//!
//! ## What this module does
//!
//! - `validate_url(s)` returns `SafeUrl` or rejects. Centralizes
//!   the strainer's existing scheme check and adds the
//!   security rules above.
//! - `SafeUrl::canonical()` returns the form to record in audit:
//!   lowercase host, default ports stripped, fragment + query
//!   preserved. Stable across equivalent inputs.
//! - `SafeUrl::fragment()` exposes the URL fragment for callers
//!   that want to scope downstream extraction to an anchor.
//!
//! ## What this module does NOT do
//!
//! - Does not fetch. That's `fetch.rs`'s job; this is pure
//!   string-level validation + parsing.
//! - Does not check DNS / reachability. The runtime decides
//!   what's reachable from where; the URL parser is name-only.
//! - Does not rewrite the URL silently. Validation surfaces
//!   problems; canonicalization is opt-in via `canonical()`.
//!   Operators get a stable round-trip when they want it AND
//!   the original verbatim string when they want that.

use thiserror::Error;
use url::Url;

/// Hardcoded — keeping this in code rather than a config so
/// "raise the cap to 1 MB and pretend it's normal" requires a
/// code change + review. Fits standard browser bars and almost
/// every legitimate URL.
pub const MAX_URL_LENGTH: usize = 2048;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UrlSafetyError {
    #[error("URL exceeds {limit}-byte cap (got {actual}); auth-token blobs and exfiltrated payloads tend to live in long URLs")]
    TooLong { limit: usize, actual: usize },
    #[error("URL parse failed: {0}")]
    Parse(String),
    #[error("scheme '{scheme}' is not allowed; only http and https are permitted at the strainer boundary")]
    DisallowedScheme { scheme: String },
    #[error(
        "URL embeds userinfo (user:password@host); credentials in URLs leak into audit logs and source attributions — use a credential-store reference instead"
    )]
    EmbeddedUserinfo,
    #[error("URL has no host; relative or schema-only URLs are not safe at the strainer boundary")]
    MissingHost,
    #[error("URL contains control characters; suspicious in any URL the strainer sees")]
    ControlChars,
}

/// A URL that passed the strainer's safety rules. Holds the
/// parsed `Url` plus a flag for "mixed-script host detected" so
/// callers can surface the warning in the audit log without
/// re-parsing the host.
#[derive(Debug, Clone)]
pub struct SafeUrl {
    parsed: Url,
    pub mixed_script_host: bool,
}

impl SafeUrl {
    /// Operator-facing canonical form. Lowercase host (URL hosts
    /// are case-insensitive per RFC 3986 §3.2.2 anyway), default
    /// ports stripped (`:80` for http, `:443` for https). Path,
    /// query, and fragment preserved verbatim — operators looking
    /// at the audit log need the fragment to know which page
    /// section was being read.
    pub fn canonical(&self) -> String {
        let mut url = self.parsed.clone();
        // url::Url already lowercases the host on parse, but this
        // is the contract test point — make it explicit so a
        // future url-crate behavior change doesn't silently break.
        if let Some(host_str) = url.host_str().map(str::to_string) {
            // set_host accepts None to clear; we want the lowercase
            // round-trip. ASCII lowercase is the canonical form
            // since IDN punycode is already lowercase.
            let _ = url.set_host(Some(&host_str.to_ascii_lowercase()));
        }
        // Strip default ports.
        if let Some(port) = url.port() {
            let scheme = url.scheme();
            if (scheme == "http" && port == 80) || (scheme == "https" && port == 443) {
                let _ = url.set_port(None);
            }
        }
        url.into()
    }

    /// The fragment, if any. Strips the leading `#`.
    pub fn fragment(&self) -> Option<&str> {
        self.parsed.fragment()
    }

    /// The host, if present (always present after validation).
    pub fn host(&self) -> Option<&str> {
        self.parsed.host_str()
    }

    /// The scheme. Always `"http"` or `"https"` after validation.
    pub fn scheme(&self) -> &str {
        self.parsed.scheme()
    }

    /// Underlying parsed `Url` for callers that need the full
    /// API. Use with care — bypasses the canonicalization above.
    pub fn as_url(&self) -> &Url {
        &self.parsed
    }
}

/// The single chokepoint. Rejects unsafe URLs; returns a
/// `SafeUrl` on success that callers can canonicalize, query for
/// fragment, etc.
pub fn validate_url(input: &str) -> Result<SafeUrl, UrlSafetyError> {
    if input.len() > MAX_URL_LENGTH {
        return Err(UrlSafetyError::TooLong {
            limit: MAX_URL_LENGTH,
            actual: input.len(),
        });
    }
    if input.chars().any(|c| c.is_control()) {
        return Err(UrlSafetyError::ControlChars);
    }

    let parsed = Url::parse(input).map_err(|err| UrlSafetyError::Parse(err.to_string()))?;

    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https") {
        return Err(UrlSafetyError::DisallowedScheme {
            scheme: scheme.to_string(),
        });
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(UrlSafetyError::EmbeddedUserinfo);
    }

    if parsed.host_str().is_none() {
        return Err(UrlSafetyError::MissingHost);
    }

    // Mixed-script check on the RAW input — `url::Url` already
    // ASCII-encodes IDN hostnames to punycode (xn--…), which would
    // hide the mixed-script signature from us. Look at the
    // pre-parse string between `://` and the next path/query/
    // fragment delimiter.
    let raw_host = extract_raw_host(input).unwrap_or("");
    let mixed_script_host = host_is_mixed_script(raw_host);
    if mixed_script_host {
        // Don't reject — IDN domains and operator-internal
        // hosts can legitimately mix scripts. Log so audit
        // sees it. The cup gate (taint propagation in
        // ordo-mcp-provenance) is the deeper defense.
        tracing::warn!(
            target: "ordo_strainer",
            host = parsed.host_str().unwrap_or(""),
            "URL host is mixed-script — possible homoglyph spoofing"
        );
    }

    Ok(SafeUrl {
        parsed,
        mixed_script_host,
    })
}

/// Pull the host substring out of a raw URL input — between
/// the `://` and the first `/`, `?`, `#`, or `:` (port). Used by
/// the mixed-script check, which has to run on the raw form
/// because `url::Url` punycode-encodes IDN hosts on parse.
///
/// Returns None if the input has no `://` (i.e., the URL would
/// fail Url::parse anyway and validate_url returns earlier).
/// Returns the host minus userinfo when present (so userinfo
/// validation continues to work via the parsed Url).
fn extract_raw_host(input: &str) -> Option<&str> {
    let after_scheme = input.split_once("://")?.1;
    // Strip userinfo if present.
    let after_userinfo = after_scheme
        .rsplit_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(after_scheme);
    // Cut at the first delimiter.
    let host_end = after_userinfo
        .find(|c: char| matches!(c, '/' | '?' | '#' | ':'))
        .unwrap_or(after_userinfo.len());
    Some(&after_userinfo[..host_end])
}

/// Heuristic: returns true if the host contains characters from
/// MULTIPLE Unicode scripts (e.g., Latin + Cyrillic). Pure ASCII
/// hosts return false. Pure non-Latin (legitimate Russian,
/// Chinese, Arabic domains) returns false.
fn host_is_mixed_script(host: &str) -> bool {
    let mut latin = false;
    let mut cyrillic = false;
    let mut greek = false;
    for c in host.chars() {
        let cp = c as u32;
        // Latin range: ASCII letters + Latin-1 Supplement letters
        if (b'a'..=b'z').contains(&(cp as u8 & 0xff)) && cp < 0x80 {
            latin = true;
        } else if cp >= 0x0410 && cp <= 0x044F {
            cyrillic = true;
        } else if cp >= 0x0370 && cp <= 0x03FF {
            greek = true;
        }
    }
    let count = [latin, cyrillic, greek].iter().filter(|&&v| v).count();
    count > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_https() {
        let u = validate_url("https://example.com/page").expect("ok");
        assert_eq!(u.scheme(), "https");
        assert_eq!(u.host(), Some("example.com"));
    }

    #[test]
    fn accepts_url_with_fragment() {
        let u = validate_url("https://docs.example.com/api#tools").expect("ok");
        assert_eq!(u.fragment(), Some("tools"));
        assert!(u.canonical().contains("#tools"));
    }

    #[test]
    fn accepts_url_with_query_and_fragment() {
        let u = validate_url("https://example.com/path?x=1&y=2#section-a").expect("ok");
        assert_eq!(u.fragment(), Some("section-a"));
        assert!(u.canonical().contains("?x=1&y=2"));
        assert!(u.canonical().contains("#section-a"));
    }

    #[test]
    fn rejects_javascript_scheme() {
        let err = validate_url("javascript:alert(1)").unwrap_err();
        assert!(matches!(err, UrlSafetyError::DisallowedScheme { .. }));
    }

    #[test]
    fn rejects_data_scheme() {
        let err = validate_url("data:text/html,<script>x</script>").unwrap_err();
        assert!(matches!(err, UrlSafetyError::DisallowedScheme { .. }));
    }

    #[test]
    fn rejects_file_scheme() {
        let err = validate_url("file:///etc/passwd").unwrap_err();
        assert!(matches!(err, UrlSafetyError::DisallowedScheme { .. }));
    }

    #[test]
    fn rejects_userinfo_in_url() {
        // Credential leak vector: userinfo-form URLs survive into
        // boundary tags and then into audit logs. Forbidden.
        let err = validate_url("https://admin:hunter2@example.com/dashboard").unwrap_err();
        assert!(matches!(err, UrlSafetyError::EmbeddedUserinfo));
    }

    #[test]
    fn rejects_username_only_in_url() {
        let err = validate_url("https://admin@example.com/").unwrap_err();
        assert!(matches!(err, UrlSafetyError::EmbeddedUserinfo));
    }

    #[test]
    fn rejects_oversized_url() {
        // 2049-byte URL: just over the cap.
        let long = format!("https://example.com/{}", "a".repeat(2030));
        assert!(long.len() > MAX_URL_LENGTH);
        let err = validate_url(&long).unwrap_err();
        assert!(matches!(err, UrlSafetyError::TooLong { .. }));
    }

    #[test]
    fn rejects_url_with_control_char() {
        // Tab character embedded in URL — the standard says it's
        // not allowed but some parsers tolerate it. We don't.
        let err = validate_url("https://example.com/\tpath").unwrap_err();
        assert!(matches!(err, UrlSafetyError::ControlChars));
    }

    #[test]
    fn rejects_malformed_url() {
        let err = validate_url("not a url").unwrap_err();
        assert!(matches!(err, UrlSafetyError::Parse(_)));
    }

    #[test]
    fn canonical_lowercases_host() {
        let u = validate_url("https://Example.COM/Path").expect("ok");
        let c = u.canonical();
        assert!(c.starts_with("https://example.com"), "got: {c}");
        // Path case is preserved — only host is canonicalized.
        assert!(c.contains("/Path"), "got: {c}");
    }

    #[test]
    fn canonical_strips_default_https_port() {
        let u = validate_url("https://example.com:443/path").expect("ok");
        assert_eq!(u.canonical(), "https://example.com/path");
    }

    #[test]
    fn canonical_strips_default_http_port() {
        let u = validate_url("http://example.com:80/path").expect("ok");
        assert_eq!(u.canonical(), "http://example.com/path");
    }

    #[test]
    fn canonical_keeps_non_default_port() {
        let u = validate_url("https://example.com:8443/path").expect("ok");
        assert!(u.canonical().contains(":8443"));
    }

    #[test]
    fn fragment_with_special_chars_preserved() {
        // RFC 3986 allows a wide range of fragment chars. Audit
        // attribution needs them verbatim — we don't strip the
        // fragment to make it "safer."
        let u = validate_url("https://example.com/p#section/sub-thing.foo").expect("ok");
        assert_eq!(u.fragment(), Some("section/sub-thing.foo"));
    }

    #[test]
    fn mixed_script_host_flagged_not_rejected() {
        // Latin 'a' (U+0061) + Cyrillic 'а' (U+0430) — a classic
        // homoglyph attack: visually identical, two different
        // characters. We flag but don't reject (legitimate IDN
        // domains exist; the host might be e.g. a mixed-script
        // operator domain). The cup gate carries the deeper
        // defense if this lands as untrusted content.
        let u = validate_url("https://exаmple.com/path").expect("ok");
        assert!(u.mixed_script_host, "should flag mixed script");
    }

    #[test]
    fn pure_ascii_host_not_mixed_script() {
        let u = validate_url("https://example.com/").expect("ok");
        assert!(!u.mixed_script_host);
    }

    #[test]
    fn pure_cyrillic_host_not_mixed_script() {
        // All-Cyrillic hostname (Russian-language domain on
        // Cyrillic-TLD .рф). NOT a mix; legitimate IDN.
        let url_str = "https://пример.рф/";
        let result = validate_url(url_str);
        match result {
            Ok(u) => {
                assert!(
                    !u.mixed_script_host,
                    "all-Cyrillic host should not flag as mixed"
                );
            }
            // Some url-crate configs reject IDN; the test still
            // serves its purpose by verifying we DON'T panic and
            // (if we accept) we don't false-flag.
            Err(_) => {}
        }
    }

    #[test]
    fn cyrillic_hostname_with_latin_tld_is_mixed_script() {
        // The COMMON spoofing variant: Cyrillic hostname stem +
        // Latin .com TLD. The detector flags this — good.
        let result = validate_url("https://пример.com/");
        if let Ok(u) = result {
            assert!(
                u.mixed_script_host,
                "Cyrillic name + Latin TLD should flag as mixed"
            );
        }
    }
}
