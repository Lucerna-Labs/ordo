use ordo_protocol::{
    CryptoSuite, HandshakeSelection, NatKind, PairingMode, PeerHello, PeerPresence, TransportKind,
    TrustTier,
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NegotiationError {
    #[error("protocol versions do not match: local={local}, remote={remote}")]
    ProtocolVersionMismatch { local: String, remote: String },
    #[error("no compatible transport between peers")]
    NoCompatibleTransport,
    #[error("no compatible crypto suite for selected transport")]
    NoCompatibleCryptoSuite,
}

pub fn build_hello(peer: PeerPresence) -> PeerHello {
    PeerHello { peer }
}

pub fn negotiate_handshake(
    local: &PeerHello,
    remote: &PeerHello,
    preferred_transport: TransportKind,
) -> Result<HandshakeSelection, NegotiationError> {
    if local.peer.protocol_version != remote.peer.protocol_version {
        return Err(NegotiationError::ProtocolVersionMismatch {
            local: local.peer.protocol_version.clone(),
            remote: remote.peer.protocol_version.clone(),
        });
    }

    let transport = select_transport(local, remote, preferred_transport)?;
    let crypto_suite = select_crypto_suite(local, remote, &transport)?;
    let relay_required = matches!(transport, TransportKind::RelayQuic)
        || matches!(
            remote.peer.nat_kind,
            NatKind::Symmetric | NatKind::RelayOnly
        );
    let pairing_required = requires_pairing(local, remote);

    Ok(HandshakeSelection {
        transport,
        crypto_suite,
        relay_required,
        pairing_required,
    })
}

fn select_transport(
    local: &PeerHello,
    remote: &PeerHello,
    preferred_transport: TransportKind,
) -> Result<TransportKind, NegotiationError> {
    if transport_supported(local, remote, &preferred_transport) {
        return Ok(preferred_transport);
    }

    let fallbacks = [
        TransportKind::Quic,
        TransportKind::TcpNoise,
        TransportKind::RelayQuic,
        TransportKind::InProcess,
    ];

    fallbacks
        .into_iter()
        .find(|candidate| transport_supported(local, remote, candidate))
        .ok_or(NegotiationError::NoCompatibleTransport)
}

fn select_crypto_suite(
    local: &PeerHello,
    remote: &PeerHello,
    transport: &TransportKind,
) -> Result<CryptoSuite, NegotiationError> {
    if matches!(transport, TransportKind::InProcess) {
        return Ok(CryptoSuite::InProcess);
    }

    let candidates = [CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519];

    candidates
        .into_iter()
        .find(|candidate| {
            local
                .peer
                .crypto_suites
                .iter()
                .any(|suite| suite == candidate)
                && remote
                    .peer
                    .crypto_suites
                    .iter()
                    .any(|suite| suite == candidate)
        })
        .ok_or(NegotiationError::NoCompatibleCryptoSuite)
}

fn transport_supported(local: &PeerHello, remote: &PeerHello, transport: &TransportKind) -> bool {
    local
        .peer
        .transports
        .iter()
        .any(|candidate| candidate == transport)
        && remote
            .peer
            .transports
            .iter()
            .any(|candidate| candidate == transport)
}

fn requires_pairing(local: &PeerHello, remote: &PeerHello) -> bool {
    if matches!(local.peer.trust_tier, TrustTier::LocalProcess)
        && matches!(remote.peer.trust_tier, TrustTier::LocalProcess)
    {
        return false;
    }

    matches!(
        (
            local.peer.pairing_mode.clone(),
            remote.peer.pairing_mode.clone()
        ),
        (PairingMode::PairingRequired, _)
            | (_, PairingMode::PairingRequired)
            | (PairingMode::TrustedOnly, _)
            | (_, PairingMode::TrustedOnly)
    ) || matches!(remote.peer.trust_tier, TrustTier::UnknownPeer)
        || matches!(local.peer.trust_tier, TrustTier::UnknownPeer)
}

#[cfg(test)]
mod tests {
    use ordo_protocol::{NodeId, PairingMode};

    use super::{build_hello, negotiate_handshake, NegotiationError};
    use ordo_protocol::{CryptoSuite, NatKind, PeerPresence, TransportKind, TrustTier};

    fn peer(
        trust_tier: TrustTier,
        pairing_mode: PairingMode,
        nat_kind: NatKind,
        transports: Vec<TransportKind>,
        crypto_suites: Vec<CryptoSuite>,
    ) -> PeerPresence {
        PeerPresence {
            id: NodeId::new(),
            label: "peer".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier,
            pairing_mode,
            nat_kind,
            transports,
            crypto_suites,
            endpoints: vec!["quic://peer".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        }
    }

    #[test]
    fn hybrid_pq_is_preferred_when_both_peers_support_it() {
        let local = build_hello(peer(
            TrustTier::PairedPeer,
            PairingMode::PairingRequired,
            NatKind::Cone,
            vec![TransportKind::Quic, TransportKind::RelayQuic],
            vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
        ));
        let remote = build_hello(peer(
            TrustTier::PairedPeer,
            PairingMode::PairingRequired,
            NatKind::Cone,
            vec![TransportKind::Quic],
            vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
        ));

        let selection =
            negotiate_handshake(&local, &remote, TransportKind::Quic).expect("handshake");

        assert_eq!(selection.transport, TransportKind::Quic);
        assert_eq!(selection.crypto_suite, CryptoSuite::HybridPqNoiseX25519);
        assert!(selection.pairing_required);
    }

    #[test]
    fn falls_back_to_noise_when_remote_lacks_pq_suite() {
        let local = build_hello(peer(
            TrustTier::PairedPeer,
            PairingMode::PairingRequired,
            NatKind::Cone,
            vec![TransportKind::Quic],
            vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
        ));
        let remote = build_hello(peer(
            TrustTier::PairedPeer,
            PairingMode::PairingRequired,
            NatKind::Cone,
            vec![TransportKind::Quic],
            vec![CryptoSuite::NoiseX25519],
        ));

        let selection =
            negotiate_handshake(&local, &remote, TransportKind::Quic).expect("handshake");

        assert_eq!(selection.crypto_suite, CryptoSuite::NoiseX25519);
    }

    #[test]
    fn symmetric_nat_can_force_relay_fallback() {
        let local = build_hello(peer(
            TrustTier::PairedPeer,
            PairingMode::PairingRequired,
            NatKind::Cone,
            vec![TransportKind::Quic, TransportKind::RelayQuic],
            vec![CryptoSuite::HybridPqNoiseX25519],
        ));
        let remote = build_hello(peer(
            TrustTier::UnknownPeer,
            PairingMode::PairingRequired,
            NatKind::Symmetric,
            vec![TransportKind::RelayQuic],
            vec![CryptoSuite::HybridPqNoiseX25519],
        ));

        let selection =
            negotiate_handshake(&local, &remote, TransportKind::Quic).expect("relay handshake");

        assert_eq!(selection.transport, TransportKind::RelayQuic);
        assert!(selection.relay_required);
        assert!(selection.pairing_required);
    }

    #[test]
    fn protocol_mismatch_is_rejected() {
        let local = build_hello(peer(
            TrustTier::PairedPeer,
            PairingMode::PairingRequired,
            NatKind::Cone,
            vec![TransportKind::Quic],
            vec![CryptoSuite::NoiseX25519],
        ));
        let mut remote_peer = peer(
            TrustTier::PairedPeer,
            PairingMode::PairingRequired,
            NatKind::Cone,
            vec![TransportKind::Quic],
            vec![CryptoSuite::NoiseX25519],
        );
        remote_peer.protocol_version = "ordo/0.2".to_string();
        let remote = build_hello(remote_peer);

        let error =
            negotiate_handshake(&local, &remote, TransportKind::Quic).expect_err("mismatch");

        assert_eq!(
            error,
            NegotiationError::ProtocolVersionMismatch {
                local: "ordo/0.1".to_string(),
                remote: "ordo/0.2".to_string(),
            }
        );
    }
}
