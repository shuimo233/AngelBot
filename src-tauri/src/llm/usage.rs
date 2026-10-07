//! Usage tracking for LLM API calls
//!
//! Issue #43: Provider cost tracking and usage statistics
//!
//! Tracks token consumption and cost per API call for budget monitoring.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

/// Cost per 1M tokens for different providers/models
/// Based on current pricing (2024-2025)
pub mod pricing {
    /// Cost per million tokens (input, output)
    pub struct ModelPricing {
        pub input_per_m: f64,
        pub output_per_m: f64,
    }

    /// Default pricing table
    pub fn get_pricing(provider: &str, model: &str) -> (f64, f64) {
        match (
            provider.to_lowercase().as_str(),
            model.to_lowercase().as_str(),
        ) {
            // Anthropic
            ("anthropic", "claude-3-5-sonnet-20241022") => (3.0, 15.0),
            ("anthropic", "claude-3-5-sonnet") => (3.0, 15.0),
            ("anthropic", "claude-3-opus") => (15.0, 75.0),
            ("anthropic", "claude-3-haiku") => (0.8, 4.0),

            // OpenAI
            ("openai", "gpt-4o") => (2.5, 10.0),
            ("openai", "gpt-4o-mini") => (0.15, 0.6),
            ("openai", "gpt-4-turbo") => (10.0, 30.0),
            ("openai", "gpt-4") => (30.0, 60.0),
            ("openai", "gpt-3.5-turbo") => (0.5, 1.5),

            // OpenAI Compatible (Ollama, etc.)
            ("ollama", _) => (0.0, 0.0),            // Local, no cost
            ("openai-compatible", _) => (0.0, 0.0), // Unknown, assume free

            // DeepSeek V4: pricing for V4 series — TODO(#103) confirm actual
            // V4-flash prices from DeepSeek before relying on these for billing.
            // Legacy `deepseek-chat` row is kept so old usage logs still
            // resolve; new sessions should pick V4 names.
            ("deepseek", "deepseek-v4-flash") => (0.14, 0.28),
            ("deepseek", "deepseek-v4-pro") => (0.14, 0.28),
            ("deepseek", "deepseek-chat") => (0.14, 0.28),
            ("deepseek", "deepseek-coder") => (0.14, 0.28),

            // Groq
            ("groq", _) => (0.0, 0.0), // Per-token pricing varies, assume free for now

            // Default: assume moderate pricing
            _ => (1.0, 5.0),
        }
    }
}

/// Usage statistics for a single API call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub id: String,
    pub session_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cost_usd: f64,
    pub latency_ms: Option<u32>,
    pub created_at: i64,
}

/// Aggregated cost summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub by_provider: HashMap<String, ProviderCost>,
    pub by_model: HashMap<String, ModelCost>,
}

/// Cost breakdown by provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCost {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

/// Cost breakdown by model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCost {
    pub provider: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub call_count: u64,
}

/// Usage tracker for managing token consumption
pub struct UsageTracker {
    db: Arc<Mutex<Connection>>,
}

impl UsageTracker {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        Self { db }
    }

    /// Get a locked connection
    fn with_conn<T, F>(&self, f: F) -> Result<T, rusqlite::Error>
    where
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    {
        let conn = self.db.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        f(&conn)
    }

    /// Record usage from an API call
    pub fn record_usage(
        &self,
        session_id: Option<&str>,
        provider: &str,
        model: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
        latency_ms: Option<u32>,
    ) -> Result<UsageStats, rusqlite::Error> {
        let total_tokens = prompt_tokens + completion_tokens;

        // Calculate cost
        let (input_cost, output_cost) = pricing::get_pricing(provider, model);
        let cost_usd = (prompt_tokens as f64 * input_cost / 1_000_000.0)
            + (completion_tokens as f64 * output_cost / 1_000_000.0);

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO usage_stats (id, session_id, provider, model, prompt_tokens, completion_tokens, total_tokens, cost_usd, latency_ms, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id,
                    session_id,
                    provider,
                    model,
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                    cost_usd,
                    latency_ms,
                    now
                ],
            )?;

            // Update cache
            conn.execute(
                "UPDATE cost_summary_cache SET 
                 total_prompt_tokens = (SELECT COALESCE(SUM(prompt_tokens), 0) FROM usage_stats),
                 total_completion_tokens = (SELECT COALESCE(SUM(completion_tokens), 0) FROM usage_stats),
                 total_cost_usd = (SELECT COALESCE(SUM(cost_usd), 0.0) FROM usage_stats),
                 updated_at = ?1
                 WHERE id = 1",
                params![now],
            )?;

            Ok(UsageStats {
                id,
                session_id: session_id.map(String::from),
                provider: provider.to_string(),
                model: model.to_string(),
                prompt_tokens,
                completion_tokens,
                total_tokens,
                cost_usd,
                latency_ms,
                created_at: now,
            })
        })
    }

    /// Get usage for a specific session
    pub fn get_session_usage(&self, session_id: &str) -> Result<Vec<UsageStats>, rusqlite::Error> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, provider, model, prompt_tokens, completion_tokens, total_tokens, cost_usd, latency_ms, created_at
                 FROM usage_stats WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 100",
            )?;

            let rows = stmt.query_map(params![session_id], |r| {
                Ok(UsageStats {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    provider: r.get(2)?,
                    model: r.get(3)?,
                    prompt_tokens: r.get::<_, i32>(4)? as u32,
                    completion_tokens: r.get::<_, i32>(5)? as u32,
                    total_tokens: r.get::<_, i32>(6)? as u32,
                    cost_usd: r.get(7)?,
                    latency_ms: r.get(8)?,
                    created_at: r.get(9)?,
                })
            })?;

            rows.collect()
        })
    }

    /// Get total cost summary
    pub fn get_cost_summary(&self) -> Result<CostSummary, rusqlite::Error> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0), COALESCE(SUM(total_tokens), 0), COALESCE(SUM(cost_usd), 0.0)
                 FROM usage_stats",
            )?;

            let (total_prompt, total_completion, total_tokens, total_cost): (i64, i64, i64, f64) = stmt
                .query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;

            // Get breakdown by provider
            let mut by_provider: HashMap<String, ProviderCost> = HashMap::new();
            let mut stmt = conn.prepare(
                "SELECT provider, SUM(prompt_tokens), SUM(completion_tokens), SUM(total_tokens), SUM(cost_usd)
                 FROM usage_stats GROUP BY provider",
            )?;

            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? as u64,
                    r.get::<_, i64>(3)? as u64,
                    r.get::<_, f64>(4)?,
                ))
            })?;

            for row in rows.flatten() {
                by_provider.insert(
                    row.0.clone(),
                    ProviderCost {
                        prompt_tokens: row.1,
                        completion_tokens: row.2,
                        total_tokens: row.3,
                        cost_usd: row.4,
                    },
                );
            }

            // Get breakdown by model
            let mut by_model: HashMap<String, ModelCost> = HashMap::new();
            let mut stmt = conn.prepare(
                "SELECT model, provider, SUM(prompt_tokens), SUM(completion_tokens), SUM(total_tokens), SUM(cost_usd), COUNT(*)
                 FROM usage_stats GROUP BY model",
            )?;

            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? as u64,
                    r.get::<_, i64>(3)? as u64,
                    r.get::<_, i64>(4)? as u64,
                    r.get::<_, f64>(5)?,
                    r.get::<_, i64>(6)? as u64,
                ))
            })?;

            for row in rows.flatten() {
                by_model.insert(
                    row.0.clone(),
                    ModelCost {
                        provider: row.1,
                        prompt_tokens: row.2,
                        completion_tokens: row.3,
                        total_tokens: row.4,
                        cost_usd: row.5,
                        call_count: row.6,
                    },
                );
            }

            Ok(CostSummary {
                total_prompt_tokens: total_prompt as u64,
                total_completion_tokens: total_completion as u64,
                total_tokens: total_tokens as u64,
                total_cost_usd: total_cost,
                by_provider,
                by_model,
            })
        })
    }

    /// Get usage history within a time range
    pub fn get_usage_history(
        &self,
        from_timestamp: i64,
        to_timestamp: i64,
    ) -> Result<Vec<UsageStats>, rusqlite::Error> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, provider, model, prompt_tokens, completion_tokens, total_tokens, cost_usd, latency_ms, created_at
                 FROM usage_stats WHERE created_at >= ?1 AND created_at <= ?2 ORDER BY created_at DESC LIMIT 1000",
            )?;

            let rows = stmt.query_map(params![from_timestamp, to_timestamp], |r| {
                Ok(UsageStats {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    provider: r.get(2)?,
                    model: r.get(3)?,
                    prompt_tokens: r.get::<_, i32>(4)? as u32,
                    completion_tokens: r.get::<_, i32>(5)? as u32,
                    total_tokens: r.get::<_, i32>(6)? as u32,
                    cost_usd: r.get(7)?,
                    latency_ms: r.get(8)?,
                    created_at: r.get(9)?,
                })
            })?;

            rows.collect()
        })
    }
}

/// Wrapper for LLM responses that includes usage information
#[derive(Debug, Clone)]
pub struct UsageWrapper<T> {
    pub inner: T,
    pub usage: Option<UsageStats>,
}

impl<T> UsageWrapper<T> {
    pub fn new(inner: T) -> Self {
        Self { inner, usage: None }
    }

    pub fn with_usage(inner: T, usage: UsageStats) -> Self {
        Self {
            inner,
            usage: Some(usage),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pricing_lookup() {
        let (input, output) = pricing::get_pricing("anthropic", "claude-3-5-sonnet-20241022");
        assert_eq!(input, 3.0);
        assert_eq!(output, 15.0);

        // Unknown provider uses default
        let (input, output) = pricing::get_pricing("unknown", "unknown");
        assert_eq!(input, 1.0);
        assert_eq!(output, 5.0);
    }

    #[test]
    fn test_cost_calculation() {
        // 1000 prompt tokens, 500 completion tokens at $3/$15 per million
        let prompt_tokens = 1000u32;
        let completion_tokens = 500u32;
        let (input_cost, output_cost) = (3.0, 15.0);

        let cost = (prompt_tokens as f64 * input_cost / 1_000_000.0)
            + (completion_tokens as f64 * output_cost / 1_000_000.0);

        assert!(cost > 0.0 && cost < 0.1);
    }
}
