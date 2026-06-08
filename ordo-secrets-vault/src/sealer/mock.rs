//! Explicit mock sealer for tests and CI. Deliberately
//! distinguishable in logs â€” its tier is `MockForTests` so an
//! operator scanning logs can never confuse it with a real
//! sealer. Wrapping is a trivial XOR against a fixed key;
//! unwrapping is the inverse. Do not ship this in production.

use async_trait::async_trait;
use ordo_protocol::SealingTier;

use crate::bytes::SecureBytes;
use crate::sealer::{Sealer, SealerError, SealerResult};

const MOCK_KEY: [u8; 32] = *b"MOCK-FOR-TESTS-DO-NOT-USE-IN-PRD";

pub struct MockSealer;

#[async_trait]
impl Sealer for MockSealer {
    fn tier(&self) -> SealingTier {
        SealingTier::MockForTests
    }

    fn label(&self) -> &str {
        "mock-for-tests"
    }

    async fn wrap(&self, plaintext_key: &SecureBytes) -> SealerResult<Vec<u8>> {
        if plaintext_key.len() != 32 {
            return Err(SealerError::Crypto(
                "mock wrap: DEK must be 32 bytes".into(),
            ));
        }
        let mut out = vec![0u8; 32];
        for i in 0..32 {
            out[i] = plaintext_key.as_slice()[i] ^ MOCK_KEY[i];
        }
        Ok(out)
    }

    async fn unwrap(&self, sealed: &[u8]) -> SealerResult<SecureBytes> {
        if sealed.len() != 32 {
            return Err(SealerError::Crypto(
                "mock unwrap: blob must be 32 bytes".into(),
            ));
        }
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = sealed[i] ^ MOCK_KEY[i];
        }
        Ok(SecureBytes::from_slice(&out))
    }

    async fn probe(&self) -> SealerResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tier_is_mock_and_cannot_sign_anchors() {
        assert_eq!(MockSealer.tier(), SealingTier::MockForTests);
        assert!(!MockSealer.tier().can_sign_transparency_anchors());
    }

    #[tokio::test]
    async fn round_trip() {
        let dek = SecureBytes::from_slice(&[9u8; 32]);
        let sealed = MockSealer.wrap(&dek).await.unwrap();
        let opened = MockSealer.unwrap(&sealed).await.unwrap();
        assert_eq!(opened.as_slice(), &[9u8; 32]);
    }
}
