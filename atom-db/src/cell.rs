use crate::{Bond, Digest, digest};
use std::collections::BTreeMap;

pub(crate) const ATOM_DOMAIN: &[u8] = b"atom-db/atom/v1\0";
pub(crate) const BOND_DOMAIN: &[u8] = b"atom-db/bond/v1\0";
pub(crate) const ROOT_DOMAIN: &[u8] = b"atom-db/root/v1\0";
pub(crate) const CELL_DOMAIN: &[u8] = b"atom-db/cell/v1\0";

pub(crate) fn atom_identity(bytes: &[u8]) -> Digest {
    digest(&[ATOM_DOMAIN, bytes])
}

pub(crate) fn bond_identity(bond: Bond) -> Digest {
    let mut payload = [0; 96];
    payload[..32].copy_from_slice(bond.source.as_bytes());
    payload[32..64].copy_from_slice(bond.relation.as_bytes());
    payload[64..].copy_from_slice(bond.target.as_bytes());
    digest(&[BOND_DOMAIN, &payload])
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Cell {
    pub(crate) atoms: BTreeMap<Digest, Vec<u8>>,
    pub(crate) bonds: BTreeMap<Digest, Bond>,
    pub(crate) roots: BTreeMap<Digest, Option<Digest>>,
}

impl Cell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put_atom(&mut self, bytes: impl AsRef<[u8]>) -> Digest {
        let bytes = bytes.as_ref();
        let identity = atom_identity(bytes);
        self.atoms.entry(identity).or_insert_with(|| bytes.to_vec());
        identity
    }

    pub fn put_bond(&mut self, bond: Bond) -> Digest {
        let identity = bond_identity(bond);
        self.bonds.entry(identity).or_insert(bond);
        identity
    }

    pub fn set_root(&mut self, name: Digest, target: Digest) {
        self.roots.insert(name, Some(target));
    }

    pub fn clear_root(&mut self, name: Digest) {
        self.roots.insert(name, None);
    }

    pub fn atom_count(&self) -> usize {
        self.atoms.len()
    }

    pub fn bond_count(&self) -> usize {
        self.bonds.len()
    }

    pub fn root_change_count(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty() && self.bonds.is_empty() && self.roots.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellReceipt {
    pub identity: Digest,
    pub committed: bool,
    pub first_sequence: u64,
    pub commit_sequence: u64,
    pub atoms_added: u64,
    pub bonds_added: u64,
    pub roots_changed: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootVersion {
    pub identity: Digest,
    pub sequence: u64,
    pub commit_sequence: u64,
    pub cell: Digest,
    pub previous_version: Option<Digest>,
    pub previous: Option<Digest>,
    pub target: Option<Digest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    sequence: u64,
    roots: BTreeMap<Digest, Digest>,
}

impl Snapshot {
    pub(crate) fn new(sequence: u64, roots: BTreeMap<Digest, Digest>) -> Self {
        Self { sequence, roots }
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn root(&self, name: Digest) -> Option<Digest> {
        self.roots.get(&name).copied()
    }

    pub fn roots(&self) -> Vec<(Digest, Digest)> {
        self.roots
            .iter()
            .map(|(name, target)| (*name, *target))
            .collect()
    }
}
