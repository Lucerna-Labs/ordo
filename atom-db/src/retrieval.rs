use crate::{
    AtomDb, Bond, Cell, CellReceipt, CognitiveConfig, CognitiveEngine, Digest, Error, RecallReport,
};
use atom_retrieval_field::{
    Arc, Cue, FieldConfig, FieldReport, activate, passages_with_overlap, terms,
};
pub use atom_retrieval_field::{ContextPacket, Evidence, EvidenceThread, RetrievalCue};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

const TERM_DOMAIN: &[u8] = b"atom-db/retrieval/term/v1\0";
const MENTIONS: &[u8] = b"atom-db/retrieval/mentions/v1";
const METADATA_MENTIONS: &[u8] = b"atom-db/retrieval/metadata-mentions/v1";
const FROM_SOURCE: &[u8] = b"atom-db/retrieval/from-source/v1";
const FEEDBACK_DOMAIN: &str = "atom-db/retrieval/feedback/v1";
const FEEDBACK_MARKER: &[u8] = b"atom-db/retrieval/feedback-marker/v1";
const LEARNED_BRIDGE: &[u8] = b"atom-db/retrieval/learned-bridge/v1";

/// Conductance deltas per feedback count (per mille).
const STRENGTHEN_DELTA: i64 = 250;
const WEAKEN_DELTA: i64 = 400;
const MENTIONS_BASE: i64 = 1_400;
const MAX_CONDUCTANCE: i64 = 4_000;

/// The polarity of a reinforcement event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Polarity {
    /// The evidence helped: its mention arcs gain conductance.
    Strengthen,
    /// The evidence misled: its mention arcs lose conductance.
    Weaken,
}

impl Polarity {
    fn slug(self) -> &'static str {
        match self {
            Self::Strengthen => "strengthen",
            Self::Weaken => "weaken",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "strengthen" => Some(Self::Strengthen),
            "weaken" => Some(Self::Weaken),
            _ => None,
        }
    }
}

/// What one reinforcement event committed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReinforceReceipt {
    pub cell: CellReceipt,
    pub passage: Digest,
    pub polarity: Polarity,
    /// The new feedback count for this (passage, polarity) pair.
    pub count: u64,
    /// Effective per-mille shift now applied to the passage's mention arcs.
    pub effective_delta_per_mille: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetrievalConfig {
    pub field: FieldConfig,
    /// The original AtomDB cognitive observation field. The lexical field is
    /// only a bounded natural-language adapter; this engine owns contextual
    /// recall, conductance learning, inhibition, and cognitive ranking.
    pub cognitive: CognitiveConfig,
    pub passage_bytes: usize,
    pub max_document_bytes: usize,
    pub max_terms_per_passage: usize,
    pub max_query_terms: usize,
    pub max_evidence: usize,
    pub max_context_bytes: usize,
    pub minimum_cue_support: usize,
    /// Trailing bytes shared between consecutive passages so an answer
    /// spanning a boundary survives whole in the next window. Zero keeps
    /// the pre-Stage-8 hard-boundary behavior.
    pub passage_overlap_bytes: usize,
    /// Stage 8: rare cues inject more activation (document-frequency
    /// information gain) instead of a flat signal per term.
    pub information_gain: bool,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            field: FieldConfig::default(),
            cognitive: CognitiveConfig::default(),
            passage_bytes: 768,
            max_document_bytes: 8 * 1024 * 1024,
            max_terms_per_passage: 256,
            max_query_terms: 24,
            max_evidence: 8,
            max_context_bytes: 8 * 1024,
            minimum_cue_support: 2,
            passage_overlap_bytes: 128,
            information_gain: true,
        }
    }
}

impl RetrievalConfig {
    fn validate(self) -> Result<Self, String> {
        self.field.validate()?;
        if !(64..=65_536).contains(&self.passage_bytes) {
            return Err("passage bytes must be between 64 and 65536".into());
        }
        if self.passage_overlap_bytes >= self.passage_bytes / 2 {
            return Err("passage overlap must stay below half the passage size".into());
        }
        if self.max_document_bytes < self.passage_bytes {
            return Err("document budget must cover at least one passage".into());
        }
        for (name, value, limit) in [
            ("terms per passage", self.max_terms_per_passage, 16_384),
            ("query terms", self.max_query_terms, 256),
            ("evidence count", self.max_evidence, 4_096),
            ("cue support", self.minimum_cue_support, 256),
        ] {
            if value == 0 || value > limit {
                return Err(format!("{name} must be between 1 and {limit}"));
            }
        }
        if self.max_context_bytes == 0 || self.max_context_bytes > 64 * 1024 * 1024 {
            return Err("context bytes must be between 1 and 67108864".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RememberReceipt {
    pub cell: CellReceipt,
    pub source: Digest,
    pub passages: Vec<Digest>,
    pub unique_terms: usize,
}

/// Stage 8 derived index: the retrieval graph plus per-term document
/// frequency, rebuilt only when the durable snapshot sequence changes.
/// A query against an unchanged snapshot pays no rebuild.
#[derive(Debug, Default)]
struct DerivedIndex {
    snapshot_sequence: u64,
    nodes: Vec<Digest>,
    node_index: BTreeMap<Digest, usize>,
    edges: Vec<Arc>,
    edge_ids: Vec<Digest>,
    passage_ids: BTreeSet<Digest>,
    passage_sources: BTreeMap<Digest, BTreeSet<Digest>>,
    /// mentions-bond count per term atom (document frequency).
    term_passages: BTreeMap<Digest, u64>,
    /// Latest feedback counts per passage: (strengthen, weaken).
    feedback: BTreeMap<Digest, (u64, u64)>,
    /// How many times this index has been rebuilt (telemetry for the
    /// unchanged-snapshot law).
    rebuilds: usize,
}

#[derive(Debug)]
pub struct Retriever {
    config: RetrievalConfig,
    index: Mutex<DerivedIndex>,
    cognitive: Mutex<CognitiveEngine>,
}

impl Retriever {
    pub fn new(config: RetrievalConfig) -> Result<Self, String> {
        let config = config.validate()?;
        Ok(Self {
            cognitive: Mutex::new(CognitiveEngine::new(config.cognitive)?),
            config,
            index: Mutex::new(DerivedIndex::default()),
        })
    }

    /// How many times the derived index has been rebuilt. After the first
    /// query, an unchanged snapshot must never move this counter.
    pub fn index_rebuilds(&self) -> usize {
        self.index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rebuilds
    }

    /// Number of explicit outcomes learned by AtomDB's cognitive engine.
    pub fn cognitive_observations(&self) -> u64 {
        self.cognitive
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .observations()
    }

    /// Number of bounded guarded-relay context traces currently retained.
    pub fn cognitive_context_traces(&self) -> usize {
        self.cognitive
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .context_trace_count()
    }

    pub fn remember(
        &self,
        db: &mut AtomDb,
        source: &str,
        document: &str,
    ) -> Result<RememberReceipt, Error> {
        self.remember_with_metadata(db, source, document, "")
    }

    /// Remember a document and attach searchable source metadata to each of
    /// its passages without mixing that metadata into the returned evidence.
    /// This keeps provenance identifiers, titles, URIs, and tags available to
    /// the same native activation field as body terms.
    pub fn remember_with_metadata(
        &self,
        db: &mut AtomDb,
        source: &str,
        document: &str,
        metadata: &str,
    ) -> Result<RememberReceipt, Error> {
        if source.trim().is_empty() || document.trim().is_empty() {
            return Err(Error::Invalid(
                "source and document must not be empty".into(),
            ));
        }
        if document.len() > self.config.max_document_bytes {
            return Err(Error::Invalid(
                "document exceeds retrieval ingestion budget".into(),
            ));
        }
        let passage_texts = passages_with_overlap(
            document,
            self.config.passage_bytes,
            self.config.passage_overlap_bytes,
        )
        .map_err(Error::Invalid)?;
        let mut cell = db.begin_cell();
        let source = cell.put_atom(source.as_bytes());
        let mentions = cell.put_atom(MENTIONS);
        let metadata_mentions = cell.put_atom(METADATA_MENTIONS);
        let from_source = cell.put_atom(FROM_SOURCE);
        let metadata_terms = terms(metadata, self.config.max_terms_per_passage);
        let mut passage_ids = Vec::new();
        let mut unique_terms = BTreeSet::new();
        for passage in passage_texts {
            let passage_id = cell.put_atom(passage.as_bytes());
            passage_ids.push(passage_id);
            cell.put_bond(Bond {
                source: passage_id,
                relation: from_source,
                target: source,
            });
            for term in terms(&passage, self.config.max_terms_per_passage) {
                unique_terms.insert(term.clone());
                let term_id = cell.put_atom(term_bytes(&term));
                cell.put_bond(Bond {
                    source: term_id,
                    relation: mentions,
                    target: passage_id,
                });
            }
            for term in &metadata_terms {
                unique_terms.insert(term.clone());
                let term_id = cell.put_atom(term_bytes(term));
                cell.put_bond(Bond {
                    source: term_id,
                    relation: metadata_mentions,
                    target: passage_id,
                });
            }
        }
        let cell = db.commit_cell(cell)?;
        db.sync()?;
        Ok(RememberReceipt {
            cell,
            source,
            passages: passage_ids,
            unique_terms: unique_terms.len(),
        })
    }

    /// Stage 9 feedback loop: record one reinforcement event for a passage
    /// as a durable, atomic fact. Retrieval never writes; reinforcement is
    /// the only write path and it is always explicit. Each event bumps the
    /// (passage, polarity) count; the derived index folds the effective
    /// delta into the passage's mention-arc conductance on the next rebuild.
    pub fn reinforce(
        &self,
        db: &mut AtomDb,
        passage: Digest,
        polarity: Polarity,
    ) -> Result<ReinforceReceipt, Error> {
        self.reinforce_for_query(db, passage, polarity, "")
    }

    /// Reinforce a passage and, for useful evidence, durably connect the
    /// query's normalized cues to that passage. This is the native write half
    /// of a self-learning RAG loop: later synonymous/user vocabulary can
    /// retrieve confirmed evidence without an embedding model.
    pub fn reinforce_for_query(
        &self,
        db: &mut AtomDb,
        passage: Digest,
        polarity: Polarity,
        query: &str,
    ) -> Result<ReinforceReceipt, Error> {
        if !db.contains_atom(passage) {
            return Err(Error::Invalid(
                "cannot reinforce an atom that does not exist".into(),
            ));
        }
        let current = {
            let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
            let snapshot_sequence = db.snapshot().sequence();
            if index.snapshot_sequence != snapshot_sequence {
                let rebuilds = index.rebuilds + 1;
                *index = build_index(db, snapshot_sequence)?;
                index.rebuilds = rebuilds;
            }
            let (strengthen, weaken) = index.feedback.get(&passage).copied().unwrap_or((0, 0));
            match polarity {
                Polarity::Strengthen => strengthen,
                Polarity::Weaken => weaken,
            }
        };
        let count = current + 1;
        let relation = format!("{FEEDBACK_DOMAIN}/{}/{count}", polarity.slug());
        let mut cell = db.begin_cell();
        let relation_id = cell.put_atom(relation.as_bytes());
        let marker = cell.put_atom(FEEDBACK_MARKER);
        cell.put_bond(Bond {
            source: passage,
            relation: relation_id,
            target: marker,
        });
        if polarity == Polarity::Strengthen {
            let mentions = cell.put_atom(MENTIONS);
            for term in terms(query, self.config.max_query_terms) {
                let term = cell.put_atom(term_bytes(&term));
                cell.put_bond(Bond {
                    source: term,
                    relation: mentions,
                    target: passage,
                });
            }
        }
        let cell = db.commit_cell(cell)?;
        db.sync()?;
        let receipt = ReinforceReceipt {
            cell,
            passage,
            polarity,
            count,
            effective_delta_per_mille: effective_delta(match polarity {
                Polarity::Strengthen => (count, 0),
                Polarity::Weaken => (0, count),
            }),
        };
        // The durable event is the restart receipt; AtomDB's original
        // cognitive field remains the authority that learns which traversed
        // route helped or misled in this context.
        self.observe_feedback_for_query(db, passage, polarity == Polarity::Strengthen, query)?;
        Ok(receipt)
    }

    /// Add a durable semantic bridge between two normalized retrieval terms.
    /// Used only when importing learned associations from an older backend.
    pub fn link_terms(&self, db: &mut AtomDb, left: &str, right: &str) -> Result<(), Error> {
        let Some(left) = terms(left, 1).into_iter().next() else {
            return Ok(());
        };
        let Some(right) = terms(right, 1).into_iter().next() else {
            return Ok(());
        };
        if left == right {
            return Ok(());
        }
        let mut cell = db.begin_cell();
        let left = cell.put_atom(term_bytes(&left));
        let right = cell.put_atom(term_bytes(&right));
        let relation = cell.put_atom(LEARNED_BRIDGE);
        cell.put_bond(Bond {
            source: left,
            relation,
            target: right,
        });
        db.commit_cell(cell)?;
        db.sync()?;
        Ok(())
    }

    /// Feed an explicit retrieval judgment into AtomDB's original cognitive
    /// observation engine without creating a second durable fact. Ordo uses
    /// this during learning-slice replay to reconstruct derived conductance
    /// exactly from the immutable feedback history.
    pub fn observe_feedback_for_query(
        &self,
        db: &AtomDb,
        passage: Digest,
        useful: bool,
        query: &str,
    ) -> Result<bool, Error> {
        if !db.contains_atom(passage) {
            return Err(Error::Invalid(
                "cannot teach cognition about an atom that does not exist".into(),
            ));
        }
        let Some((cue, contexts)) = self.cognitive_inputs(db, query) else {
            return Ok(false);
        };
        self.cognitive
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recall_with_feedback(db, cue, &contexts, passage, useful)
            .map_err(Error::Invalid)?;
        Ok(true)
    }

    fn cognitive_inputs(&self, db: &AtomDb, query: &str) -> Option<(Digest, Vec<Digest>)> {
        let mut inputs = terms(query, self.config.max_query_terms)
            .into_iter()
            .map(|term| identity(&term_bytes(&term)))
            .filter(|identity| db.contains_atom(*identity))
            .collect::<Vec<_>>();
        if inputs.is_empty() {
            return None;
        }
        let cue = inputs.remove(0);
        Some((cue, inputs))
    }

    fn cognitive_recall(&self, db: &AtomDb, query: &str) -> Result<Option<RecallReport>, Error> {
        let Some((cue, contexts)) = self.cognitive_inputs(db, query) else {
            return Ok(None);
        };
        self.cognitive
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recall(db, cue, &contexts, None)
            .map(Some)
            .map_err(Error::Invalid)
    }

    pub fn retrieve(&self, db: &mut AtomDb, query: &str) -> Result<ContextPacket<Digest>, Error> {
        self.retrieve_bounded(
            db,
            query,
            None,
            self.config.max_evidence,
            self.config.max_context_bytes,
        )
    }

    /// Retrieve only passages in `allowed_passages`, with request-local output
    /// bounds. The traversal field may observe the whole immutable graph, but
    /// stale document versions and out-of-scope collections cannot enter the
    /// returned context packet.
    pub fn retrieve_filtered(
        &self,
        db: &mut AtomDb,
        query: &str,
        allowed_passages: &BTreeSet<Digest>,
        max_evidence: usize,
        max_context_bytes: usize,
    ) -> Result<ContextPacket<Digest>, Error> {
        if max_evidence == 0 || max_context_bytes == 0 {
            return Err(Error::Invalid(
                "filtered retrieval bounds must be nonzero".into(),
            ));
        }
        self.retrieve_bounded(
            db,
            query,
            Some(allowed_passages),
            max_evidence,
            max_context_bytes,
        )
    }

    fn retrieve_bounded(
        &self,
        db: &mut AtomDb,
        query: &str,
        allowed_passages: Option<&BTreeSet<Digest>>,
        max_evidence: usize,
        max_context_bytes: usize,
    ) -> Result<ContextPacket<Digest>, Error> {
        let snapshot_sequence = db.snapshot().sequence();
        let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        if index.snapshot_sequence != snapshot_sequence {
            let rebuilds = index.rebuilds + 1;
            *index = build_index(db, snapshot_sequence)?;
            index.rebuilds = rebuilds;
        }
        let index = &*index;

        let query_terms = terms(query, self.config.max_query_terms);
        let mut cues = Vec::new();
        let mut field_cues = Vec::new();
        let mut known_terms = Vec::new();
        let total_passages = index.passage_ids.len() as u64;
        for term in query_terms {
            let identity = identity(&term_bytes(&term));
            let node = index.node_index.get(&identity).copied();
            cues.push(RetrievalCue {
                term: term.clone(),
                identity,
                known: node.is_some(),
            });
            if let Some(node) = node {
                known_terms.push(term.clone());
                let activation = cue_activation(
                    identity,
                    total_passages,
                    index,
                    self.config.information_gain,
                );
                field_cues.push(Cue { node, activation });
            }
        }
        let scoped_edges;
        let field_edges = if let Some(allowed) = allowed_passages {
            // Immutable history may contain replaced versions and passages
            // from other collections. They must not participate as hidden
            // relay nodes in a scoped query, not merely be removed from the
            // final evidence list.
            scoped_edges = index
                .edges
                .iter()
                .copied()
                .filter(|arc| {
                    [arc.from, arc.to].into_iter().all(|node| {
                        index.nodes.get(node).is_none_or(|identity| {
                            !index.passage_ids.contains(identity) || allowed.contains(identity)
                        })
                    })
                })
                .collect::<Vec<_>>();
            scoped_edges.as_slice()
        } else {
            index.edges.as_slice()
        };
        let field =
            activate(field_edges, &field_cues, self.config.field).map_err(Error::Invalid)?;
        let cognitive = self.cognitive_recall(db, query)?;
        self.packet(
            db,
            query,
            snapshot_sequence,
            cues,
            known_terms,
            &index.nodes,
            &index.edge_ids,
            &index.passage_ids,
            &index.passage_sources,
            field,
            cognitive,
            allowed_passages,
            max_evidence,
            max_context_bytes,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn packet(
        &self,
        db: &mut AtomDb,
        query: &str,
        snapshot_sequence: u64,
        cues: Vec<RetrievalCue<Digest>>,
        known_terms: Vec<String>,
        nodes: &[Digest],
        edge_ids: &[Digest],
        passage_ids: &BTreeSet<Digest>,
        passage_sources: &BTreeMap<Digest, BTreeSet<Digest>>,
        mut field: FieldReport,
        cognitive: Option<RecallReport>,
        allowed_passages: Option<&BTreeSet<Digest>>,
        max_evidence: usize,
        max_context_bytes: usize,
    ) -> Result<ContextPacket<Digest>, Error> {
        // The multi-cue field supplies bounded textual candidate generation
        // and per-cue provenance. AtomDB's original CognitiveEngine owns the
        // contextual ordering of those candidates. A field-only candidate is
        // retained as a fail-closed fallback, but every cognitively reached
        // passage sorts ahead by learned activation.
        let cognitive_rank = cognitive
            .into_iter()
            .flat_map(|report| report.ranked)
            .map(|atom| (atom.identity, atom))
            .collect::<BTreeMap<_, _>>();
        field.hits.sort_by(|left, right| {
            let left = nodes
                .get(left.node)
                .and_then(|identity| cognitive_rank.get(identity));
            let right = nodes
                .get(right.node)
                .and_then(|identity| cognitive_rank.get(identity));
            match (left, right) {
                (Some(left), Some(right)) => right
                    .activation
                    .cmp(&left.activation)
                    .then_with(|| right.incoming_signals.cmp(&left.incoming_signals))
                    .then_with(|| left.depth.cmp(&right.depth)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        let mut evidence = Vec::new();
        let mut context_bytes = 0;
        for hit in &field.hits {
            let Some(identity) = nodes.get(hit.node).copied() else {
                continue;
            };
            if !passage_ids.contains(&identity)
                || allowed_passages.is_some_and(|allowed| !allowed.contains(&identity))
                || evidence.len() == max_evidence
            {
                continue;
            }
            let Some(bytes) = db.get_atom(identity)? else {
                continue;
            };
            let full = String::from_utf8_lossy(&bytes);
            let remaining = max_context_bytes.saturating_sub(context_bytes);
            if remaining == 0 {
                break;
            }
            let (text, truncated) = excerpt(&full, remaining);
            context_bytes += text.len();
            let mut sources = Vec::new();
            for source in passage_sources.get(&identity).into_iter().flatten() {
                if let Some(bytes) = db.get_atom(*source)? {
                    sources.push(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
            let supporting_cues = hit
                .supporting_cues
                .iter()
                .filter_map(|index| known_terms.get(*index).cloned())
                .collect::<Vec<_>>();
            let threads = hit
                .threads
                .iter()
                .filter_map(|thread| {
                    let cue = known_terms.get(thread.cue_index)?.clone();
                    let atoms = thread
                        .nodes
                        .iter()
                        .filter_map(|node| nodes.get(*node).copied())
                        .collect();
                    let bonds = thread
                        .edges
                        .iter()
                        .filter_map(|edge| edge_ids.get(*edge).copied())
                        .collect();
                    Some(EvidenceThread { cue, atoms, bonds })
                })
                .collect();
            let cognitive = cognitive_rank.get(&identity);
            evidence.push(Evidence {
                identity,
                text,
                sources,
                activation: cognitive.map_or(hit.activation, |atom| {
                    atom.activation.saturating_add(hit.activation)
                }),
                depth: cognitive.map_or(hit.depth, |atom| atom.depth),
                incoming_signals: cognitive
                    .map_or(hit.incoming_signals, |atom| atom.incoming_signals),
                supporting_cues,
                threads,
                truncated,
            });
        }
        let required = self
            .config
            .minimum_cue_support
            .min(known_terms.len())
            .max(1);
        let answerable = evidence
            .first()
            .is_some_and(|item| item.supporting_cues.len() >= required);
        Ok(ContextPacket {
            query: query.to_string(),
            snapshot_sequence,
            answerable,
            insufficient_evidence: !answerable,
            cues,
            evidence,
            rounds: field.rounds,
            explored_edges: field.explored_edges,
            working_set_peak: field.working_set_peak,
            promoted_edges: field.promoted_edges,
            stabilized: field.stabilized,
            budget_exhausted: field.budget_exhausted,
            context_bytes,
        })
    }
}

impl Default for Retriever {
    fn default() -> Self {
        Self::new(RetrievalConfig::default()).expect("default retrieval config is valid")
    }
}

/// Rebuild the derived index from the committed bond set. Called only when
/// the snapshot sequence moved. Two passes over the in-memory bond list:
/// feedback counts first (hash order puts them anywhere), then arcs.
fn build_index(db: &mut AtomDb, snapshot_sequence: u64) -> Result<DerivedIndex, Error> {
    let mentions = identity(MENTIONS);
    let metadata_mentions = identity(METADATA_MENTIONS);
    let from_source = identity(FROM_SOURCE);
    let mut index = DerivedIndex {
        snapshot_sequence,
        ..DerivedIndex::default()
    };
    let bonds = db.all_bonds();
    // Pass 1: fold feedback events into per-passage counts. Feedback bonds
    // never enter the traversal graph as ordinary arcs.
    let mut feedback_bonds = BTreeSet::new();
    for (edge_id, bond) in &bonds {
        if let Some(bytes) = db.get_atom(bond.relation)?
            && let Some((polarity, count)) = parse_feedback(&bytes)
        {
            let entry = index.feedback.entry(bond.source).or_insert((0, 0));
            match polarity {
                Polarity::Strengthen => entry.0 = entry.0.max(count),
                Polarity::Weaken => entry.1 = entry.1.max(count),
            }
            feedback_bonds.insert(*edge_id);
        }
    }
    // Pass 2: build the traversal graph; reinforcement shifts only arcs
    // INTO a reinforced passage.
    let boost = |to: Digest| -> i64 {
        let (strengthen, weaken) = index.feedback.get(&to).copied().unwrap_or((0, 0));
        effective_delta((strengthen, weaken))
    };
    for (edge_id, bond) in &bonds {
        if feedback_bonds.contains(edge_id) {
            continue;
        }
        let source = intern(bond.source, &mut index.nodes, &mut index.node_index);
        let target = intern(bond.target, &mut index.nodes, &mut index.node_index);
        let conductance = if bond.relation == mentions || bond.relation == metadata_mentions {
            index.passage_ids.insert(bond.target);
            *index.term_passages.entry(bond.source).or_insert(0) += 1;
            MENTIONS_BASE
        } else if bond.relation == from_source {
            index
                .passage_sources
                .entry(bond.source)
                .or_default()
                .insert(bond.target);
            600
        } else {
            900
        };
        let edge = index.edge_ids.len();
        index.edge_ids.push(*edge_id);
        let into = (conductance + boost(bond.target)).clamp(1, MAX_CONDUCTANCE) as u16;
        index.edges.push(Arc {
            from: source,
            to: target,
            edge,
            conductance_per_mille: into,
        });
        // Metadata is searchable, but it is not a co-occurrence bridge:
        // body cues cannot fan out through a shared title/tag/URI token into
        // unrelated passages.
        if bond.relation != metadata_mentions {
            index.edges.push(Arc {
                from: target,
                to: source,
                edge,
                conductance_per_mille: conductance as u16,
            });
        }
    }
    Ok(index)
}

/// Parse a feedback relation atom: `<domain>/<polarity>/<count>`.
fn parse_feedback(bytes: &[u8]) -> Option<(Polarity, u64)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let tail = text.strip_prefix(FEEDBACK_DOMAIN)?;
    let (polarity, count) = tail.strip_prefix('/')?.split_once('/')?;
    Some((Polarity::from_slug(polarity)?, count.parse().ok()?))
}

/// Effective per-mille conductance shift for a (strengthen, weaken) pair.
fn effective_delta(counts: (u64, u64)) -> i64 {
    counts.0 as i64 * STRENGTHEN_DELTA - counts.1 as i64 * WEAKEN_DELTA
}

/// Stage 8 information gain: a cue's injected activation scales with how
/// rare its term is across the committed passages (1 + log2(1 + N/df)).
/// With information gain disabled, every cue injects the flat pre-Stage-8
/// signal. Deterministic for a given snapshot and configuration.
fn cue_activation(
    term: Digest,
    total_passages: u64,
    index: &DerivedIndex,
    information_gain: bool,
) -> u64 {
    const BASE: u64 = 1_000_000;
    if !information_gain {
        return BASE;
    }
    let df = index.term_passages.get(&term).copied().unwrap_or(0);
    if total_passages == 0 || df == 0 {
        return BASE;
    }
    let gain = 1.0 + (1.0 + total_passages as f64 / df as f64).log2();
    (BASE as f64 * gain) as u64
}

fn identity(bytes: &[u8]) -> Digest {
    Cell::new().put_atom(bytes)
}

fn term_bytes(term: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(TERM_DOMAIN.len() + term.len());
    bytes.extend_from_slice(TERM_DOMAIN);
    bytes.extend_from_slice(term.as_bytes());
    bytes
}

fn intern(identity: Digest, nodes: &mut Vec<Digest>, index: &mut BTreeMap<Digest, usize>) -> usize {
    if let Some(node) = index.get(&identity) {
        return *node;
    }
    let node = nodes.len();
    nodes.push(identity);
    index.insert(identity, node);
    node
}

fn excerpt(text: &str, budget: usize) -> (String, bool) {
    if text.len() <= budget {
        return (text.to_string(), false);
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1
    }
    (text[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "atom-db-retrieval-{label}-{}-{}.atoms",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn multi_cue_retrieval_returns_grounded_provenance_without_mutation() {
        let path = path("grounded");
        let mut db = AtomDb::open(&path).unwrap();
        let retriever = Retriever::default();
        retriever.remember(
            &mut db, "observer-leases.md",
            "A writer crash makes the operating system release its lease. Lease recovery permits a replacement writer.",
        ).unwrap();
        retriever
            .remember(&mut db, "astronomy.md", "Earth orbits the Sun.")
            .unwrap();
        drop(db);
        let mut db = AtomDb::open_read_only(&path).unwrap();
        let before = db.stats().unwrap();
        let packet = retriever
            .retrieve(&mut db, "writer crash lease recovery")
            .unwrap();
        let after = db.stats().unwrap();
        assert!(packet.answerable);
        assert!(packet.evidence[0].text.contains("writer crash"));
        assert!(packet.evidence[0].supporting_cues.len() >= 3);
        assert_eq!(packet.evidence[0].sources, vec!["observer-leases.md"]);
        assert!(!packet.evidence[0].threads.is_empty());
        assert_eq!(before, after);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn unknown_cues_fail_closed() {
        let path = path("unknown");
        let mut db = AtomDb::open(&path).unwrap();
        let packet = Retriever::default()
            .retrieve(&mut db, "quasar marmalade")
            .unwrap();
        assert!(packet.insufficient_evidence);
        assert!(packet.evidence.is_empty());
        assert!(packet.cues.iter().all(|cue| !cue.known));
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn source_metadata_is_searchable_but_not_injected_into_evidence() {
        let path = path("metadata");
        let mut db = AtomDb::open(&path).unwrap();
        let retriever = Retriever::default();
        retriever
            .remember_with_metadata(
                &mut db,
                "opaque-source-id",
                "The process is repaired after repeated failures.",
                "self-heal-skill.md Self-heal operator guide",
            )
            .unwrap();
        let packet = retriever.retrieve(&mut db, "self heal skill").unwrap();
        assert!(packet.answerable);
        assert_eq!(
            packet.evidence[0].text,
            "The process is repaired after repeated failures."
        );
        assert!(!packet.evidence[0].text.contains("operator guide"));
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn unchanged_snapshot_never_rebuilds_the_index() {
        let path = path("cache-law");
        let mut db = AtomDb::open(&path).unwrap();
        let retriever = Retriever::default();
        retriever
            .remember(&mut db, "a.md", "leases recover after writer crashes.")
            .unwrap();
        assert_eq!(retriever.index_rebuilds(), 0);
        retriever.retrieve(&mut db, "writer crash lease").unwrap();
        assert_eq!(retriever.index_rebuilds(), 1);
        // Unchanged snapshot: identical result, no rebuild.
        let first = retriever.retrieve(&mut db, "writer crash lease").unwrap();
        let second = retriever.retrieve(&mut db, "writer crash lease").unwrap();
        assert_eq!(first, second);
        assert_eq!(retriever.index_rebuilds(), 1);
        // A committed change moves the snapshot and forces one rebuild.
        retriever
            .remember(&mut db, "b.md", "astronomy has nothing to do with leases.")
            .unwrap();
        retriever.retrieve(&mut db, "writer crash lease").unwrap();
        assert_eq!(retriever.index_rebuilds(), 2);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rare_cues_inject_more_activation_than_common_cues() {
        let path = path("gain-law");
        let mut db = AtomDb::open(&path).unwrap();
        let retriever = Retriever::default();
        // "common" appears in every passage; "zephyr" appears in exactly one.
        retriever
            .remember(&mut db, "d1.md", "common words everywhere. common again.")
            .unwrap();
        retriever
            .remember(&mut db, "d2.md", "common ground, common sense.")
            .unwrap();
        retriever
            .remember(&mut db, "d3.md", "a zephyr is anything but common.")
            .unwrap();
        let packet = retriever.retrieve(&mut db, "common zephyr").unwrap();
        assert!(packet.answerable);
        // The passage supported by the rare cue must win despite the common
        // term appearing in three times as many passages.
        assert_eq!(packet.evidence[0].sources, vec!["d3.md"]);
        assert!(
            packet.evidence[0]
                .supporting_cues
                .contains(&"zephyr".to_string())
        );
        // Direct law check: the rare cue's injected activation dominates.
        let index = retriever.index.lock().unwrap_or_else(|e| e.into_inner());
        let total = index.passage_ids.len() as u64;
        let zephyr = identity(&term_bytes("zephyr"));
        let common = identity(&term_bytes("common"));
        let rare_activation = cue_activation(zephyr, total, &index, true);
        let common_activation = cue_activation(common, total, &index, true);
        assert!(rare_activation > common_activation);
        drop(index);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn cognitive_feedback_alone_controls_contextual_ranking() {
        let path = path("cognitive-authority");
        let mut db = AtomDb::open(&path).unwrap();
        let retriever = Retriever::default();
        retriever
            .remember(&mut db, "money.md", "bank route money account balance")
            .unwrap();
        let river = retriever
            .remember(&mut db, "river.md", "bank route river water channel")
            .unwrap();

        // This path deliberately bypasses Retriever's durable reinforcement
        // facts. A ranking change therefore has to come from the original
        // AtomDB CognitiveEngine and its context-gated conductance memory.
        for _ in 0..8 {
            assert!(
                retriever
                    .observe_feedback_for_query(&db, river.passages[0], true, "bank route")
                    .unwrap()
            );
        }
        let packet = retriever.retrieve(&mut db, "bank route").unwrap();

        assert_eq!(retriever.cognitive_observations(), 8);
        assert_eq!(packet.evidence[0].sources, vec!["river.md"]);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reinforcement_lifts_helpful_evidence_and_is_durable() {
        let path = path("feedback-strengthen");
        let mut db = AtomDb::open(&path).unwrap();
        let retriever = Retriever::default();
        // Two competing passages about "lease": one strong (multi-cue), one
        // weak (single cue, off-topic tail).
        let strong = retriever.remember(
            &mut db, "strong.md",
            "A writer crash makes the operating system release its lease. Lease recovery permits a replacement writer.",
        ).unwrap();
        let weak = retriever
            .remember(&mut db, "weak.md", "The lease expired quietly.")
            .unwrap();
        let before = retriever
            .retrieve(&mut db, "writer crash lease recovery")
            .unwrap();
        assert_eq!(before.evidence[0].sources, vec!["strong.md"]);
        let weak_activation_before = before
            .evidence
            .iter()
            .find(|item| item.sources == vec!["weak.md"])
            .map(|item| item.activation)
            .unwrap_or(0);

        // Reinforce the weak passage as if it kept helping.
        let mut receipt = retriever
            .reinforce(&mut db, weak.passages[0], Polarity::Strengthen)
            .unwrap();
        assert_eq!(receipt.count, 1);
        assert_eq!(receipt.effective_delta_per_mille, STRENGTHEN_DELTA);
        receipt = retriever
            .reinforce(&mut db, weak.passages[0], Polarity::Strengthen)
            .unwrap();
        assert_eq!(receipt.count, 2);

        let after = retriever
            .retrieve(&mut db, "writer crash lease recovery")
            .unwrap();
        let weak_after = after
            .evidence
            .iter()
            .find(|item| item.sources == vec!["weak.md"])
            .expect("weak passage must still surface");
        assert!(
            weak_after.activation > weak_activation_before,
            "reinforcement must lift activation: before={weak_activation_before} after={}",
            weak_after.activation
        );

        // Durability: the feedback survives a reopen.
        let strong_id = strong.passages[0];
        let weak_id = weak.passages[0];
        drop(db);
        let mut db = AtomDb::open_read_only(&path).unwrap();
        let reopened = retriever
            .retrieve(&mut db, "writer crash lease recovery")
            .unwrap();
        let weak_reopened = reopened
            .evidence
            .iter()
            .find(|item| item.identity == weak_id)
            .expect("reinforced passage must survive reopen");
        assert!(weak_reopened.activation > weak_activation_before);
        // The unreinforced passage keeps its base conductance effect.
        assert!(
            reopened
                .evidence
                .iter()
                .any(|item| item.identity == strong_id)
        );
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn weakening_buries_misleading_evidence() {
        let path = path("feedback-weaken");
        let mut db = AtomDb::open(&path).unwrap();
        let retriever = Retriever::default();
        let receipt = retriever
            .remember(
                &mut db,
                "mislead.md",
                "Lease recovery after a crash is simple: just reboot everything, always.",
            )
            .unwrap();
        retriever
            .remember(
                &mut db,
                "careful.md",
                "A writer crash releases its lease; a replacement writer may recover.",
            )
            .unwrap();
        let before = retriever
            .retrieve(&mut db, "lease crash recovery writer")
            .unwrap();
        let target = before
            .evidence
            .iter()
            .find(|item| item.sources == vec!["mislead.md"])
            .expect("misleading passage must surface before suppression");
        let activation_before = target.activation;

        for _ in 0..4 {
            retriever
                .reinforce(&mut db, receipt.passages[0], Polarity::Weaken)
                .unwrap();
        }
        let after = retriever
            .retrieve(&mut db, "lease crash recovery writer")
            .unwrap();
        let suppressed = after
            .evidence
            .iter()
            .find(|item| item.sources == vec!["mislead.md"]);
        match suppressed {
            Some(item) => assert!(
                item.activation < activation_before,
                "weakening must drop activation: before={activation_before} after={}",
                item.activation
            ),
            None => {
                // Clamped to the minimum and outranked out of the packet:
                // also a lawful burial.
            }
        }
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reinforce_rejects_unknown_atoms_and_queries_stay_read_only() {
        let path = path("feedback-guards");
        let mut db = AtomDb::open(&path).unwrap();
        let retriever = Retriever::default();
        let missing = Cell::new().put_atom(b"no such passage");
        assert!(
            retriever
                .reinforce(&mut db, missing, Polarity::Strengthen)
                .is_err()
        );
        retriever
            .remember(&mut db, "a.md", "leases recover after writer crashes.")
            .unwrap();
        let before = db.stats().unwrap();
        retriever.retrieve(&mut db, "writer crash lease").unwrap();
        let after = db.stats().unwrap();
        assert_eq!(before, after, "queries must never mutate durable state");
        drop(db);
        fs::remove_file(path).unwrap();
    }
}
