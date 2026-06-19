//! Semantic fact recall. Given the current turn text and an embedder,
//! rank every stored fact by cosine similarity and return the top-K.

use std::sync::Arc;

use ordo_models::{EmbeddingClient, EmbeddingRequest};
use parking_lot::Mutex;

use crate::store::AssistantStore;
use crate::types::{AssistantError, AssistantResult, Fact, FactSummary, NewFact, RecalledFact};

/// Thin coordinator around a shared store + embedder. Holds the
/// embedder as `Arc<dyn EmbeddingClient>` so the runtime can reuse
/// the same one it set up for the RAG lane.
#[derive(Clone)]
pub struct FactStore {
    store: Arc<Mutex<AssistantStore>>,
    embedder: Arc<dyn EmbeddingClient>,
}

impl FactStore {
    pub fn new(store: Arc<Mutex<AssistantStore>>, embedder: Arc<dyn EmbeddingClient>) -> Self {
        Self { store, embedder }
    }

    /// Embed the fact's canonical form + persist.
    pub async fn remember(&self, new_fact: NewFact) -> AssistantResult<Fact> {
        let embedding = embed(
            self.embedder.as_ref(),
            &format!(
                "{} {} {}",
                new_fact.subject, new_fact.predicate, new_fact.object,
            ),
        )?;
        let mut store = self.store.lock();
        store.insert_fact(new_fact, embedding)
    }

    pub fn list(&self, subject: Option<&str>) -> AssistantResult<Vec<Fact>> {
        self.store.lock().list_facts(subject)
    }

    pub fn forget(&self, id: uuid::Uuid) -> AssistantResult<bool> {
        self.store.lock().delete_fact(id)
    }

    pub fn get(&self, id: uuid::Uuid) -> AssistantResult<Option<Fact>> {
        self.store.lock().get_fact(id)
    }

    pub fn reinforce(&self, id: uuid::Uuid) -> AssistantResult<Option<Fact>> {
        self.store.lock().reinforce_fact(id)
    }

    /// Recall facts relevant to `query`, ranked by cosine similarity
    /// of query-embedding vs fact-embedding. `top_k` is clamped to
    /// the current fact count.
    ///
    /// Legacy entry point: searches all scopes. Pre-mode callers use
    /// this; mode-aware callers should use [`recall_in_scopes`].
    pub async fn recall(&self, query: &str, top_k: usize) -> AssistantResult<Vec<RecalledFact>> {
        self.recall_inner(query, top_k, None).await
    }

    /// Recall, restricted to the supplied set of scopes. Used by the
    /// `assistant.recall_memory` meta-tool with the active mode's
    /// `memory_scope` list. Empty `scopes` returns no facts (fail-
    /// closed) so a caller that meant to allow `["global"]` and
    /// passed `[]` doesn't accidentally surface everything.
    pub async fn recall_in_scopes(
        &self,
        query: &str,
        top_k: usize,
        scopes: &[String],
    ) -> AssistantResult<Vec<RecalledFact>> {
        self.recall_inner(query, top_k, Some(scopes)).await
    }

    async fn recall_inner(
        &self,
        query: &str,
        top_k: usize,
        scopes: Option<&[String]>,
    ) -> AssistantResult<Vec<RecalledFact>> {
        if top_k == 0 {
            return Ok(Vec::new());
        }
        let facts = match scopes {
            Some(scopes) => self.store.lock().list_facts_in_scopes(None, scopes)?,
            None => self.store.lock().list_facts(None)?,
        };
        if facts.is_empty() {
            return Ok(Vec::new());
        }
        let query_embedding = embed(self.embedder.as_ref(), query)?;
        let mut scored: Vec<(f32, &Fact)> = facts
            .iter()
            .map(|fact| (cosine_similarity(&query_embedding, &fact.embedding), fact))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let cutoff = top_k.min(scored.len());
        Ok(scored
            .into_iter()
            .take(cutoff)
            .filter(|(score, _)| score.is_finite())
            .map(|(score, fact)| RecalledFact {
                fact: FactSummary::from(fact),
                score,
            })
            .collect())
    }
}

fn embed(embedder: &dyn EmbeddingClient, text: &str) -> AssistantResult<Vec<f32>> {
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
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_models::HashingEmbedder;

    #[tokio::test]
    async fn recall_ranks_closest_facts_first() {
        let store = Arc::new(Mutex::new(AssistantStore::in_memory().expect("store")));
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(HashingEmbedder::new(96));
        let fact_store = FactStore::new(store, embedder);

        fact_store
            .remember(NewFact {
                subject: "user".into(),
                predicate: "prefers".into(),
                object: "terse response without marketing clichÃ©s".into(),
                source: "operator".into(),
                confidence: 1.0,
                scope: None,
            })
            .await
            .expect("terse fact");
        fact_store
            .remember(NewFact {
                subject: "user".into(),
                predicate: "lives".into(),
                object: "in Chicago".into(),
                source: "operator".into(),
                confidence: 1.0,
                scope: None,
            })
            .await
            .expect("city fact");
        fact_store
            .remember(NewFact {
                subject: "operator profile".into(),
                predicate: "avoids".into(),
                object: "exclamation points".into(),
                source: "operator".into(),
                confidence: 1.0,
                scope: None,
            })
            .await
            .expect("operator profile fact");

        let recalled = fact_store
            .recall("what's our operator style like?", 2)
            .await
            .expect("recall");
        assert!(!recalled.is_empty());
        // Top hit should mention operator profile or voice (terse/exclamation),
        // not Chicago.
        let top = &recalled[0];
        assert!(
            top.fact.object.contains("clichÃ©s") || top.fact.object.contains("exclamation"),
            "top fact was {:?}",
            top.fact
        );
    }

    #[tokio::test]
    async fn recall_in_scopes_filters_to_visible_facts_only() {
        let store = Arc::new(Mutex::new(AssistantStore::in_memory().expect("store")));
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(HashingEmbedder::new(96));
        let fact_store = FactStore::new(store, embedder);

        // Three facts: one global, one tagged for vibe_coding, one
        // tagged for planning.
        fact_store
            .remember(NewFact {
                subject: "user".into(),
                predicate: "lives".into(),
                object: "in Chicago".into(),
                source: "operator".into(),
                confidence: 1.0,
                scope: None, // -> "global"
            })
            .await
            .expect("global fact");
        fact_store
            .remember(NewFact {
                subject: "project".into(),
                predicate: "uses".into(),
                object: "rust 2021 edition".into(),
                source: "operator".into(),
                confidence: 1.0,
                scope: Some("mode:vibe_coding".into()),
            })
            .await
            .expect("vibe fact");
        fact_store
            .remember(NewFact {
                subject: "operator profile".into(),
                predicate: "voice".into(),
                object: "warm and concise".into(),
                source: "operator".into(),
                confidence: 1.0,
                scope: Some("mode:planning".into()),
            })
            .await
            .expect("planning fact");

        // Vibe Coding sees global + its own; not planning.
        let vibe_scopes = vec!["global".to_string(), "mode:vibe_coding".to_string()];
        let vibe_recall = fact_store
            .recall_in_scopes("anything", 10, &vibe_scopes)
            .await
            .expect("vibe recall");
        let vibe_objects: Vec<&str> = vibe_recall.iter().map(|r| r.fact.object.as_str()).collect();
        assert!(
            vibe_objects.iter().any(|o| o.contains("Chicago")),
            "global fact should be visible from vibe_coding"
        );
        assert!(
            vibe_objects.iter().any(|o| o.contains("rust")),
            "mode:vibe_coding fact should be visible from vibe_coding"
        );
        assert!(
            !vibe_objects.iter().any(|o| o.contains("warm")),
            "mode:planning fact should NOT be visible from vibe_coding"
        );

        // Planning sees global + its own; not vibe_coding.
        let planning_scopes = vec!["global".to_string(), "mode:planning".to_string()];
        let planning_recall = fact_store
            .recall_in_scopes("anything", 10, &planning_scopes)
            .await
            .expect("planning recall");
        let planning_objects: Vec<&str> = planning_recall
            .iter()
            .map(|r| r.fact.object.as_str())
            .collect();
        assert!(planning_objects.iter().any(|o| o.contains("Chicago")));
        assert!(planning_objects.iter().any(|o| o.contains("warm")));
        assert!(!planning_objects.iter().any(|o| o.contains("rust")));
    }

    #[tokio::test]
    async fn recall_in_scopes_with_empty_scopes_returns_nothing() {
        // Fail-closed: empty scope list = no facts. Caller should
        // pass at least ["global"] when they intend to allow all.
        let store = Arc::new(Mutex::new(AssistantStore::in_memory().expect("store")));
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(HashingEmbedder::new(96));
        let fact_store = FactStore::new(store, embedder);
        fact_store
            .remember(NewFact {
                subject: "user".into(),
                predicate: "is".into(),
                object: "test".into(),
                source: "operator".into(),
                confidence: 1.0,
                scope: None,
            })
            .await
            .unwrap();

        let recalled = fact_store
            .recall_in_scopes("anything", 10, &[])
            .await
            .expect("recall with empty scopes");
        assert!(recalled.is_empty(), "empty scopes must return no facts");
    }
}
