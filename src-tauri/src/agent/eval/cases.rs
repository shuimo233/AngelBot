//! Evaluation cases — Issue #058
//!
//! Defines the `EvalCase` structure and utilities for loading/storing evals.

use serde::{Deserialize, Serialize};

/// Available evaluation dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalDimension {
    MemoryRecall,
    ConstraintFollow,
    ToolSelection,
    ErrorRecovery,
    Personality,
}

/// A single evaluation case for behavioral testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    pub id: String,
    pub dimension: EvalDimension,
    pub description: String,
    #[serde(default)]
    pub setup_messages: Vec<String>,
    #[serde(default)]
    pub setup_memories: Vec<String>,
    pub input: String,
    #[serde(default)]
    pub expected_keywords: Vec<String>,
    pub threshold: f32,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl EvalCase {
    /// Returns true if this case should run for the given dimension.
    pub fn matches_dimension(&self, dimension: &EvalDimension) -> bool {
        &self.dimension == dimension
    }

    /// Returns true if any of the expected keywords appear in the response.
    pub fn check_keywords(&self, response: &str) -> Vec<String> {
        self.expected_keywords
            .iter()
            .filter(|kw| response.contains(kw.as_str()))
            .cloned()
            .collect()
    }

    /// Returns the keyword hit rate (matched / total).
    pub fn keyword_hit_rate(&self, response: &str) -> f32 {
        if self.expected_keywords.is_empty() {
            return 1.0; // No keywords = automatic pass
        }
        let matched = self.check_keywords(response).len() as f32;
        matched / self.expected_keywords.len() as f32
    }
}
