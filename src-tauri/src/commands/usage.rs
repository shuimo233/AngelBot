//! Usage tracking commands
//!
//! Issue #43: Provider cost tracking
//!
//! Provides commands for querying usage statistics and cost summaries.

use crate::llm::{CostSummary, UsageStats, UsageTracker};
use crate::AppState;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Get usage stats for a specific session
#[tauri::command]
pub fn get_session_usage(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<UsageStats>, String> {
    let tracker = UsageTracker::new(state.db.clone());
    tracker
        .get_session_usage(&session_id)
        .map_err(|e| e.to_string())
}

/// Get total cost summary
#[tauri::command]
pub fn get_cost_summary(state: State<'_, AppState>) -> Result<CostSummary, String> {
    let tracker = UsageTracker::new(state.db.clone());
    tracker.get_cost_summary().map_err(|e| e.to_string())
}

/// Get usage history within a time range
#[tauri::command]
pub fn get_usage_history(
    state: State<'_, AppState>,
    from_timestamp: i64,
    to_timestamp: i64,
) -> Result<Vec<UsageStats>, String> {
    let tracker = UsageTracker::new(state.db.clone());
    tracker
        .get_usage_history(from_timestamp, to_timestamp)
        .map_err(|e| e.to_string())
}

/// Record a usage entry manually (for testing or external tracking)
#[tauri::command]
pub fn record_usage(
    state: State<'_, AppState>,
    session_id: Option<String>,
    provider: String,
    model: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    latency_ms: Option<u32>,
) -> Result<UsageStats, String> {
    let tracker = UsageTracker::new(state.db.clone());
    tracker
        .record_usage(
            session_id.as_deref(),
            &provider,
            &model,
            prompt_tokens,
            completion_tokens,
            latency_ms,
        )
        .map_err(|e| e.to_string())
}

/// Get usage summary for the last N days
#[tauri::command]
pub fn get_usage_last_n_days(state: State<'_, AppState>, days: i64) -> Result<CostSummary, String> {
    let now = chrono::Utc::now().timestamp();
    let from = now - (days * 24 * 60 * 60);

    let tracker = UsageTracker::new(state.db.clone());
    let history = tracker
        .get_usage_history(from, now)
        .map_err(|e| e.to_string())?;

    // Build summary from history
    let mut total_prompt: u64 = 0;
    let mut total_completion: u64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut by_provider: std::collections::HashMap<String, crate::llm::usage::ProviderCost> =
        std::collections::HashMap::new();
    let by_model: std::collections::HashMap<String, crate::llm::usage::ModelCost> =
        std::collections::HashMap::new();

    for stat in history {
        total_prompt += stat.prompt_tokens as u64;
        total_completion += stat.completion_tokens as u64;
        total_cost += stat.cost_usd;

        // Update by_provider
        let entry =
            by_provider
                .entry(stat.provider.clone())
                .or_insert(crate::llm::usage::ProviderCost {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    total_tokens: 0,
                    cost_usd: 0.0,
                });
        entry.prompt_tokens += stat.prompt_tokens as u64;
        entry.completion_tokens += stat.completion_tokens as u64;
        entry.total_tokens += stat.total_tokens as u64;
        entry.cost_usd += stat.cost_usd;
    }

    Ok(CostSummary {
        total_prompt_tokens: total_prompt,
        total_completion_tokens: total_completion,
        total_tokens: total_prompt + total_completion,
        total_cost_usd: total_cost,
        by_provider,
        by_model,
    })
}
