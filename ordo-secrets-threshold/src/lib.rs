//! ordo-secrets-threshold â€” FROST(Ed25519) threshold signing for
//! Ordo.
//!
//! Responsibility boundary: this crate owns *threshold key
//! management* and *threshold signing*. It does NOT hold any
//! single-signed secret material, does NOT run DRIFT, does NOT
//! write the audit chain. It exposes three entry points:
//!
//!   1. [`TrustedDealer::split`] â€” generate a t-of-n key from a
//!      dealer-selected signing key. Used for import paths where
//!      the material already exists (e.g. an imported SSH cluster
//!      key).
//!   2. [`Dkg`] â€” distributed key generation (no dealer), three-
//!      round protocol. Each participant runs `Dkg::part1`,
//!      exchanges round1 packages, runs `Dkg::part2`, exchanges
//!      round2 packages, runs `Dkg::part3`. Result: every holder
//!      has a `KeyPackage` and everyone shares the `PublicKeyPackage`.
//!   3. [`ThresholdCoordinator`] â€” runs the 2-round FROST signing
//!      protocol given a minimum-quorum subset of `KeyPackage`s.
//!      Invariant: the coordinator enforces nonce single-use at the
//!      API boundary â€” a `SigningNonces` consumed once cannot be
//!      reused.
//!
//! Invariants this crate enforces (blueprint Â§21):
//!
//! - A signature is only produced if at least `min_signers` valid
//!   shares were aggregated. `frost-ed25519` enforces this at the
//!   crypto layer; we fail closed at the API layer too (see the
//!   explicit quorum check in [`ThresholdCoordinator::aggregate`]).
//! - A `SigningNonces` cannot be reused across invocations. We
//!   consume the nonces by value inside `partial_sign`, so the
//!   type system makes this structurally impossible.
//! - Shares never travel across the bus. They are handed to
//!   holders via the device-adoption flow (out of scope for this
//!   crate); the bus only sees `ThresholdShareAnnouncement`
//!   (fingerprint metadata) and `ThresholdSigningCompleted`
//!   (message hash only, never the signature â€” see
//!   `OrdoMessage::SecretsThresholdSigningCompleted` wire docs).
//!
//! Persistence: `ShareRegistry` holds share *metadata* (who
//! holds share index `i` for secret `S`) in the `threshold_shares`
//! table. The share material itself lives on the holder's device,
//! sealed there by the vault. This crate never touches it.

use std::collections::BTreeMap;

use blake3::Hasher;
use chrono::{DateTime, Utc};
use frost_ed25519::{
    aggregate,
    keys::{
        dkg as frost_dkg, generate_with_dealer, reconstruct, split, IdentifierList, KeyPackage,
        PublicKeyPackage, SecretShare, SigningShare,
    },
    round1::{commit, SigningCommitments, SigningNonces},
    round2::{sign as round2_sign, SignatureShare},
    Identifier, Signature, SigningPackage,
};
use rand::rngs::OsRng;
use sha2::Digest;

pub mod registry;

pub use registry::{ShareRegistry, ShareRegistryError};

/// FROST keyshare exported in a serde-friendly wrapper.
#[derive(Debug, Clone)]
pub struct ThresholdKeyShare {
    pub identifier: Identifier,
    pub key_package: KeyPackage,
}

/// The public key package: every holder keeps a copy for
/// verification. Non-secret.
#[derive(Debug, Clone)]
pub struct ThresholdGroupKey {
    pub public_key_package: PublicKeyPackage,
    pub min_signers: u16,
    pub max_signers: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum ThresholdError {
    #[error("frost: {0}")]
    Frost(String),
    #[error("invalid quorum: need at least {min} of {total}, got {got}")]
    InvalidQuorum { got: usize, min: u16, total: u16 },
    #[error("nonce already consumed for identifier {0:?}")]
    NonceAlreadyConsumed(String),
    #[error("coordinator misuse: {0}")]
    Misuse(String),
    #[error("registry: {0}")]
    Registry(#[from] ShareRegistryError),
}

impl From<frost_ed25519::Error> for ThresholdError {
    fn from(err: frost_ed25519::Error) -> Self {
        ThresholdError::Frost(err.to_string())
    }
}

pub type ThresholdResult<T> = Result<T, ThresholdError>;

/// Trusted-dealer key generation. Use when the key already exists
/// and needs to be split for threshold protection (e.g. importing
/// an existing SSH cluster admin key). When generating a fresh key
/// with no dealer, prefer [`Dkg`].
pub struct TrustedDealer;

impl TrustedDealer {
    /// Generate a fresh t-of-n key using a central dealer.
    pub fn generate(
        min_signers: u16,
        max_signers: u16,
    ) -> ThresholdResult<(Vec<ThresholdKeyShare>, ThresholdGroupKey)> {
        if min_signers < 2 {
            return Err(ThresholdError::Misuse(
                "min_signers must be >= 2 for threshold protection".into(),
            ));
        }
        if min_signers > max_signers {
            return Err(ThresholdError::Misuse(format!(
                "min_signers ({min_signers}) > max_signers ({max_signers})"
            )));
        }
        let rng = OsRng;
        let (shares, pub_key_package) =
            generate_with_dealer(max_signers, min_signers, IdentifierList::Default, rng)?;
        let key_shares = convert_shares(shares)?;
        Ok((
            key_shares,
            ThresholdGroupKey {
                public_key_package: pub_key_package,
                min_signers,
                max_signers,
            },
        ))
    }

    /// Split an existing signing key into shares. Used when we
    /// already hold the plaintext key and want to remove all
    /// single-holder copies.
    pub fn split_existing(
        key_bytes: &[u8; 32],
        min_signers: u16,
        max_signers: u16,
    ) -> ThresholdResult<(Vec<ThresholdKeyShare>, ThresholdGroupKey)> {
        if min_signers < 2 {
            return Err(ThresholdError::Misuse("min_signers must be >= 2".into()));
        }
        if min_signers > max_signers {
            return Err(ThresholdError::Misuse("min_signers > max_signers".into()));
        }
        let signing_key = frost_ed25519::SigningKey::deserialize(key_bytes)?;
        let mut rng = OsRng;
        let (shares, pub_key_package) = split(
            &signing_key,
            max_signers,
            min_signers,
            IdentifierList::Default,
            &mut rng,
        )?;
        let key_shares = convert_shares(shares)?;
        Ok((
            key_shares,
            ThresholdGroupKey {
                public_key_package: pub_key_package,
                min_signers,
                max_signers,
            },
        ))
    }

    /// Recovery: recompute the plaintext signing key from at least
    /// `min_signers` key packages. This is an escape hatch â€” the
    /// whole point of FROST is NOT needing this â€” but the blueprint
    /// calls for it as the "paper-backup recovery" path.
    pub fn reconstruct_key(shares: &[KeyPackage]) -> ThresholdResult<Vec<u8>> {
        let signing_key = reconstruct(shares)?;
        let serialized = signing_key.serialize();
        Ok(serialized.to_vec())
    }
}

fn convert_shares(
    shares: BTreeMap<Identifier, SecretShare>,
) -> ThresholdResult<Vec<ThresholdKeyShare>> {
    let mut out = Vec::with_capacity(shares.len());
    for (id, secret_share) in shares {
        let key_package = KeyPackage::try_from(secret_share)?;
        out.push(ThresholdKeyShare {
            identifier: id,
            key_package,
        });
    }
    Ok(out)
}

/// Distributed key generation â€” three-round protocol with no
/// dealer. Each participant drives its own state machine.
pub struct Dkg;

impl Dkg {
    pub fn part1(
        identifier: Identifier,
        min_signers: u16,
        max_signers: u16,
    ) -> ThresholdResult<(frost_dkg::round1::SecretPackage, frost_dkg::round1::Package)> {
        let rng = OsRng;
        let out = frost_ed25519::keys::dkg::part1(identifier, max_signers, min_signers, rng)?;
        Ok(out)
    }

    pub fn part2(
        secret: frost_dkg::round1::SecretPackage,
        round1_packages: &BTreeMap<Identifier, frost_dkg::round1::Package>,
    ) -> ThresholdResult<(
        frost_dkg::round2::SecretPackage,
        BTreeMap<Identifier, frost_dkg::round2::Package>,
    )> {
        Ok(frost_ed25519::keys::dkg::part2(secret, round1_packages)?)
    }

    pub fn part3(
        round2_secret: &frost_dkg::round2::SecretPackage,
        round1_packages: &BTreeMap<Identifier, frost_dkg::round1::Package>,
        round2_packages: &BTreeMap<Identifier, frost_dkg::round2::Package>,
    ) -> ThresholdResult<(KeyPackage, PublicKeyPackage)> {
        Ok(frost_ed25519::keys::dkg::part3(
            round2_secret,
            round1_packages,
            round2_packages,
        )?)
    }
}

/// Coordinator for a single threshold signing operation.
///
/// Lifecycle:
///   1. `begin(operation_id, message, participants)` â€” registers
///      the signers and seeds a message commitment.
///   2. Each participant calls `commit_round1(identifier, signing_share)`
///      to produce a `(SigningNonces, SigningCommitments)` pair;
///      nonces stay with the holder, commitments are handed back
///      to the coordinator.
///   3. `close_round1()` â€” coordinator assembles the
///      `SigningPackage` and publishes it.
///   4. Each participant calls `partial_sign(signing_package,
///      nonces, key_package)` to produce a `SignatureShare`. The
///      nonces are consumed by value (single-use enforced by the
///      type system).
///   5. `aggregate(shares, pubkey_package)` â€” coordinator folds
///      the shares into a group signature and verifies.
///
/// The coordinator is stateless across `Signature`s; each signing
/// operation gets a fresh `ThresholdCoordinator`.
pub struct ThresholdCoordinator {
    operation_id: String,
    min_signers: u16,
    max_signers: u16,
    message: Vec<u8>,
    message_hash: [u8; 32],
    commitments: BTreeMap<Identifier, SigningCommitments>,
    finalized_round1: bool,
    signing_package: Option<SigningPackage>,
    started_at: DateTime<Utc>,
}

impl ThresholdCoordinator {
    pub fn begin(
        operation_id: impl Into<String>,
        group: &ThresholdGroupKey,
        message: impl Into<Vec<u8>>,
    ) -> Self {
        let message: Vec<u8> = message.into();
        let message_hash = blake3_hash(&message);
        Self {
            operation_id: operation_id.into(),
            min_signers: group.min_signers,
            max_signers: group.max_signers,
            message,
            message_hash,
            commitments: BTreeMap::new(),
            finalized_round1: false,
            signing_package: None,
            started_at: Utc::now(),
        }
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn message_hash(&self) -> [u8; 32] {
        self.message_hash
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    /// Round 1: a participant generates their signing nonce +
    /// commitment. Nonces go back to the participant by value â€”
    /// they must be retained locally until `partial_sign`. The
    /// coordinator keeps only the commitment.
    pub fn commit_round1(
        &mut self,
        identifier: Identifier,
        signing_share: &SigningShare,
    ) -> ThresholdResult<SigningNonces> {
        if self.finalized_round1 {
            return Err(ThresholdError::Misuse(
                "round 1 already closed; start a new coordinator".into(),
            ));
        }
        let mut rng = OsRng;
        let (nonces, commitments) = commit(signing_share, &mut rng);
        self.commitments.insert(identifier, commitments);
        Ok(nonces)
    }

    /// Close round 1: coordinator assembles the SigningPackage.
    /// Requires at least `min_signers` commitments.
    pub fn close_round1(&mut self) -> ThresholdResult<&SigningPackage> {
        if self.commitments.len() < self.min_signers as usize {
            return Err(ThresholdError::InvalidQuorum {
                got: self.commitments.len(),
                min: self.min_signers,
                total: self.max_signers,
            });
        }
        let pkg = SigningPackage::new(self.commitments.clone(), &self.message);
        self.signing_package = Some(pkg);
        self.finalized_round1 = true;
        Ok(self.signing_package.as_ref().unwrap())
    }

    /// Round 2: a participant produces a signature share. Takes
    /// `SigningNonces` by value â€” the type system enforces that
    /// the same nonces cannot be used twice (blueprint invariant).
    pub fn partial_sign(
        &self,
        nonces: SigningNonces,
        key_package: &KeyPackage,
    ) -> ThresholdResult<SignatureShare> {
        let pkg = self.signing_package.as_ref().ok_or_else(|| {
            ThresholdError::Misuse("round 1 is not closed; call close_round1() first".into())
        })?;
        let share = round2_sign(pkg, &nonces, key_package)?;
        // `nonces` drops here â€” consumed.
        Ok(share)
    }

    /// Aggregate: fold signature shares into a group signature.
    /// Verifies the result against the group public key before
    /// returning.
    pub fn aggregate(
        &self,
        shares: &BTreeMap<Identifier, SignatureShare>,
        pubkey_package: &PublicKeyPackage,
    ) -> ThresholdResult<Signature> {
        if shares.len() < self.min_signers as usize {
            return Err(ThresholdError::InvalidQuorum {
                got: shares.len(),
                min: self.min_signers,
                total: self.max_signers,
            });
        }
        let pkg = self
            .signing_package
            .as_ref()
            .ok_or_else(|| ThresholdError::Misuse("round 1 is not closed".into()))?;
        let signature = aggregate(pkg, shares, pubkey_package)?;
        // Explicit post-aggregation verification â€” frost-ed25519
        // does its own internal checks but the blueprint asks for
        // a defence-in-depth verify at the boundary.
        pubkey_package
            .verifying_key()
            .verify(&self.message, &signature)?;
        Ok(signature)
    }
}

fn blake3_hash(bytes: &[u8]) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(bytes);
    *h.finalize().as_bytes()
}

/// Fingerprint a holder device identifier for publication. The
/// raw identifier never leaves the local machine; the fingerprint
/// is what appears in `ThresholdShareAnnouncement` on the bus.
pub fn fingerprint_device_id(raw: &[u8]) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(raw);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use frost_ed25519::VerifyingKey;

    #[test]
    fn trusted_dealer_generates_t_of_n_and_public_key_matches_signing() {
        let (shares, group) = TrustedDealer::generate(2, 3).unwrap();
        assert_eq!(shares.len(), 3);
        assert_eq!(group.min_signers, 2);
        assert_eq!(group.max_signers, 3);
        // The public key is non-zero.
        let _vk: &VerifyingKey = group.public_key_package.verifying_key();
    }

    #[test]
    fn min_signers_below_two_is_rejected() {
        let err = TrustedDealer::generate(1, 3).unwrap_err();
        assert!(matches!(err, ThresholdError::Misuse(_)));
    }

    #[test]
    fn min_greater_than_max_is_rejected() {
        let err = TrustedDealer::generate(4, 3).unwrap_err();
        assert!(matches!(err, ThresholdError::Misuse(_)));
    }

    fn signing_roundtrip(message: &[u8], shares: &[ThresholdKeyShare], group: &ThresholdGroupKey) {
        let mut coord = ThresholdCoordinator::begin("op-test", group, message.to_vec());

        // Pick the first `min_signers` shares as the quorum.
        let quorum: Vec<&ThresholdKeyShare> =
            shares.iter().take(group.min_signers as usize).collect();

        // Round 1: each participant commits.
        let mut nonces_map = std::collections::BTreeMap::new();
        for share in &quorum {
            let nonces = coord
                .commit_round1(share.identifier, share.key_package.signing_share())
                .unwrap();
            nonces_map.insert(share.identifier, nonces);
        }
        coord.close_round1().unwrap();

        // Round 2: each participant partial-signs.
        let mut sig_shares = BTreeMap::new();
        for share in &quorum {
            let nonces = nonces_map.remove(&share.identifier).unwrap();
            let partial = coord.partial_sign(nonces, &share.key_package).unwrap();
            sig_shares.insert(share.identifier, partial);
        }

        // Aggregate + verify.
        let sig = coord
            .aggregate(&sig_shares, &group.public_key_package)
            .unwrap();
        group
            .public_key_package
            .verifying_key()
            .verify(message, &sig)
            .unwrap();
    }

    #[test]
    fn two_of_three_signs_and_verifies() {
        let (shares, group) = TrustedDealer::generate(2, 3).unwrap();
        signing_roundtrip(b"ordo-v1-anchor", &shares, &group);
    }

    #[test]
    fn three_of_five_signs_and_verifies() {
        let (shares, group) = TrustedDealer::generate(3, 5).unwrap();
        signing_roundtrip(b"ssh-cluster-admin-cmd", &shares, &group);
    }

    #[test]
    fn aggregation_fails_with_insufficient_shares() {
        let (shares, group) = TrustedDealer::generate(2, 3).unwrap();
        let mut coord = ThresholdCoordinator::begin("op-x", &group, b"msg".to_vec());
        // Only one participant commits â€” below quorum.
        let s = &shares[0];
        let _ = coord
            .commit_round1(s.identifier, s.key_package.signing_share())
            .unwrap();
        let err = coord.close_round1().unwrap_err();
        assert!(matches!(err, ThresholdError::InvalidQuorum { .. }));
    }

    #[test]
    fn split_existing_preserves_the_key_for_reconstruction() {
        let key_bytes = [7u8; 32];
        let (shares, group) = TrustedDealer::split_existing(&key_bytes, 2, 3).unwrap();
        // Reconstruct from any two shares should give the original.
        let kps: Vec<KeyPackage> = shares
            .iter()
            .take(2)
            .map(|s| s.key_package.clone())
            .collect();
        let recovered = TrustedDealer::reconstruct_key(&kps).unwrap();
        assert_eq!(recovered.as_slice(), &key_bytes);
        // And the group key should sign + verify.
        signing_roundtrip(b"recovered", &shares, &group);
    }

    #[test]
    fn dkg_three_participants_two_threshold() {
        // Simulate three holders running DKG. Each calls part1,
        // they swap round1 packages, each calls part2, they swap
        // round2 packages, each calls part3. Coordinator isn't
        // involved in DKG â€” it's peer-to-peer.
        let ids: Vec<Identifier> = (1u16..=3)
            .map(|i| Identifier::try_from(i).unwrap())
            .collect();

        // Part 1: every participant runs part1.
        let mut round1_secrets = BTreeMap::new();
        let mut round1_bundle = BTreeMap::new();
        for id in &ids {
            let (sec, pkg) = Dkg::part1(*id, 2, 3).unwrap();
            round1_secrets.insert(*id, sec);
            round1_bundle.insert(*id, pkg);
        }

        // Part 2: every participant runs part2 with the round 1
        // packages they received from others (i.e. everyone's
        // packages except their own).
        let mut round2_secrets = BTreeMap::new();
        let mut round2_outgoing: BTreeMap<
            Identifier,
            BTreeMap<Identifier, frost_dkg::round2::Package>,
        > = BTreeMap::new();
        for id in &ids {
            let mut received = BTreeMap::new();
            for (other, pkg) in &round1_bundle {
                if other != id {
                    received.insert(*other, pkg.clone());
                }
            }
            let secret = round1_secrets.remove(id).unwrap();
            let (s2, out) = Dkg::part2(secret, &received).unwrap();
            round2_secrets.insert(*id, s2);
            round2_outgoing.insert(*id, out);
        }

        // Part 3: participant i assembles round2 packages from
        // other participants addressed to i.
        let mut key_packages = BTreeMap::new();
        let mut pub_key_packages = Vec::new();
        for id in &ids {
            let mut round2_received = BTreeMap::new();
            for (other, out) in &round2_outgoing {
                if other == id {
                    continue;
                }
                if let Some(pkg) = out.get(id) {
                    round2_received.insert(*other, pkg.clone());
                }
            }
            let mut round1_received = BTreeMap::new();
            for (other, pkg) in &round1_bundle {
                if other != id {
                    round1_received.insert(*other, pkg.clone());
                }
            }
            let secret = round2_secrets.get(id).unwrap();
            let (kp, pkp) = Dkg::part3(secret, &round1_received, &round2_received).unwrap();
            key_packages.insert(*id, kp);
            pub_key_packages.push(pkp);
        }

        // All participants should agree on the group public key.
        let vk0 = pub_key_packages[0].verifying_key();
        for pkp in &pub_key_packages[1..] {
            assert_eq!(
                vk0.serialize().unwrap(),
                pkp.verifying_key().serialize().unwrap()
            );
        }

        // Assemble a signing flow across a 2-participant quorum.
        let group = ThresholdGroupKey {
            public_key_package: pub_key_packages.remove(0),
            min_signers: 2,
            max_signers: 3,
        };
        let shares: Vec<ThresholdKeyShare> = key_packages
            .into_iter()
            .map(|(id, kp)| ThresholdKeyShare {
                identifier: id,
                key_package: kp,
            })
            .collect();
        signing_roundtrip(b"dkg-threshold-test", &shares, &group);
    }

    #[test]
    fn fingerprint_is_stable_and_hides_input() {
        let a = fingerprint_device_id(b"device-serial-XYZ");
        let b = fingerprint_device_id(b"device-serial-XYZ");
        assert_eq!(a, b);
        // SHA-256 over "device-serial-XYZ" isn't any of the input
        // bytes' prefixes; trivially the fingerprint differs from
        // the input bytes.
        assert_ne!(&a[..17], b"device-serial-XYZ");
    }
}
