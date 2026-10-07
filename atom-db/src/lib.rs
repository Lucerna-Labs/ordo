//! Atom DB: immutable atom/bond storage plus bounded cognitive retrieval.
//!
//! Ordo keeps this complete workspace-local copy so the native retrieval and
//! learning laws stay attached to the persistence substrate they were built
//! for.

mod cell;
mod cognitive;
mod digest;
mod retrieval;
mod store;

pub use cell::{Cell, CellReceipt, RootVersion, Snapshot};
pub use cognitive::{
    ActivatedAtom, BondMemory, CognitiveConfig, CognitiveEngine, LearningScope, RecallReport,
    RecallThread,
};
pub use digest::{Digest, digest};
pub use retrieval::{
    ContextPacket, Evidence, EvidenceThread, Polarity, ReinforceReceipt, RememberReceipt,
    RetrievalConfig, RetrievalCue, Retriever,
};
pub use store::{AccessMode, AtomDb, Bond, Error, Stats};
