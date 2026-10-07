//! Embedding provider abstraction — supports real embedding APIs with TF fallback.
//!
//! Integrates with OpenAI-compatible `/v1/embeddings` endpoints for semantic vector
//! search, while preserving the simple TF-based embedding as a fallback.

use serde::{Deserialize, Serialize};

/// Embedding configuration from settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Whether real embedding API is enabled
    pub enabled: bool,
    /// Model name (e.g. "text-embedding-3-small")
    pub model: String,
    /// API base URL (e.g. "https://api.openai.com")
    pub base_url: String,
    /// API key for the embedding endpoint
    pub api_key: String,
    /// Vector dimensions (e.g. 1536 for text-embedding-3-small)
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: "text-embedding-3-small".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: String::new(),
            dimensions: 1536,
        }
    }
}

/// Trait for embedding providers
pub trait EmbeddingProvider: Send + Sync {
    /// Generate an embedding vector for the given text.
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;

    /// Get the dimension of the embedding vectors
    fn dimensions(&self) -> usize;
}

/// OpenAI-compatible embedding provider
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    model: String,
    base_url: String,
    api_key: String,
    dimensions: usize,
}

impl OpenAiEmbeddingProvider {
    pub fn new(api_key: String, model: String, base_url: String, dimensions: usize) -> Self {
        Self {
            client: reqwest::Client::new(),
            model,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            dimensions,
        }
    }
}

impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let url = format!("{}/v1/embeddings", self.base_url);

        let body = serde_json::json!({
            "model": self.model,
            "input": text,
        });

        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async {
            let resp = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("Embedding API request failed: {}", e))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let err_body = resp.text().await.unwrap_or_default();
                return Err(format!("Embedding API error {}: {}", status, err_body));
            }

            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse embedding response: {}", e))?;

            let embedding: Vec<f32> = json["data"][0]["embedding"]
                .as_array()
                .ok_or_else(|| "Missing embedding in response".to_string())?
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();

            if embedding.is_empty() {
                return Err("Empty embedding returned".to_string());
            }

            Ok(embedding)
        })
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

/// Build an EmbeddingConfig from environment variables or settings table.
/// Falls back to disabled if no embedding API key is available.
pub fn create_embedding_config_from_env() -> EmbeddingConfig {
    let mut config = EmbeddingConfig::default();

    // Check for OpenAI API key (most common embedding provider)
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.is_empty() {
            config.enabled = true;
            config.api_key = key;
            config.base_url = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string());
            config.model = std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".to_string());
            return config;
        }
    }

    // Check for custom embedding endpoint
    if let Ok(key) = std::env::var("EMBEDDING_API_KEY") {
        if !key.is_empty() {
            config.enabled = true;
            config.api_key = key;
            config.base_url = std::env::var("EMBEDDING_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string());
            config.model = std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".to_string());
            return config;
        }
    }

    config
}

/// Create an embedding provider from configuration.
/// Returns None if embedding is disabled (caller should fall back to TF).
pub fn create_embedding_provider() -> Option<Box<dyn EmbeddingProvider>> {
    let config = create_embedding_config_from_env();
    if !config.enabled || config.api_key.is_empty() {
        return None;
    }

    Some(Box::new(OpenAiEmbeddingProvider::new(
        config.api_key,
        config.model,
        config.base_url,
        config.dimensions,
    )))
}
