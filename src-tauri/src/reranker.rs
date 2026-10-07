//! Multi-factor memory reranker — context-gatekeeper style two-stage retrieval.
//! Stage 1: Recall (vector + keyword) → Stage 2: Rerank (4-factor scoring)
//! Issue #39

/// Weighted rerank factors
pub struct RerankWeights {
    pub semantic: f64,   // vector similarity
    pub keyword: f64,    // FTS5 BM25
    pub recency: f64,    // time decay
    pub importance: f64, // priority + importance
}

impl Default for RerankWeights {
    fn default() -> Self {
        Self {
            semantic: 0.4,
            keyword: 0.3,
            recency: 0.2,
            importance: 0.1,
        }
    }
}

/// Priority weight mapping (context-gatekeeper design)
pub fn priority_weight(priority: &str) -> f64 {
    match priority {
        "anchored" => 1.0,
        "constraint" => 0.8,
        "decision" => 0.6,
        "preference" => 0.4,
        _ => 0.2, // fact or default
    }
}

/// A reranked memory result
#[derive(Debug, serde::Serialize)]
pub struct RerankedMemory {
    pub memory_id: String,
    pub content: String,
    pub category: String,
    pub priority: String,
    pub semantic_score: f64,
    pub keyword_score: f64,
    pub recency_score: f64,
    pub importance_score: f64,
    pub final_score: f64,
}

/// Rerank a list of memory IDs with multi-factor scoring.
/// Takes memory metadata + vector/keyword scores + time info.
pub fn rerank_memories(
    candidates: &[RerankCandidate],
    query_embedding: &[f64],
    weights: &RerankWeights,
    now: i64,
) -> Vec<RerankedMemory> {
    let max_keyword = candidates
        .iter()
        .map(|c| c.keyword_score)
        .fold(0.0f64, f64::max)
        .max(1.0);
    let mut results: Vec<RerankedMemory> = candidates
        .iter()
        .map(|c| {
            let keyword_norm = if max_keyword > 0.0 {
                c.keyword_score / max_keyword
            } else {
                0.0
            };
            let recency = time_recency_score(c.last_mentioned, now);
            let importance = priority_weight(&c.priority) * (c.importance as f64 / 10.0);
            let final_score = c.vector_score * weights.semantic
                + keyword_norm * weights.keyword
                + recency * weights.recency
                + importance * weights.importance;
            RerankedMemory {
                memory_id: c.memory_id.clone(),
                content: c.content.clone(),
                category: c.category.clone(),
                priority: c.priority.clone(),
                semantic_score: c.vector_score,
                keyword_score: keyword_norm,
                recency_score: recency,
                importance_score: importance,
                final_score,
            }
        })
        .collect();
    results.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

/// Candidate input for reranking
pub struct RerankCandidate {
    pub memory_id: String,
    pub content: String,
    pub category: String,
    pub priority: String,
    pub importance: i32,
    pub last_mentioned: Option<i64>,
    pub vector_score: f64,
    pub keyword_score: f64,
}

/// Time-based recency score (exponential decay, days)
fn time_recency_score(last_mentioned: Option<i64>, now: i64) -> f64 {
    last_mentioned
        .map(|ts| {
            let days = (now - ts) as f64 / 86400.0;
            (-0.1 * days).exp().min(1.0)
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_weights() {
        assert!(priority_weight("anchored") > priority_weight("constraint"));
        assert!(priority_weight("constraint") > priority_weight("decision"));
        assert!(priority_weight("fact") < priority_weight("preference"));
    }

    #[test]
    fn test_rerank_ordering() {
        let now = 1_000_000;
        let candidates = vec![
            RerankCandidate {
                memory_id: "1".into(),
                content: "a".into(),
                category: "fact".into(),
                priority: "anchored".into(),
                importance: 10,
                last_mentioned: Some(now),
                vector_score: 0.5,
                keyword_score: 0.0,
            },
            RerankCandidate {
                memory_id: "2".into(),
                content: "b".into(),
                category: "fact".into(),
                priority: "fact".into(),
                importance: 1,
                last_mentioned: Some(now - 86400 * 30),
                vector_score: 0.5,
                keyword_score: 0.0,
            },
        ];
        let results = rerank_memories(
            &candidates,
            &[1.0, 0.0, 0.0],
            &RerankWeights::default(),
            now,
        );
        assert!(results[0].memory_id == "1"); // anchored should rank higher
        assert!(results[0].final_score > results[1].final_score);
    }
}
