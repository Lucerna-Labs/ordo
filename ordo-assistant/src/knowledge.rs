//! Self-knowledge RAG (push 3) â€” the assistant's own playbook.
//!
//! Where `FactStore` remembers facts *about the operator*, the
//! `KnowledgeStore` remembers things *about the assistant*: skill
//! cards, persona guides, capability notes, and observations about
//! what worked or didn't on past turns. The LLM reaches this layer
//! via the `assistant.knowledge_lookup` meta-tool â€” that's how it
//! discovers what it can do without having every capability dumped
//! into the system prompt up front.
//!
//! The shape mirrors `FactStore`: a thin wrapper over `AssistantStore`
//! that runs the shared embedder on the way in and does cosine
//! similarity ranking on the way out.

use std::sync::Arc;

use ordo_models::{EmbeddingClient, EmbeddingRequest};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::store::AssistantStore;
use crate::types::{
    AssistantError, AssistantResult, KnowledgeEntry, KnowledgeKind, KnowledgeSummary, NewKnowledge,
    RecalledKnowledge,
};

#[derive(Clone)]
pub struct KnowledgeStore {
    store: Arc<Mutex<AssistantStore>>,
    embedder: Arc<dyn EmbeddingClient>,
}

impl KnowledgeStore {
    pub fn new(store: Arc<Mutex<AssistantStore>>, embedder: Arc<dyn EmbeddingClient>) -> Self {
        Self { store, embedder }
    }

    pub async fn remember(&self, new_entry: NewKnowledge) -> AssistantResult<KnowledgeEntry> {
        let semantic = format!("{} {}", new_entry.title, new_entry.body);
        let embedding = embed(&self.embedder, &semantic)?;
        let mut guard = self.store.lock();
        guard.insert_knowledge(new_entry, embedding)
    }

    /// Insert or update a knowledge entry keyed by `source`. Used by
    /// the boot-time seeder so re-running doesn't duplicate rows.
    pub async fn upsert_by_source(
        &self,
        new_entry: NewKnowledge,
    ) -> AssistantResult<KnowledgeEntry> {
        let semantic = format!("{} {}", new_entry.title, new_entry.body);
        let embedding = embed(&self.embedder, &semantic)?;
        let mut guard = self.store.lock();
        guard.upsert_knowledge_by_source(new_entry, embedding)
    }

    pub fn get(&self, id: Uuid) -> AssistantResult<Option<KnowledgeEntry>> {
        self.store.lock().get_knowledge(id)
    }

    pub fn list(
        &self,
        kind: Option<KnowledgeKind>,
        domain: Option<&str>,
    ) -> AssistantResult<Vec<KnowledgeEntry>> {
        self.store.lock().list_knowledge(kind, domain)
    }

    pub fn forget(&self, id: Uuid) -> AssistantResult<bool> {
        self.store.lock().delete_knowledge(id)
    }

    pub fn reinforce(&self, id: Uuid) -> AssistantResult<Option<KnowledgeEntry>> {
        self.store.lock().reinforce_knowledge(id)
    }

    /// Top-K cosine-similarity lookup over the knowledge table.
    /// Optionally scoped to a kind and/or domain.
    pub async fn recall(
        &self,
        query: &str,
        top_k: usize,
        kind: Option<KnowledgeKind>,
        domain: Option<&str>,
    ) -> AssistantResult<Vec<RecalledKnowledge>> {
        if query.trim().is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }
        let query_vec = embed(&self.embedder, query)?;
        let entries = self.list(kind, domain)?;
        let mut scored: Vec<(f32, KnowledgeSummary)> = entries
            .iter()
            .map(|entry| {
                let score = cosine_similarity(&query_vec, &entry.embedding);
                (score, KnowledgeSummary::from(entry))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored
            .into_iter()
            .map(|(score, entry)| RecalledKnowledge { entry, score })
            .collect())
    }
}

fn embed(embedder: &Arc<dyn EmbeddingClient>, text: &str) -> AssistantResult<Vec<f32>> {
    let response = embedder
        .embed(EmbeddingRequest {
            input: text.to_string(),
        })
        .map_err(|err| AssistantError::Embedding(err.to_string()))?;
    Ok(response.vector)
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}
