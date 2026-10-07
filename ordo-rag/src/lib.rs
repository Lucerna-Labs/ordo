use std::{
    cmp::Ordering,
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use futures::StreamExt;
use ordo_bus::Bus;
use ordo_models::{EmbeddingClient, EmbeddingRequest};
use ordo_protocol::{
    CorrelationId, Envelope, NodeId, NodeStatus, OrdoMessage, RAG_COLLECTION_MAIN,
    RagCollectionSummary, RagDocument, RagHit, default_rag_collection_name, topics,
};
use ordo_store::{StorageTask, StorageTaskError};
use serde::{Deserialize, Serialize};
use tokio::task;

mod atom_persistence;
mod native_store;

pub use native_store::RagStore;

type DynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct RagChunkRecord {
    document_id: String,
    #[serde(default = "default_rag_collection_name")]
    collection: String,
    uri: String,
    title: String,
    tags: Vec<String>,
    chunk_index: usize,
    text: String,
    #[serde(default)]
    embedding: Vec<f32>,
}

/// Durable catalog metadata for one bounded passage produced by AtomDB's
/// native retrieval membrane. The passage text itself remains an immutable
/// atom; this record only maps it back to Ordo's document protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NativePassageRecord {
    identity: String,
    chunk_index: usize,
    #[serde(default)]
    embedding: Vec<f32>,
}

/// Immutable events in AtomDB's neutral, general-retrieval slice.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum GeneralRagEvent {
    NativeUpsertDocument {
        storage_document_id: String,
        source_document_id: String,
        collection: String,
        uri: String,
        title: String,
        tags: Vec<String>,
        indexed_at: String,
        version: String,
        #[serde(default)]
        size_bytes: usize,
        passages: Vec<NativePassageRecord>,
    },
    UpsertDocument {
        storage_document_id: String,
        source_document_id: String,
        collection: String,
        indexed_at: String,
        chunks: Vec<RagChunkRecord>,
    },
    UpsertChunk {
        storage_document_id: String,
        source_document_id: String,
        collection: String,
        indexed_at: String,
        chunk: RagChunkRecord,
    },
    RemoveDocument {
        storage_document_id: String,
    },
}

/// Immutable events in AtomDB's bounded self-learning slice.  The payloads
/// preserve judgments and routing observations without altering source facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum LearningRagEvent {
    NativeFeedback {
        query: String,
        storage_document_id: String,
        document_version: String,
        passage_identity: String,
        useful: bool,
    },
    ExplicitFeedback {
        query: String,
        storage_document_id: String,
        collection: String,
        chunk_index: usize,
        content_hash: String,
        useful: bool,
    },
    ImplicitRouting {
        query_terms: Vec<String>,
        collection: String,
    },
    /// Compatibility representation for aggregate learning events written by
    /// early AtomDB builds. New learning is always stored as individual events.
    FeedbackSeed {
        chunk_key: String,
        useful: f64,
        useless: f64,
    },
    RoutingSeed {
        term: String,
        collection: String,
        affinity: f64,
    },
    SemanticBridgeSeed {
        term_a: String,
        term_b: String,
        count: i64,
    },
}

#[derive(Clone)]
pub struct RagStorageTask {
    path: Option<PathBuf>,
    inner: StorageTask<RagStore>,
}

#[derive(Debug, Clone, Copy)]
pub struct RagStorageBudget {
    pub max_bytes: usize,
}

impl Default for RagStorageBudget {
    fn default() -> Self {
        Self {
            max_bytes: 100 * 1024 * 1024 * 1024,
        }
    }
}

#[allow(dead_code)]
impl RagStorageTask {
    pub fn from_store(store: RagStore) -> Self {
        Self {
            path: store.path().map(PathBuf::from),
            inner: StorageTask::start("rag-store", store),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub async fn chunk_count(&self) -> Result<usize, StorageTaskError> {
        self.inner.call(|store| Ok(store.chunk_count())).await
    }

    pub async fn document_count(&self) -> Result<usize, StorageTaskError> {
        self.inner.call(|store| Ok(store.document_count())).await
    }

    pub async fn list_collections(&self) -> Result<Vec<RagCollectionSummary>, StorageTaskError> {
        self.inner
            .call(|store| {
                store
                    .collection_summaries_result()
                    .map_err(|err| err.to_string())
            })
            .await
    }

    pub async fn upsert_document(&self, document: RagDocument) -> Result<usize, StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .upsert_document(&document)
                    .map_err(|err| err.to_string())
            })
            .await
    }

    pub async fn search(
        &self,
        query: String,
        top_k: usize,
    ) -> Result<Vec<RagHit>, StorageTaskError> {
        self.search_in_collections(query, top_k, Vec::new()).await
    }

    pub async fn search_in_collections(
        &self,
        query: String,
        top_k: usize,
        collections: Vec<String>,
    ) -> Result<Vec<RagHit>, StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .search_result(&query, top_k, &collections)
                    .map_err(|err| err.to_string())
            })
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_feedback(
        &self,
        query: String,
        document_id: String,
        collection: String,
        chunk_index: usize,
        content_hash: String,
        useful: bool,
    ) -> Result<bool, StorageTaskError> {
        self.inner
            .call(move |store| {
                store
                    .record_feedback(
                        &query,
                        &document_id,
                        &collection,
                        chunk_index,
                        &content_hash,
                        useful,
                    )
                    .map_err(|err| err.to_string())
            })
            .await
    }
}

pub struct RagPeer {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
    store: RagStorageTask,
}

impl RagPeer {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self::with_store(bus, RagStore::in_memory())
    }

    pub fn with_store(bus: Arc<dyn Bus>, store: RagStore) -> Self {
        Self::with_storage(bus, RagStorageTask::from_store(store))
    }

    pub fn with_storage(bus: Arc<dyn Bus>, store: RagStorageTask) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
            store,
        }
    }

    pub fn capabilities() -> Vec<String> {
        vec![
            "rag.ingest_document".to_string(),
            "rag.query".to_string(),
            "rag.feedback".to_string(),
        ]
    }

    pub async fn log_online(&self) -> Result<(), DynError> {
        let document_count = self.store.document_count().await.map_err(storage_error)?;
        let chunk_count = self.store.chunk_count().await.map_err(storage_error)?;
        match self.store.path() {
            Some(path) => println!(
                "[RAG] Peer online with {} document(s) and {} chunk(s) at {}",
                document_count,
                chunk_count,
                path.display()
            ),
            None => println!(
                "[RAG] Peer online with {} in-memory document(s) and {} chunk(s)",
                document_count, chunk_count
            ),
        }
        Ok(())
    }

    pub fn spawn_heartbeat(&self, started_at: Instant) {
        let heartbeat_bus = self.bus.clone();
        let heartbeat_node = self.node_id.clone();
        let version = env!("CARGO_PKG_VERSION").to_string();
        let capabilities = Self::capabilities();
        task::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3));
            loop {
                interval.tick().await;
                let status = NodeStatus {
                    id: heartbeat_node.clone(),
                    name: "rag-peer".to_string(),
                    uptime_secs: started_at.elapsed().as_secs(),
                    version: version.clone(),
                    capabilities: capabilities.clone(),
                };
                let envelope =
                    Envelope::new(heartbeat_node.clone(), OrdoMessage::Heartbeat(status));
                let _ = heartbeat_bus.publish(topics::HEARTBEAT, envelope).await;
            }
        });
    }

    pub async fn handle_ingest_envelope(
        &mut self,
        envelope: Envelope<OrdoMessage>,
    ) -> Result<(), DynError> {
        let correlation_id = envelope.correlation_id.clone();
        if let OrdoMessage::RagIngestRequested { document } = envelope.payload {
            println!("[RAG] Indexing '{}' from {}", document.title, document.uri);
            let document_id = document.document_id.clone();
            let chunk_count = self
                .store
                .upsert_document(document)
                .await
                .map_err(storage_error)?;
            let response = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::RagDocumentIndexed {
                    document_id,
                    chunk_count,
                },
            );
            let response = with_correlation(response, correlation_id);
            let _ = self
                .bus
                .publish(topics::RAG_INGEST_RESPONSE, response)
                .await;
        }
        Ok(())
    }

    pub async fn handle_query_envelope(
        &mut self,
        envelope: Envelope<OrdoMessage>,
    ) -> Result<(), DynError> {
        let correlation_id = envelope.correlation_id.clone();
        if let OrdoMessage::RagQueryRequested {
            query,
            top_k,
            collections,
        } = envelope.payload
        {
            if collections.is_empty() {
                println!("[RAG] Query '{}' top_k={} collections=all", query, top_k);
            } else {
                println!(
                    "[RAG] Query '{}' top_k={} collections={:?}",
                    query, top_k, collections
                );
            }
            let hits = self
                .store
                .search_in_collections(query.clone(), top_k, collections)
                .await
                .map_err(storage_error)?;
            let response = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::RagQueryCompleted { query, hits },
            );
            let response = with_correlation(response, correlation_id);
            let _ = self.bus.publish(topics::RAG_QUERY_RESPONSE, response).await;
        }
        Ok(())
    }

    pub async fn handle_feedback_envelope(
        &mut self,
        envelope: Envelope<OrdoMessage>,
    ) -> Result<(), DynError> {
        let correlation_id = envelope.correlation_id.clone();
        if let OrdoMessage::RagFeedbackSubmitted {
            query,
            document_id,
            chunk_index,
            collection,
            content_hash,
            useful,
        } = envelope.payload
        {
            println!(
                "[RAG] Feedback {} for '{}' chunk {} in {}",
                if useful { "useful" } else { "not-useful" },
                document_id,
                chunk_index,
                collection
            );
            let recorded = self
                .store
                .record_feedback(
                    query,
                    document_id.clone(),
                    collection.clone(),
                    chunk_index,
                    content_hash,
                    useful,
                )
                .await
                .map_err(storage_error)?;
            let response = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::RagFeedbackRecorded {
                    document_id,
                    chunk_index,
                    collection,
                    useful,
                    recorded,
                },
            );
            let response = with_correlation(response, correlation_id);
            let _ = self
                .bus
                .publish(topics::RAG_FEEDBACK_RESPONSE, response)
                .await;
        }
        Ok(())
    }

    pub async fn handle_collections_envelope(
        &mut self,
        envelope: Envelope<OrdoMessage>,
    ) -> Result<(), DynError> {
        let correlation_id = envelope.correlation_id.clone();
        if let OrdoMessage::RagCollectionsRequested = envelope.payload {
            let collections = self.store.list_collections().await.map_err(storage_error)?;
            let response = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::RagCollectionsListed { collections },
            );
            let response = with_correlation(response, correlation_id);
            let _ = self
                .bus
                .publish(topics::RAG_COLLECTIONS_RESPONSE, response)
                .await;
        }
        Ok(())
    }

    pub async fn run(&mut self) -> Result<(), DynError> {
        let mut ingest_sub = self.bus.subscribe(topics::RAG_INGEST_REQUEST).await?;
        let mut collections_sub = self.bus.subscribe(topics::RAG_COLLECTIONS_REQUEST).await?;
        let mut query_sub = self.bus.subscribe(topics::RAG_QUERY_REQUEST).await?;
        let mut feedback_sub = self.bus.subscribe(topics::RAG_FEEDBACK_REQUEST).await?;
        let started_at = Instant::now();

        self.log_online().await?;
        self.spawn_heartbeat(started_at);

        loop {
            tokio::select! {
                ingest = ingest_sub.next() => {
                    let Some(envelope) = ingest else {
                        break;
                    };
                    self.handle_ingest_envelope(envelope).await?;
                }
                collections = collections_sub.next() => {
                    let Some(envelope) = collections else {
                        break;
                    };
                    self.handle_collections_envelope(envelope).await?;
                }
                query = query_sub.next() => {
                    let Some(envelope) = query else {
                        break;
                    };
                    self.handle_query_envelope(envelope).await?;
                }
                feedback = feedback_sub.next() => {
                    let Some(envelope) = feedback else {
                        break;
                    };
                    self.handle_feedback_envelope(envelope).await?;
                }
            }
        }

        Ok(())
    }
}

fn storage_error(error: StorageTaskError) -> DynError {
    Box::new(std::io::Error::other(error.to_string()))
}

/// Dense vectors are only produced when a generative embedder is wired
/// in; the default path stores an empty vector and relies on the
/// lexical-statistical index. Free function (not a method) so it can
/// run while a transaction holds the store's connection.
fn embed_or_empty(
    embedder: Option<&Arc<dyn EmbeddingClient>>,
    text: &str,
) -> Result<Vec<f32>, DynError> {
    match embedder {
        Some(embedder) => Ok(embedder
            .embed(EmbeddingRequest {
                input: text.to_string(),
            })?
            .vector),
        None => Ok(Vec::new()),
    }
}

fn with_correlation(
    envelope: Envelope<OrdoMessage>,
    correlation_id: Option<CorrelationId>,
) -> Envelope<OrdoMessage> {
    match correlation_id {
        Some(cid) => envelope.with_correlation(cid),
        None => envelope,
    }
}

fn balance_rag_hits(hits: Vec<RagHit>, collections: &[String], top_k: usize) -> Vec<RagHit> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();

    if collections
        .iter()
        .any(|collection| collection == RAG_COLLECTION_MAIN)
    {
        if let Some(hit) = hits
            .iter()
            .find(|hit| hit.collection == RAG_COLLECTION_MAIN)
            .cloned()
        {
            seen.insert(hit_key(&hit));
            selected.push(hit);
        }
    }

    for collection in collections
        .iter()
        .filter(|collection| collection.as_str() != RAG_COLLECTION_MAIN)
    {
        if selected.len() >= top_k {
            break;
        }

        if let Some(hit) = hits
            .iter()
            .find(|hit| hit.collection == *collection && !seen.contains(&hit_key(hit)))
            .cloned()
        {
            seen.insert(hit_key(&hit));
            selected.push(hit);
        }
    }

    for hit in hits {
        if selected.len() >= top_k {
            break;
        }

        if seen.insert(hit_key(&hit)) {
            selected.push(hit);
        }
    }

    selected.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.collection.cmp(&right.collection))
            .then_with(|| left.document_id.cmp(&right.document_id))
            .then_with(|| left.chunk_index.cmp(&right.chunk_index))
    });
    selected.truncate(top_k);
    selected
}

fn hit_key(hit: &RagHit) -> String {
    // Unit-separator join so '::'-bearing collection or document names
    // cannot alias two distinct hits in balance_rag_hits' seen-set.
    format!(
        "{}\u{1f}{}\u{1f}{}",
        hit.collection, hit.document_id, hit.chunk_index
    )
}

/// Unit separator, not "::": collection names are only trimmed and
/// lowercased, so a ':'-bearing collection/document pair like
/// ("notes", "v2::guide") vs ("notes::v2", "guide") would collide on
/// one "::"-joined identity and silently replace each other's chunks.
fn storage_document_id(collection: &str, document_id: &str) -> String {
    format!("{collection}\u{1f}{document_id}")
}

fn content_hash(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use super::{RagPeer, RagStorageBudget, RagStore};
    use futures::StreamExt;
    use ordo_bus::{Bus, InProcessBus};
    use ordo_models::{EmbeddingClient, EmbeddingRequest, EmbeddingResponse, ModelResult};
    use ordo_protocol::{CorrelationId, Envelope, NodeId, OrdoMessage, RagDocument, topics};

    /// Deterministic stand-in for a semantic embedding model: any text
    /// mentioning cats/felines lands on one axis, everything else on
    /// the other.
    struct KeywordEmbedder;

    impl EmbeddingClient for KeywordEmbedder {
        fn embed(&self, request: EmbeddingRequest) -> ModelResult<EmbeddingResponse> {
            let text = request.input.to_ascii_lowercase();
            let feline = text.contains("cat") || text.contains("feline");
            let vector = if feline {
                vec![1.0, 0.0]
            } else {
                vec![0.0, 1.0]
            };
            Ok(EmbeddingResponse { vector })
        }

        fn dimensions(&self) -> usize {
            2
        }

        fn backend_name(&self) -> &str {
            "test-keyword"
        }
    }

    #[test]
    fn store_returns_relevant_hits() {
        let mut store = RagStore::in_memory();
        store
            .upsert_document(&RagDocument {
                document_id: "architecture".to_string(),
                uri: "docs/architecture.md".to_string(),
                title: "Architecture".to_string(),
                tags: vec!["docs".to_string(), "architecture".to_string()],
                collection: "main".to_string(),
                content: "The transport adapter seam keeps routing policy separate from transport delivery.".to_string(),
            })
            .expect("index architecture doc");
        store
            .upsert_document(&RagDocument {
                document_id: "dones".to_string(),
                uri: "docs/dones.md".to_string(),
                title: "Done Log".to_string(),
                tags: vec!["docs".to_string(), "history".to_string()],
                collection: "main".to_string(),
                content: "The project now has a done log and a runtime demo.".to_string(),
            })
            .expect("index done log");

        let hits = store.search("transport adapter routing", 2);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].document_id, "architecture");
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn store_filters_hits_by_collection() {
        let mut store = RagStore::in_memory();
        store
            .upsert_document(&RagDocument {
                document_id: "ops-runbook".to_string(),
                uri: "docs/domains/operations.md".to_string(),
                title: "Operations".to_string(),
                tags: vec!["docs".to_string(), "operations".to_string()],
                collection: "operations".to_string(),
                content: "Operations runbooks cover restarts, logs, and local runtime checks."
                    .to_string(),
            })
            .expect("index operations doc");
        store
            .upsert_document(&RagDocument {
                document_id: "research-plan".to_string(),
                uri: "docs/domains/research.md".to_string(),
                title: "Research".to_string(),
                tags: vec!["docs".to_string(), "research".to_string()],
                collection: "research".to_string(),
                content: "Research notes track citations, source quality, and evidence review."
                    .to_string(),
            })
            .expect("index research doc");

        let operations_hits = store.search_in_collections(
            "citations evidence source review",
            5,
            &["operations".to_string()],
        );
        assert!(
            operations_hits
                .iter()
                .all(|hit| hit.collection == "operations" && hit.document_id != "research-plan")
        );

        let research_hits = store.search_in_collections(
            "citations evidence source review",
            5,
            &["research".to_string()],
        );
        assert_eq!(research_hits.len(), 1);
        assert_eq!(research_hits[0].document_id, "research-plan");
        assert_eq!(research_hits[0].collection, "research");
    }
    #[test]
    fn store_reports_collection_summaries() {
        let mut store = RagStore::in_memory();
        store
            .upsert_document(&RagDocument {
                document_id: "design-basics".to_string(),
                uri: "docs/rag/main/design-basics.md".to_string(),
                title: "Design Basics".to_string(),
                tags: vec!["docs".to_string(), "design".to_string()],
                collection: "main".to_string(),
                content: "Hierarchy and spacing help design work read clearly.".to_string(),
            })
            .expect("index main document");
        store
            .upsert_document(&RagDocument {
                document_id: "research-domain".to_string(),
                uri: "docs/domains/research.md".to_string(),
                title: "Research Domain".to_string(),
                tags: vec!["docs".to_string(), "research".to_string()],
                collection: "research".to_string(),
                content: "Research notes track citations and evidence review.".to_string(),
            })
            .expect("index research document");

        let summaries = store.collection_summaries();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "main");
        assert_eq!(summaries[0].document_count, 1);
        assert_eq!(summaries[1].name, "research");
        assert_eq!(summaries[1].label, "Research");
        assert_eq!(summaries[1].chunk_count, 1);
    }
    #[test]
    fn budget_prunes_oldest_documents() {
        // Each document below stores ~77 bytes now that the default
        // path keeps no dense embedding JSON; 120 bytes fits one
        // document but not two.
        let mut store = RagStore::in_memory_with_budget(RagStorageBudget { max_bytes: 120 });
        store
            .upsert_document(&RagDocument {
                document_id: "old".to_string(),
                uri: "docs/old.md".to_string(),
                title: "Old".to_string(),
                tags: vec!["docs".to_string()],
                collection: "main".to_string(),
                content: "transport relay fallback old baseline".to_string(),
            })
            .expect("index old");
        store
            .upsert_document(&RagDocument {
                document_id: "new".to_string(),
                uri: "docs/new.md".to_string(),
                title: "New".to_string(),
                tags: vec!["docs".to_string()],
                collection: "main".to_string(),
                content: "transport relay fallback new baseline".to_string(),
            })
            .expect("index new");

        let hits = store.search("new baseline", 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].document_id, "new");
        assert_eq!(store.document_count(), 1);
    }

    #[test]
    fn configured_embedder_reaches_semantic_only_matches() {
        let mut store = RagStore::in_memory_with_embedder(
            Arc::new(KeywordEmbedder),
            RagStorageBudget::default(),
        );
        store
            .upsert_document(&RagDocument {
                document_id: "felines".to_string(),
                uri: "docs/felines.md".to_string(),
                title: "Felines".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "Felines enjoy napping in warm places.".to_string(),
            })
            .expect("index felines doc");
        store
            .upsert_document(&RagDocument {
                document_id: "routing".to_string(),
                uri: "docs/routing.md".to_string(),
                title: "Routing".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "Transport routing policy delivery.".to_string(),
            })
            .expect("index routing doc");

        // Zero term overlap with the felines chunk — only the
        // embedding channel can retrieve it.
        let hits = store.search("cats sleeping", 3);
        assert!(!hits.is_empty(), "semantic-only match must be reachable");
        assert_eq!(hits[0].document_id, "felines");
    }

    #[test]
    fn stopword_phrase_queries_fall_back_to_substring_match() {
        let mut store = RagStore::in_memory();
        store
            .upsert_document(&RagDocument {
                document_id: "faq".to_string(),
                uri: "docs/faq.md".to_string(),
                title: "FAQ".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "Who are you and what do you want?".to_string(),
            })
            .expect("index faq doc");

        // Every query token is a stopword, so the term index has
        // nothing; the exact-phrase fallback must still find it.
        let hits = store.search("who are you", 3);
        assert!(
            !hits.is_empty(),
            "stopword phrase should match via fallback"
        );
        assert_eq!(hits[0].document_id, "faq");
    }

    #[test]
    fn stopword_fallback_keeps_every_requested_collection_reachable() {
        let mut store = RagStore::in_memory();
        // Enough main-collection matches to exhaust a global candidate
        // cap (50) before the research row would be reached in table
        // order.
        for index in 0..60 {
            store
                .upsert_document(&RagDocument {
                    document_id: format!("faq-{index}"),
                    uri: format!("docs/faq-{index}.md"),
                    title: format!("FAQ {index}"),
                    tags: vec![],
                    collection: "main".to_string(),
                    content: format!("Who are you and what do you want, number {index}?"),
                })
                .expect("index main doc");
        }
        store
            .upsert_document(&RagDocument {
                document_id: "research-faq".to_string(),
                uri: "docs/research-faq.md".to_string(),
                title: "Research FAQ".to_string(),
                tags: vec![],
                collection: "research".to_string(),
                content: "Who are you and why does it matter for citations?".to_string(),
            })
            .expect("index research doc");

        let hits = store.search_in_collections(
            "who are you",
            5,
            &["main".to_string(), "research".to_string()],
        );
        assert!(
            hits.iter().any(|hit| hit.collection == "research"),
            "fallback must not let one collection starve another: {hits:?}"
        );
    }

    #[test]
    fn colon_bearing_identifiers_stay_distinct_documents() {
        let mut store = RagStore::in_memory();
        store
            .upsert_document(&RagDocument {
                document_id: "v2::guide".to_string(),
                uri: "docs/a.md".to_string(),
                title: "A".to_string(),
                tags: vec![],
                collection: "notes".to_string(),
                content: "alpha content one".to_string(),
            })
            .expect("index first doc");
        store
            .upsert_document(&RagDocument {
                document_id: "guide".to_string(),
                uri: "docs/b.md".to_string(),
                title: "B".to_string(),
                tags: vec![],
                collection: "notes::v2".to_string(),
                content: "beta content two".to_string(),
            })
            .expect("index second doc");

        assert_eq!(
            store.document_count(),
            2,
            "(notes, v2::guide) and (notes::v2, guide) must not collide"
        );
        let hits = store.search("alpha", 3);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].document_id, "v2::guide");
    }

    fn twin_documents(store: &mut RagStore) {
        for document_id in ["twin-a", "twin-b"] {
            store
                .upsert_document(&RagDocument {
                    document_id: document_id.to_string(),
                    uri: format!("docs/{document_id}.md"),
                    title: document_id.to_string(),
                    tags: vec![],
                    collection: "main".to_string(),
                    content: "transport relay protocols overview".to_string(),
                })
                .expect("index twin doc");
        }
    }

    #[test]
    fn useful_feedback_reranks_equal_hits() {
        let mut store = RagStore::in_memory();
        twin_documents(&mut store);
        assert_eq!(store.cognitive_observations(), 0);

        let hits = store.search("transport relay", 2);
        assert_eq!(hits[0].document_id, "twin-a", "ties break by document id");

        store
            .record_feedback("transport relay", "twin-b", "main", 0, "", true)
            .expect("record feedback");
        assert_eq!(
            store.cognitive_observations(),
            1,
            "the original AtomDB cognitive engine must receive the judgment"
        );
        assert_eq!(store.cognitive_context_traces(), 1);
        let hits = store.search("transport relay", 2);
        assert_eq!(
            hits[0].document_id, "twin-b",
            "reinforced chunk should outrank its identical twin"
        );
    }

    #[test]
    fn useless_feedback_suppresses_hits() {
        let mut store = RagStore::in_memory();
        twin_documents(&mut store);

        for _ in 0..3 {
            store
                .record_feedback("transport relay", "twin-a", "main", 0, "", false)
                .expect("record feedback");
        }
        let hits = store.search("transport relay", 2);
        assert_eq!(
            hits[0].document_id, "twin-b",
            "repeatedly-not-useful chunk must sink below its twin"
        );
    }

    #[test]
    fn feedback_resolving_missing_chunk_reports_unrecorded() {
        let mut store = RagStore::in_memory();
        let recorded = store
            .record_feedback("anything", "ghost", "main", 0, "", true)
            .expect("feedback call");
        assert!(!recorded, "feedback on a nonexistent chunk is a no-op");
    }

    #[test]
    fn stale_feedback_after_reingestion_is_rejected() {
        let mut store = RagStore::in_memory();
        store
            .upsert_document(&RagDocument {
                document_id: "guide".to_string(),
                uri: "docs/guide.md".to_string(),
                title: "Guide".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "transport relay overview".to_string(),
            })
            .expect("index original doc");
        let hits = store.search("transport relay", 1);
        let stale_hash = hits[0].content_hash.clone();
        assert!(!stale_hash.is_empty());

        // The document changes completely between the hit and the
        // feedback — same id, same chunk index, different text.
        store
            .upsert_document(&RagDocument {
                document_id: "guide".to_string(),
                uri: "docs/guide.md".to_string(),
                title: "Guide".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "garden flowers bloom nicely".to_string(),
            })
            .expect("replace doc");

        let recorded = store
            .record_feedback("transport relay", "guide", "main", 0, &stale_hash, true)
            .expect("feedback call");
        assert!(
            !recorded,
            "stale feedback must be rejected, not learned from"
        );
        assert!(
            store.search("transport relay", 3).is_empty(),
            "rejected feedback must not bridge old vocabulary to new text"
        );
    }

    #[test]
    fn useful_feedback_teaches_semantic_bridge() {
        let mut store = RagStore::in_memory();
        store
            .upsert_document(&RagDocument {
                document_id: "felines".to_string(),
                uri: "docs/felines.md".to_string(),
                title: "Felines".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "Felines enjoy napping in warm places.".to_string(),
            })
            .expect("index felines doc");
        store
            .upsert_document(&RagDocument {
                document_id: "routing".to_string(),
                uri: "docs/routing.md".to_string(),
                title: "Routing".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "Transport routing policy delivery.".to_string(),
            })
            .expect("index routing doc");

        // No lexical overlap and no embedding model: unreachable today.
        assert!(store.search("cats sleeping", 3).is_empty());

        // One human confirmation bridges the vocabularies.
        let recorded = store
            .record_feedback("cats sleeping", "felines", "main", 0, "", true)
            .expect("record feedback");
        assert!(recorded);

        let hits = store.search("cats sleeping", 3);
        assert!(
            !hits.is_empty(),
            "learned co-occurrence bridge should make the chunk reachable"
        );
        assert_eq!(hits[0].document_id, "felines");
    }

    #[test]
    fn feedback_learns_collection_routing() {
        let mut store = RagStore::in_memory();
        for (document_id, collection) in [("ops-doc", "operations"), ("res-doc", "research")] {
            store
                .upsert_document(&RagDocument {
                    document_id: document_id.to_string(),
                    uri: format!("docs/{document_id}.md"),
                    title: document_id.to_string(),
                    tags: vec![],
                    collection: collection.to_string(),
                    content: "transport relay overview".to_string(),
                })
                .expect("index doc");
        }

        for _ in 0..2 {
            store
                .record_feedback("transport relay", "res-doc", "research", 0, "", true)
                .expect("record feedback");
        }

        let hits = store.search("transport relay", 2);
        assert_eq!(
            hits[0].document_id, "res-doc",
            "learned routing and reinforcement should steer the unscoped query"
        );
    }

    #[test]
    fn feedback_is_scoped_to_the_document_version() {
        let mut store = RagStore::in_memory();
        let document = RagDocument {
            document_id: "guide".to_string(),
            uri: "docs/guide.md".to_string(),
            title: "Guide".to_string(),
            tags: vec![],
            collection: "main".to_string(),
            content: "transport relay overview".to_string(),
        };
        store.upsert_document(&document).expect("index doc");
        store
            .record_feedback("transport", "guide", "main", 0, "", true)
            .expect("record feedback");

        let mut replacement = document;
        replacement.content = "garden flowers bloom nicely".to_string();
        store.upsert_document(&replacement).expect("replace doc");
        assert!(
            store.search("transport", 3).is_empty(),
            "feedback for the replaced passage must not revive its old version"
        );
    }

    #[test]
    fn default_store_needs_no_embedding_model() {
        let store = RagStore::in_memory();
        assert_eq!(store.embedding_backend(), "atomdb-retrieval-field");
        assert_eq!(store.embedding_dimensions(), 0);
    }

    #[test]
    fn typo_queries_match_via_fuzzy_vocabulary() {
        let mut store = RagStore::in_memory();
        store
            .upsert_document(&RagDocument {
                document_id: "architecture".to_string(),
                uri: "docs/architecture.md".to_string(),
                title: "Architecture".to_string(),
                tags: vec!["docs".to_string()],
                collection: "main".to_string(),
                content: "The transport adapter seam keeps routing policy separate.".to_string(),
            })
            .expect("index architecture doc");
        store
            .upsert_document(&RagDocument {
                document_id: "calendar".to_string(),
                uri: "docs/calendar.md".to_string(),
                title: "Calendar".to_string(),
                tags: vec!["docs".to_string()],
                collection: "main".to_string(),
                content: "Calendar invoices export weekly summaries.".to_string(),
            })
            .expect("index calendar doc");

        // "transprot" is a transposition typo that appears nowhere in
        // the corpus; padded-bigram Jaccard maps it onto "transport".
        let hits = store.search("transprot adapter", 3);
        assert!(!hits.is_empty(), "typo query should still match");
        assert_eq!(hits[0].document_id, "architecture");
    }

    #[test]
    fn cooccurrence_expansion_reaches_related_chunks() {
        let mut store = RagStore::in_memory();
        // Teach the corpus that relay and transport keep company.
        store
            .upsert_document(&RagDocument {
                document_id: "association".to_string(),
                uri: "docs/association.md".to_string(),
                title: "Association".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "relay transport relay transport relay transport".to_string(),
            })
            .expect("index association doc");
        // Target mentions transport but never relay.
        store
            .upsert_document(&RagDocument {
                document_id: "target".to_string(),
                uri: "docs/target.md".to_string(),
                title: "Target".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "transport pipeline delivery".to_string(),
            })
            .expect("index target doc");
        store
            .upsert_document(&RagDocument {
                document_id: "distractor".to_string(),
                uri: "docs/distractor.md".to_string(),
                title: "Distractor".to_string(),
                tags: vec![],
                collection: "main".to_string(),
                content: "garden flowers bloom nicely".to_string(),
            })
            .expect("index distractor doc");

        let hits = store.search("relay", 5);
        assert!(
            hits.iter().any(|hit| hit.document_id == "target"),
            "AtomDB activation should pull in the transport-only passage: {hits:?}"
        );
        assert!(
            hits.iter().all(|hit| hit.document_id != "distractor"),
            "unrelated chunks must not surface"
        );
    }

    #[test]
    fn reindexing_same_document_keeps_counts_stable() {
        let mut store = RagStore::in_memory();
        let document = RagDocument {
            document_id: "architecture".to_string(),
            uri: "docs/architecture.md".to_string(),
            title: "Architecture".to_string(),
            tags: vec![],
            collection: "main".to_string(),
            content: "The transport adapter seam keeps routing policy separate.".to_string(),
        };
        store.upsert_document(&document).expect("first upsert");
        let documents_first = store.document_count();
        let chunks_first = store.chunk_count();

        store.upsert_document(&document).expect("second upsert");
        assert_eq!(documents_first, 1);
        assert_eq!(store.document_count(), documents_first);
        assert_eq!(store.chunk_count(), chunks_first);
    }

    #[test]
    fn atomdb_reopen_rebuilds_native_derived_index() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/ordo-rag-tests")
            .join(format!("replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("rag.atomdb");
        let _ = std::fs::remove_file(&path);

        {
            let mut store = RagStore::open(&path).expect("open new store");
            store
                .upsert_document(&RagDocument {
                    document_id: "architecture".to_string(),
                    uri: "docs/architecture.md".to_string(),
                    title: "Architecture".to_string(),
                    tags: vec![],
                    collection: "main".to_string(),
                    content: "The transport adapter seam keeps routing policy separate."
                        .to_string(),
                })
                .expect("index doc");
            assert!(!store.search("adapter transport routing", 3).is_empty());
        }

        let store = RagStore::open(&path).expect("reopen AtomDB store");
        let hits = store.search("adapter transport routing", 3);
        assert!(
            !hits.is_empty(),
            "reopening must rebuild the index from chunk text"
        );
        assert_eq!(hits[0].document_id, "architecture");

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomdb_keeps_general_and_learning_roots_and_replays_feedback() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/ordo-rag-tests")
            .join(format!("two-slices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let path = dir.join("rag.atomdb");
        let _ = std::fs::remove_file(&path);

        {
            let mut store = RagStore::open(&path).expect("open AtomDB store");
            twin_documents(&mut store);
            store
                .record_feedback("transport relay", "twin-b", "main", 0, "", true)
                .expect("persist feedback");
            assert_eq!(store.search("transport relay", 2)[0].document_id, "twin-b");
        }

        {
            let store = RagStore::open(&path).expect("replay both AtomDB slices");
            assert_eq!(store.document_count(), 2);
            assert_eq!(
                store.cognitive_observations(),
                1,
                "the learning slice must reconstruct AtomDB cognition on restart"
            );
            assert_eq!(store.search("transport relay", 2)[0].document_id, "twin-b");
        }

        let db = atom_db::AtomDb::open_read_only(&path).expect("inspect AtomDB roots");
        assert_eq!(
            db.roots().len(),
            2,
            "general retrieval and self-learning must have distinct roots"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rag_peer_indexes_and_queries_over_bus() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut peer = RagPeer::new(bus.clone());
        let peer_task = tokio::spawn(async move {
            let _ = peer.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let ingest_correlation = CorrelationId::new();
        let mut ingest_sub = bus
            .subscribe(topics::RAG_INGEST_RESPONSE)
            .await
            .expect("subscribe ingest");
        bus.publish(
            topics::RAG_INGEST_REQUEST,
            Envelope::new(
                NodeId::new(),
                OrdoMessage::RagIngestRequested {
                    document: RagDocument {
                        document_id: "readme".to_string(),
                        uri: "README.md".to_string(),
                        title: "Readme".to_string(),
                        tags: vec!["docs".to_string()],
                        collection: "main".to_string(),
                        content: "Tokio bus routing and retrieval are both local first."
                            .to_string(),
                    },
                },
            )
            .with_correlation(ingest_correlation.clone()),
        )
        .await
        .expect("publish ingest");

        let ingest_response = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = ingest_sub.next().await.expect("ingest response");
                if envelope.correlation_id.as_ref() == Some(&ingest_correlation) {
                    break envelope;
                }
            }
        })
        .await
        .expect("ingest timeout");

        match ingest_response.payload {
            OrdoMessage::RagDocumentIndexed { chunk_count, .. } => {
                assert!(chunk_count > 0);
            }
            other => panic!("unexpected ingest payload: {other:?}"),
        }

        let query_correlation = CorrelationId::new();
        let mut query_sub = bus
            .subscribe(topics::RAG_QUERY_RESPONSE)
            .await
            .expect("subscribe query");
        bus.publish(
            topics::RAG_QUERY_REQUEST,
            Envelope::new(
                NodeId::new(),
                OrdoMessage::RagQueryRequested {
                    query: "tokio retrieval".to_string(),
                    top_k: 3,
                    collections: vec!["main".to_string()],
                },
            )
            .with_correlation(query_correlation.clone()),
        )
        .await
        .expect("publish query");

        let query_response = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = query_sub.next().await.expect("query response");
                if envelope.correlation_id.as_ref() == Some(&query_correlation) {
                    break envelope;
                }
            }
        })
        .await
        .expect("query timeout");

        match query_response.payload {
            OrdoMessage::RagQueryCompleted { hits, .. } => {
                assert!(!hits.is_empty());
                assert_eq!(hits[0].document_id, "readme");
            }
            other => panic!("unexpected query payload: {other:?}"),
        }

        let feedback_correlation = CorrelationId::new();
        let mut feedback_sub = bus
            .subscribe(topics::RAG_FEEDBACK_RESPONSE)
            .await
            .expect("subscribe feedback");
        bus.publish(
            topics::RAG_FEEDBACK_REQUEST,
            Envelope::new(
                NodeId::new(),
                OrdoMessage::RagFeedbackSubmitted {
                    query: "tokio retrieval".to_string(),
                    document_id: "readme".to_string(),
                    chunk_index: 0,
                    collection: "main".to_string(),
                    content_hash: String::new(),
                    useful: true,
                },
            )
            .with_correlation(feedback_correlation.clone()),
        )
        .await
        .expect("publish feedback");

        let feedback_response = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = feedback_sub.next().await.expect("feedback response");
                if envelope.correlation_id.as_ref() == Some(&feedback_correlation) {
                    break envelope;
                }
            }
        })
        .await
        .expect("feedback timeout");

        match feedback_response.payload {
            OrdoMessage::RagFeedbackRecorded {
                document_id,
                recorded,
                useful,
                ..
            } => {
                assert_eq!(document_id, "readme");
                assert!(useful);
                assert!(recorded, "feedback for an existing chunk must record");
            }
            other => panic!("unexpected feedback payload: {other:?}"),
        }

        peer_task.abort();
    }
}
