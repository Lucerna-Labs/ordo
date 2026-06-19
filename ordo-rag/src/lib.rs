use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use chrono::Utc;
use futures::StreamExt;
use ordo_bus::Bus;
use ordo_models::{cosine_similarity, EmbeddingClient, EmbeddingRequest, HashingEmbedder};
use ordo_protocol::{
    default_rag_collection_name, normalize_rag_collection_name, normalize_rag_collections,
    rag_collection_group, rag_collection_label, topics, CorrelationId, Envelope, NodeId,
    NodeStatus, OrdoMessage, RagCollectionSummary, RagDocument, RagHit, RAG_COLLECTION_MAIN,
};
use ordo_store::{OrdoDatabase, StorageTask, StorageTaskError};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tokio::task;

type DynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ChunkingConfig {
    pub target_words: usize,
    pub overlap_words: usize,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            target_words: 120,
            overlap_words: 30,
        }
    }
}

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

pub struct RagStore {
    db: OrdoDatabase,
    config: ChunkingConfig,
    embedder: Arc<dyn EmbeddingClient>,
    budget: RagStorageBudget,
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

impl RagStore {
    pub fn in_memory() -> Self {
        Self::in_memory_with_budget(RagStorageBudget::default())
    }

    pub fn in_memory_with_budget(budget: RagStorageBudget) -> Self {
        Self::in_memory_with_embedder(Arc::new(HashingEmbedder::default()), budget)
    }

    pub fn in_memory_with_embedder(
        embedder: Arc<dyn EmbeddingClient>,
        budget: RagStorageBudget,
    ) -> Self {
        Self {
            db: OrdoDatabase::in_memory().expect("open in-memory sqlite database"),
            config: ChunkingConfig::default(),
            embedder,
            budget,
        }
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        Self::open_with_budget(path, RagStorageBudget::default())
    }

    pub fn open_with_budget(
        path: impl Into<PathBuf>,
        budget: RagStorageBudget,
    ) -> Result<Self, DynError> {
        Self::open_with_embedder(path, Arc::new(HashingEmbedder::default()), budget)
    }

    pub fn open_with_embedder(
        path: impl Into<PathBuf>,
        embedder: Arc<dyn EmbeddingClient>,
        budget: RagStorageBudget,
    ) -> Result<Self, DynError> {
        Ok(Self {
            db: OrdoDatabase::open(path)?,
            config: ChunkingConfig::default(),
            embedder,
            budget,
        })
    }

    pub fn embedding_backend(&self) -> &str {
        self.embedder.backend_name()
    }

    pub fn embedding_dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    pub fn path(&self) -> Option<&Path> {
        self.db.path()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunk_count_result().expect("count rag chunks")
    }

    pub fn document_count(&self) -> usize {
        self.document_count_result().expect("count rag documents")
    }

    pub fn collection_summaries(&self) -> Vec<RagCollectionSummary> {
        self.collection_summaries_result()
            .expect("list rag collections")
    }

    pub fn is_empty(&self) -> bool {
        self.chunk_count() == 0
    }

    pub fn upsert_document(&mut self, document: &RagDocument) -> Result<usize, DynError> {
        let collection = normalize_rag_collection_name(&document.collection);
        let source_document_id = document.document_id.trim().to_string();
        let storage_document_id = storage_document_id(&collection, &source_document_id);
        let chunk_texts = chunk_text(&document.content, &self.config);
        let tx = self.db.conn_mut().transaction()?;
        let indexed_at = Utc::now().to_rfc3339();
        tx.execute(
            "
            DELETE FROM rag_chunks
            WHERE document_id = ?1
               OR source_document_id = ?2
               OR (source_document_id = '' AND document_id = ?2)
            ",
            params![&storage_document_id, &source_document_id],
        )?;

        for (chunk_index, text) in chunk_texts.iter().enumerate() {
            let tags_json = serde_json::to_string(&document.tags)?;
            let embedding_json = serde_json::to_string(
                &self
                    .embedder
                    .embed(EmbeddingRequest {
                        input: text.clone(),
                    })?
                    .vector,
            )?;
            let size_bytes = (storage_document_id.len()
                + source_document_id.len()
                + collection.len()
                + document.uri.len()
                + document.title.len()
                + tags_json.len()
                + text.len()
                + embedding_json.len()) as i64;
            tx.execute(
                "
                INSERT INTO rag_chunks (
                    document_id, uri, title, tags_json, chunk_index, text, embedding_json,
                    indexed_at, size_bytes, collection_name, source_document_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ",
                params![
                    &storage_document_id,
                    &document.uri,
                    &document.title,
                    tags_json,
                    chunk_index as i64,
                    text,
                    embedding_json,
                    &indexed_at,
                    size_bytes,
                    &collection,
                    &source_document_id,
                ],
            )?;
        }

        tx.commit()?;
        self.prune_to_budget()?;
        Ok(chunk_texts.len())
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
            .expect("query rag store")
    }

    pub fn import_legacy_jsonl(&mut self, path: &Path) -> Result<usize, DynError> {
        if !path.exists() {
            return Ok(0);
        }

        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let tx = self.db.conn_mut().transaction()?;
        let mut imported = 0usize;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let chunk: RagChunkRecord = serde_json::from_str(&line)?;
            let collection = normalize_rag_collection_name(&chunk.collection);
            let storage_document_id = storage_document_id(&collection, &chunk.document_id);
            let embedding = if chunk.embedding.is_empty() {
                self.embedder
                    .embed(EmbeddingRequest {
                        input: chunk.text.clone(),
                    })?
                    .vector
            } else {
                chunk.embedding.clone()
            };
            let tags_json = serde_json::to_string(&chunk.tags)?;
            let embedding_json = serde_json::to_string(&embedding)?;
            let indexed_at = Utc::now().to_rfc3339();
            let size_bytes = (storage_document_id.len()
                + chunk.document_id.len()
                + collection.len()
                + chunk.uri.len()
                + chunk.title.len()
                + tags_json.len()
                + chunk.text.len()
                + embedding_json.len()) as i64;
            tx.execute(
                "
                INSERT OR REPLACE INTO rag_chunks (
                    document_id, uri, title, tags_json, chunk_index, text, embedding_json,
                    indexed_at, size_bytes, collection_name, source_document_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ",
                params![
                    &storage_document_id,
                    &chunk.uri,
                    &chunk.title,
                    tags_json,
                    chunk.chunk_index as i64,
                    &chunk.text,
                    embedding_json,
                    &indexed_at,
                    size_bytes,
                    &collection,
                    &chunk.document_id,
                ],
            )?;
            imported += 1;
        }

        tx.commit()?;
        self.prune_to_budget()?;
        Ok(imported)
    }

    fn chunk_count_result(&self) -> Result<usize, DynError> {
        let count = self
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM rag_chunks", [], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(count as usize)
    }

    fn document_count_result(&self) -> Result<usize, DynError> {
        let count = self.db.conn().query_row(
            "
            SELECT COUNT(
                DISTINCT collection_name || ':' ||
                COALESCE(NULLIF(source_document_id, ''), document_id)
            )
            FROM rag_chunks
            ",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count as usize)
    }

    fn collection_summaries_result(&self) -> Result<Vec<RagCollectionSummary>, DynError> {
        let chunks = self.load_chunks()?;
        let mut by_collection = HashMap::<String, RagCollectionAccumulator>::new();

        for chunk in chunks {
            let entry = by_collection
                .entry(chunk.collection.clone())
                .or_insert_with(|| RagCollectionAccumulator {
                    chunk_count: 0,
                    document_ids: HashSet::new(),
                    sample_titles: Vec::new(),
                });
            entry.chunk_count += 1;

            if entry.document_ids.insert(chunk.document_id.clone())
                && !entry
                    .sample_titles
                    .iter()
                    .any(|title| title == &chunk.title)
                && entry.sample_titles.len() < 3
            {
                entry.sample_titles.push(chunk.title.clone());
            }
        }

        let ordered_names =
            normalize_rag_collections(&by_collection.keys().cloned().collect::<Vec<_>>());
        Ok(ordered_names
            .into_iter()
            .filter_map(|name| {
                by_collection
                    .remove(&name)
                    .map(|entry| RagCollectionSummary {
                        label: rag_collection_label(&name).to_string(),
                        group: rag_collection_group(&name),
                        name,
                        document_count: entry.document_ids.len(),
                        chunk_count: entry.chunk_count,
                        sample_titles: entry.sample_titles,
                    })
            })
            .collect())
    }

    fn search_result(
        &self,
        query: &str,
        top_k: usize,
        collections: &[String],
    ) -> Result<Vec<RagHit>, DynError> {
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let normalized_collections = normalize_rag_collections(collections);
        let requested_specialized_collections = normalized_collections
            .iter()
            .any(|collection| collection != RAG_COLLECTION_MAIN);
        let chunks = self.load_chunks()?;
        let chunks = if normalized_collections.is_empty() {
            chunks
        } else {
            chunks
                .into_iter()
                .filter(|chunk| normalized_collections.contains(&chunk.collection))
                .collect::<Vec<_>>()
        };
        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }
        let query_embedding = self
            .embedder
            .embed(EmbeddingRequest {
                input: query.to_string(),
            })?
            .vector;

        let mut chunk_term_counts = Vec::with_capacity(chunks.len());
        let mut document_frequencies: HashMap<String, usize> = HashMap::new();
        for chunk in &chunks {
            let term_counts = count_terms(&chunk.text);
            for term in term_counts.keys() {
                *document_frequencies.entry(term.clone()).or_insert(0) += 1;
            }
            chunk_term_counts.push(term_counts);
        }

        let total_chunks = chunks.len() as f32;
        let lowered_query = query.to_ascii_lowercase();
        let mut hits = Vec::new();

        for (chunk, term_counts) in chunks.iter().zip(chunk_term_counts.iter()) {
            let mut score = 0.0;
            for token in &query_tokens {
                if let Some(term_frequency) = term_counts.get(token) {
                    let document_frequency = *document_frequencies.get(token).unwrap_or(&1) as f32;
                    let inverse_document_frequency =
                        ((total_chunks + 1.0) / (document_frequency + 1.0)).ln() + 1.0;
                    score += *term_frequency as f32 * inverse_document_frequency;
                }
            }

            let normalized_text = chunk.text.to_ascii_lowercase();
            if normalized_text.contains(&lowered_query) {
                score += 3.0;
            }

            let semantic_score = cosine_similarity(&query_embedding, &chunk.embedding).max(0.0);
            score += semantic_score * 2.5;

            if requested_specialized_collections && chunk.collection != RAG_COLLECTION_MAIN {
                score += 1.25;
            }

            if score > 0.0 {
                hits.push(RagHit {
                    document_id: chunk.document_id.clone(),
                    uri: chunk.uri.clone(),
                    title: chunk.title.clone(),
                    chunk_index: chunk.chunk_index,
                    score,
                    snippet: excerpt(&chunk.text, &lowered_query, &query_tokens),
                    tags: chunk.tags.clone(),
                    collection: chunk.collection.clone(),
                });
            }
        }

        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.document_id.cmp(&right.document_id))
                .then_with(|| left.chunk_index.cmp(&right.chunk_index))
        });
        if requested_specialized_collections {
            hits = balance_rag_hits(hits, &normalized_collections, top_k);
        } else {
            hits.truncate(top_k);
        }
        Ok(hits)
    }

    fn load_chunks(&self) -> Result<Vec<RagChunkRecord>, DynError> {
        let mut stmt = self.db.conn().prepare(
            "
            SELECT document_id, uri, title, tags_json, chunk_index, text, embedding_json,
                   collection_name, source_document_id
            FROM rag_chunks
            ORDER BY document_id ASC, chunk_index ASC
            ",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;

        let mut chunks = Vec::new();
        for row in rows {
            let (
                document_id,
                uri,
                title,
                tags_json,
                chunk_index,
                text,
                embedding_json,
                collection_name,
                source_document_id,
            ) = row?;
            chunks.push(RagChunkRecord {
                document_id: visible_document_id(&document_id, &source_document_id),
                collection: normalize_rag_collection_name(&collection_name),
                uri,
                title,
                tags: serde_json::from_str(&tags_json)?,
                chunk_index: chunk_index as usize,
                text,
                embedding: serde_json::from_str(&embedding_json)?,
            });
        }
        Ok(chunks)
    }

    fn prune_to_budget(&mut self) -> Result<(), DynError> {
        let budget = self.budget.max_bytes as i64;
        loop {
            let current_bytes: i64 = self.db.conn().query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM rag_chunks",
                [],
                |row| row.get(0),
            )?;

            if current_bytes <= budget {
                break;
            }

            let document_count: i64 = self.db.conn().query_row(
                "SELECT COUNT(DISTINCT document_id) FROM rag_chunks",
                [],
                |row| row.get(0),
            )?;

            if document_count <= 1 {
                break;
            }
            let next_document: Option<String> = self
                .db
                .conn()
                .query_row(
                    "
                SELECT document_id
                FROM rag_chunks
                GROUP BY document_id
                ORDER BY MIN(indexed_at) ASC, document_id ASC
                LIMIT 1
                ",
                    [],
                    |row| row.get(0),
                )
                .ok();

            let Some(document_id) = next_document else {
                break;
            };

            let deleted = self.db.conn_mut().execute(
                "DELETE FROM rag_chunks WHERE document_id = ?1",
                params![document_id],
            )?;

            if deleted == 0 {
                break;
            }
        }

        Ok(())
    }
}

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
        vec!["rag.ingest_document".to_string(), "rag.query".to_string()]
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
            }
        }

        Ok(())
    }
}

#[derive(Debug, Default)]
struct RagCollectionAccumulator {
    chunk_count: usize,
    document_ids: HashSet<String>,
    sample_titles: Vec<String>,
}

fn storage_error(error: StorageTaskError) -> DynError {
    Box::new(std::io::Error::other(error.to_string()))
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

fn chunk_text(text: &str, config: &ChunkingConfig) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let target_words = config.target_words.max(1);
    let overlap_words = config.overlap_words.min(target_words.saturating_sub(1));
    let step = (target_words - overlap_words).max(1);

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < words.len() {
        let end = (start + target_words).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += step;
    }

    chunks
}

fn count_terms(text: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for token in tokenize(text) {
        *counts.entry(token).or_insert(0) += 1;
    }
    counts
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(std::string::ToString::to_string)
        .collect()
}

fn excerpt(text: &str, lowered_query: &str, query_tokens: &[String]) -> String {
    let normalized_text = text.to_ascii_lowercase();
    let anchor = normalized_text
        .find(lowered_query)
        .or_else(|| {
            query_tokens
                .iter()
                .find_map(|token| normalized_text.find(token))
        })
        .unwrap_or(0);

    let mut start = anchor.saturating_sub(80);
    let mut end = (anchor + lowered_query.len().max(24) + 120).min(text.len());

    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }

    text[start..end]
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
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
    format!(
        "{}::{}::{}",
        hit.collection, hit.document_id, hit.chunk_index
    )
}

fn storage_document_id(collection: &str, document_id: &str) -> String {
    format!("{collection}::{document_id}")
}

fn visible_document_id(storage_document_id: &str, source_document_id: &str) -> String {
    if !source_document_id.trim().is_empty() {
        return source_document_id.to_string();
    }

    storage_document_id
        .split_once("::")
        .map(|(_, document_id)| document_id.to_string())
        .unwrap_or_else(|| storage_document_id.to_string())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use futures::StreamExt;
    use ordo_bus::{Bus, InProcessBus};
    use ordo_protocol::{topics, CorrelationId, Envelope, NodeId, OrdoMessage, RagDocument};

    use super::{RagPeer, RagStorageBudget, RagStore};

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
        assert!(operations_hits
            .iter()
            .all(|hit| hit.collection == "operations" && hit.document_id != "research-plan"));

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
        let mut store = RagStore::in_memory_with_budget(RagStorageBudget { max_bytes: 1200 });
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

        peer_task.abort();
    }
}
