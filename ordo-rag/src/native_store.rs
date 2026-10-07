//! Ordo's live RAG backend, built on AtomDB's native retrieval membrane.
//!
//! The immutable AtomDB graph is the only live RAG database. A small Rust
//! catalog reconstructed from the general root maps active passage atoms back
//! to Ordo document metadata; the learning root records source-scoped feedback.

use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    DynError, GeneralRagEvent, LearningRagEvent, NativePassageRecord, RagChunkRecord,
    RagStorageBudget,
    atom_persistence::{AtomRagPersistence, Slice as AtomSlice},
    balance_rag_hits, content_hash, embed_or_empty, storage_document_id,
};
use atom_db::{CognitiveConfig, Digest, LearningScope, Polarity, RetrievalConfig, Retriever};
use chrono::Utc;
use ordo_models::{EmbeddingClient, EmbeddingRequest, cosine_similarity};
use ordo_protocol::{
    RAG_COLLECTION_MAIN, RagCollectionSummary, RagDocument, RagHit, normalize_rag_collection_name,
    normalize_rag_collections, rag_collection_group, rag_collection_label,
};

const NATIVE_PASSAGE_BYTES: usize = 768;
const MAX_REQUEST_CONTEXT_BYTES: usize = 8 * 1024 * 1024;
const USEFUL_SOURCE_BONUS: f32 = 0.75;
const USELESS_SOURCE_PENALTY: f32 = 0.9;
static EPHEMERAL_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
struct PassageCatalogEntry {
    identity: Digest,
    chunk_index: usize,
    embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
struct ActiveDocument {
    storage_document_id: String,
    source_document_id: String,
    collection: String,
    uri: String,
    title: String,
    tags: Vec<String>,
    indexed_at: String,
    version: String,
    size_bytes: usize,
    passages: Vec<PassageCatalogEntry>,
}

#[derive(Default)]
struct NativeCatalog {
    documents: BTreeMap<String, ActiveDocument>,
    source_feedback: HashMap<(String, String, Digest), f32>,
}

#[derive(Debug, Clone)]
struct LegacyDocument {
    source_document_id: String,
    collection: String,
    uri: String,
    title: String,
    tags: Vec<String>,
    chunks: BTreeMap<usize, String>,
}

pub struct RagStore {
    persistence: RefCell<AtomRagPersistence>,
    retriever: Retriever,
    catalog: NativeCatalog,
    path: Option<PathBuf>,
    embedder: Option<Arc<dyn EmbeddingClient>>,
    budget: RagStorageBudget,
}

impl RagStore {
    pub fn in_memory() -> Self {
        Self::in_memory_with_budget(RagStorageBudget::default())
    }

    pub fn in_memory_with_budget(budget: RagStorageBudget) -> Self {
        Self::from_atomdb(ephemeral_path(), None, budget, None)
            .expect("open workspace-local ephemeral AtomDB RAG store")
    }

    pub fn in_memory_with_embedder(
        embedder: Arc<dyn EmbeddingClient>,
        budget: RagStorageBudget,
    ) -> Self {
        Self::from_atomdb(ephemeral_path(), Some(embedder), budget, None)
            .expect("open workspace-local ephemeral AtomDB RAG store")
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        Self::open_with_budget(path, RagStorageBudget::default())
    }

    pub fn open_with_budget(
        path: impl Into<PathBuf>,
        budget: RagStorageBudget,
    ) -> Result<Self, DynError> {
        let path = path.into();
        Self::from_atomdb(path.clone(), None, budget, Some(path))
    }

    pub fn open_with_embedder(
        path: impl Into<PathBuf>,
        embedder: Arc<dyn EmbeddingClient>,
        budget: RagStorageBudget,
    ) -> Result<Self, DynError> {
        let path = path.into();
        Self::from_atomdb(path.clone(), Some(embedder), budget, Some(path))
    }

    fn from_atomdb(
        physical_path: PathBuf,
        embedder: Option<Arc<dyn EmbeddingClient>>,
        budget: RagStorageBudget,
        exposed_path: Option<PathBuf>,
    ) -> Result<Self, DynError> {
        let mut persistence = AtomRagPersistence::open(physical_path)?;
        let general_events: Vec<GeneralRagEvent> = persistence.replay(AtomSlice::General)?;
        let learning_events: Vec<LearningRagEvent> = persistence.replay(AtomSlice::Learning)?;

        let mut catalog = NativeCatalog::default();
        let mut legacy_documents = BTreeMap::<String, LegacyDocument>::new();
        for event in &general_events {
            replay_general_event(event, &mut catalog, &mut legacy_documents)?;
        }
        for event in &learning_events {
            replay_learning_event(event, &mut catalog)?;
        }

        let retrieval_config = RetrievalConfig {
            passage_bytes: NATIVE_PASSAGE_BYTES,
            // Stage 4 is AtomDB's latest cognitive law: related contexts may
            // share learned conductance only when their bounded graph fields
            // clear the relay guard; opposing contexts remain isolated.
            cognitive: CognitiveConfig {
                learning_scope: LearningScope::GuardedRelay,
                ..CognitiveConfig::default()
            },
            ..RetrievalConfig::default()
        };
        let mut store = Self {
            persistence: RefCell::new(persistence),
            retriever: Retriever::new(retrieval_config).map_err(invalid_native_config)?,
            catalog,
            path: exposed_path,
            embedder,
            budget,
        };

        // Upgrade stores produced by the incomplete persistence-only merge.
        // The old immutable events stay readable, while one native upsert per
        // active document makes future opens use the retrieval field directly.
        for legacy in legacy_documents.into_values() {
            let content = join_legacy_chunks(legacy.chunks.values().cloned());
            if content.trim().is_empty() {
                continue;
            }
            store.upsert_document(&RagDocument {
                document_id: legacy.source_document_id,
                uri: legacy.uri,
                title: legacy.title,
                tags: legacy.tags,
                collection: legacy.collection,
                content,
            })?;
        }
        store.replay_legacy_learning(&learning_events)?;
        store.prune_to_budget()?;
        Ok(store)
    }

    pub fn embedding_backend(&self) -> &str {
        self.embedder
            .as_deref()
            .map(EmbeddingClient::backend_name)
            .unwrap_or("atomdb-retrieval-field")
    }

    pub fn embedding_dimensions(&self) -> usize {
        self.embedder
            .as_deref()
            .map(EmbeddingClient::dimensions)
            .unwrap_or(0)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn chunk_count(&self) -> usize {
        self.catalog
            .documents
            .values()
            .map(|document| document.passages.len())
            .sum()
    }

    pub fn document_count(&self) -> usize {
        self.catalog.documents.len()
    }

    pub fn cognitive_observations(&self) -> u64 {
        self.retriever.cognitive_observations()
    }

    pub fn cognitive_context_traces(&self) -> usize {
        self.retriever.cognitive_context_traces()
    }

    pub fn collection_summaries(&self) -> Vec<RagCollectionSummary> {
        self.collection_summaries_result()
            .expect("list AtomDB RAG collections")
    }

    pub(crate) fn collection_summaries_result(
        &self,
    ) -> Result<Vec<RagCollectionSummary>, DynError> {
        #[derive(Default)]
        struct Summary {
            documents: usize,
            chunks: usize,
            titles: Vec<String>,
        }
        let mut grouped = BTreeMap::<String, Summary>::new();
        for document in self.catalog.documents.values() {
            let summary = grouped.entry(document.collection.clone()).or_default();
            summary.documents += 1;
            summary.chunks += document.passages.len();
            if summary.titles.len() < 3 && !summary.titles.contains(&document.title) {
                summary.titles.push(document.title.clone());
            }
        }
        let names = normalize_rag_collections(&grouped.keys().cloned().collect::<Vec<_>>());
        Ok(names
            .into_iter()
            .filter_map(|name| {
                grouped.remove(&name).map(|summary| RagCollectionSummary {
                    label: rag_collection_label(&name).to_string(),
                    group: rag_collection_group(&name),
                    name,
                    document_count: summary.documents,
                    chunk_count: summary.chunks,
                    sample_titles: summary.titles,
                })
            })
            .collect())
    }

    pub fn is_empty(&self) -> bool {
        self.catalog.documents.is_empty()
    }

    pub fn upsert_document(&mut self, document: &RagDocument) -> Result<usize, DynError> {
        let collection = normalize_rag_collection_name(&document.collection);
        let source_document_id = document.document_id.trim().to_string();
        let storage_document_id = storage_document_id(&collection, &source_document_id);
        let version = content_hash(&document.content);
        if let Some(existing) = self.catalog.documents.get(&storage_document_id) {
            if existing.version == version
                && existing.uri == document.uri
                && existing.title == document.title
                && existing.tags == document.tags
            {
                return Ok(existing.passages.len());
            }
        }

        let source_label = format!(
            "ordo-rag/v2/{}/{}/{}",
            version,
            content_hash(&collection),
            content_hash(&source_document_id)
        );
        let source_metadata = format!(
            "{} {} {} {} {}",
            source_document_id,
            document.uri,
            document.title,
            collection,
            document.tags.join(" ")
        );
        let receipt = {
            let mut persistence = self.persistence.borrow_mut();
            self.retriever.remember_with_metadata(
                persistence.db_mut(),
                &source_label,
                &document.content,
                &source_metadata,
            )?
        };

        let mut passages = Vec::with_capacity(receipt.passages.len());
        let mut size_bytes = storage_document_id.len()
            + source_document_id.len()
            + collection.len()
            + document.uri.len()
            + document.title.len()
            + document.tags.iter().map(String::len).sum::<usize>();
        for (chunk_index, identity) in receipt.passages.into_iter().enumerate() {
            let text = self
                .read_atom_text(identity)?
                .ok_or_else(|| format!("AtomDB omitted remembered passage {identity}"))?;
            size_bytes += text.len();
            passages.push(PassageCatalogEntry {
                identity,
                chunk_index,
                embedding: embed_or_empty(self.embedder.as_ref(), &text)?,
            });
        }
        let indexed_at = Utc::now().to_rfc3339();
        let event = GeneralRagEvent::NativeUpsertDocument {
            storage_document_id: storage_document_id.clone(),
            source_document_id: source_document_id.clone(),
            collection: collection.clone(),
            uri: document.uri.clone(),
            title: document.title.clone(),
            tags: document.tags.clone(),
            indexed_at: indexed_at.clone(),
            version: version.clone(),
            size_bytes,
            passages: passages
                .iter()
                .map(|passage| NativePassageRecord {
                    identity: passage.identity.to_string(),
                    chunk_index: passage.chunk_index,
                    embedding: passage.embedding.clone(),
                })
                .collect(),
        };
        self.persistence
            .borrow_mut()
            .append(AtomSlice::General, &event)?;
        let passage_count = passages.len();
        self.catalog.documents.insert(
            storage_document_id.clone(),
            ActiveDocument {
                storage_document_id,
                source_document_id,
                collection,
                uri: document.uri.clone(),
                title: document.title.clone(),
                tags: document.tags.clone(),
                indexed_at,
                version,
                size_bytes,
                passages,
            },
        );
        self.prune_to_budget()?;
        Ok(passage_count)
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<RagHit> {
        self.search_in_collections(query, top_k, &[])
    }

    pub fn search_in_collections(
        &self,
        query: &str,
        top_k: usize,
        collections: &[String],
    ) -> Vec<RagHit> {
        self.search_result(query, top_k, collections)
            .expect("query native AtomDB RAG store")
    }

    pub(crate) fn search_result(
        &self,
        query: &str,
        top_k: usize,
        collections: &[String],
    ) -> Result<Vec<RagHit>, DynError> {
        if top_k == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let normalized_collections = normalize_rag_collections(collections);
        let allowed = self.allowed_passages(&normalized_collections);
        if allowed.is_empty() {
            return Ok(Vec::new());
        }
        let evidence_limit = (top_k.saturating_mul(4)).clamp(1, 4_096);
        let context_bytes = evidence_limit
            .saturating_mul(NATIVE_PASSAGE_BYTES)
            .clamp(NATIVE_PASSAGE_BYTES, MAX_REQUEST_CONTEXT_BYTES);
        let packet = {
            let mut persistence = self.persistence.borrow_mut();
            self.retriever.retrieve_filtered(
                persistence.db_mut(),
                query,
                &allowed,
                evidence_limit,
                context_bytes,
            )?
        };

        let mut hits = Vec::new();
        for evidence in packet.evidence {
            for document in self.documents_for_passage(evidence.identity, &normalized_collections) {
                let Some(passage) = document
                    .passages
                    .iter()
                    .find(|passage| passage.identity == evidence.identity)
                else {
                    continue;
                };
                let source_bonus = self
                    .catalog
                    .source_feedback
                    .get(&(
                        document.storage_document_id.clone(),
                        document.version.clone(),
                        evidence.identity,
                    ))
                    .copied()
                    .unwrap_or(0.0);
                hits.push(RagHit {
                    document_id: document.source_document_id.clone(),
                    uri: document.uri.clone(),
                    title: document.title.clone(),
                    chunk_index: passage.chunk_index,
                    // Native activation already combines independent cue
                    // convergence, information gain, graph conductance, and
                    // learned reinforcement. Preserve that primary rank.
                    score: evidence.activation as f32
                        * evidence.supporting_cues.len().max(1) as f32
                        / 1_000_000.0
                        + source_bonus,
                    snippet: normalize_evidence(&evidence.text),
                    tags: document.tags.clone(),
                    collection: document.collection.clone(),
                    content_hash: content_hash(&evidence.text),
                });
            }
        }

        // The native term membrane intentionally drops all-stopword queries.
        // Exact/fuzzy and optional embedding fallbacks operate over active
        // passage atoms only; they never revive stale versions.
        let missing_collection_coverage = normalized_collections.len() > 1
            && normalized_collections
                .iter()
                .any(|collection| !hits.iter().any(|hit| &hit.collection == collection));

        if hits.is_empty() || self.embedder.is_some() || missing_collection_coverage {
            self.add_fallback_hits(query, &normalized_collections, &mut hits)?;
        }
        deduplicate_hits(&mut hits);
        hits.sort_by(compare_hits);
        if normalized_collections
            .iter()
            .any(|collection| collection != RAG_COLLECTION_MAIN)
        {
            hits = balance_rag_hits(hits, &normalized_collections, top_k);
        } else {
            hits.truncate(top_k);
        }
        Ok(hits)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_feedback(
        &mut self,
        query: &str,
        document_id: &str,
        collection: &str,
        chunk_index: usize,
        content_hash: &str,
        useful: bool,
    ) -> Result<bool, DynError> {
        let collection = normalize_rag_collection_name(collection);
        let storage_id = storage_document_id(&collection, document_id.trim());
        let Some(document) = self.catalog.documents.get(&storage_id) else {
            return Ok(false);
        };
        let Some(passage) = document
            .passages
            .iter()
            .find(|passage| passage.chunk_index == chunk_index)
        else {
            return Ok(false);
        };
        let identity = passage.identity;
        let version = document.version.clone();
        let text = self
            .read_atom_text(identity)?
            .ok_or_else(|| format!("missing active AtomDB passage {identity}"))?;
        if !content_hash.is_empty() && crate::content_hash(&text) != content_hash {
            return Ok(false);
        }
        {
            let mut persistence = self.persistence.borrow_mut();
            self.retriever.reinforce_for_query(
                persistence.db_mut(),
                identity,
                if useful {
                    Polarity::Strengthen
                } else {
                    Polarity::Weaken
                },
                query,
            )?;
        }
        let event = LearningRagEvent::NativeFeedback {
            query: query.to_string(),
            storage_document_id: storage_id.clone(),
            document_version: version.clone(),
            passage_identity: identity.to_string(),
            useful,
        };
        self.persistence
            .borrow_mut()
            .append(AtomSlice::Learning, &event)?;
        apply_source_feedback(&mut self.catalog, storage_id, version, identity, useful);
        Ok(true)
    }

    pub fn import_legacy_jsonl(&mut self, path: &Path) -> Result<usize, DynError> {
        if !path.exists() {
            return Ok(0);
        }
        let mut grouped = BTreeMap::<String, LegacyDocument>::new();
        for line in BufReader::new(File::open(path)?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let chunk: RagChunkRecord = serde_json::from_str(&line)?;
            let collection = normalize_rag_collection_name(&chunk.collection);
            let storage_id = storage_document_id(&collection, &chunk.document_id);
            let document = grouped
                .entry(storage_id.clone())
                .or_insert_with(|| LegacyDocument {
                    source_document_id: chunk.document_id.clone(),
                    collection: collection.clone(),
                    uri: chunk.uri.clone(),
                    title: chunk.title.clone(),
                    tags: chunk.tags.clone(),
                    chunks: BTreeMap::new(),
                });
            document.chunks.insert(chunk.chunk_index, chunk.text);
        }
        let imported = grouped.values().map(|doc| doc.chunks.len()).sum();
        for document in grouped.into_values() {
            self.upsert_legacy_document(document)?;
        }
        Ok(imported)
    }

    fn upsert_legacy_document(&mut self, document: LegacyDocument) -> Result<(), DynError> {
        let content = join_legacy_chunks(document.chunks.into_values());
        if content.trim().is_empty() {
            return Ok(());
        }
        self.upsert_document(&RagDocument {
            document_id: document.source_document_id,
            uri: document.uri,
            title: document.title,
            tags: document.tags,
            collection: document.collection,
            content,
        })?;
        Ok(())
    }

    fn replay_legacy_learning(&mut self, events: &[LearningRagEvent]) -> Result<(), DynError> {
        for event in events {
            match event {
                LearningRagEvent::NativeFeedback {
                    query,
                    storage_document_id,
                    document_version,
                    passage_identity,
                    useful,
                } => {
                    let identity = Digest::from_str(passage_identity).map_err(invalid_digest)?;
                    let active =
                        self.catalog
                            .documents
                            .get(storage_document_id)
                            .is_some_and(|document| {
                                document.version == *document_version
                                    && document
                                        .passages
                                        .iter()
                                        .any(|passage| passage.identity == identity)
                            });
                    if active {
                        let persistence = self.persistence.borrow();
                        self.retriever.observe_feedback_for_query(
                            persistence.db(),
                            identity,
                            *useful,
                            query,
                        )?;
                    }
                }
                LearningRagEvent::SemanticBridgeSeed { term_a, term_b, .. } => {
                    let mut persistence = self.persistence.borrow_mut();
                    self.retriever
                        .link_terms(persistence.db_mut(), term_a, term_b)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn allowed_passages(&self, collections: &[String]) -> BTreeSet<Digest> {
        self.catalog
            .documents
            .values()
            .filter(|document| collections.is_empty() || collections.contains(&document.collection))
            .flat_map(|document| document.passages.iter().map(|passage| passage.identity))
            .collect()
    }

    fn documents_for_passage<'a>(
        &'a self,
        identity: Digest,
        collections: &'a [String],
    ) -> impl Iterator<Item = &'a ActiveDocument> + 'a {
        self.catalog.documents.values().filter(move |document| {
            (collections.is_empty() || collections.contains(&document.collection))
                && document
                    .passages
                    .iter()
                    .any(|passage| passage.identity == identity)
        })
    }

    fn add_fallback_hits(
        &self,
        query: &str,
        collections: &[String],
        hits: &mut Vec<RagHit>,
    ) -> Result<(), DynError> {
        let query_embedding = self.embedder.as_ref().and_then(|embedder| {
            embedder
                .embed(EmbeddingRequest {
                    input: query.to_string(),
                })
                .map(|response| response.vector)
                .map_err(|error| {
                    tracing::warn!(%error, "AtomDB RAG query embedding failed");
                    error
                })
                .ok()
        });
        let query_lower = query.to_ascii_lowercase();
        let query_terms = lexical_terms(query);
        let existing: HashSet<(String, usize)> = hits
            .iter()
            .map(|hit| {
                (
                    storage_document_id(&hit.collection, &hit.document_id),
                    hit.chunk_index,
                )
            })
            .collect();

        for document in
            self.catalog.documents.values().filter(|document| {
                collections.is_empty() || collections.contains(&document.collection)
            })
        {
            for passage in &document.passages {
                if existing.contains(&(document.storage_document_id.clone(), passage.chunk_index)) {
                    continue;
                }
                let Some(text) = self.read_atom_text(passage.identity)? else {
                    continue;
                };
                let lowered = text.to_ascii_lowercase();
                let mut score = 0.0_f32;
                if !query_lower.is_empty() && lowered.contains(&query_lower) {
                    score += 2.0;
                } else if !query_terms.is_empty() {
                    let text_terms = lexical_terms(&text);
                    let matched = query_terms
                        .iter()
                        .filter(|query| {
                            text_terms.iter().any(|term| {
                                *query == term || padded_bigram_jaccard(query, term) >= 0.58
                            })
                        })
                        .count();
                    if matched == query_terms.len() {
                        score += matched as f32 * 0.8;
                    }
                }
                if let (Some(query_embedding), Some(embedder)) =
                    (query_embedding.as_ref(), self.embedder.as_ref())
                {
                    let passage_embedding = if passage.embedding.is_empty() {
                        embed_or_empty(Some(embedder), &text)?
                    } else {
                        passage.embedding.clone()
                    };
                    score += cosine_similarity(query_embedding, &passage_embedding).max(0.0) * 2.5;
                }
                if score <= 0.0 {
                    continue;
                }
                score += self
                    .catalog
                    .source_feedback
                    .get(&(
                        document.storage_document_id.clone(),
                        document.version.clone(),
                        passage.identity,
                    ))
                    .copied()
                    .unwrap_or(0.0);
                hits.push(RagHit {
                    document_id: document.source_document_id.clone(),
                    uri: document.uri.clone(),
                    title: document.title.clone(),
                    chunk_index: passage.chunk_index,
                    score,
                    snippet: normalize_evidence(&text),
                    tags: document.tags.clone(),
                    collection: document.collection.clone(),
                    content_hash: content_hash(&text),
                });
            }
        }
        Ok(())
    }

    fn read_atom_text(&self, identity: Digest) -> Result<Option<String>, DynError> {
        let bytes = self.persistence.borrow_mut().db_mut().get_atom(identity)?;
        Ok(bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    fn prune_to_budget(&mut self) -> Result<(), DynError> {
        while self.active_size_bytes() > self.budget.max_bytes && self.catalog.documents.len() > 1 {
            let Some(storage_id) = self
                .catalog
                .documents
                .values()
                .min_by(|left, right| {
                    left.indexed_at
                        .cmp(&right.indexed_at)
                        .then_with(|| left.storage_document_id.cmp(&right.storage_document_id))
                })
                .map(|document| document.storage_document_id.clone())
            else {
                break;
            };
            let event = GeneralRagEvent::RemoveDocument {
                storage_document_id: storage_id.clone(),
            };
            self.persistence
                .borrow_mut()
                .append(AtomSlice::General, &event)?;
            self.catalog.documents.remove(&storage_id);
        }
        Ok(())
    }

    fn active_size_bytes(&self) -> usize {
        self.catalog
            .documents
            .values()
            .map(|document| document.size_bytes)
            .sum()
    }
}

fn replay_general_event(
    event: &GeneralRagEvent,
    catalog: &mut NativeCatalog,
    legacy: &mut BTreeMap<String, LegacyDocument>,
) -> Result<(), DynError> {
    match event {
        GeneralRagEvent::NativeUpsertDocument {
            storage_document_id,
            source_document_id,
            collection,
            uri,
            title,
            tags,
            indexed_at,
            version,
            size_bytes,
            passages,
        } => {
            let passages = passages
                .iter()
                .map(|passage| {
                    Ok(PassageCatalogEntry {
                        identity: Digest::from_str(&passage.identity).map_err(invalid_digest)?,
                        chunk_index: passage.chunk_index,
                        embedding: passage.embedding.clone(),
                    })
                })
                .collect::<Result<_, DynError>>()?;
            catalog.documents.insert(
                storage_document_id.clone(),
                ActiveDocument {
                    storage_document_id: storage_document_id.clone(),
                    source_document_id: source_document_id.clone(),
                    collection: normalize_rag_collection_name(collection),
                    uri: uri.clone(),
                    title: title.clone(),
                    tags: tags.clone(),
                    indexed_at: indexed_at.clone(),
                    version: version.clone(),
                    size_bytes: *size_bytes,
                    passages,
                },
            );
            legacy.remove(storage_document_id);
        }
        GeneralRagEvent::UpsertDocument {
            storage_document_id,
            source_document_id,
            collection,
            chunks,
            ..
        } => {
            let first = chunks.first();
            legacy.insert(
                storage_document_id.clone(),
                LegacyDocument {
                    source_document_id: source_document_id.clone(),
                    collection: normalize_rag_collection_name(collection),
                    uri: first.map_or_else(String::new, |chunk| chunk.uri.clone()),
                    title: first.map_or_else(String::new, |chunk| chunk.title.clone()),
                    tags: first.map_or_else(Vec::new, |chunk| chunk.tags.clone()),
                    chunks: chunks
                        .iter()
                        .map(|chunk| (chunk.chunk_index, chunk.text.clone()))
                        .collect(),
                },
            );
            catalog.documents.remove(storage_document_id);
        }
        GeneralRagEvent::UpsertChunk {
            storage_document_id,
            source_document_id,
            collection,
            chunk,
            ..
        } => {
            let document = legacy
                .entry(storage_document_id.clone())
                .or_insert_with(|| LegacyDocument {
                    source_document_id: source_document_id.clone(),
                    collection: normalize_rag_collection_name(collection),
                    uri: chunk.uri.clone(),
                    title: chunk.title.clone(),
                    tags: chunk.tags.clone(),
                    chunks: BTreeMap::new(),
                });
            document
                .chunks
                .insert(chunk.chunk_index, chunk.text.clone());
            catalog.documents.remove(storage_document_id);
        }
        GeneralRagEvent::RemoveDocument {
            storage_document_id,
        } => {
            catalog.documents.remove(storage_document_id);
            legacy.remove(storage_document_id);
        }
    }
    Ok(())
}

fn replay_learning_event(
    event: &LearningRagEvent,
    catalog: &mut NativeCatalog,
) -> Result<(), DynError> {
    if let LearningRagEvent::NativeFeedback {
        storage_document_id,
        document_version,
        passage_identity,
        useful,
        ..
    } = event
    {
        let identity = Digest::from_str(passage_identity).map_err(invalid_digest)?;
        apply_source_feedback(
            catalog,
            storage_document_id.clone(),
            document_version.clone(),
            identity,
            *useful,
        );
    }
    Ok(())
}

fn apply_source_feedback(
    catalog: &mut NativeCatalog,
    storage_id: String,
    version: String,
    identity: Digest,
    useful: bool,
) {
    let score = catalog
        .source_feedback
        .entry((storage_id, version, identity))
        .or_default();
    *score = (*score
        + if useful {
            USEFUL_SOURCE_BONUS
        } else {
            -USELESS_SOURCE_PENALTY
        })
    .clamp(-3.0, 3.0);
}

fn join_legacy_chunks(chunks: impl IntoIterator<Item = String>) -> String {
    let mut joined = Vec::<String>::new();
    for chunk in chunks {
        let words = chunk.split_whitespace().collect::<Vec<_>>();
        if joined.is_empty() {
            joined.extend(words.into_iter().map(str::to_string));
            continue;
        }
        let overlap = (1..=joined.len().min(words.len()).min(64))
            .rev()
            .find(|length| {
                joined[joined.len() - *length..]
                    .iter()
                    .map(String::as_str)
                    .eq(words[..*length].iter().copied())
            })
            .unwrap_or(0);
        joined.extend(words[overlap..].iter().map(|word| (*word).to_string()));
    }
    joined.join(" ")
}

fn normalize_evidence(text: &str) -> String {
    text.replace(['\r', '\n'], " ").trim().to_string()
}

fn lexical_terms(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| term.len() >= 3)
        .filter(|term| seen.insert((*term).to_string()))
        .map(str::to_string)
        .collect()
}

fn padded_bigram_jaccard(left: &str, right: &str) -> f32 {
    fn bigrams(value: &str) -> HashSet<[char; 2]> {
        let chars = format!("^{value}$").chars().collect::<Vec<_>>();
        chars.windows(2).map(|pair| [pair[0], pair[1]]).collect()
    }
    let left = bigrams(left);
    let right = bigrams(right);
    let union = left.union(&right).count();
    if union == 0 {
        return 0.0;
    }
    left.intersection(&right).count() as f32 / union as f32
}

fn deduplicate_hits(hits: &mut Vec<RagHit>) {
    let mut best = HashMap::<(String, String, usize), RagHit>::new();
    for hit in hits.drain(..) {
        let key = (
            hit.collection.clone(),
            hit.document_id.clone(),
            hit.chunk_index,
        );
        match best.get(&key) {
            Some(existing) if existing.score >= hit.score => {}
            _ => {
                best.insert(key, hit);
            }
        }
    }
    hits.extend(best.into_values());
}

fn compare_hits(left: &RagHit, right: &RagHit) -> Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.document_id.cmp(&right.document_id))
        .then_with(|| left.chunk_index.cmp(&right.chunk_index))
}

fn ephemeral_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = EPHEMERAL_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/ordo-rag-ephemeral");
    fs::create_dir_all(&directory).expect("create workspace-local AtomDB scratch directory");
    directory.join(format!(
        "rag-{}-{nonce}-{sequence}.atomdb",
        std::process::id()
    ))
}

fn invalid_native_config(error: String) -> DynError {
    format!("invalid AtomDB retrieval configuration: {error}").into()
}

fn invalid_digest(error: String) -> DynError {
    format!("invalid AtomDB passage identity: {error}").into()
}
