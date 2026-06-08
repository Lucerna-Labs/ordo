//! AEAD wrapping for sealed secret material.
//!
//! XChaCha20-Poly1305 with a random 24-byte nonce per secret.
//! Nonce is persisted alongside the ciphertext; key comes from
//! the active `Sealer` (which may derive it from a passphrase,
//! read it from the OS keychain, or unwrap it via the TPM).

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use zeroize::Zeroize;

use crate::bytes::SecureBytes;

#[derive(Debug, thiserror::Error)]
pub enum AeadError {
    #[error("key must be 32 bytes, got {0}")]
    BadKey(usize),
    #[error("seal failed: {0}")]
    Seal(String),
    #[error("open failed: {0}")]
    Open(String),
}

pub type AeadResult<T> = Result<T, AeadError>;

/// Result of a seal operation. `ciphertext` and `nonce` are
/// persisted together; `aad` is the authenticated additional data
/// and is also persisted so verification can reproduce it.
#[derive(Debug)]
pub struct SealedMaterial {
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 24],
    pub aad: Vec<u8>,
}

/// Encrypt `plaintext` under `key`, binding `aad`. Nonce is
/// freshly generated via `OsRng`.
pub fn seal(key: &[u8], plaintext: &SecureBytes, aad: &[u8]) -> AeadResult<SealedMaterial> {
    if key.len() != 32 {
        return Err(AeadError::BadKey(key.len()));
    }
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(
            &nonce,
            chacha20poly1305::aead::Payload {
                msg: plaintext.as_slice(),
                aad,
            },
        )
        .map_err(|err| AeadError::Seal(err.to_string()))?;

    let mut nonce_bytes = [0u8; 24];
    nonce_bytes.copy_from_slice(nonce.as_slice());

    Ok(SealedMaterial {
        ciphertext,
        nonce: nonce_bytes,
        aad: aad.to_vec(),
    })
}

/// Decrypt, returning the plaintext wrapped in `SecureBytes` so
/// it auto-zeroizes on drop. AAD mismatch fails with `Open` —
/// caller must reproduce the exact AAD used at seal time.
pub fn open(
    key: &[u8],
    ciphertext: &[u8],
    nonce: &[u8; 24],
    aad: &[u8],
) -> AeadResult<SecureBytes> {
    if key.len() != 32 {
        return Err(AeadError::BadKey(key.len()));
    }
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = XNonce::from_slice(nonce);

    let mut plaintext = cipher
        .decrypt(
            nonce,
            chacha20poly1305::aead::Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|err| AeadError::Open(err.to_string()))?;

    // `chacha20poly1305::aead::Vec` returns a fresh Vec; wrap it
    // into SecureBytes so it zeroizes on drop.
    let secure = SecureBytes::from_slice(&plaintext);
    plaintext.zeroize();
    Ok(secure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_aad() {
        let key = [7u8; 32];
        let pt = SecureBytes::from_slice(b"hello, world");
        let aad = b"secret_id=01ARZ3";
        let sealed = seal(&key, &pt, aad).unwrap();
        let opened = open(&key, &sealed.ciphertext, &sealed.nonce, aad).unwrap();
        assert_eq!(opened.as_slice(), b"hello, world");
    }

    #[test]
    fn aad_tamper_fails_open() {
        let key = [7u8; 32];
        let pt = SecureBytes::from_slice(b"x");
        let sealed = seal(&key, &pt, b"correct").unwrap();
        let err = open(&key, &sealed.ciphertext, &sealed.nonce, b"wrong").unwrap_err();
        assert!(matches!(err, AeadError::Open(_)));
    }

    #[test]
    fn key_tamper_fails_open() {
        let key = [7u8; 32];
        let pt = SecureBytes::from_slice(b"x");
        let sealed = seal(&key, &pt, b"aad").unwrap();
        let mut bad_key = key;
        bad_key[0] ^= 1;
        let err = open(&bad_key, &sealed.ciphertext, &sealed.nonce, b"aad").unwrap_err();
        assert!(matches!(err, AeadError::Open(_)));
    }

    #[test]
    fn nonce_is_random_per_seal() {
        let key = [1u8; 32];
        let pt = SecureBytes::from_slice(b"same");
        let a = seal(&key, &pt, b"").unwrap();
        let b = seal(&key, &pt, b"").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn wrong_key_length_rejected() {
        let pt = SecureBytes::from_slice(b"x");
        let err = seal(&[0u8; 16], &pt, b"").unwrap_err();
        assert!(matches!(err, AeadError::BadKey(16)));
    }
}
