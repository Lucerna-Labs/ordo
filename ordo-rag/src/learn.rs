//! The self-learning retrieval tree — usage feedback as plain counts.
//!
//! Retrieval quality improves from use, with no model and no gradient:
//! every learning signal is an integer or a damped count in SQLite,
//! learned at three levels of the tree:
//!
//! - **Leaves — chunk reinforcement** (`rag_chunk_feedback`): explicit
//!   useful/not-useful feedback becomes a bounded log-ratio bonus on
//!   that chunk's fused score.
//! - **Branches — collection routing** (`rag_routing_stats`): feedback
//!   (strongly) and top hits of unscoped searches (weakly) teach which
//!   collections answer which terms. Unscoped queries then lean toward
//!   the collections that historically answered their terms.
//! - **Semantic layer** — useful feedback bridges the query's terms to
//!   the chunk's dominant terms in the EXISTING `rag_cooc` matrix, so
//!   the engine's own PPMI expansion learns vocabulary the corpus
//!   never used ("cats" judged useful against a "felines" chunk makes
//!   future "cats" queries expand to "felines"). One confirmation
//!   writes `EXPANSION_MIN_COOC` worth of evidence — a human judgment
//!   outweighs an accidental adjacency.
//!
//! Learning is bounded everywhere: reinforcement bonuses are clamped,
//! routing affinities fade multiplicatively past a cap (so old lessons
//! decay as new ones arrive), and learned co-occurrence bridges are
//! capped per pair. A full index rebuild (`rebuild_if_needed`) resets
//! the learned SEMANTIC bridges (they live in the rebuilt tables) but
//! preserves leaf and branch learning, which live in their own tables.

use std::collections::HashMap;

use ordo_models::{is_common_stopword, lexical_tokens};
use rusqlite::{params, Connection, Transaction};

/// Scale of the chunk-reinforcement bonus: `ln(1+useful) - ln(1+useless)`.
const FEEDBACK_WEIGHT: f64 = 1.2;
/// Reinforcement can never outweigh strong lexical evidence.
const FEEDBACK_MAX_BONUS: f64 = 2.5;
/// Ceiling on the routing bonus an unscoped query can receive.
const ROUTING_WEIGHT: f64 = 0.75;
/// Routing evidence from one explicit feedback event.
const EXPLICIT_ROUTING_INCREMENT: f64 = 1.0;
/// Routing evidence from the top hit of one unscoped search — an order
/// of magnitude weaker, because retrieval confirming itself is biased.
const IMPLICIT_ROUTING_INCREMENT: f64 = 0.1;
/// When a term's affinity for one collection exceeds this, ALL of that
/// term's affinities are halved: ratios (the routing signal) survive,
/// magnitudes fade, stale lessons wash out.
const AFFINITY_FADE_THRESHOLD: f64 = 64.0;
/// Evidence written per learned co-occurrence bridge. Matches the
/// expansion evidence floor so ONE explicit confirmation activates the
/// bridge.
const LEARNED_COOC_INCREMENT: i64 = 2;
/// Learned bridges saturate here — feedback cannot make a pair look
/// infinitely associated.
const LEARNED_COOC_CAP: i64 = 512;
/// At most this many chunk terms participate in bridge-building.
const BRIDGE_CHUNK_TERMS: usize = 8;
/// At most this many query terms participate in bridge-building.
const BRIDGE_QUERY_TERMS: usize = 4;

/// Content (non-stopword) terms of a query, in order, deduplicated.
pub(crate) fn content_terms(query: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    lexical_tokens(query)
        .into_iter()
        .filter(|token| !is_common_stopword(token))
        .filter(|token| seen.insert(token.clone()))
        .collect()
}

/// Record explicit feedback for one chunk inside `tx`. The caller has
/// already resolved `chunk_key` to an existing chunk.
pub(crate) fn record_feedback(
    tx: &Transaction<'_>,
    chunk_key: &str,
    collection: &str,
    query: &str,
    useful: bool,
) -> rusqlite::Result<()> {
    let (useful_delta, useless_delta) = if useful { (1.0, 0.0) } else { (0.0, 1.0) };
    tx.execute(
        "
        INSERT INTO rag_chunk_feedback (chunk_key, useful, useless) VALUES (?1, ?2, ?3)
        ON CONFLICT(chunk_key) DO UPDATE
            SET useful = useful + excluded.useful,
                useless = useless + excluded.useless
        ",
        params![chunk_key, useful_delta, useless_delta],
    )?;

    if !useful {
        // Negative feedback only counts against the chunk. Teaching
        // routing or semantics from a rejection would need to know WHY
        // it was rejected, which a count cannot carry.
        return Ok(());
    }

    let query_terms = content_terms(query);
    for term in &query_terms {
        reinforce_routing_term(tx, term, collection, EXPLICIT_ROUTING_INCREMENT)?;
    }
    build_semantic_bridges(tx, chunk_key, &query_terms)?;
    Ok(())
}

/// Weakly reinforce term->collection routing from the top hit of an
/// unscoped search. Runs on a plain connection (searches take `&self`)
/// under an unchecked transaction.
pub(crate) fn reinforce_routing_implicit(
    conn: &Connection,
    query_terms: &[String],
    collection: &str,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for term in query_terms {
        reinforce_routing_term(&tx, term, collection, IMPLICIT_ROUTING_INCREMENT)?;
    }
    tx.commit()
}

fn reinforce_routing_term(
    tx: &Transaction<'_>,
    term: &str,
    collection: &str,
    increment: f64,
) -> rusqlite::Result<()> {
    tx.execute(
        "
        INSERT INTO rag_routing_stats (term, collection_name, affinity) VALUES (?1, ?2, ?3)
        ON CONFLICT(term, collection_name) DO UPDATE
            SET affinity = affinity + excluded.affinity
        ",
        params![term, collection, increment],
    )?;
    let affinity: f64 = tx.query_row(
        "SELECT affinity FROM rag_routing_stats WHERE term = ?1 AND collection_name = ?2",
        params![term, collection],
        |row| row.get(0),
    )?;
    if affinity > AFFINITY_FADE_THRESHOLD {
        tx.execute(
            "UPDATE rag_routing_stats SET affinity = affinity * 0.5 WHERE term = ?1",
            params![term],
        )?;
    }
    Ok(())
}

/// Bridge the query's terms to the chunk's dominant terms in the
/// co-occurrence matrix, and make sure each query term exists in
/// `rag_terms` (df stays 0 — the term still matches nothing lexically —
/// but a nonzero cf gives PPMI a margin to expand through).
fn build_semantic_bridges(
    tx: &Transaction<'_>,
    chunk_key: &str,
    query_terms: &[String],
) -> rusqlite::Result<()> {
    let chunk_terms: Vec<String> = {
        let mut stmt = tx.prepare_cached(
            "SELECT term FROM rag_postings WHERE chunk_key = ?1 ORDER BY tf DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![chunk_key, BRIDGE_CHUNK_TERMS as i64], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<Result<_, _>>()?
    };
    if chunk_terms.is_empty() {
        return Ok(());
    }

    let mut ensure_term = tx.prepare_cached(
        "INSERT INTO rag_terms (term, df, cf) VALUES (?1, 0, 1) ON CONFLICT(term) DO NOTHING",
    )?;
    // The cap applies to LEARNED growth only: a pair whose organic
    // corpus count already exceeds the cap must keep its count, not be
    // clamped down to it.
    let mut bridge = tx.prepare_cached(
        "
        INSERT INTO rag_cooc (term_a, term_b, cooc_count) VALUES (?1, ?2, ?3)
        ON CONFLICT(term_a, term_b) DO UPDATE
            SET cooc_count = CASE
                WHEN cooc_count >= ?4 THEN cooc_count
                ELSE MIN(cooc_count + excluded.cooc_count, ?4)
            END
        ",
    )?;

    for query_term in query_terms.iter().take(BRIDGE_QUERY_TERMS) {
        ensure_term.execute(params![query_term])?;
        for chunk_term in &chunk_terms {
            if query_term == chunk_term {
                continue;
            }
            let (term_a, term_b) = if query_term < chunk_term {
                (query_term.as_str(), chunk_term.as_str())
            } else {
                (chunk_term.as_str(), query_term.as_str())
            };
            bridge.execute(params![
                term_a,
                term_b,
                LEARNED_COOC_INCREMENT,
                LEARNED_COOC_CAP
            ])?;
        }
    }
    Ok(())
}

/// All chunk reinforcement bonuses, keyed by chunk key. The table only
/// holds chunks that ever received feedback, so loading it whole is
/// cheap and serves both fusion paths with one query.
pub(crate) fn feedback_bonuses(conn: &Connection) -> rusqlite::Result<HashMap<String, f32>> {
    let mut stmt =
        conn.prepare_cached("SELECT chunk_key, useful, useless FROM rag_chunk_feedback")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, f64>(1)?,
            row.get::<_, f64>(2)?,
        ))
    })?;
    let mut bonuses = HashMap::new();
    for row in rows {
        let (chunk_key, useful, useless) = row?;
        let bonus = (FEEDBACK_WEIGHT * ((1.0 + useful).ln() - (1.0 + useless).ln()))
            .clamp(-FEEDBACK_MAX_BONUS, FEEDBACK_MAX_BONUS);
        if bonus != 0.0 {
            bonuses.insert(chunk_key, bonus as f32);
        }
    }
    Ok(bonuses)
}

/// Learned routing bonus per collection for this query: the mean, over
/// query terms with routing history, of the collection's share of that
/// term's affinity — scaled by `ROUTING_WEIGHT`. Empty map when
/// nothing has been learned about these terms yet.
pub(crate) fn routing_bonuses(
    conn: &Connection,
    query_terms: &[String],
) -> rusqlite::Result<HashMap<String, f32>> {
    let mut per_collection: HashMap<String, f64> = HashMap::new();
    let mut terms_with_history = 0usize;
    let mut stmt = conn.prepare_cached(
        "SELECT collection_name, affinity FROM rag_routing_stats WHERE term = ?1",
    )?;
    for term in query_terms {
        let rows = stmt.query_map(params![term], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        let rows: Vec<(String, f64)> = rows.collect::<Result<_, _>>()?;
        let total: f64 = rows.iter().map(|(_, affinity)| affinity).sum();
        if total <= 0.0 {
            continue;
        }
        terms_with_history += 1;
        for (collection, affinity) in rows {
            // Dividing by total + 1 instead of total folds evidence
            // saturation into the share: one stray 0.1 implicit
            // observation yields a ~0.09 share, not full confidence,
            // while well-evidenced affinities approach the true share.
            *per_collection.entry(collection).or_insert(0.0) += affinity / (total + 1.0);
        }
    }
    if terms_with_history == 0 {
        return Ok(HashMap::new());
    }
    Ok(per_collection
        .into_iter()
        .map(|(collection, share_sum)| {
            let bonus = ROUTING_WEIGHT * (share_sum / terms_with_history as f64);
            (collection, bonus as f32)
        })
        .collect())
}

/// Drop feedback rows whose chunks no longer exist. Called after a
/// full index rebuild; incremental deletes clean up inline.
pub(crate) fn prune_orphaned_feedback(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute(
        "
        DELETE FROM rag_chunk_feedback
        WHERE chunk_key NOT IN (SELECT chunk_key FROM rag_chunk_stats)
        ",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::content_terms;

    #[test]
    fn content_terms_drop_stopwords_and_duplicates() {
        let terms = content_terms("who is the transport relay for the transport");
        assert_eq!(terms, vec!["transport".to_string(), "relay".to_string()]);
    }
}
