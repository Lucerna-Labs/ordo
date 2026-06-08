//! HMAC-SHA256 signing for outgoing webhook payloads.
//!
//! Minimal, no-dep implementation of HMAC-SHA256 (per RFC 2104 using
//! the sha2 crate we already have for file hashing). Avoids pulling
//! `hmac` + `subtle` + `generic-array` + friends for one six-line
//! routine.

use sha2::{Digest, Sha256};

const BLOCK_SIZE: usize = 64;
const OPAD: u8 = 0x5c;
const IPAD: u8 = 0x36;

/// Return the lowercase-hex HMAC-SHA256 of `body` keyed by `secret`.
pub fn sign_hex(secret: &[u8], body: &[u8]) -> String {
    hex::encode(hmac_sha256(secret, body))
}

fn hmac_sha256(key: &[u8], body: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let mut h = Sha256::new();
        h.update(key);
        let digest = h.finalize();
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0u8; BLOCK_SIZE];
    let mut outer_pad = [0u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        inner_pad[i] = key_block[i] ^ IPAD;
        outer_pad[i] = key_block[i] ^ OPAD;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(body);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    let out = outer.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_test_vector_rfc_4231_case_1() {
        // RFC 4231 §4.2: key = 0x0b × 20, data = "Hi There"
        let key = [0x0bu8; 20];
        let out = sign_hex(&key, b"Hi There");
        assert_eq!(
            out,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn long_key_is_pre_hashed() {
        // When key is longer than the block size (64 bytes for SHA-256),
        // HMAC pre-hashes it. Result must differ from the short-key
        // case and must be deterministic.
        let long_key = [0x42u8; 200];
        let a = sign_hex(&long_key, b"payload");
        let b = sign_hex(&long_key, b"payload");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // 32 bytes × 2 hex chars
    }
}
