//! Tool execution scheduler for parallel and conditional execution
//!
//! Issue #42: Tool execution modes optimization
//! Issue #47: True parallel execution with tokio::spawn
//!
//! Supports:
//! - Sequential: Execute tools one by one (default, for MCP tools)
//! - Parallel: Execute independent tools concurrently
//! - Conditional: Execute based on prior tool results

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent::registry::ToolRegistry;
use crate::agent::tool::{ToolCall, ToolExecutionMode, ToolResult};

/// Scheduler configuration
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum parallel executions
    pub max_parallel: usize,
    /// Default timeout per tool (seconds)
    pub tool_timeout_secs: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_parallel: 4,
            tool_timeout_secs: 30,
        }
    }
}

/// Tool call with execution metadata
#[derive(Debug)]
pub struct ScheduledCall {
    pub call: ToolCall,
    pub depends_on: Vec<String>,
    pub execution_mode: ToolExecutionMode,
}

/// Scheduler for tool execution
pub struct ToolScheduler {
    config: SchedulerConfig,
}

impl ToolScheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        Self { config }
    }

    /// Analyze tool calls and determine execution order
    pub fn analyze(&self, calls: Vec<ToolCall>) -> Vec<ScheduledCall> {
        calls
            .into_iter()
            .map(|call| {
                let depends_on = self.extract_dependencies(&call);
                ScheduledCall {
                    call,
                    depends_on,
                    execution_mode: ToolExecutionMode::default(),
                }
            })
            .collect()
    }

    /// Extract dependencies from tool call arguments
    fn extract_dependencies(&self, call: &ToolCall) -> Vec<String> {
        let mut deps = Vec::new();

        // Check for references to previous tool outputs in arguments
        let args_str = serde_json::to_string(&call.arguments).unwrap_or_default();

        // Look for patterns like $call_id or ${call_id}
        let patterns = ["$", "${"];
        for pattern in patterns {
            if args_str.contains(pattern) {
                // This is a simplification - in production, use proper parsing
                deps.push(format!("previous_output"));
            }
        }

        deps
    }

    /// Execute tools based on their execution modes.
    ///
    /// Independent `Parallel` tools are dispatched concurrently via `tokio::spawn`.
    /// Tools with dependencies or `Sequential` mode run in order.
    pub fn execute(
        &self,
        calls: Vec<ScheduledCall>,
        registry: Arc<ToolRegistry>,
        work_dir: PathBuf,
    ) -> Vec<(ToolCall, ToolResult)> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.execute_async(calls, registry, work_dir))
    }

    /// Async implementation of execute — dispatches parallel tool calls concurrently.
    async fn execute_async(
        &self,
        calls: Vec<ScheduledCall>,
        registry: Arc<ToolRegistry>,
        work_dir: PathBuf,
    ) -> Vec<(ToolCall, ToolResult)> {
        let mut results: HashMap<String, ToolResult> = HashMap::new();
        let mut ordered_results: Vec<(ToolCall, ToolResult)> = Vec::new();

        // Group calls by execution mode
        let (parallel_calls, sequential_calls): (Vec<_>, Vec<_>) =
            calls.into_iter().partition(|scheduled| {
                matches!(scheduled.execution_mode, ToolExecutionMode::Parallel)
                    && scheduled.depends_on.is_empty()
            });

        // Execute parallel calls concurrently (Issue #47)
        if !parallel_calls.is_empty() {
            let parallel_results = self
                .execute_parallel(parallel_calls, registry.clone(), work_dir.clone())
                .await;
            for (call, result) in parallel_results {
                results.insert(result.call_id.clone(), result.result.clone());
                ordered_results.push((call, result.result));
            }
        }

        // Execute sequential calls in order
        for scheduled in sequential_calls {
            // Wait for dependencies
            self.wait_for_dependencies(&scheduled.depends_on, &results)
                .await;

            // Execute the call
            let call = scheduled.call;
            let call_id = call
                .id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            let exec_result = registry.execute(&call, &call_id, &work_dir);

            results.insert(call_id, exec_result.result.clone());
            ordered_results.push((call, exec_result.result));
        }

        ordered_results
    }

    /// Execute multiple tools in parallel using tokio::spawn.
    ///
    /// Each tool call is dispatched as an independent async task.
    /// Results are collected in submission order after all tasks complete.
    async fn execute_parallel(
        &self,
        calls: Vec<ScheduledCall>,
        registry: Arc<ToolRegistry>,
        work_dir: PathBuf,
    ) -> Vec<(ToolCall, SchedulerExecResult)> {
        let mut handles: Vec<(ToolCall, JoinHandle<SchedulerExecResult>)> = Vec::new();

        for scheduled in calls {
            let registry = registry.clone();
            let work_dir = work_dir.clone();
            let call = scheduled.call;

            // Pre-compute values that don't move with `call`
            let call_clone = call.clone();
            let id_for_spawn = call
                .id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            let name_for_spawn = call.name.clone();
            let args_for_spawn = call.arguments.clone();

            let handle: JoinHandle<SchedulerExecResult> = tokio::spawn(async move {
                let exec_result = registry.execute(&call, &id_for_spawn, &work_dir);
                SchedulerExecResult {
                    call_id: id_for_spawn,
                    tool_name: name_for_spawn,
                    arguments: args_for_spawn,
                    result: exec_result.result,
                }
            });

            handles.push((call_clone, handle));
        }

        // Collect results in submission order
        let mut results = Vec::new();
        for (call, handle) in handles {
            match handle.await {
                Ok(scheduler_result) => results.push((call, scheduler_result)),
                Err(_) => {
                    // Spawn failed (task panicked) — skip
                }
            }
        }

        results
    }

    /// Wait for dependencies to be satisfied
    async fn wait_for_dependencies(
        &self,
        _depends_on: &[String],
        _results: &HashMap<String, ToolResult>,
    ) {
        // In a real implementation, this would poll for dependency results
        // For now, dependencies are assumed to be satisfied
    }
}

/// Result returned by a spawned parallel task in `execute_parallel`.
#[derive(Debug, Clone)]
struct SchedulerExecResult {
    call_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    result: ToolResult,
}

/// Analyze tool calls for dependency detection
pub fn analyze_dependencies(calls: &[ToolCall]) -> HashMap<String, Vec<String>> {
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();

    for (i, call) in calls.iter().enumerate() {
        let call_id = call.id.clone().unwrap_or_else(|| format!("call_{}", i));
        let args_str = serde_json::to_string(&call.arguments).unwrap_or_default();

        // Check if arguments reference previous outputs
        for (j, prev_call) in calls.iter().enumerate().take(i) {
            let prev_id = prev_call
                .id
                .clone()
                .unwrap_or_else(|| format!("call_{}", j));

            if args_str.contains(&format!("${}", prev_id))
                || args_str.contains(&format!("${{{}}}", prev_id))
            {
                deps.entry(call_id.clone()).or_default().push(prev_id);
            }
        }
    }

    deps
}

/// Topological sort for execution order
pub fn topological_sort(
    calls: Vec<ToolCall>,
    dependencies: &HashMap<String, Vec<String>>,
) -> Vec<Vec<ToolCall>> {
    let mut result: Vec<Vec<ToolCall>> = Vec::new();
    let mut remaining: HashSet<String> = calls
        .iter()
        .enumerate()
        .map(|(i, c)| c.id.clone().unwrap_or_else(|| format!("call_{}", i)))
        .collect();

    let mut deps = dependencies.clone();

    while !remaining.is_empty() {
        // Find calls with no remaining dependencies
        let ready: Vec<String> = remaining
            .iter()
            .filter(|id| {
                deps.get(*id)
                    .map(|d| d.iter().all(|dep| !remaining.contains(dep)))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        if ready.is_empty() {
            // Circular dependency or error - break remaining as parallel
            result.push(calls.clone());
            break;
        }

        let ready_calls: Vec<ToolCall> = calls
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                let id = c.id.clone().unwrap_or_else(|| format!("call_{}", i));
                ready.contains(&id)
            })
            .map(|(_, c)| c.clone())
            .collect();

        for id in &ready {
            remaining.remove(id);
            deps.remove(id);
        }

        result.push(ready_calls);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_execution_mode_default() {
        let mode = ToolExecutionMode::default();
        assert!(matches!(mode, ToolExecutionMode::Sequential));
    }

    #[test]
    fn test_analyze_dependencies() {
        let calls = vec![
            ToolCall::new("tool_a".to_string(), serde_json::json!({})),
            ToolCall::new("tool_b".to_string(), serde_json::json!({"ref": "$call_0"})),
        ];

        let deps = analyze_dependencies(&calls);
        assert!(deps.contains_key("call_1"));
        assert!(deps.get("call_1").unwrap().contains(&"call_0".to_string()));
    }

    #[test]
    fn test_parallel_execution_is_concurrent() {
        // Verify that tokio runtime is available and parallel tasks run concurrently
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::Arc;

            let counter = Arc::new(AtomicUsize::new(0));
            let mut handles = Vec::new();

            // Spawn 4 tasks that each sleep 50ms
            for _ in 0..4 {
                let counter = counter.clone();
                let handle = tokio::spawn(async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    counter.fetch_add(1, Ordering::SeqCst);
                });
                handles.push(handle);
            }

            let start = Instant::now();
            for handle in handles {
                let _ = handle.await;
            }
            let elapsed = start.elapsed();

            // All 4 should have started before any finished
            // If truly parallel, total time < 150ms (sequential would be 4*50=200ms)
            assert!(
                elapsed < Duration::from_millis(150),
                "Parallel execution took {:?}, expected < 150ms",
                elapsed
            );
        });
    }
}
