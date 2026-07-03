//! Generative-free retrieval engine — the default search path.
//!
//! Text similarity is decomposed into exact sparse primitives instead of
//! being approximated by a dense hash embedding:
//!
//! - **BM25** over an inverted index (`rag_postings` + `rag_terms` +
//!   `rag_chunk_stats`): term counts, document frequencies, and length
//!   normalization. Scoring touches only chunks that share a term with
//!   the query, never the whole corpus.
//! - **PPMI query expansion** over a term co-occurrence matrix
//!   (`rag_cooc`): terms that keep company in this corpus become
//!   statistically associated, so a query term pulls in its strongest
//!   neighbors at damped weight. Semantics from counting and one
//!   logarithm — no model.
//! - **Fuzzy vocabulary matching** via padded character-bigram Jaccard:
//!   a misspelled or reshaped query token maps onto the closest indexed
//!   terms by set intersection.
//!
//! Every table cell is a plain count. Incremental add/remove is exact
//! integer arithmetic (a chunk's features are recomputed from its stored
//! text and subtracted), and the whole index rebuilds from
//! `rag_chunks.text` when it is missing or drifted.
//!
//! These functions are mechanism only. Fusion policy — how BM25 blends
//! with phrase bonuses, optional model embeddings, and collection
//! boosts — lives with the caller in `lib.rs`.

use std::collections::{HashMap, HashSet};

use ordo_models::{is_common_stopword, lexical_tokens, lexical_variants};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};

/// Bump when the tokenizer, stopword list, or feature extraction
/// changes: incremental decrements are only exact when the features
/// subtracted match the features that were added, so a stamp mismatch
/// forces a full rebuild from chunk text on the next open.
const INDEX_VERSION: i64 = 1;
/// Weight of a stemmed variant relative to the surface token, applied
/// symmetrically at index and query time.
const VARIANT_WEIGHT: f64 = 0.4;
/// Sliding-window radius (in content tokens) for co-occurrence counting.
const COOC_WINDOW: usize = 4;
/// BM25 term-frequency saturation.
const BM25_K1: f64 = 1.4;
/// BM25 length-normalization strength.
const BM25_B: f64 = 0.75;
/// How many co-occurrence neighbors a query term may recruit.
const EXPANSION_NEIGHBORS: usize = 3;
/// Ceiling on the query weight an expansion neighbor can receive.
const EXPANSION_WEIGHT: f64 = 0.3;
/// A co-occurrence pair needs at least this much evidence before it
/// can recruit a neighbor — one accidental adjacency is noise.
const EXPANSION_MIN_COOC: i64 = 2;
/// How many raw co-occurrence rows (by descending count) are even
/// considered per query term; bounds expansion work on hub terms.
const EXPANSION_CANDIDATES: usize = 64;
/// Fuzzy matching only applies to tokens at least this long — shorter
/// tokens produce too few bigrams to discriminate.
const FUZZY_MIN_TOKEN_LEN: usize = 4;
/// Minimum padded-bigram Jaccard similarity to accept a fuzzy match.
const FUZZY_MIN_JACCARD: f64 = 0.5;
/// Query weight scale for fuzzy-matched terms.
const FUZZY_WEIGHT: f64 = 0.8;
/// At most this many fuzzy candidates per unknown token.
const FUZZY_MAX_MATCHES: usize = 2;
/// At most this many unknown tokens get the fuzzy treatment per query.
const FUZZY_MAX_UNKNOWN_TOKENS: usize = 4;

/// Stable identity of one chunk across all index tables. Unit separator
/// keeps document ids containing '#' or ':' unambiguous.
pub(crate) fn chunk_key(document_id: &str, chunk_index: usize) -> String {
    format!("{document_id}\u{1f}{chunk_index}")
}

/// FNV-1a fingerprint of chunk text, carried on `RagHit` and echoed on
/// feedback so learning can verify the caller saw THIS text and not a
/// replacement that landed at the same chunk index since.
pub(crate) fn content_hash(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Everything the index stores about one chunk, recomputable from its
/// text alone — which is what makes exact decrement-on-delete possible.
struct ChunkFeatures {
    /// term -> weighted tf (surface tokens at 1.0, variants at
    /// `VARIANT_WEIGHT`); these become posting-list rows and df counts.
    weighted_terms: HashMap<String, f64>,
    /// term -> surface occurrence count; these become cf counts, the
    /// margins of the co-occurrence matrix.
    surface_counts: HashMap<String, i64>,
    /// canonical (a < b) term pair -> co-occurrence count within
    /// `COOC_WINDOW`.
    cooc: HashMap<(String, String), i64>,
    /// number of content (non-stopword) tokens; BM25 chunk length.
    content_token_count: i64,
}

fn extract_features(text: &str) -> ChunkFeatures {
    let mut weighted_terms: HashMap<String, f64> = HashMap::new();
    let mut surface_counts: HashMap<String, i64> = HashMap::new();
    let mut content_tokens: Vec<String> = Vec::new();

    for token in lexical_tokens(text) {
        if is_common_stopword(&token) {
            continue;
        }
        *weighted_terms.entry(token.clone()).or_insert(0.0) += 1.0;
        for variant in lexical_variants(&token) {
            *weighted_terms.entry(variant).or_insert(0.0) += VARIANT_WEIGHT;
        }
        *surface_counts.entry(token.clone()).or_insert(0) += 1;
        content_tokens.push(token);
    }

    let mut cooc: HashMap<(String, String), i64> = HashMap::new();
    for (position, left) in content_tokens.iter().enumerate() {
        let window_end = (position + 1 + COOC_WINDOW).min(content_tokens.len());
        for right in &content_tokens[position + 1..window_end] {
            if left == right {
                continue;
            }
            let pair = if left < right {
                (left.clone(), right.clone())
            } else {
                (right.clone(), left.clone())
            };
            *cooc.entry(pair).or_insert(0) += 1;
        }
    }

    ChunkFeatures {
        weighted_terms,
        surface_counts,
        cooc,
        content_token_count: content_tokens.len() as i64,
    }
}

/// Add one chunk to the index. Must run in the same transaction as the
/// `rag_chunks` insert so the two can never disagree.
pub(crate) fn index_chunk(
    tx: &Transaction<'_>,
    document_id: &str,
    chunk_index: usize,
    collection: &str,
    text: &str,
) -> rusqlite::Result<()> {
    let key = chunk_key(document_id, chunk_index);
    let features = extract_features(text);

    tx.execute(
        "
        INSERT OR REPLACE INTO rag_chunk_stats
            (chunk_key, document_id, chunk_index, collection_name, token_count)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ",
        params![
            &key,
            document_id,
            chunk_index as i64,
            collection,
            features.content_token_count,
        ],
    )?;

    {
        let mut posting = tx.prepare_cached(
            "INSERT OR REPLACE INTO rag_postings (term, chunk_key, tf) VALUES (?1, ?2, ?3)",
        )?;
        let mut term_stat = tx.prepare_cached(
            "
            INSERT INTO rag_terms (term, df, cf) VALUES (?1, 1, ?2)
            ON CONFLICT(term) DO UPDATE SET df = df + 1, cf = cf + excluded.cf
            ",
        )?;
        for (term, tf) in &features.weighted_terms {
            posting.execute(params![term, &key, tf])?;
            let surface = features.surface_counts.get(term).copied().unwrap_or(0);
            term_stat.execute(params![term, surface])?;
        }
    }

    {
        let mut cooc = tx.prepare_cached(
            "
            INSERT INTO rag_cooc (term_a, term_b, cooc_count) VALUES (?1, ?2, ?3)
            ON CONFLICT(term_a, term_b) DO UPDATE
                SET cooc_count = cooc_count + excluded.cooc_count
            ",
        )?;
        for ((term_a, term_b), count) in &features.cooc {
            cooc.execute(params![term_a, term_b, count])?;
        }
    }

    tx.execute(
        "
        INSERT INTO rag_corpus_stats (stat_key, stat_value)
        VALUES ('total_content_tokens', ?1)
        ON CONFLICT(stat_key) DO UPDATE SET stat_value = stat_value + excluded.stat_value
        ",
        params![features.content_token_count],
    )?;

    Ok(())
}

/// Remove one chunk from the index by recomputing its features from the
/// stored text and subtracting them. Must run in the same transaction
/// as the `rag_chunks` delete, before the row disappears.
pub(crate) fn deindex_chunk(
    tx: &Transaction<'_>,
    document_id: &str,
    chunk_index: usize,
    text: &str,
) -> rusqlite::Result<()> {
    let key = chunk_key(document_id, chunk_index);
    let features = extract_features(text);

    tx.execute(
        "DELETE FROM rag_postings WHERE chunk_key = ?1",
        params![&key],
    )?;
    tx.execute(
        "DELETE FROM rag_chunk_stats WHERE chunk_key = ?1",
        params![&key],
    )?;
    // Learned reinforcement follows its chunk out.
    tx.execute(
        "DELETE FROM rag_chunk_feedback WHERE chunk_key = ?1",
        params![&key],
    )?;

    {
        let mut term_stat = tx.prepare_cached(
            "UPDATE rag_terms SET df = df - 1, cf = cf - ?2 WHERE term = ?1",
        )?;
        // Point-delete only rows this chunk touched — a corpus-wide
        // `DELETE WHERE df <= 0` would scan the whole table once per
        // removed chunk.
        let mut term_gc = tx.prepare_cached(
            "DELETE FROM rag_terms WHERE term = ?1 AND df <= 0 AND cf <= 0",
        )?;
        for term in features.weighted_terms.keys() {
            let surface = features.surface_counts.get(term).copied().unwrap_or(0);
            term_stat.execute(params![term, surface])?;
            term_gc.execute(params![term])?;
        }
    }

    {
        let mut cooc = tx.prepare_cached(
            "UPDATE rag_cooc SET cooc_count = cooc_count - ?3
             WHERE term_a = ?1 AND term_b = ?2",
        )?;
        let mut cooc_gc = tx.prepare_cached(
            "DELETE FROM rag_cooc WHERE term_a = ?1 AND term_b = ?2 AND cooc_count <= 0",
        )?;
        for ((term_a, term_b), count) in &features.cooc {
            cooc.execute(params![term_a, term_b, count])?;
            cooc_gc.execute(params![term_a, term_b])?;
        }
    }

    tx.execute(
        "
        UPDATE rag_corpus_stats SET stat_value = MAX(0, stat_value - ?1)
        WHERE stat_key = 'total_content_tokens'
        ",
        params![features.content_token_count],
    )?;

    Ok(())
}

/// Rebuild the index from `rag_chunks.text` when it is absent (a
/// database created before the index existed), has drifted (row counts
/// disagree), or was written by a different tokenizer generation
/// (version stamp mismatch). Safe to call on every open — it is a
/// no-op when the index is consistent.
pub(crate) fn rebuild_if_needed(conn: &mut Connection) -> rusqlite::Result<()> {
    let chunk_rows: i64 = conn.query_row("SELECT COUNT(*) FROM rag_chunks", [], |row| row.get(0))?;
    let stat_rows: i64 =
        conn.query_row("SELECT COUNT(*) FROM rag_chunk_stats", [], |row| row.get(0))?;
    let stamped_version: Option<i64> = conn
        .query_row(
            "SELECT stat_value FROM rag_corpus_stats WHERE stat_key = 'index_version'",
            [],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    if chunk_rows == stat_rows && stamped_version == Some(INDEX_VERSION) {
        return Ok(());
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM rag_postings", [])?;
    tx.execute("DELETE FROM rag_terms", [])?;
    tx.execute("DELETE FROM rag_chunk_stats", [])?;
    tx.execute("DELETE FROM rag_cooc", [])?;
    tx.execute("DELETE FROM rag_corpus_stats", [])?;

    let chunks: Vec<(String, i64, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT document_id, chunk_index, collection_name, text FROM rag_chunks",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<Result<_, _>>()?
    };

    for (document_id, chunk_index, collection, text) in chunks {
        index_chunk(&tx, &document_id, chunk_index as usize, &collection, &text)?;
    }
    crate::learn::prune_orphaned_feedback(&tx)?;
    tx.execute(
        "
        INSERT INTO rag_corpus_stats (stat_key, stat_value)
        VALUES ('index_version', ?1)
        ON CONFLICT(stat_key) DO UPDATE SET stat_value = excluded.stat_value
        ",
        params![INDEX_VERSION],
    )?;
    tx.commit()
}

/// One BM25-scored chunk. `document_id` is the storage id
/// (collection-prefixed); hydration back into `RagHit` happens in the
/// caller.
#[derive(Debug)]
pub(crate) struct ScoredChunk {
    pub document_id: String,
    pub chunk_index: usize,
    pub collection: String,
    pub score: f64,
}


/// Score the corpus against `query`: expand the query (variants, fuzzy
/// vocabulary matches, PPMI co-occurrence neighbors), then run BM25
/// over the posting lists of the expanded term set. Returns chunks in
/// descending score order, already filtered to `allowed_collections`
/// (empty slice = all collections).
///
/// BM25 statistics (chunk count, average length, document frequency)
/// are computed WITHIN the requested collection scope, so chunks in
/// non-requested collections — including retired ones still present in
/// old databases — cannot distort ranking. Expansion and fuzzy
/// matching stay corpus-global on purpose: co-occurrence is corpus
/// knowledge, and a scoped df of zero already silences any recruited
/// term that is absent from the scope.
pub(crate) fn score_query(
    conn: &Connection,
    query: &str,
    allowed_collections: &[String],
) -> rusqlite::Result<Vec<ScoredChunk>> {
    let (chunk_count, avg_len) = scoped_corpus_stats(conn, allowed_collections)?;
    if chunk_count == 0 {
        return Ok(Vec::new());
    }
    let total_tokens = global_total_tokens(conn)?;

    // Base query terms: surface tokens at full weight, variants damped.
    let mut weights: HashMap<String, f64> = HashMap::new();
    let mut surface_tokens: Vec<String> = Vec::new();
    for token in lexical_tokens(query) {
        if is_common_stopword(&token) {
            continue;
        }
        raise_weight(&mut weights, &token, 1.0);
        for variant in lexical_variants(&token) {
            raise_weight(&mut weights, &variant, VARIANT_WEIGHT);
        }
        surface_tokens.push(token);
    }
    if weights.is_empty() {
        return Ok(Vec::new());
    }

    let base_stats = term_stats(conn, weights.keys())?;

    // Fuzzy: map unknown surface tokens onto the closest vocabulary
    // terms by padded-bigram Jaccard.
    let unknown: Vec<&String> = surface_tokens
        .iter()
        .filter(|token| token.chars().count() >= FUZZY_MIN_TOKEN_LEN)
        .filter(|token| !base_stats.contains_key(token.as_str()))
        .take(FUZZY_MAX_UNKNOWN_TOKENS)
        .collect();
    for token in unknown {
        let vocabulary = load_vocabulary_band(conn, token)?;
        for (term, similarity) in fuzzy_matches(token, &vocabulary) {
            raise_weight(&mut weights, &term, FUZZY_WEIGHT * similarity);
        }
    }

    // Semantic expansion: each known query term recruits its strongest
    // co-occurrence neighbors. PPMI is damped absolutely
    // (`p / (p + 1)`), not against the batch maximum, so a term's top
    // neighbor earns high weight only when the association itself is
    // strong — a weak batch can no longer promote its least-weak
    // member to full expansion weight.
    for token in &surface_tokens {
        let Some(&(_, cf)) = base_stats.get(token.as_str()) else {
            continue;
        };
        if cf <= 0 {
            continue;
        }
        for (neighbor, ppmi) in expansion_neighbors(conn, token, cf, total_tokens)? {
            let weight = EXPANSION_WEIGHT * (ppmi / (ppmi + 1.0));
            raise_weight(&mut weights, &neighbor, weight);
        }
    }

    // BM25 over the expanded term set. Posting rows are fetched once
    // per term and filtered to the collection scope; the surviving row
    // count IS the scoped document frequency.
    let mut scores: HashMap<String, ScoredChunk> = HashMap::new();
    let mut postings = conn.prepare_cached(
        "
        SELECT p.chunk_key, p.tf, s.document_id, s.chunk_index,
               s.collection_name, s.token_count
        FROM rag_postings p
        JOIN rag_chunk_stats s ON s.chunk_key = p.chunk_key
        WHERE p.term = ?1
        ",
    )?;
    for (term, query_weight) in &weights {
        let rows = postings.query_map(params![term], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        let scoped: Vec<(String, f64, String, i64, String, i64)> = rows
            .filter_map(|row| match row {
                Ok(row) => {
                    if allowed_collections.is_empty() || allowed_collections.contains(&row.4) {
                        Some(Ok(row))
                    } else {
                        None
                    }
                }
                Err(err) => Some(Err(err)),
            })
            .collect::<Result<_, _>>()?;
        let scoped_df = scoped.len() as f64;
        if scoped_df <= 0.0 {
            continue;
        }
        let idf =
            (1.0 + ((chunk_count as f64 - scoped_df + 0.5) / (scoped_df + 0.5)).max(0.0)).ln();
        for (key, tf, document_id, chunk_index, collection, token_count) in scoped {
            let length_norm = 1.0 - BM25_B + BM25_B * (token_count as f64 / avg_len.max(1.0));
            let contribution =
                query_weight * idf * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * length_norm);
            scores
                .entry(key)
                .or_insert_with(|| ScoredChunk {
                    document_id,
                    chunk_index: chunk_index as usize,
                    collection,
                    score: 0.0,
                })
                .score += contribution;
        }
    }

    let mut scored: Vec<ScoredChunk> = scores
        .into_values()
        .filter(|chunk| chunk.score > 0.0)
        .collect();
    scored.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.document_id.cmp(&right.document_id))
            .then_with(|| left.chunk_index.cmp(&right.chunk_index))
    });
    Ok(scored)
}

fn raise_weight(weights: &mut HashMap<String, f64>, term: &str, weight: f64) {
    let entry = weights.entry(term.to_string()).or_insert(0.0);
    if weight > *entry {
        *entry = weight;
    }
}

/// Chunk count and average length WITHIN the collection scope (empty
/// scope = whole corpus), so BM25 normalization reflects only the
/// chunks a query can actually return.
fn scoped_corpus_stats(
    conn: &Connection,
    allowed_collections: &[String],
) -> rusqlite::Result<(i64, f64)> {
    let mut stmt = conn.prepare_cached(
        "
        SELECT collection_name, COUNT(*), COALESCE(SUM(token_count), 0)
        FROM rag_chunk_stats
        GROUP BY collection_name
        ",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut chunk_count = 0i64;
    let mut token_sum = 0i64;
    for row in rows {
        let (collection, count, tokens) = row?;
        if allowed_collections.is_empty() || allowed_collections.contains(&collection) {
            chunk_count += count;
            token_sum += tokens;
        }
    }
    let avg_len = if chunk_count > 0 {
        token_sum as f64 / chunk_count as f64
    } else {
        0.0
    };
    Ok((chunk_count, avg_len))
}

fn global_total_tokens(conn: &Connection) -> rusqlite::Result<i64> {
    Ok(conn
        .query_row(
            "SELECT stat_value FROM rag_corpus_stats WHERE stat_key = 'total_content_tokens'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0))
}

fn term_stats<'a>(
    conn: &Connection,
    terms: impl Iterator<Item = &'a String>,
) -> rusqlite::Result<HashMap<String, (i64, i64)>> {
    let mut stmt = conn.prepare_cached("SELECT df, cf FROM rag_terms WHERE term = ?1")?;
    let mut stats = HashMap::new();
    for term in terms {
        let row = stmt
            .query_row(params![term], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        if let Some((df, cf)) = row {
            stats.insert(term.clone(), (df, cf));
        }
    }
    Ok(stats)
}

/// Vocabulary terms whose length could plausibly clear
/// `FUZZY_MIN_JACCARD` against `token`, keeping a large vocabulary
/// from being fully scanned per unknown token.
///
/// The LOWER bound is sound: Jaccard >= 0.5 forces the two padded-
/// bigram SET sizes within a factor of two, a term's set size is at
/// most its length + 1, so any true match has
/// `LENGTH(term) >= ceil(|bigrams(token)| / 2) - 1` (note: the
/// token's distinct-bigram count, not its length — repetitive strings
/// like "abab" have small sets). The UPPER bound is a heuristic:
/// length does not bound set size from below, so a highly repetitive
/// vocabulary term longer than twice the token could in principle
/// still match and is sacrificed here — irrelevant for natural
/// language and identifiers, which is what the vocabulary holds.
fn load_vocabulary_band(conn: &Connection, token: &str) -> rusqlite::Result<Vec<String>> {
    let token_len = token.chars().count();
    let bigram_count = padded_bigrams(token).len();
    let low = (bigram_count.div_ceil(2).saturating_sub(1)).max(FUZZY_MIN_TOKEN_LEN) as i64;
    let high = (2 * token_len + 1) as i64;
    let mut stmt = conn.prepare_cached(
        "SELECT term FROM rag_terms WHERE df > 0 AND LENGTH(term) BETWEEN ?1 AND ?2",
    )?;
    let rows = stmt.query_map(params![low, high], |row| row.get::<_, String>(0))?;
    rows.collect()
}

/// Best fuzzy matches for `token` among `vocabulary`, as
/// `(term, jaccard)` pairs in descending similarity.
fn fuzzy_matches(token: &str, vocabulary: &[String]) -> Vec<(String, f64)> {
    let token_grams = padded_bigrams(token);
    if token_grams.is_empty() {
        return Vec::new();
    }
    let mut matches: Vec<(String, f64)> = Vec::new();
    for term in vocabulary {
        if term.chars().count() < FUZZY_MIN_TOKEN_LEN {
            continue;
        }
        let term_grams = padded_bigrams(term);
        let intersection = token_grams.intersection(&term_grams).count();
        if intersection == 0 {
            continue;
        }
        let union = token_grams.len() + term_grams.len() - intersection;
        let jaccard = intersection as f64 / union as f64;
        if jaccard >= FUZZY_MIN_JACCARD {
            matches.push((term.clone(), jaccard));
        }
    }
    matches.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    matches.truncate(FUZZY_MAX_MATCHES);
    matches
}

/// Character bigrams of `^token$` — the padding makes word boundaries
/// count toward similarity, which sharpens short-word discrimination.
fn padded_bigrams(token: &str) -> HashSet<(char, char)> {
    let padded: Vec<char> = std::iter::once('^')
        .chain(token.chars())
        .chain(std::iter::once('$'))
        .collect();
    padded.windows(2).map(|pair| (pair[0], pair[1])).collect()
}

/// Top co-occurrence neighbors of `token` by PPMI, as
/// `(neighbor, ppmi)` pairs.
///
/// PMI uses corpus token counts as margins:
/// `ln(cooc * total_tokens / (cf(a) * cf(b)))`, clamped at zero.
/// Candidates are pre-limited in SQL to the `EXPANSION_CANDIDATES`
/// highest co-occurrence counts (with an `EXPANSION_MIN_COOC` evidence
/// floor) so hub terms with tens of thousands of co-occurrence rows
/// cost a bounded amount of work per query.
fn expansion_neighbors(
    conn: &Connection,
    token: &str,
    token_cf: i64,
    total_tokens: i64,
) -> rusqlite::Result<Vec<(String, f64)>> {
    if total_tokens <= 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "
        SELECT other, cooc_count FROM (
            SELECT term_b AS other, cooc_count FROM rag_cooc WHERE term_a = ?1
            UNION ALL
            SELECT term_a AS other, cooc_count FROM rag_cooc WHERE term_b = ?1
        )
        WHERE cooc_count >= ?2
        ORDER BY cooc_count DESC
        LIMIT ?3
        ",
    )?;
    let rows = stmt.query_map(
        params![token, EXPANSION_MIN_COOC, EXPANSION_CANDIDATES as i64],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let pairs: Vec<(String, i64)> = rows.collect::<Result<_, _>>()?;
    if pairs.is_empty() {
        return Ok(Vec::new());
    }

    let neighbor_names: Vec<String> = pairs.iter().map(|(name, _)| name.clone()).collect();
    let neighbor_stats = term_stats(conn, neighbor_names.iter())?;

    let mut ranked: Vec<(String, f64)> = Vec::new();
    for (neighbor, cooc_count) in pairs {
        let Some(&(_, neighbor_cf)) = neighbor_stats.get(neighbor.as_str()) else {
            continue;
        };
        if neighbor_cf <= 0 {
            continue;
        }
        let pmi = ((cooc_count as f64 * total_tokens as f64)
            / (token_cf as f64 * neighbor_cf as f64))
            .ln();
        if pmi > 0.0 {
            ranked.push((neighbor, pmi));
        }
    }
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked.truncate(EXPANSION_NEIGHBORS);
    Ok(ranked)
}

/// Exact-substring fallback for queries the term index cannot serve —
/// most notably queries made entirely of stopwords ("who are you"),
/// which the postings skip by design. Scans chunk text with `instr`
/// (no LIKE escaping needed); only runs when BM25 found nothing, so
/// the corpus scan is a rare path, and it is bounded by `limit`.
pub(crate) fn phrase_fallback(
    conn: &Connection,
    lowered_query: &str,
    allowed_collections: &[String],
    limit: usize,
) -> rusqlite::Result<Vec<ScoredChunk>> {
    if lowered_query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "
        SELECT document_id, chunk_index, collection_name FROM rag_chunks
        WHERE INSTR(LOWER(text), ?1) > 0
        ",
    )?;
    let rows = stmt.query_map(params![lowered_query], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut matches = Vec::new();
    // Cap per collection when a scope is requested (mirroring
    // pool_candidates): with one global cap, a collection whose rows
    // happen to sit earlier in the table would exhaust the budget
    // before another requested collection is ever reached, starving
    // balance_rag_hits.
    let mut per_collection: HashMap<String, usize> = HashMap::new();
    let total_cap = limit * allowed_collections.len().max(1);
    for row in rows {
        let (document_id, chunk_index, collection) = row?;
        if !allowed_collections.is_empty() && !allowed_collections.contains(&collection) {
            continue;
        }
        if !allowed_collections.is_empty() {
            let count = per_collection.entry(collection.clone()).or_insert(0);
            if *count >= limit {
                continue;
            }
            *count += 1;
        }
        matches.push(ScoredChunk {
            document_id,
            chunk_index: chunk_index as usize,
            collection,
            // The fusion layer adds the exact-phrase bonus during
            // hydration; the fallback itself carries no BM25 signal.
            score: 0.0,
        });
        if matches.len() >= total_cap {
            break;
        }
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::{extract_features, fuzzy_matches, padded_bigrams};

    #[test]
    fn features_count_surface_terms_and_cooccurrence() {
        let features = extract_features("transport relay transport");
        assert_eq!(features.content_token_count, 3);
        assert_eq!(features.surface_counts.get("transport"), Some(&2));
        assert_eq!(features.surface_counts.get("relay"), Some(&1));
        assert_eq!(
            features
                .cooc
                .get(&("relay".to_string(), "transport".to_string())),
            Some(&2),
            "relay pairs with both transport occurrences inside the window"
        );
        assert!(features.weighted_terms.get("transport").copied() >= Some(2.0));
    }

    #[test]
    fn features_skip_stopwords_entirely() {
        let features = extract_features("the transport of the relay");
        assert_eq!(features.content_token_count, 2);
        assert!(!features.weighted_terms.contains_key("the"));
        assert!(!features.weighted_terms.contains_key("of"));
    }

    #[test]
    fn fuzzy_matching_survives_a_transposition() {
        let vocabulary = vec![
            "transport".to_string(),
            "adapter".to_string(),
            "calendar".to_string(),
        ];
        let matches = fuzzy_matches("transprot", &vocabulary);
        assert_eq!(
            matches.first().map(|(term, _)| term.as_str()),
            Some("transport")
        );
    }

    #[test]
    fn fuzzy_matching_rejects_unrelated_words() {
        let vocabulary = vec!["calendar".to_string(), "invoice".to_string()];
        assert!(fuzzy_matches("transport", &vocabulary).is_empty());
    }

    #[test]
    fn padded_bigrams_include_word_boundaries() {
        let grams = padded_bigrams("ab");
        assert!(grams.contains(&('^', 'a')));
        assert!(grams.contains(&('a', 'b')));
        assert!(grams.contains(&('b', '$')));
    }
}
