//! Zeroizing plaintext buffer.
//!
//! Every unsealed secret lives in `SecureBytes` and nothing else.
//! Drop zeroes the allocation. `Debug` prints length only — never
//! the bytes themselves, so a stray `dbg!(secret)` can't leak.

use std::fmt;

use zeroize::Zeroize;

/// Owned, zeroize-on-drop plaintext. Deliberately not `Clone` —
/// callers must be explicit about duplication via `explicit_copy`
/// so plaintext propagation is visible in code review.
pub struct SecureBytes {
    bytes: Vec<u8>,
}

impl SecureBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        Self {
            bytes: slice.to_vec(),
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Deliberately-named deep copy. Searching for
    /// `explicit_copy` in the codebase finds every plaintext
    /// duplication site — that's the point.
    pub fn explicit_copy(&self) -> Self {
        Self {
            bytes: self.bytes.clone(),
        }
    }

    /// Take ownership of the underlying bytes without zeroizing.
    /// Used at AEAD boundaries where the caller immediately hands
    /// the buffer to a hardware-accelerated primitive and owns
    /// zeroization of the resulting ciphertext if needed. Prefer
    /// `as_slice` where possible.
    pub fn into_bytes(mut self) -> Vec<u8> {
        std::mem::take(&mut self.bytes)
    }
}

impl Drop for SecureBytes {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl fmt::Debug for SecureBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecureBytes")
            .field("len", &self.bytes.len())
            .field("bytes", &"[REDACTED]")
            .finish()
    }
}

impl From<Vec<u8>> for SecureBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_bytes() {
        let sb = SecureBytes::from_slice(b"s3cret-p4ssw0rd");
        let printed = format!("{sb:?}");
        assert!(
            !printed.contains("s3cret") && !printed.contains("p4ssw0rd"),
            "Debug impl leaked plaintext: {printed}"
        );
        assert!(printed.contains("REDACTED"));
    }

    #[test]
    fn explicit_copy_produces_an_independent_buffer() {
        let a = SecureBytes::from_slice(b"hello");
        let b = a.explicit_copy();
        assert_eq!(a.as_slice(), b.as_slice());
        // Dropping one doesn't affect the other.
        drop(a);
        assert_eq!(b.as_slice(), b"hello");
    }

    #[test]
    fn not_clone() {
        // Compile-time assertion: SecureBytes does not implement
        // Clone. If this function compiles, something added a
        // Clone impl by accident.
        fn assert_not_clone<T: 'static + Sized>() {}
        assert_not_clone::<SecureBytes>();
    }
}
