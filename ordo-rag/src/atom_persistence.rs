//! Durable AtomDB persistence for Ordo RAG.
//!
//! General retrieval data and learned retrieval signals intentionally use
//! different roots.  Each root is the head of an immutable event chain; one
//! atomic AtomDB cell stores the event, its link to the previous head, and the
//! root update.  The separation prevents feedback from mutating or replacing
//! neutral source material while still allowing both slices to share one
//! compact AtomDB file.

use std::{collections::HashSet, fs, path::PathBuf};

use atom_db::{AtomDb, Bond, Cell, Digest};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

type DynError = Box<dyn std::error::Error + Send + Sync>;

const SCHEMA_VERSION: u32 = 1;
const GENERAL_ROOT_BYTES: &[u8] = b"ordo/rag/general/head/v1";
const LEARNING_ROOT_BYTES: &[u8] = b"ordo/rag/self-learning/head/v1";
const PREVIOUS_RELATION_BYTES: &[u8] = b"ordo/rag/event/previous/v1";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Slice {
    General,
    Learning,
}

impl Slice {
    fn label(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Learning => "self-learning",
        }
    }

    fn root_bytes(self) -> &'static [u8] {
        match self {
            Self::General => GENERAL_ROOT_BYTES,
            Self::Learning => LEARNING_ROOT_BYTES,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableRecord<T> {
    schema_version: u32,
    slice: String,
    ordinal: u64,
    payload: T,
}

pub(crate) struct AtomRagPersistence {
    db: AtomDb,
    general_head: Option<Digest>,
    learning_head: Option<Digest>,
    next_ordinal: u64,
}

impl AtomRagPersistence {
    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        let path = path.into();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let db = AtomDb::open_writer(&path)?;
        let general_head = db.root(atom_identity(GENERAL_ROOT_BYTES));
        let learning_head = db.root(atom_identity(LEARNING_ROOT_BYTES));
        let next_ordinal = db.snapshot().sequence().saturating_add(1);
        Ok(Self {
            db,
            general_head,
            learning_head,
            next_ordinal,
        })
    }

    pub(crate) fn append<T: Serialize>(
        &mut self,
        slice: Slice,
        payload: &T,
    ) -> Result<(), DynError> {
        let record = DurableRecord {
            schema_version: SCHEMA_VERSION,
            slice: slice.label().to_string(),
            ordinal: self.next_ordinal,
            payload,
        };
        let bytes = serde_json::to_vec(&record)?;

        let mut cell = Cell::new();
        let root = cell.put_atom(slice.root_bytes());
        let previous_relation = cell.put_atom(PREVIOUS_RELATION_BYTES);
        let event = cell.put_atom(bytes);
        if let Some(previous) = self.head(slice) {
            cell.put_bond(Bond {
                source: event,
                relation: previous_relation,
                target: previous,
            });
        }
        cell.set_root(root, event);
        self.db.commit_cell(cell)?;
        self.db.sync()?;
        *self.head_mut(slice) = Some(event);
        self.next_ordinal = self.next_ordinal.saturating_add(1);
        Ok(())
    }

    pub(crate) fn replay<T: DeserializeOwned>(&mut self, slice: Slice) -> Result<Vec<T>, DynError> {
        let relation = atom_identity(PREVIOUS_RELATION_BYTES);
        let mut cursor = self.head(slice);
        let mut visited = HashSet::new();
        let mut records = Vec::new();

        while let Some(identity) = cursor {
            if !visited.insert(identity) {
                return Err(
                    format!("cycle in AtomDB {} RAG slice at {identity}", slice.label()).into(),
                );
            }
            let bytes = self
                .db
                .get_atom(identity)?
                .ok_or_else(|| format!("missing AtomDB RAG event {identity}"))?;
            let record: DurableRecord<T> = serde_json::from_slice(&bytes)?;
            if record.schema_version != SCHEMA_VERSION || record.slice != slice.label() {
                return Err(format!(
                    "unexpected AtomDB RAG record at {identity}: schema {}, slice {}",
                    record.schema_version, record.slice
                )
                .into());
            }
            records.push((record.ordinal, record.payload));
            cursor = self
                .db
                .bonds_from(identity)
                .into_iter()
                .find_map(|(_, bond)| (bond.relation == relation).then_some(bond.target));
        }

        records.sort_by_key(|(ordinal, _)| *ordinal);
        Ok(records.into_iter().map(|(_, payload)| payload).collect())
    }

    pub(crate) fn db_mut(&mut self) -> &mut AtomDb {
        &mut self.db
    }

    pub(crate) fn db(&self) -> &AtomDb {
        &self.db
    }

    fn head(&self, slice: Slice) -> Option<Digest> {
        match slice {
            Slice::General => self.general_head,
            Slice::Learning => self.learning_head,
        }
    }

    fn head_mut(&mut self, slice: Slice) -> &mut Option<Digest> {
        match slice {
            Slice::General => &mut self.general_head,
            Slice::Learning => &mut self.learning_head,
        }
    }
}

fn atom_identity(bytes: &[u8]) -> Digest {
    let mut cell = Cell::new();
    cell.put_atom(bytes)
}
