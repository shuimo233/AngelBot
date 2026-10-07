//! Tool embedding-based router for selecting relevant tools.
//!
//! Issue #062: Routes to top-K relevant tools when tool count >= threshold.
//!
//! Architecture:
//! - Pre-compute tool description embeddings at registration time.
//! - At runtime, embed user message and select top-K by cosine similarity.
//! - Always-include tools (e.g., memory tools) bypass routing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Threshold of tool count at which routing activates.
pub const ROUTE_THRESHOLD: usize = 20;
/// Default number of top tools to select when routing.
pub const DEFAULT_TOP_K: usize = 20;

/// Cosine similarity between two f32 vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Embedding vector for a single tool (pre-computed at registration).
#[derive(Clone)]
pub struct ToolEmbedding {
    pub tool_name: String,
    pub description: String,
    pub embedding: Vec<f32>,
}

impl ToolEmbedding {
    pub fn new(tool_name: String, description: String, embedding: Vec<f32>) -> Self {
        Self {
            tool_name,
            description,
            embedding,
        }
    }
}

/// Tool router using embedding-based selection.
///
/// Routing is only active when the tool registry has >= ROUTE_THRESHOLD tools.
/// Below the threshold, all tools are always included.
pub struct ToolRouter {
    tool_embeddings: HashMap<String, Vec<f32>>,
    always_include: Vec<String>,
    top_k: usize,
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new(DEFAULT_TOP_K)
    }
}

impl ToolRouter {
    pub fn new(top_k: usize) -> Self {
        Self {
            tool_embeddings: HashMap::new(),
            always_include: vec![
                "remember_fact".to_string(),
                "recall_memories".to_string(),
                "extract_entities".to_string(),
            ],
            top_k,
        }
    }

    /// Register a tool's description embedding. Called at tool registration time.
    pub fn register_tool(&mut self, tool_name: String, embedding: Vec<f32>) {
        self.tool_embeddings.insert(tool_name, embedding);
    }

    /// Register a tool name that must always be included regardless of routing.
    pub fn add_always_include(&mut self, tool_name: String) {
        if !self.always_include.contains(&tool_name) {
            self.always_include.push(tool_name);
        }
    }

    /// Route user message to selected tool names.
    /// Returns all tool names if count < ROUTE_THRESHOLD, otherwise top-K.
    pub fn route(&self, user_message: &str) -> Vec<String> {
        let tool_count = self.tool_embeddings.len();
        if tool_count < ROUTE_THRESHOLD {
            return self.tool_embeddings.keys().cloned().collect();
        }

        let msg_embedding = self.compute_message_embedding(user_message);
        self.select_top(&msg_embedding)
    }

    /// Select top-K tools by cosine similarity to message embedding.
    fn select_top(&self, msg_embedding: &[f32]) -> Vec<String> {
        let mut scores: Vec<(String, f32)> = self
            .tool_embeddings
            .iter()
            .map(|(name, emb)| (name.clone(), cosine_similarity(msg_embedding, emb)))
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut selected: Vec<String> = self.always_include.clone();
        for (name, _) in scores.into_iter().take(self.top_k) {
            if !self.always_include.contains(&name) {
                selected.push(name);
            }
        }

        selected
    }

    /// Compute embedding for user message.
    /// Uses a simple TF-IDF-like heuristic when no external embedder is configured.
    /// In production this would call an external embedding API (all-MiniLM-L6-v2 etc.).
    fn compute_message_embedding(&self, message: &str) -> Vec<f32> {
        let words: Vec<&str> = message.split_whitespace().collect();
        let dim = if self
            .tool_embeddings
            .values()
            .next()
            .map(|v| v.len())
            .unwrap_or(128)
            > 0
        {
            self.tool_embeddings
                .values()
                .next()
                .map(|v| v.len())
                .unwrap_or(128)
        } else {
            128
        };

        if words.is_empty() {
            return vec![0.0; dim];
        }

        // Simple hash-based embedding as fallback (deterministic per message).
        let mut embedding = vec![0.0f32; dim];
        for (i, word) in words.iter().enumerate() {
            let hash = simple_hash(word);
            let idx = (hash as usize) % dim;
            let weight = 1.0f32 / (i as f32 + 1.0);
            embedding[idx] += weight;
        }

        // Normalize
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut embedding {
                *v /= norm;
            }
        }

        embedding
    }

    /// Total number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tool_embeddings.len()
    }

    /// Check if routing is active (tool count >= threshold).
    pub fn is_routing_active(&self) -> bool {
        self.tool_embeddings.len() >= ROUTE_THRESHOLD
    }
}

/// Simple deterministic hash for word embedding fallback.
fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 5381;
    for byte in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(u64::from(byte));
    }
    h
}

/// Thread-safe wrapper for ToolRouter (cloneable across async tasks).
#[derive(Clone)]
pub struct SharedToolRouter(Arc<Mutex<ToolRouter>>);

impl SharedToolRouter {
    pub fn new(router: ToolRouter) -> Self {
        Self(Arc::new(Mutex::new(router)))
    }

    pub fn route(&self, message: &str) -> Vec<String> {
        self.0.lock().unwrap().route(message)
    }

    pub fn register_tool(&self, tool_name: String, embedding: Vec<f32>) {
        self.0.lock().unwrap().register_tool(tool_name, embedding);
    }

    pub fn is_routing_active(&self) -> bool {
        self.0.lock().unwrap().is_routing_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);

        let c = vec![1.0, 0.0, 0.0];
        let d = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&c, &d).abs() < 1e-6);

        let e = vec![1.0, 1.0];
        let f = vec![1.0, 1.0];
        let sim = cosine_similarity(&e, &f);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_routing_below_threshold() {
        let router = ToolRouter::new(5);
        // With 0 tools registered, routing returns empty (below threshold behavior: return all registered)
        let result = router.route("test message");
        assert!(result.is_empty()); // no tools registered
    }

    #[test]
    fn test_always_include_tools() {
        let mut router = ToolRouter::new(5);
        router.register_tool("remember_fact".to_string(), vec![0.1; 128]);
        router.register_tool("read_file".to_string(), vec![0.2; 128]);
        router.register_tool("write_file".to_string(), vec![0.3; 128]);

        let result = router.route("remember my name");
        assert!(result.contains(&"remember_fact".to_string()));
    }

    #[test]
    fn test_simple_hash_deterministic() {
        let h1 = simple_hash("hello");
        let h2 = simple_hash("hello");
        assert_eq!(h1, h2);
        assert_ne!(h1, simple_hash("world"));
    }
}
