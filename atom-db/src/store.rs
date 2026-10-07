use crate::{
    Digest,
    cell::{
        CELL_DOMAIN, Cell, CellReceipt, ROOT_DOMAIN, RootVersion, Snapshot, atom_identity,
        bond_identity,
    },
    digest,
};
use std::{
    collections::BTreeMap,
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

const FILE_MAGIC: &[u8; 8] = b"ATOMDB\x01\n";
const FRAME_MAGIC: &[u8; 4] = b"ATOM";
const HEADER_LEN: usize = 88;
const MAX_PAYLOAD: u64 = 64 * 1024 * 1024;
const MAX_CELL_ITEMS: u64 = 1_000_000;
const ATOM_KIND: u8 = 1;
const BOND_KIND: u8 = 2;
const CELL_BEGIN_KIND: u8 = 3;
const ROOT_KIND: u8 = 4;
const CELL_COMMIT_KIND: u8 = 5;
const FRAME_DOMAIN: &[u8] = b"atom-db/frame/v1\0";

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Invalid(String),
    Corrupt { offset: u64, reason: String },
    Missing(Digest),
    Busy(PathBuf),
}

#[cfg(windows)]
fn open_store_file(path: &Path, access_mode: AccessMode) -> io::Result<File> {
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    let mut options = OpenOptions::new();
    options.read(true).truncate(false);
    match access_mode {
        AccessMode::Writer => {
            options.create(true).write(true).share_mode(FILE_SHARE_READ);
        }
        AccessMode::ReadOnly => {
            options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        }
    }
    options.open(path)
}

#[cfg(unix)]
fn open_store_file(path: &Path, access_mode: AccessMode) -> io::Result<File> {
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe extern "C" {
        fn flock(file_descriptor: i32, operation: i32) -> i32;
    }
    let mut options = OpenOptions::new();
    options.read(true).truncate(false);
    if access_mode == AccessMode::Writer {
        options.create(true).write(true);
    }
    let file = options.open(path)?;
    if access_mode == AccessMode::Writer {
        // SAFETY: the descriptor belongs to `file`, remains valid for the call,
        // and the kernel releases this advisory lease when the file is dropped.
        if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_store_file(path: &Path, access_mode: AccessMode) -> io::Result<File> {
    if access_mode == AccessMode::Writer {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "writer leases require Windows or Unix file-lock support",
        ));
    }
    OpenOptions::new().read(true).open(path)
}

fn is_writer_contention(error: &io::Error, access_mode: AccessMode) -> bool {
    if access_mode != AccessMode::Writer {
        return false;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(unix)]
    {
        error.kind() == io::ErrorKind::WouldBlock || matches!(error.raw_os_error(), Some(11 | 35))
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Invalid(reason) => write!(f, "invalid operation: {reason}"),
            Self::Corrupt { offset, reason } => {
                write!(f, "corrupt store at byte {offset}: {reason}")
            }
            Self::Missing(identity) => write!(f, "unknown fact identity {identity}"),
            Self::Busy(path) => write!(f, "store {} already has an active writer", path.display()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessMode {
    Writer,
    ReadOnly,
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bond {
    pub source: Digest,
    pub relation: Digest,
    pub target: Digest,
}

impl Bond {
    pub(crate) fn encode(self) -> [u8; 96] {
        let mut out = [0; 96];
        out[..32].copy_from_slice(self.source.as_bytes());
        out[32..64].copy_from_slice(self.relation.as_bytes());
        out[64..].copy_from_slice(self.target.as_bytes());
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 96 {
            return Err("a bond must contain exactly three identities".into());
        }
        Ok(Self {
            source: digest_from(&bytes[..32]),
            relation: digest_from(&bytes[32..64]),
            target: digest_from(&bytes[64..96]),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub atoms: u64,
    pub bonds: u64,
    pub roots: u64,
    pub root_updates: u64,
    pub cells: u64,
    pub facts: u64,
    pub frames: u64,
    pub durable_bytes: u64,
    pub repaired_tail_bytes: u64,
    pub provisional_tail_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct Location {
    payload_offset: u64,
    length: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CellMeta {
    identity: Digest,
    atoms: u64,
    bonds: u64,
    roots: u64,
}

impl CellMeta {
    fn encode(self) -> [u8; 56] {
        let mut bytes = [0; 56];
        bytes[..32].copy_from_slice(self.identity.as_bytes());
        bytes[32..40].copy_from_slice(&self.atoms.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.bonds.to_le_bytes());
        bytes[48..56].copy_from_slice(&self.roots.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 56 {
            return Err("a cell membrane must contain exactly 56 bytes".into());
        }
        let meta = Self {
            identity: digest_from(&bytes[..32]),
            atoms: u64::from_le_bytes(bytes[32..40].try_into().expect("fixed range")),
            bonds: u64::from_le_bytes(bytes[40..48].try_into().expect("fixed range")),
            roots: u64::from_le_bytes(bytes[48..56].try_into().expect("fixed range")),
        };
        if meta.atoms > MAX_CELL_ITEMS || meta.bonds > MAX_CELL_ITEMS || meta.roots > MAX_CELL_ITEMS
        {
            return Err("a cell count crosses the bounded item capacity".into());
        }
        Ok(meta)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RootChange {
    name: Digest,
    previous_version: Option<Digest>,
    previous: Option<Digest>,
    target: Option<Digest>,
}

impl RootChange {
    fn encode(self) -> [u8; 131] {
        let mut bytes = [0; 131];
        bytes[..32].copy_from_slice(self.name.as_bytes());
        if let Some(previous_version) = self.previous_version {
            bytes[32] = 1;
            bytes[33..65].copy_from_slice(previous_version.as_bytes());
        }
        if let Some(previous) = self.previous {
            bytes[65] = 1;
            bytes[66..98].copy_from_slice(previous.as_bytes());
        }
        if let Some(target) = self.target {
            bytes[98] = 1;
            bytes[99..131].copy_from_slice(target.as_bytes());
        }
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 131 {
            return Err("a root transition must contain exactly 131 bytes".into());
        }
        if bytes[32] > 1 || bytes[65] > 1 || bytes[98] > 1 {
            return Err("a root transition contains an invalid presence flag".into());
        }
        if bytes[32] == 0 && bytes[33..65] != [0; 32] {
            return Err("an absent previous version contains nonzero identity bytes".into());
        }
        if bytes[65] == 0 && bytes[66..98] != [0; 32] {
            return Err("an absent previous root contains nonzero identity bytes".into());
        }
        if bytes[98] == 0 && bytes[99..131] != [0; 32] {
            return Err("an absent target root contains nonzero identity bytes".into());
        }
        Ok(Self {
            name: digest_from(&bytes[..32]),
            previous_version: (bytes[32] == 1).then(|| digest_from(&bytes[33..65])),
            previous: (bytes[65] == 1).then(|| digest_from(&bytes[66..98])),
            target: (bytes[98] == 1).then(|| digest_from(&bytes[99..131])),
        })
    }

    fn identity(self) -> Digest {
        digest(&[ROOT_DOMAIN, &self.encode()])
    }
}

struct PreparedCell {
    identity: Digest,
    atoms: BTreeMap<Digest, Vec<u8>>,
    bonds: BTreeMap<Digest, Bond>,
    roots: Vec<(Digest, RootChange)>,
}

impl PreparedCell {
    fn meta(&self) -> CellMeta {
        CellMeta {
            identity: self.identity,
            atoms: self.atoms.len() as u64,
            bonds: self.bonds.len() as u64,
            roots: self.roots.len() as u64,
        }
    }

    fn is_empty(&self) -> bool {
        self.atoms.is_empty() && self.bonds.is_empty() && self.roots.is_empty()
    }
}

struct PendingCell {
    start_offset: u64,
    first_sequence: u64,
    meta: CellMeta,
    atoms: BTreeMap<Digest, Location>,
    bonds: BTreeMap<Digest, Bond>,
    roots: Vec<(Digest, RootChange, u64)>,
}

pub struct AtomDb {
    path: PathBuf,
    file: File,
    atoms: BTreeMap<Digest, Location>,
    bonds: BTreeMap<Digest, Bond>,
    outgoing: BTreeMap<Digest, Vec<Digest>>,
    roots: BTreeMap<Digest, Digest>,
    root_history: BTreeMap<Digest, Vec<RootVersion>>,
    committed_cells: u64,
    root_updates: u64,
    next_sequence: u64,
    repaired_tail_bytes: u64,
    provisional_tail_bytes: u64,
    access_mode: AccessMode,
}

impl AtomDb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_writer(path)
    }

    pub fn open_writer(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_with_mode(path.as_ref(), AccessMode::Writer)
    }

    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_with_mode(path.as_ref(), AccessMode::ReadOnly)
    }

    fn open_with_mode(path: &Path, access_mode: AccessMode) -> Result<Self, Error> {
        let path = path.to_path_buf();
        let mut file = open_store_file(&path, access_mode).map_err(|error| {
            if is_writer_contention(&error, access_mode) {
                Error::Busy(path.clone())
            } else {
                Error::Io(error)
            }
        })?;
        let length = file.metadata()?.len();
        if length == 0 {
            if access_mode == AccessMode::ReadOnly {
                return Err(Error::Invalid(
                    "read-only observer found a store without a durable header".into(),
                ));
            }
            file.write_all(FILE_MAGIC)?;
            file.sync_all()?;
        } else {
            let mut magic = [0; 8];
            if length < magic.len() as u64
                || file.read_exact(&mut magic).is_err()
                || &magic != FILE_MAGIC
            {
                return Err(Error::Corrupt {
                    offset: 0,
                    reason: "file identity or format version is invalid".into(),
                });
            }
        }
        let mut db = Self {
            path,
            file,
            atoms: BTreeMap::new(),
            bonds: BTreeMap::new(),
            outgoing: BTreeMap::new(),
            roots: BTreeMap::new(),
            root_history: BTreeMap::new(),
            committed_cells: 0,
            root_updates: 0,
            next_sequence: 0,
            repaired_tail_bytes: 0,
            provisional_tail_bytes: 0,
            access_mode,
        };
        db.recover(access_mode == AccessMode::Writer)?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn access_mode(&self) -> AccessMode {
        self.access_mode
    }

    pub fn refresh(&mut self) -> Result<bool, Error> {
        if self.access_mode != AccessMode::ReadOnly {
            return Err(Error::Invalid(
                "refresh is available only to read-only observers".into(),
            ));
        }
        let refreshed = Self::open_read_only(&self.path)?;
        let changed = refreshed.next_sequence != self.next_sequence
            || refreshed.provisional_tail_bytes != self.provisional_tail_bytes;
        *self = refreshed;
        Ok(changed)
    }

    pub fn begin_cell(&self) -> Cell {
        Cell::new()
    }

    pub fn commit_cell(&mut self, cell: Cell) -> Result<CellReceipt, Error> {
        self.require_writer()?;
        let prepared = self.prepare_cell(cell)?;
        let first_sequence = self.next_sequence;
        if prepared.is_empty() {
            return Ok(CellReceipt {
                identity: prepared.identity,
                committed: false,
                first_sequence,
                commit_sequence: first_sequence,
                atoms_added: 0,
                bonds_added: 0,
                roots_changed: 0,
            });
        }

        let start_offset = self.file.seek(SeekFrom::End(0))?;
        let mut sequence = first_sequence;
        let meta = prepared.meta();
        let meta_bytes = meta.encode();
        let mut atom_locations = Vec::with_capacity(prepared.atoms.len());
        let mut root_sequences = Vec::with_capacity(prepared.roots.len());
        let write_result = (|| -> io::Result<()> {
            write_frame(
                &mut self.file,
                sequence,
                CELL_BEGIN_KIND,
                prepared.identity,
                &meta_bytes,
            )?;
            sequence += 1;
            for (identity, bytes) in &prepared.atoms {
                let location = write_frame(&mut self.file, sequence, ATOM_KIND, *identity, bytes)?;
                atom_locations.push((*identity, location));
                sequence += 1;
            }
            for (identity, bond) in &prepared.bonds {
                write_frame(
                    &mut self.file,
                    sequence,
                    BOND_KIND,
                    *identity,
                    &bond.encode(),
                )?;
                sequence += 1;
            }
            for (identity, change) in &prepared.roots {
                root_sequences.push(sequence);
                write_frame(
                    &mut self.file,
                    sequence,
                    ROOT_KIND,
                    *identity,
                    &change.encode(),
                )?;
                sequence += 1;
            }
            write_frame(
                &mut self.file,
                sequence,
                CELL_COMMIT_KIND,
                prepared.identity,
                &meta_bytes,
            )?;
            sequence += 1;
            self.file.sync_data()
        })();
        if let Err(error) = write_result {
            if let Err(rollback) = self.rollback_write(start_offset) {
                return Err(Error::Io(rollback));
            }
            return Err(Error::Io(error));
        }

        for (identity, location) in atom_locations {
            self.atoms.insert(identity, location);
        }
        for (identity, bond) in &prepared.bonds {
            self.bonds.insert(*identity, *bond);
            self.outgoing
                .entry(bond.source)
                .or_default()
                .push(*identity);
        }
        let commit_sequence = sequence - 1;
        for ((identity, change), root_sequence) in prepared.roots.iter().zip(root_sequences) {
            self.apply_root(
                *identity,
                *change,
                root_sequence,
                commit_sequence,
                prepared.identity,
            );
        }
        self.committed_cells = self.committed_cells.saturating_add(1);
        self.next_sequence = sequence;
        Ok(CellReceipt {
            identity: prepared.identity,
            committed: true,
            first_sequence,
            commit_sequence,
            atoms_added: prepared.atoms.len() as u64,
            bonds_added: prepared.bonds.len() as u64,
            roots_changed: prepared.roots.len() as u64,
        })
    }

    pub fn put_atom(&mut self, bytes: &[u8]) -> Result<Digest, Error> {
        self.require_writer()?;
        if bytes.len() as u64 > MAX_PAYLOAD {
            return Err(Error::Invalid(
                "atom exceeds the 64 MiB Stage 1 boundary".into(),
            ));
        }
        let identity = atom_identity(bytes);
        if self.atoms.contains_key(&identity) {
            return Ok(identity);
        }
        let location = self.append(ATOM_KIND, identity, bytes)?;
        self.atoms.insert(identity, location);
        Ok(identity)
    }

    pub fn put_bond(&mut self, bond: Bond) -> Result<Digest, Error> {
        self.require_writer()?;
        for identity in [bond.source, bond.relation, bond.target] {
            if !self.atoms.contains_key(&identity) {
                return Err(Error::Missing(identity));
            }
        }
        let identity = bond_identity(bond);
        if self.bonds.contains_key(&identity) {
            return Ok(identity);
        }
        self.append(BOND_KIND, identity, &bond.encode())?;
        self.bonds.insert(identity, bond);
        self.outgoing.entry(bond.source).or_default().push(identity);
        Ok(identity)
    }

    pub fn get_atom(&mut self, identity: Digest) -> Result<Option<Vec<u8>>, Error> {
        let Some(location) = self.atoms.get(&identity).copied() else {
            return Ok(None);
        };
        self.file.seek(SeekFrom::Start(location.payload_offset))?;
        let mut bytes = vec![0; location.length as usize];
        self.file.read_exact(&mut bytes)?;
        Ok(Some(bytes))
    }

    pub fn get_bond(&self, identity: Digest) -> Option<Bond> {
        self.bonds.get(&identity).copied()
    }

    pub fn bonds_from(&self, source: Digest) -> Vec<(Digest, Bond)> {
        self.outgoing
            .get(&source)
            .into_iter()
            .flatten()
            .filter_map(|identity| self.bonds.get(identity).map(|bond| (*identity, *bond)))
            .collect()
    }

    pub fn all_bonds(&self) -> Vec<(Digest, Bond)> {
        self.bonds
            .iter()
            .map(|(identity, bond)| (*identity, *bond))
            .collect()
    }

    pub fn contains_atom(&self, identity: Digest) -> bool {
        self.atoms.contains_key(&identity)
    }

    pub fn contains_fact(&self, identity: Digest) -> bool {
        self.atoms.contains_key(&identity) || self.bonds.contains_key(&identity)
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

    pub fn root_history(&self, name: Digest) -> Vec<RootVersion> {
        self.root_history.get(&name).cloned().unwrap_or_default()
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot::new(self.next_sequence, self.roots.clone())
    }

    pub fn snapshot_at(&self, sequence: u64) -> Result<Snapshot, Error> {
        if sequence > self.next_sequence {
            return Err(Error::Invalid(format!(
                "snapshot sequence {sequence} is beyond the durable frontier {}",
                self.next_sequence
            )));
        }
        let mut roots = BTreeMap::new();
        for (name, versions) in &self.root_history {
            if let Some(version) = versions
                .iter()
                .rev()
                .find(|version| version.commit_sequence < sequence)
                && let Some(target) = version.target
            {
                roots.insert(*name, target);
            }
        }
        Ok(Snapshot::new(sequence, roots))
    }

    pub fn sync(&mut self) -> Result<(), Error> {
        self.require_writer()?;
        self.file.sync_all().map_err(Error::Io)
    }

    pub fn stats(&self) -> Result<Stats, Error> {
        Ok(Stats {
            atoms: self.atoms.len() as u64,
            bonds: self.bonds.len() as u64,
            roots: self.roots.len() as u64,
            root_updates: self.root_updates,
            cells: self.committed_cells,
            facts: self.atoms.len() as u64 + self.bonds.len() as u64 + self.root_updates,
            frames: self.next_sequence,
            durable_bytes: self.file.metadata()?.len(),
            repaired_tail_bytes: self.repaired_tail_bytes,
            provisional_tail_bytes: self.provisional_tail_bytes,
        })
    }

    fn require_writer(&self) -> Result<(), Error> {
        if self.access_mode == AccessMode::Writer {
            Ok(())
        } else {
            Err(Error::Invalid(
                "read-only observers cannot change durable state".into(),
            ))
        }
    }

    fn prepare_cell(&self, cell: Cell) -> Result<PreparedCell, Error> {
        if cell.atoms.len() as u64 > MAX_CELL_ITEMS
            || cell.bonds.len() as u64 > MAX_CELL_ITEMS
            || cell.roots.len() as u64 > MAX_CELL_ITEMS
        {
            return Err(Error::Invalid(
                "cell crosses the bounded item capacity".into(),
            ));
        }
        for bytes in cell.atoms.values() {
            if bytes.len() as u64 > MAX_PAYLOAD {
                return Err(Error::Invalid(
                    "cell atom exceeds the 64 MiB payload boundary".into(),
                ));
            }
        }
        let atoms = cell
            .atoms
            .into_iter()
            .filter(|(identity, _)| !self.atoms.contains_key(identity))
            .collect::<BTreeMap<_, _>>();
        let bonds = cell
            .bonds
            .into_iter()
            .filter(|(identity, _)| !self.bonds.contains_key(identity))
            .collect::<BTreeMap<_, _>>();
        for bond in bonds.values() {
            for identity in [bond.source, bond.relation, bond.target] {
                if !self.atoms.contains_key(&identity) && !atoms.contains_key(&identity) {
                    return Err(Error::Missing(identity));
                }
            }
        }
        let mut roots = Vec::new();
        for (name, target) in cell.roots {
            if !self.atoms.contains_key(&name) && !atoms.contains_key(&name) {
                return Err(Error::Invalid(format!("root name {name} is not an atom")));
            }
            if let Some(target) = target
                && !self.contains_fact(target)
                && !atoms.contains_key(&target)
                && !bonds.contains_key(&target)
            {
                return Err(Error::Missing(target));
            }
            let previous = self.roots.get(&name).copied();
            if previous == target {
                continue;
            }
            let previous_version = self
                .root_history
                .get(&name)
                .and_then(|versions| versions.last())
                .map(|version| version.identity);
            let change = RootChange {
                name,
                previous_version,
                previous,
                target,
            };
            roots.push((change.identity(), change));
        }
        roots.sort_by_key(|(_, change)| change.name);
        let identity = compute_cell_identity(
            atoms.keys().copied(),
            bonds.keys().copied(),
            roots.iter().map(|(identity, _)| *identity),
        );
        Ok(PreparedCell {
            identity,
            atoms,
            bonds,
            roots,
        })
    }

    fn append(&mut self, kind: u8, identity: Digest, payload: &[u8]) -> Result<Location, Error> {
        let start = self.file.seek(SeekFrom::End(0))?;
        let location =
            match write_frame(&mut self.file, self.next_sequence, kind, identity, payload) {
                Ok(location) => location,
                Err(error) => {
                    self.rollback_write(start)?;
                    return Err(Error::Io(error));
                }
            };
        if let Err(error) = self.file.sync_data() {
            self.rollback_write(start)?;
            return Err(Error::Io(error));
        }
        self.next_sequence += 1;
        Ok(location)
    }

    fn rollback_write(&mut self, offset: u64) -> io::Result<()> {
        self.file.set_len(offset)?;
        self.file.sync_all()?;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    fn recover(&mut self, repair: bool) -> Result<(), Error> {
        let file_length = self.file.metadata()?.len();
        let mut offset = FILE_MAGIC.len() as u64;
        let mut sequence = 0_u64;
        let mut pending: Option<PendingCell> = None;
        while offset < file_length {
            let remaining = file_length - offset;
            if remaining < HEADER_LEN as u64 {
                let (valid_offset, valid_sequence) =
                    pending.as_ref().map_or((offset, sequence), |cell| {
                        (cell.start_offset, cell.first_sequence)
                    });
                return self.finish_incomplete_tail(
                    repair,
                    valid_offset,
                    file_length,
                    valid_sequence,
                );
            }
            self.file.seek(SeekFrom::Start(offset))?;
            let mut header = [0; HEADER_LEN];
            self.file.read_exact(&mut header)?;
            if &header[..4] != FRAME_MAGIC {
                return Err(corrupt(offset, "frame marker is invalid"));
            }
            if header[5..8] != [0, 0, 0] {
                return Err(corrupt(offset, "reserved frame bits are nonzero"));
            }
            let kind = header[4];
            let found_sequence = u64::from_le_bytes(header[8..16].try_into().expect("fixed range"));
            let length = u64::from_le_bytes(header[16..24].try_into().expect("fixed range"));
            if found_sequence != sequence {
                return Err(corrupt(offset, "causal sequence is discontinuous"));
            }
            if length > MAX_PAYLOAD {
                return Err(corrupt(offset, "payload crosses the size boundary"));
            }
            let frame_length = HEADER_LEN as u64 + length;
            if remaining < frame_length {
                let (valid_offset, valid_sequence) =
                    pending.as_ref().map_or((offset, sequence), |cell| {
                        (cell.start_offset, cell.first_sequence)
                    });
                return self.finish_incomplete_tail(
                    repair,
                    valid_offset,
                    file_length,
                    valid_sequence,
                );
            }
            let payload_offset = offset + HEADER_LEN as u64;
            let mut payload = vec![0; length as usize];
            self.file.read_exact(&mut payload)?;
            let identity = digest_from(&header[24..56]);
            let checksum = digest(&[FRAME_DOMAIN, &header[..56], &payload]);
            if checksum.as_bytes() != &header[56..88] {
                return Err(corrupt(offset, "frame checksum does not balance"));
            }
            let location = Location {
                payload_offset,
                length,
            };
            match kind {
                ATOM_KIND => {
                    if identity != atom_identity(&payload) {
                        return Err(corrupt(offset, "atom content identity does not balance"));
                    }
                    if let Some(cell) = pending.as_mut() {
                        if self.atoms.contains_key(&identity)
                            || cell.atoms.insert(identity, location).is_some()
                        {
                            return Err(corrupt(offset, "duplicate atom inside a cell"));
                        }
                    } else if self.atoms.insert(identity, location).is_some() {
                        return Err(corrupt(offset, "duplicate atom fact"));
                    }
                }
                BOND_KIND => {
                    let bond = Bond::decode(&payload).map_err(|reason| corrupt(offset, &reason))?;
                    if identity != bond_identity(bond) {
                        return Err(corrupt(offset, "bond content identity does not balance"));
                    }
                    if let Some(cell) = pending.as_mut() {
                        if self.bonds.contains_key(&identity)
                            || cell.bonds.insert(identity, bond).is_some()
                        {
                            return Err(corrupt(offset, "duplicate bond inside a cell"));
                        }
                    } else {
                        self.validate_bond_members(bond, None, offset)?;
                        if self.bonds.insert(identity, bond).is_some() {
                            return Err(corrupt(offset, "duplicate bond fact"));
                        }
                        self.outgoing.entry(bond.source).or_default().push(identity);
                    }
                }
                CELL_BEGIN_KIND => {
                    if pending.is_some() {
                        return Err(corrupt(offset, "a cell begins inside another cell"));
                    }
                    let meta =
                        CellMeta::decode(&payload).map_err(|reason| corrupt(offset, &reason))?;
                    if identity != meta.identity {
                        return Err(corrupt(
                            offset,
                            "cell-begin identity does not match its membrane",
                        ));
                    }
                    pending = Some(PendingCell {
                        start_offset: offset,
                        first_sequence: sequence,
                        meta,
                        atoms: BTreeMap::new(),
                        bonds: BTreeMap::new(),
                        roots: Vec::new(),
                    });
                }
                ROOT_KIND => {
                    let Some(cell) = pending.as_mut() else {
                        return Err(corrupt(offset, "a root transition exists outside a cell"));
                    };
                    let change =
                        RootChange::decode(&payload).map_err(|reason| corrupt(offset, &reason))?;
                    if identity != change.identity() {
                        return Err(corrupt(offset, "root transition identity does not balance"));
                    }
                    cell.roots.push((identity, change, sequence));
                }
                CELL_COMMIT_KIND => {
                    let Some(cell) = pending.take() else {
                        return Err(corrupt(offset, "a cell closes without an open membrane"));
                    };
                    let commit_meta =
                        CellMeta::decode(&payload).map_err(|reason| corrupt(offset, &reason))?;
                    if identity != commit_meta.identity || commit_meta != cell.meta {
                        return Err(corrupt(offset, "cell membranes do not balance"));
                    }
                    self.validate_and_apply_cell(cell, offset, sequence)?;
                }
                _ => return Err(corrupt(offset, "fact kind is unknown")),
            }
            sequence += 1;
            offset += frame_length;
        }
        if let Some(cell) = pending {
            return self.finish_incomplete_tail(
                repair,
                cell.start_offset,
                file_length,
                cell.first_sequence,
            );
        }
        self.next_sequence = sequence;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    fn validate_and_apply_cell(
        &mut self,
        cell: PendingCell,
        offset: u64,
        commit_sequence: u64,
    ) -> Result<(), Error> {
        if cell.atoms.len() as u64 != cell.meta.atoms
            || cell.bonds.len() as u64 != cell.meta.bonds
            || cell.roots.len() as u64 != cell.meta.roots
        {
            return Err(corrupt(
                offset,
                "cell contents do not match membrane counts",
            ));
        }
        for bond in cell.bonds.values() {
            self.validate_bond_members(*bond, Some(&cell.atoms), offset)?;
        }
        let mut prior_name = None;
        for (_, change, _) in &cell.roots {
            if prior_name.is_some_and(|name| name >= change.name) {
                return Err(corrupt(offset, "cell roots are not in canonical order"));
            }
            prior_name = Some(change.name);
            if !self.atoms.contains_key(&change.name) && !cell.atoms.contains_key(&change.name) {
                return Err(corrupt(offset, "root name is not an atom"));
            }
            if let Some(target) = change.target
                && !self.contains_fact(target)
                && !cell.atoms.contains_key(&target)
                && !cell.bonds.contains_key(&target)
            {
                return Err(corrupt(offset, "root target does not exist"));
            }
            let previous_version = self
                .root_history
                .get(&change.name)
                .and_then(|versions| versions.last())
                .map(|version| version.identity);
            if self.roots.get(&change.name).copied() != change.previous
                || previous_version != change.previous_version
            {
                return Err(corrupt(offset, "root history does not form a causal chain"));
            }
        }
        let computed = compute_cell_identity(
            cell.atoms.keys().copied(),
            cell.bonds.keys().copied(),
            cell.roots.iter().map(|(identity, _, _)| *identity),
        );
        if computed != cell.meta.identity {
            return Err(corrupt(
                offset,
                "cell identity does not balance its contents",
            ));
        }
        for (identity, location) in cell.atoms {
            self.atoms.insert(identity, location);
        }
        for (identity, bond) in cell.bonds {
            self.bonds.insert(identity, bond);
            self.outgoing.entry(bond.source).or_default().push(identity);
        }
        for (identity, change, sequence) in cell.roots {
            self.apply_root(
                identity,
                change,
                sequence,
                commit_sequence,
                cell.meta.identity,
            );
        }
        self.committed_cells = self.committed_cells.saturating_add(1);
        Ok(())
    }

    fn validate_bond_members(
        &self,
        bond: Bond,
        pending_atoms: Option<&BTreeMap<Digest, Location>>,
        offset: u64,
    ) -> Result<(), Error> {
        for member in [bond.source, bond.relation, bond.target] {
            let pending = pending_atoms.is_some_and(|atoms| atoms.contains_key(&member));
            if !self.atoms.contains_key(&member) && !pending {
                return Err(corrupt(offset, "bond precedes one of its atoms"));
            }
        }
        Ok(())
    }

    fn apply_root(
        &mut self,
        identity: Digest,
        change: RootChange,
        sequence: u64,
        commit_sequence: u64,
        cell: Digest,
    ) {
        match change.target {
            Some(target) => {
                self.roots.insert(change.name, target);
            }
            None => {
                self.roots.remove(&change.name);
            }
        }
        self.root_history
            .entry(change.name)
            .or_default()
            .push(RootVersion {
                identity,
                sequence,
                commit_sequence,
                cell,
                previous_version: change.previous_version,
                previous: change.previous,
                target: change.target,
            });
        self.root_updates = self.root_updates.saturating_add(1);
    }

    fn repair_tail(
        &mut self,
        valid_length: u64,
        prior_length: u64,
        valid_sequence: u64,
    ) -> Result<(), Error> {
        self.repaired_tail_bytes = prior_length - valid_length;
        self.file.set_len(valid_length)?;
        self.file.sync_all()?;
        self.file.seek(SeekFrom::End(0))?;
        self.next_sequence = valid_sequence;
        Ok(())
    }

    fn finish_incomplete_tail(
        &mut self,
        repair: bool,
        valid_length: u64,
        prior_length: u64,
        valid_sequence: u64,
    ) -> Result<(), Error> {
        if repair {
            return self.repair_tail(valid_length, prior_length, valid_sequence);
        }
        self.provisional_tail_bytes = prior_length - valid_length;
        self.next_sequence = valid_sequence;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }
}

fn write_frame(
    file: &mut File,
    sequence: u64,
    kind: u8,
    identity: Digest,
    payload: &[u8],
) -> io::Result<Location> {
    let start = file.seek(SeekFrom::End(0))?;
    let mut header = [0; HEADER_LEN];
    header[..4].copy_from_slice(FRAME_MAGIC);
    header[4] = kind;
    header[8..16].copy_from_slice(&sequence.to_le_bytes());
    header[16..24].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    header[24..56].copy_from_slice(identity.as_bytes());
    let checksum = digest(&[FRAME_DOMAIN, &header[..56], payload]);
    header[56..88].copy_from_slice(checksum.as_bytes());
    file.write_all(&header)?;
    file.write_all(payload)?;
    Ok(Location {
        payload_offset: start + HEADER_LEN as u64,
        length: payload.len() as u64,
    })
}

fn compute_cell_identity(
    atoms: impl IntoIterator<Item = Digest>,
    bonds: impl IntoIterator<Item = Digest>,
    roots: impl IntoIterator<Item = Digest>,
) -> Digest {
    let atoms = atoms.into_iter().collect::<Vec<_>>();
    let bonds = bonds.into_iter().collect::<Vec<_>>();
    let roots = roots.into_iter().collect::<Vec<_>>();
    let mut manifest = Vec::with_capacity(24 + 33 * (atoms.len() + bonds.len() + roots.len()));
    manifest.extend_from_slice(&(atoms.len() as u64).to_le_bytes());
    manifest.extend_from_slice(&(bonds.len() as u64).to_le_bytes());
    manifest.extend_from_slice(&(roots.len() as u64).to_le_bytes());
    for (kind, identities) in [(ATOM_KIND, atoms), (BOND_KIND, bonds), (ROOT_KIND, roots)] {
        for identity in identities {
            manifest.push(kind);
            manifest.extend_from_slice(identity.as_bytes());
        }
    }
    digest(&[CELL_DOMAIN, &manifest])
}

fn digest_from(bytes: &[u8]) -> Digest {
    let mut identity = [0; 32];
    identity.copy_from_slice(bytes);
    Digest(identity)
}

fn corrupt(offset: u64, reason: &str) -> Error {
    Error::Corrupt {
        offset,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_file(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/atom-db-tests");
        fs::create_dir_all(&directory).unwrap();
        directory.join(format!(
            "atom-db-{name}-{}-{nonce}.atoms",
            std::process::id()
        ))
    }

    #[test]
    fn atoms_deduplicate_and_survive_reopen() {
        let path = temp_file("reopen");
        let identity = {
            let mut db = AtomDb::open(&path).unwrap();
            let first = db.put_atom(b"the same fact").unwrap();
            assert_eq!(first, db.put_atom(b"the same fact").unwrap());
            assert_eq!(db.stats().unwrap().atoms, 1);
            first
        };
        let mut db = AtomDb::open(&path).unwrap();
        assert_eq!(db.get_atom(identity).unwrap().unwrap(), b"the same fact");
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn bonds_require_and_connect_atoms() {
        let path = temp_file("bond");
        let mut db = AtomDb::open(&path).unwrap();
        let source = db.put_atom(b"earth").unwrap();
        let relation = db.put_atom(b"orbits").unwrap();
        let target = db.put_atom(b"sun").unwrap();
        let bond = Bond {
            source,
            relation,
            target,
        };
        let id = db.put_bond(bond).unwrap();
        assert_eq!(db.get_bond(id), Some(bond));
        assert_eq!(db.bonds_from(source), vec![(id, bond)]);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn incomplete_tail_is_removed_but_complete_corruption_fails() {
        let path = temp_file("recovery");
        let good_len = {
            let mut db = AtomDb::open(&path).unwrap();
            db.put_atom(b"durable").unwrap();
            db.stats().unwrap().durable_bytes
        };
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(b"ATOM\x01torn").unwrap();
        }
        let db = AtomDb::open(&path).unwrap();
        assert_eq!(db.stats().unwrap().repaired_tail_bytes, 9);
        assert_eq!(fs::metadata(&path).unwrap().len(), good_len);
        drop(db);
        {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            file.seek(SeekFrom::Start(FILE_MAGIC.len() as u64 + HEADER_LEN as u64))
                .unwrap();
            file.write_all(b"D").unwrap();
        }
        assert!(matches!(AtomDb::open(&path), Err(Error::Corrupt { .. })));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_bond_member_fails_closed() {
        let path = temp_file("missing");
        let mut db = AtomDb::open(&path).unwrap();
        let present = db.put_atom(b"present").unwrap();
        let result = db.put_bond(Bond {
            source: present,
            relation: Digest::ZERO,
            target: present,
        });
        assert!(matches!(result, Err(Error::Missing(Digest::ZERO))));
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn cell_commits_facts_root_and_snapshot_atomically() {
        let path = temp_file("cell");
        let mut db = AtomDb::open(&path).unwrap();
        let mut first = db.begin_cell();
        let root_name = first.put_atom(b"project/current");
        let version_one = first.put_atom(b"version-one");
        let relation = first.put_atom(b"supersedes");
        first.set_root(root_name, version_one);
        let receipt = db.commit_cell(first).unwrap();
        assert!(receipt.committed);
        assert_eq!(receipt.atoms_added, 3);
        assert_eq!(receipt.roots_changed, 1);
        assert_eq!(db.root(root_name), Some(version_one));
        let snapshot = db.snapshot();

        let mut second = db.begin_cell();
        let version_two = second.put_atom(b"version-two");
        second.put_bond(Bond {
            source: version_two,
            relation,
            target: version_one,
        });
        second.set_root(root_name, version_two);
        db.commit_cell(second).unwrap();
        assert_eq!(db.root(root_name), Some(version_two));
        assert_eq!(snapshot.root(root_name), Some(version_one));
        assert_eq!(db.root_history(root_name).len(), 2);
        drop(db);

        let db = AtomDb::open(&path).unwrap();
        assert_eq!(db.root(root_name), Some(version_two));
        assert_eq!(db.root_history(root_name).len(), 2);
        assert_eq!(
            db.snapshot_at(receipt.commit_sequence + 1)
                .unwrap()
                .root(root_name),
            Some(version_one)
        );
        assert_eq!(db.stats().unwrap().cells, 2);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn every_cell_write_boundary_preserves_only_the_committed_prefix() {
        let source = temp_file("cell-boundaries-source");
        let (baseline, staged, cell_start) = {
            let mut db = AtomDb::open(&source).unwrap();
            let baseline = db.put_atom(b"survives-every-cut").unwrap();
            let cell_start = db.stats().unwrap().durable_bytes as usize;
            let mut cell = db.begin_cell();
            let staged = cell.put_atom(b"must-not-leak");
            db.commit_cell(cell).unwrap();
            (baseline, staged, cell_start)
        };
        let bytes = fs::read(&source).unwrap();
        let begin_end = cell_start + HEADER_LEN + 56;
        let atom_end = begin_end + HEADER_LEN + b"must-not-leak".len();
        let cuts = [
            cell_start,
            cell_start + 1,
            cell_start + HEADER_LEN - 1,
            begin_end,
            begin_end + 1,
            atom_end,
            atom_end + HEADER_LEN,
            bytes.len() - 1,
        ];
        for (index, cut) in cuts.into_iter().enumerate() {
            let trial = temp_file(&format!("cell-boundary-{index}"));
            fs::write(&trial, &bytes[..cut]).unwrap();
            let mut recovered = AtomDb::open(&trial).unwrap();
            assert_eq!(
                recovered.get_atom(baseline).unwrap().as_deref(),
                Some(b"survives-every-cut".as_slice())
            );
            assert_eq!(recovered.get_atom(staged).unwrap(), None);
            assert_eq!(recovered.stats().unwrap().atoms, 1);
            assert_eq!(recovered.stats().unwrap().cells, 0);
            assert_eq!(fs::metadata(&trial).unwrap().len(), cell_start as u64);
            drop(recovered);
            fs::remove_file(trial).unwrap();
        }
        fs::remove_file(source).unwrap();
    }

    #[test]
    fn invalid_cell_changes_neither_memory_nor_file() {
        let path = temp_file("invalid-cell");
        let mut db = AtomDb::open(&path).unwrap();
        let initial_length = db.stats().unwrap().durable_bytes;
        let mut cell = db.begin_cell();
        let present = cell.put_atom(b"present-only-in-rejected-cell");
        cell.put_bond(Bond {
            source: present,
            relation: Digest::ZERO,
            target: present,
        });
        assert!(matches!(
            db.commit_cell(cell),
            Err(Error::Missing(Digest::ZERO))
        ));
        assert!(!db.contains_atom(present));
        assert_eq!(db.stats().unwrap().durable_bytes, initial_length);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn clearing_root_is_a_durable_transition() {
        let path = temp_file("clear-root");
        let mut db = AtomDb::open(&path).unwrap();
        let mut first = db.begin_cell();
        let root_name = first.put_atom(b"session/current");
        let target = first.put_atom(b"live");
        first.set_root(root_name, target);
        db.commit_cell(first).unwrap();
        let mut second = db.begin_cell();
        second.clear_root(root_name);
        db.commit_cell(second).unwrap();
        assert_eq!(db.root(root_name), None);
        assert_eq!(db.root_history(root_name).len(), 2);
        drop(db);
        let db = AtomDb::open(&path).unwrap();
        assert_eq!(db.root(root_name), None);
        assert_eq!(db.root_history(root_name).len(), 2);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn repeated_root_targets_still_form_unique_causal_versions() {
        let path = temp_file("root-cycle");
        let mut db = AtomDb::open(&path).unwrap();
        let mut first = db.begin_cell();
        let root_name = first.put_atom(b"cycle/current");
        let target_a = first.put_atom(b"target-a");
        let target_b = first.put_atom(b"target-b");
        first.set_root(root_name, target_a);
        db.commit_cell(first).unwrap();
        for target in [target_b, target_a, target_b] {
            let mut cell = db.begin_cell();
            cell.set_root(root_name, target);
            db.commit_cell(cell).unwrap();
        }
        let versions = db.root_history(root_name);
        assert_eq!(versions.len(), 4);
        assert_eq!(versions[1].previous, Some(target_a));
        assert_eq!(versions[3].previous, Some(target_a));
        assert_ne!(versions[1].identity, versions[3].identity);
        assert_eq!(versions[3].previous_version, Some(versions[2].identity));
        drop(db);

        let db = AtomDb::open(&path).unwrap();
        let versions = db.root_history(root_name);
        assert_eq!(versions.len(), 4);
        assert_eq!(db.root(root_name), Some(target_b));
        assert_ne!(versions[1].identity, versions[3].identity);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn writer_lease_excludes_writers_but_allows_observers() {
        let path = temp_file("writer-lease");
        let writer = AtomDb::open_writer(&path).unwrap();
        assert_eq!(writer.access_mode(), AccessMode::Writer);
        assert!(matches!(AtomDb::open_writer(&path), Err(Error::Busy(_))));
        let first_reader = AtomDb::open_read_only(&path).unwrap();
        let second_reader = AtomDb::open_read_only(&path).unwrap();
        assert_eq!(first_reader.access_mode(), AccessMode::ReadOnly);
        assert_eq!(second_reader.access_mode(), AccessMode::ReadOnly);
        drop(writer);
        let replacement_writer = AtomDb::open_writer(&path).unwrap();
        drop(replacement_writer);
        drop(first_reader);
        drop(second_reader);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn observer_ignores_open_cell_without_repairing_it() {
        let path = temp_file("observer-open-cell");
        let (baseline, staged, cell_start) = {
            let mut writer = AtomDb::open_writer(&path).unwrap();
            let baseline = writer.put_atom(b"committed-prefix").unwrap();
            let cell_start = writer.stats().unwrap().durable_bytes;
            let mut cell = writer.begin_cell();
            let staged = cell.put_atom(b"provisional-cell");
            writer.commit_cell(cell).unwrap();
            (baseline, staged, cell_start)
        };
        let committed_length = fs::metadata(&path).unwrap().len();
        let commit_frame_length = HEADER_LEN as u64 + 56;
        let provisional_length = committed_length - commit_frame_length;
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(provisional_length)
            .unwrap();

        let mut observer = AtomDb::open_read_only(&path).unwrap();
        assert_eq!(
            observer.get_atom(baseline).unwrap().as_deref(),
            Some(b"committed-prefix".as_slice())
        );
        assert_eq!(observer.get_atom(staged).unwrap(), None);
        assert_eq!(observer.stats().unwrap().repaired_tail_bytes, 0);
        assert_eq!(
            observer.stats().unwrap().provisional_tail_bytes,
            provisional_length - cell_start
        );
        assert_eq!(fs::metadata(&path).unwrap().len(), provisional_length);
        assert!(matches!(
            observer.put_atom(b"forbidden"),
            Err(Error::Invalid(_))
        ));

        let writer = AtomDb::open_writer(&path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), cell_start);
        assert_eq!(
            writer.stats().unwrap().repaired_tail_bytes,
            provisional_length - cell_start
        );
        drop(writer);
        drop(observer);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn observer_refreshes_to_a_new_committed_frontier() {
        let path = temp_file("observer-refresh");
        {
            let mut writer = AtomDb::open_writer(&path).unwrap();
            writer.put_atom(b"initial").unwrap();
        }
        let mut observer = AtomDb::open_read_only(&path).unwrap();
        let added = {
            let mut writer = AtomDb::open_writer(&path).unwrap();
            writer.put_atom(b"added-later").unwrap()
        };
        assert_eq!(observer.get_atom(added).unwrap(), None);
        assert!(observer.refresh().unwrap());
        assert_eq!(
            observer.get_atom(added).unwrap().as_deref(),
            Some(b"added-later".as_slice())
        );
        assert!(!observer.refresh().unwrap());
        drop(observer);
        fs::remove_file(path).unwrap();
    }
}
