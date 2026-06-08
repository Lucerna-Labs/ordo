//! Canary token generation and detection.
//!
//! A canary is a high-entropy token injected into a tool's prompt
//! context. It is never part of the secret material. If the tool
//! repeats the canary in output, we know *that specific tool
//! invocation's context* leaked â€” invariant 22: per-capability,
//! not per-secret.
//!
//! Encoding: Crockford base32 over 24 bytes of OsRng â†’ 39 chars.
//! Includes the fixed prefix `ccC1_` so it's greppable in any log
//! stream and so short random collisions with a tool's legitimate
//! output are structurally impossible.

use data_encoding::BASE32_NOPAD;
use rand::RngCore;

/// Prefix marks a token as a Ordo canary. Greppable,
/// distinct from any real API key format we know of.
pub const CANARY_PREFIX: &str = "ccC1_";

/// Produce a fresh canary token. 24 bytes of `OsRng` â†’ base32
/// no-pad â†’ ~39 chars; prefix makes it 44 chars total.
pub fn generate_canary_token() -> String {
    let mut raw = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut raw);
    let encoded = BASE32_NOPAD.encode(&raw);
    format!("{CANARY_PREFIX}{encoded}")
}

/// Substring search. Zero false negatives on byte-for-byte match;
/// a tool that base64-encodes our canary before emitting it still
/// escapes â€” that's fine, we rely on volume of canaries + log
/// searches to cover encoding games.
pub fn scan_for_canary(haystack: &[u8], token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let needle = token.as_bytes();
    if haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_prefixed_tokens() {
        let t = generate_canary_token();
        assert!(t.starts_with(CANARY_PREFIX));
        assert!(t.len() > CANARY_PREFIX.len());
    }

    #[test]
    fn each_token_is_unique() {
        let a = generate_canary_token();
        let b = generate_canary_token();
        assert_ne!(a, b);
    }

    #[test]
    fn scan_finds_embedded_token() {
        let token = generate_canary_token();
        let stream = format!("pre-amble {token} post-amble");
        assert!(scan_for_canary(stream.as_bytes(), &token));
    }

    #[test]
    fn scan_does_not_false_positive_on_unrelated_content() {
        let token = generate_canary_token();
        let noise = b"this is some innocent output with no canaries in it";
        assert!(!scan_for_canary(noise, &token));
    }

    #[test]
    fn scan_rejects_empty_needle() {
        assert!(!scan_for_canary(b"anything", ""));
    }
}
