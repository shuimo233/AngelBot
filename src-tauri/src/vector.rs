//! Vector similarity and embedding utilities
//!
//! Provides cosine similarity calculation and embedding management.
//! Falls back to time-based scoring when vector mode is disabled.

use std::collections::HashMap;

/// Calculate cosine similarity between two vectors
/// Returns value between -1.0 and 1.0
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let magnitude_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let magnitude_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();

    if magnitude_a == 0.0 || magnitude_b == 0.0 {
        return 0.0;
    }

    dot_product / (magnitude_a * magnitude_b)
}

/// Calculate memory activation score based on similarity threshold
/// Returns true if the similarity exceeds the threshold
pub fn is_relevant(similarity: f64, threshold: f64) -> bool {
    similarity >= threshold
}

/// Simple text embedding simulation using term frequency
/// In production, this would call an embedding API
pub fn text_to_embedding(text: &str, dimensions: usize) -> Vec<f64> {
    let lowercase_text = text.to_lowercase();
    let words: Vec<&str> = lowercase_text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();

    let mut freq: HashMap<&str, f64> = HashMap::new();
    for word in &words {
        *freq.entry(word).or_insert(0.0) += 1.0;
    }

    // Normalize by total word count
    let total = words.len() as f64;
    if total > 0.0 {
        for count in freq.values_mut() {
            *count /= total;
        }
    }

    // Simple hash-based embedding (for demo, not production quality)
    let mut embedding = vec![0.0; dimensions];
    for (word, freq) in freq {
        let hash = simple_hash(word);
        for i in 0..dimensions {
            // Distribute frequency across dimensions based on hash
            let idx = (hash + i * 31) % dimensions;
            let hash_val = hash.wrapping_mul((i + 1) as usize) as f64;
            embedding[idx] += freq * hash_val.cos();
        }
    }

    // Normalize embedding
    let magnitude: f64 = embedding.iter().map(|x| x * x).sum::<f64>().sqrt();
    if magnitude > 0.0 {
        for v in &mut embedding {
            *v /= magnitude;
        }
    }

    embedding
}

fn simple_hash(s: &str) -> usize {
    s.bytes().fold(0usize, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(b as usize)
    })
}

/// Calculate relevance scores between a query and multiple memories
/// Returns a map of memory_id -> similarity_score
pub fn batch_similarity_scores(
    query_embedding: &[f64],
    memory_embeddings: &[(String, Vec<f64>)],
) -> HashMap<String, f64> {
    let mut scores = HashMap::new();

    for (id, embedding) in memory_embeddings {
        let score = cosine_similarity(query_embedding, embedding);
        scores.insert(id.clone(), score);
    }

    scores
}

/// Time-based fallback scoring when vectors are disabled
/// Combines recency, frequency, and importance
pub fn time_based_score(
    frequency: i32,
    last_mentioned: Option<i64>,
    importance: i32,
    now: i64,
) -> f64 {
    let recency_weight = 0.4;
    let frequency_weight = 0.4;
    let importance_weight = 0.2;

    // Recency score (exponential decay based on days since last mention)
    let recency_score = if let Some(last_ts) = last_mentioned {
        let days_elapsed = (now - last_ts) as f64 / 86400.0; // seconds per day
        (-0.1 * days_elapsed).exp().min(1.0)
    } else {
        0.0
    };

    // Normalize frequency (log scale)
    let frequency_score = if frequency > 0 {
        (frequency as f64).ln().min(10.0) / 10.0
    } else {
        0.0
    };

    // Normalize importance
    let importance_score = (importance as f64 / 10.0).min(1.0);

    recency_score * recency_weight
        + frequency_score * frequency_weight
        + importance_score * importance_weight
}

// ─── Embedding Cache (Issue #30) ──────────────────────────────────────────────

/// Look up an embedding in the cache by content hash.
/// Returns (embedding_vec, dimension) if found and not expired.
pub fn cache_lookup(
    conn: &rusqlite::Connection,
    content_hash: &str,
    max_age_days: i64,
) -> Option<(Vec<f32>, usize)> {
    use rusqlite::params;
    let cutoff = chrono::Utc::now().timestamp() - max_age_days * 86400;
    conn.query_row(
        "SELECT embedding, dimension FROM embedding_cache WHERE content_hash = ?1 AND last_accessed_at >= ?2",
        params![content_hash, cutoff],
        |r| {
            let blob: Vec<u8> = r.get(0)?;
            let dim: usize = r.get::<_, i32>(1)? as usize;
            let embedding: Vec<f32> = blob
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            Ok((embedding, dim))
        },
    ).ok()
}

/// Store an embedding in the cache.
pub fn cache_store(conn: &rusqlite::Connection, content_hash: &str, embedding: &[f32]) {
    use rusqlite::params;
    let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    let now = chrono::Utc::now().timestamp();
    let _ = conn.execute(
        "INSERT OR REPLACE INTO embedding_cache (content_hash, embedding, dimension, created_at, last_accessed_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![content_hash, blob, embedding.len() as i32, now, now],
    );
}

/// Clean up stale embedding cache entries (older than max_age_days).
pub fn cache_cleanup(conn: &rusqlite::Connection, max_age_days: i64) -> Result<usize, String> {
    let cutoff = chrono::Utc::now().timestamp() - max_age_days * 86400;
    conn.execute(
        "DELETE FROM embedding_cache WHERE last_accessed_at < ?1",
        rusqlite::params![cutoff],
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 0.0001);
    }

    #[test]
    fn test_is_relevant() {
        assert!(is_relevant(0.8, 0.7));
        assert!(!is_relevant(0.5, 0.7));
        assert!(is_relevant(0.7, 0.7)); // threshold is inclusive
    }

    #[test]
    fn test_text_to_embedding() {
        let text = "hello world hello";
        let emb = text_to_embedding(text, 128);
        assert_eq!(emb.len(), 128);

        // Check normalization
        let magnitude: f64 = emb.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((magnitude - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_batch_similarity() {
        let query = vec![1.0, 0.0, 0.0];
        let memories = vec![
            ("mem1".to_string(), vec![1.0, 0.0, 0.0]), // identical
            ("mem2".to_string(), vec![0.0, 1.0, 0.0]), // orthogonal
            ("mem3".to_string(), vec![0.5, 0.5, 0.0]), // partial
        ];

        let scores = batch_similarity_scores(&query, &memories);

        assert!((scores["mem1"] - 1.0).abs() < 0.0001);
        assert!((scores["mem2"] - 0.0).abs() < 0.0001);
        assert!(scores["mem3"] > 0.3 && scores["mem3"] < 0.8);
    }

    #[test]
    fn test_time_based_score() {
        let now = 1000000;

        // Recent memory with high frequency
        let score1 = time_based_score(10, Some(now - 86400), 8, now);
        assert!(score1 > 0.5);

        // Old memory with low frequency
        let score2 = time_based_score(1, Some(now - 86400 * 30), 2, now);
        assert!(score2 < 0.3);
    }

    #[test]
    fn test_text_embedding_consistency() {
        // Same text should produce same embedding
        let text = "hello world";
        let emb1 = text_to_embedding(text, 128);
        let emb2 = text_to_embedding(text, 128);

        let diff: f64 = emb1
            .iter()
            .zip(emb2.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(diff < 0.0001);
    }

    #[test]
    fn test_similar_texts_produce_similar_embeddings() {
        let emb1 = text_to_embedding("I love programming in Python", 128);
        let emb2 = text_to_embedding("I enjoy coding with Python", 128);

        let similarity = cosine_similarity(&emb1, &emb2);
        // Similar texts should have high similarity
        assert!(similarity > 0.3);
    }
}
