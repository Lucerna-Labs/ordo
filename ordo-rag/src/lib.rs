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
use ordo_models::{cosine_similarity, EmbeddingClient, EmbeddingRequest};
use ordo_protocol::{
    default_rag_collection_name, normalize_rag_collection_name, normalize_rag_collections,
    rag_collection_group, rag_collection_label, topics, CorrelationId, Envelope, NodeId,
    NodeStatus, OrdoMessage, RagCollectionSummary, RagDocument, RagHit, RAG_COLLECTION_MAIN,
};
use ordo_store::{OrdoDatabase, StorageTask, StorageTaskError};
use rusqlite::{params, TransactionBehavior};
use serde::{Deserialize, Serialize};
use tokio::task;

mod index;
mod learn;

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// Signals the self-learning tree contributes to one search: chunk
/// reinforcement bonuses (keyed by chunk key) and learned routing
/// bonuses (keyed by collection; empty when the search is
/// collection-scoped or nothing is learned yet).
struct LearnedSignals {
    feedback: HashMap<String, f32>,
    routing: HashMap<String, f32>,
}

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
    /// Optional generative re-ranking channel (Ollama / llama.cpp).
    /// `None` — the default — runs the generative-free
    /// lexical-statistical engine in `index.rs` alone.
    embedder: Option<Arc<dyn EmbeddingClient>>,
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
        let db = OrdoDatabase::in_memory().expect("open in-memory sqlite database");
        Self::from_database(db, None, budget).expect("prepare in-memory rag index")
    }

    pub fn in_memory_with_embedder(
        embedder: Arc<dyn EmbeddingClient>,
        budget: RagStorageBudget,
    ) -> Self {
        let db = OrdoDatabase::in_memory().expect("open in-memory sqlite database");
        Self::from_database(db, Some(embedder), budget).expect("prepare in-memory rag index")
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DynError> {
        Self::open_with_budget(path, RagStorageBudget::default())
    }

    pub fn open_with_budget(
        path: impl Into<PathBuf>,
        budget: RagStorageBudget,
    ) -> Result<Self, DynError> {
        Self::from_database(OrdoDatabase::open(path)?, None, budget)
    }

    pub fn open_with_embedder(
        path: impl Into<PathBuf>,
        embedder: Arc<dyn EmbeddingClient>,
        budget: RagStorageBudget,
    ) -> Result<Self, DynError> {
        Self::from_database(OrdoDatabase::open(path)?, Some(embedder), budget)
    }

    fn from_database(
        mut db: OrdoDatabase,
        embedder: Option<Arc<dyn EmbeddingClient>>,
        budget: RagStorageBudget,
    ) -> Result<Self, DynError> {
        // Databases created before the lexical-statistical index — or
        // ones whose index has drifted — rebuild it from chunk text.
        index::rebuild_if_needed(db.conn_mut())?;
        Ok(Self {
            db,
            config: ChunkingConfig::default(),
            embedder,
            budget,
        })
    }

    pub fn embedding_backend(&self) -> &str {
        self.embedder
            .as_deref()
            .map(EmbeddingClient::backend_name)
            .unwrap_or("lexical-statistical")
    }

    pub fn embedding_dimensions(&self) -> usize {
        self.embedder
            .as_deref()
            .map(EmbeddingClient::dimensions)
            .unwrap_or(0)
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
        // IMMEDIATE: this transaction reads (the replaced-chunk SELECT)
        // before writing. A deferred transaction that upgrades to a
        // write after another connection commits fails instantly with
        // SQLITE_BUSY_SNAPSHOT, bypassing the busy_timeout retry.
        let tx = self
            .db
            .conn_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let indexed_at = Utc::now().to_rfc3339();

        // Subtract the outgoing chunks from the lexical-statistical
        // index before their rows disappear.
        let replaced: Vec<(String, i64, String)> = {
            let mut stmt = tx.prepare(
                "
                SELECT document_id, chunk_index, text FROM rag_chunks
                WHERE document_id = ?1
                   OR source_document_id = ?2
                   OR (source_document_id = '' AND document_id = ?2)
                ",
            )?;
            let rows = stmt.query_map(params![&storage_document_id, &source_document_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        for (chunk_document_id, chunk_index, text) in &replaced {
            index::deindex_chunk(&tx, chunk_document_id, *chunk_index as usize, text)?;
        }
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
            let embedding_json =
                serde_json::to_string(&embed_or_empty(self.embedder.as_ref(), text)?)?;
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
            index::index_chunk(&tx, &storage_document_id, chunk_index, &collection, text)?;
        }

        tx.commit()?;
        self.prune_to_budget()?;
        Ok(chunk_texts.len())
    }


    pub fn search(&self, query: &str, top_k: usize) -> Vec<RagHit> {
        self.search_in_collections(query, top_k, &[])
    }

    /// Record usage feedback for a hit previously returned by search —
    /// the write half of the self-learning retrieval tree.
    ///
    /// `content_hash` is the fingerprint the hit carried; when
    /// non-empty it must still match the stored chunk text, so
    /// feedback about text that was replaced in the meantime is
    /// rejected instead of poisoning the learned signals. Returns
    /// false when the feedback could not be pinned to the chunk the
    /// caller saw.
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
        let source_document_id = document_id.trim().to_string();
        // Hits identify documents by their source id; resolve back to
        // the storage id, trying the legacy "::" join for rows written
        // before the unit-separator format.
        let storage_candidates = [
            storage_document_id(&collection, &source_document_id),
            format!("{collection}::{source_document_id}"),
        ];

        let tx = self
            .db
            .conn_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut recorded = false;
        for storage_id in &storage_candidates {
            let text: Option<String> = tx
                .query_row(
                    "SELECT text FROM rag_chunks WHERE document_id = ?1 AND chunk_index = ?2",
                    params![storage_id, chunk_index as i64],
                    |row| row.get(0),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            let Some(text) = text else {
                continue;
            };
            if !content_hash.is_empty() && index::content_hash(&text) != content_hash {
                // Same identity, different text: the document was
                // re-ingested since the hit. Learning from this would
                // bind the query's vocabulary to text the caller never
                // judged.
                continue;
            }
            let chunk_key = index::chunk_key(storage_id, chunk_index);
            learn::record_feedback(&tx, &chunk_key, &collection, query, useful)?;
            recorded = true;
            break;
        }
        tx.commit()?;
        Ok(recorded)
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
        // IMMEDIATE for the same read-then-write reason as
        // upsert_document.
        let tx = self
            .db
            .conn_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
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
                embed_or_empty(self.embedder.as_ref(), &chunk.text)?
            } else {
                chunk.embedding.clone()
            };
            let tags_json = serde_json::to_string(&chunk.tags)?;

            // The INSERT OR REPLACE below can displace an existing row;
            // subtract that row from the index first so counts stay exact.
            let displaced: Option<String> = tx
                .query_row(
                    "SELECT text FROM rag_chunks WHERE document_id = ?1 AND chunk_index = ?2",
                    params![&storage_document_id, chunk.chunk_index as i64],
                    |row| row.get(0),
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            if let Some(old_text) = displaced {
                index::deindex_chunk(&tx, &storage_document_id, chunk.chunk_index, &old_text)?;
            }
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
            index::index_chunk(
                &tx,
                &storage_document_id,
                chunk.chunk_index,
                &collection,
                &chunk.text,
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
        // CHAR(31) (unit separator) join for the same reason as
        // storage_document_id: a ':' join would fold ('a', 'b:c') and
        // ('a:b', 'c') into one identity and undercount.
        let count = self.db.conn().query_row(
            "
            SELECT COUNT(
                DISTINCT collection_name || CHAR(31) ||
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

        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }

        // Channel 1+2+3: BM25 over the inverted index, with fuzzy
        // vocabulary matching and PPMI co-occurrence expansion folded
        // into the query term set. Touches only chunks that share a
        // term with the (expanded) query.
        let scored = index::score_query(self.db.conn(), query, &normalized_collections)?;

        // Optional generative channel: only when an embedding model is
        // wired in, and never fatal — a dead Ollama degrades search to
        // the lexical-statistical engine instead of failing it.
        let query_embedding: Option<Vec<f32>> = self.embedder.as_ref().and_then(|embedder| {
            match embedder.embed(EmbeddingRequest {
                input: query.to_string(),
            }) {
                Ok(response) => Some(response.vector),
                Err(error) => {
                    tracing::warn!(
                        backend = embedder.backend_name(),
                        %error,
                        "embedding query failed; falling back to lexical-statistical scoring"
                    );
                    None
                }
            }
        });

        let candidate_limit = (top_k * 10).max(50);
        let lowered_query = query.to_ascii_lowercase();

        // What the self-learning tree knows: chunk reinforcement
        // always applies; learned routing only steers UNSCOPED
        // searches (a scoped search already had its branch chosen).
        let routing_terms = learn::content_terms(query);
        let learned = LearnedSignals {
            feedback: learn::feedback_bonuses(self.db.conn())?,
            routing: if normalized_collections.is_empty() {
                learn::routing_bonuses(self.db.conn(), &routing_terms)?
            } else {
                HashMap::new()
            },
        };

        let mut hits = if let Some(query_embedding) = &query_embedding {
            // A working embedding model is a RETRIEVAL channel, not a
            // mere re-ranker: scan every in-scope chunk so a
            // semantic-only match (zero term overlap) is still
            // reachable, exactly as the pre-index engine behaved. The
            // corpus scan is the price of the generative channel; the
            // default model-free path below never pays it.
            self.fusion_hits_over_all_chunks(
                scored,
                query_embedding,
                &normalized_collections,
                requested_specialized_collections,
                &lowered_query,
                &query_tokens,
                &learned,
            )?
        } else {
            // Queries the term index cannot serve (e.g. all-stopword
            // phrases like "who are you") fall back to an exact
            // substring scan before giving up.
            let scored = if scored.is_empty() {
                index::phrase_fallback(
                    self.db.conn(),
                    &lowered_query,
                    &normalized_collections,
                    candidate_limit,
                )?
            } else {
                scored
            };
            if scored.is_empty() {
                return Ok(Vec::new());
            }
            let candidates =
                pool_candidates(scored, &normalized_collections, candidate_limit);
            self.hydrate_candidates(
                candidates,
                requested_specialized_collections,
                &lowered_query,
                &query_tokens,
                &learned,
            )?
        };

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

        // Implicit branch learning: the top hit of an unscoped search
        // weakly reinforces its collection for this query's terms.
        // Never fatal — a failed learning write must not fail the
        // search that triggered it.
        if normalized_collections.is_empty() && !routing_terms.is_empty() {
            if let Some(top_hit) = hits.first() {
                if let Err(error) = learn::reinforce_routing_implicit(
                    self.db.conn(),
                    &routing_terms,
                    &top_hit.collection,
                ) {
                    tracing::warn!(%error, "implicit routing reinforcement failed");
                }
            }
        }

        Ok(hits)
    }

    /// Generative-channel scoring: every in-scope chunk gets
    /// BM25 (from the index) + exact-phrase bonus + cosine + collection
    /// bonus, so purely semantic matches survive.
    #[allow(clippy::too_many_arguments)]
    fn fusion_hits_over_all_chunks(
        &self,
        scored: Vec<index::ScoredChunk>,
        query_embedding: &[f32],
        normalized_collections: &[String],
        requested_specialized_collections: bool,
        lowered_query: &str,
        query_tokens: &[String],
        learned: &LearnedSignals,
    ) -> Result<Vec<RagHit>, DynError> {
        let bm25: HashMap<(String, usize), f64> = scored
            .into_iter()
            .map(|chunk| ((chunk.document_id, chunk.chunk_index), chunk.score))
            .collect();

        let mut stmt = self.db.conn().prepare_cached(
            "
            SELECT document_id, chunk_index, uri, title, tags_json, text,
                   embedding_json, collection_name, source_document_id
            FROM rag_chunks
            ",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;

        let mut hits = Vec::new();
        for row in rows {
            let (
                document_id,
                chunk_index,
                uri,
                title,
                tags_json,
                text,
                embedding_json,
                collection_name,
                source_document_id,
            ) = row?;
            let collection = normalize_rag_collection_name(&collection_name);
            if !normalized_collections.is_empty() && !normalized_collections.contains(&collection)
            {
                continue;
            }
            let chunk_index = chunk_index as usize;

            let mut score = bm25
                .get(&(document_id.clone(), chunk_index))
                .copied()
                .unwrap_or(0.0) as f32;

            let normalized_text = text.to_ascii_lowercase();
            if normalized_text.contains(lowered_query) {
                score += 2.0;
            }

            let chunk_embedding: Vec<f32> =
                serde_json::from_str(&embedding_json).unwrap_or_default();
            score += cosine_similarity(query_embedding, &chunk_embedding).max(0.0) * 2.5;

            if requested_specialized_collections && collection != RAG_COLLECTION_MAIN {
                score += 1.25;
            }

            // Learned signals rerank matches; they never conjure a hit
            // out of a chunk with no retrieval evidence of its own.
            if score > 0.0 {
                let chunk_key = index::chunk_key(&document_id, chunk_index);
                if let Some(bonus) = learned.feedback.get(&chunk_key) {
                    score += bonus;
                }
                if let Some(bonus) = learned.routing.get(&collection) {
                    score += bonus;
                }
            }

            if score > 0.0 {
                hits.push(RagHit {
                    document_id: visible_document_id(&document_id, &source_document_id),
                    uri,
                    title,
                    chunk_index,
                    score,
                    snippet: excerpt(&text, lowered_query, query_tokens),
                    tags: serde_json::from_str(&tags_json)?,
                    collection,
                    content_hash: index::content_hash(&text),
                });
            }
        }
        Ok(hits)
    }

    /// Default-path scoring: hydrate only the pooled BM25 candidates
    /// and apply the model-free bonuses.
    fn hydrate_candidates(
        &self,
        candidates: Vec<index::ScoredChunk>,
        requested_specialized_collections: bool,
        lowered_query: &str,
        query_tokens: &[String],
        learned: &LearnedSignals,
    ) -> Result<Vec<RagHit>, DynError> {
        let mut hydrate = self.db.conn().prepare_cached(
            "
            SELECT uri, title, tags_json, text, source_document_id
            FROM rag_chunks
            WHERE document_id = ?1 AND chunk_index = ?2
            ",
        )?;

        let mut hits = Vec::new();
        for candidate in candidates {
            let row = hydrate
                .query_row(
                    params![&candidate.document_id, candidate.chunk_index as i64],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            let Some((uri, title, tags_json, text, source_document_id)) = row else {
                continue;
            };

            let mut score = candidate.score as f32;

            // Exact-phrase bonus keeps verbatim matches ahead of
            // bag-of-terms matches (and is the whole score for
            // phrase-fallback candidates).
            let normalized_text = text.to_ascii_lowercase();
            if normalized_text.contains(lowered_query) {
                score += 2.0;
            }

            if requested_specialized_collections && candidate.collection != RAG_COLLECTION_MAIN {
                score += 1.25;
            }

            // Candidates already carry retrieval evidence (BM25 or the
            // phrase fallback's exact match), so learned signals may
            // rerank — including suppressing a repeatedly-not-useful
            // chunk below the score floor.
            let chunk_key = index::chunk_key(&candidate.document_id, candidate.chunk_index);
            if let Some(bonus) = learned.feedback.get(&chunk_key) {
                score += bonus;
            }
            if let Some(bonus) = learned.routing.get(&candidate.collection) {
                score += bonus;
            }

            if score > 0.0 {
                hits.push(RagHit {
                    document_id: visible_document_id(&candidate.document_id, &source_document_id),
                    uri,
                    title,
                    chunk_index: candidate.chunk_index,
                    score,
                    snippet: excerpt(&text, lowered_query, query_tokens),
                    tags: serde_json::from_str(&tags_json)?,
                    collection: candidate.collection,
                    content_hash: index::content_hash(&text),
                });
            }
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

            let tx = self
                .db
                .conn_mut()
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let evicted: Vec<(i64, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT chunk_index, text FROM rag_chunks WHERE document_id = ?1",
                )?;
                let rows = stmt.query_map(params![&document_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<Result<_, _>>()?
            };
            for (chunk_index, text) in &evicted {
                index::deindex_chunk(&tx, &document_id, *chunk_index as usize, text)?;
            }
            let deleted = tx.execute(
                "DELETE FROM rag_chunks WHERE document_id = ?1",
                params![document_id],
            )?;
            tx.commit()?;

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

#[derive(Debug, Default)]
struct RagCollectionAccumulator {
    chunk_count: usize,
    document_ids: HashSet<String>,
    sample_titles: Vec<String>,
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

/// Cap candidates per collection instead of globally: with one shared
/// cap, a hot collection's long BM25 tail can crowd every other
/// requested collection out of the pool before `balance_rag_hits` ever
/// runs, silently breaking its per-collection representation
/// guarantee.
fn pool_candidates(
    scored: Vec<index::ScoredChunk>,
    normalized_collections: &[String],
    per_collection_limit: usize,
) -> Vec<index::ScoredChunk> {
    if normalized_collections.len() <= 1 {
        return scored.into_iter().take(per_collection_limit).collect();
    }
    let mut taken: HashMap<String, usize> = HashMap::new();
    scored
        .into_iter()
        .filter(|chunk| {
            let count = taken.entry(chunk.collection.clone()).or_insert(0);
            if *count < per_collection_limit {
                *count += 1;
                true
            } else {
                false
            }
        })
        .collect()
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

fn visible_document_id(storage_document_id: &str, source_document_id: &str) -> String {
    if !source_document_id.trim().is_empty() {
        return source_document_id.to_string();
    }

    // Current rows join with the unit separator; rows written before
    // this refactor used "::".
    storage_document_id
        .split_once('\u{1f}')
        .or_else(|| storage_document_id.split_once("::"))
        .map(|(_, document_id)| document_id.to_string())
        .unwrap_or_else(|| storage_document_id.to_string())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use futures::StreamExt;
    use ordo_bus::{Bus, InProcessBus};
    use ordo_models::{EmbeddingClient, EmbeddingRequest, EmbeddingResponse, ModelResult};
    use ordo_protocol::{topics, CorrelationId, Envelope, NodeId, OrdoMessage, RagDocument};

    use super::{RagPeer, RagStorageBudget, RagStore};

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
        assert!(!hits.is_empty(), "stopword phrase should match via fallback");
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

        let hits = store.search("transport relay", 2);
        assert_eq!(hits[0].document_id, "twin-a", "ties break by document id");

        store
            .record_feedback("transport relay", "twin-b", "main", 0, "", true)
            .expect("record feedback");
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

        let affinity: f64 = store
            .db
            .conn()
            .query_row(
                "SELECT affinity FROM rag_routing_stats
                 WHERE term = 'transport' AND collection_name = 'research'",
                [],
                |row| row.get(0),
            )
            .expect("routing affinity row");
        assert!(affinity >= 2.0, "explicit feedback should teach routing");

        let hits = store.search("transport relay", 2);
        assert_eq!(
            hits[0].document_id, "res-doc",
            "learned routing and reinforcement should steer the unscoped query"
        );
    }

    #[test]
    fn feedback_rows_follow_document_replacement() {
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

        store.upsert_document(&document).expect("replace doc");
        let feedback_rows: i64 = store
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM rag_chunk_feedback", [], |row| {
                row.get(0)
            })
            .expect("count feedback rows");
        assert_eq!(
            feedback_rows, 0,
            "replacing a document must clear its learned reinforcement"
        );
    }

    #[test]
    fn default_store_needs_no_embedding_model() {
        let store = RagStore::in_memory();
        assert_eq!(store.embedding_backend(), "lexical-statistical");
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
                content: "The transport adapter seam keeps routing policy separate."
                    .to_string(),
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
            "PPMI expansion should pull in the transport-only chunk: {hits:?}"
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
        let df_first: i64 = store
            .db
            .conn()
            .query_row(
                "SELECT df FROM rag_terms WHERE term = 'transport'",
                [],
                |row| row.get(0),
            )
            .expect("df after first upsert");

        store.upsert_document(&document).expect("second upsert");
        let df_second: i64 = store
            .db
            .conn()
            .query_row(
                "SELECT df FROM rag_terms WHERE term = 'transport'",
                [],
                |row| row.get(0),
            )
            .expect("df after second upsert");
        assert_eq!(df_first, 1);
        assert_eq!(
            df_second, 1,
            "re-upserting the same document must not inflate document frequency"
        );

        let stats_rows: i64 = store
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM rag_chunk_stats", [], |row| row.get(0))
            .expect("chunk stats count");
        assert_eq!(stats_rows as usize, store.chunk_count());
    }

    #[test]
    fn legacy_databases_rebuild_index_on_open() {
        let dir = std::env::temp_dir().join(format!("ordo-rag-rebuild-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("legacy.db");
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
            // Simulate a database from before the index existed.
            for table in [
                "rag_postings",
                "rag_terms",
                "rag_chunk_stats",
                "rag_cooc",
                "rag_corpus_stats",
            ] {
                store
                    .db
                    .conn()
                    .execute(&format!("DELETE FROM {table}"), [])
                    .expect("wipe index table");
            }
            // Reordered terms are not a contiguous substring, so the
            // exact-phrase fallback cannot serve this query — only the
            // (now wiped) term index could.
            assert!(store.search("adapter transport routing", 3).is_empty());
        }

        let store = RagStore::open(&path).expect("reopen legacy store");
        let hits = store.search("adapter transport routing", 3);
        assert!(
            !hits.is_empty(),
            "reopening must rebuild the index from chunk text"
        );
        assert_eq!(hits[0].document_id, "architecture");

        drop(store);
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

