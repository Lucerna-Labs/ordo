use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ordo_protocol::{
    CryptoSuite, NatKind, NodeId, NodeStatus, PairingMode, PeerPresence, TransportKind, TrustTier,
};

#[derive(Debug, Clone)]
pub struct PeerRecord {
    pub peer: PeerPresence,
    pub last_seen: DateTime<Utc>,
    pub heartbeat_count: u64,
}

#[derive(Debug, Default)]
pub struct PeerDirectory {
    peers: HashMap<NodeId, PeerRecord>,
}

impl PeerDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, peer: PeerPresence) -> &PeerRecord {
        let id = peer.id.clone();
        let now = Utc::now();

        let entry = self.peers.entry(id).and_modify(|record| {
            record.peer = peer.clone();
            record.last_seen = now;
            record.heartbeat_count += 1;
        });

        entry.or_insert(PeerRecord {
            peer,
            last_seen: now,
            heartbeat_count: 1,
        })
    }

    pub fn observe_heartbeat(&mut self, status: NodeStatus) -> &PeerRecord {
        let peer = PeerPresence {
            id: status.id,
            label: status.name,
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::UnknownPeer,
            pairing_mode: PairingMode::PairingRequired,
            nat_kind: NatKind::Unknown,
            transports: vec![TransportKind::InProcess],
            crypto_suites: vec![CryptoSuite::InProcess],
            endpoints: Vec::new(),
            capabilities: status.capabilities,
        };

        self.observe(peer)
    }

    pub fn get(&self, id: &NodeId) -> Option<&PeerRecord> {
        self.peers.get(id)
    }

    pub fn snapshot(&self) -> Vec<PeerPresence> {
        self.peers
            .values()
            .map(|record| record.peer.clone())
            .collect()
    }

    pub fn peers_with_capability(&self, capability: &str) -> Vec<PeerPresence> {
        self.peers
            .values()
            .filter(|record| record.peer.capabilities.iter().any(|cap| cap == capability))
            .map(|record| record.peer.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::PeerDirectory;
    use ordo_protocol::{
        CryptoSuite, NatKind, NodeId, PairingMode, PeerPresence, TransportKind, TrustTier,
    };

    #[test]
    fn directory_indexes_capabilities() {
        let mut directory = PeerDirectory::new();
        directory.observe(PeerPresence {
            id: NodeId::new(),
            label: "peer-a".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::PairedPeer,
            pairing_mode: PairingMode::PairingRequired,
            nat_kind: NatKind::Cone,
            transports: vec![TransportKind::Quic],
            crypto_suites: vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
            endpoints: vec!["quic://peer-a".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        });

        let matches = directory.peers_with_capability("filesystem.read_file");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].label, "peer-a");
    }
}
