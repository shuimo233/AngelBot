//! Tool Circuit Breaker - prevents repeated tool failures from blocking the agent loop
//!
//! Issue #064: DAG Dependency Circuit Breaker
//! - Per-tool failure tracking with consecutive failure counts
//! - State machine: Closed -> Open -> HalfOpen -> Closed
//! - Configurable thresholds and cooldown periods
//! - Async timeout execution wrapper for long-running tools

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::agent::tool::{ToolCall, ToolHandler, ToolResult};

/// Circuit breaker state for a single tool
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed — tool executes normally
    Closed,
    /// Circuit is open — tool is skipped due to repeated failures
    Open,
    /// Circuit is half-open — allowing one test request after cooldown
    HalfOpen,
}

impl Default for CircuitState {
    fn default() -> Self {
        CircuitState::Closed
    }
}

/// Per-tool circuit breaker that tracks consecutive failures and manages state transitions.
///
/// Tracks consecutive failures per tool and trips the circuit (opens it) when the
/// failure count exceeds `max_failures`. After `cooldown_secs`, the circuit moves to
/// `HalfOpen` to allow a test request.
#[derive(Debug, Clone)]
pub struct ToolCircuitBreaker {
    /// Tool name -> consecutive failure count
    failures: HashMap<String, usize>,
    /// Tool name -> unix timestamp (seconds) of last failure
    last_failure: HashMap<String, i64>,
    /// Per-tool execution timeout in seconds
    timeout_secs: u64,
    /// Number of consecutive failures required to trip the circuit
    max_failures: usize,
    /// Seconds to wait before transitioning from Open to HalfOpen
    cooldown_secs: i64,
}

impl ToolCircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(timeout_secs: u64, max_failures: usize, cooldown_secs: i64) -> Self {
        Self {
            failures: HashMap::new(),
            last_failure: HashMap::new(),
            timeout_secs,
            max_failures,
            cooldown_secs,
        }
    }

    /// Returns true if the circuit is open (or half-open and should still skip).
    /// Returns false if the circuit is closed and the tool may execute.
    pub fn should_skip(&self, tool_name: &str) -> bool {
        let state = self.state(tool_name);
        state == CircuitState::Open || state == CircuitState::HalfOpen
    }

    /// Record a failure for the given tool.
    /// Increments the consecutive failure count and updates the last failure timestamp.
    /// If the failure count reaches or exceeds `max_failures`, the circuit trips (opens).
    pub fn record_failure(&mut self, tool_name: String) {
        let count = self.failures.entry(tool_name.clone()).or_insert(0);
        *count += 1;
        let now = chrono::Utc::now().timestamp();
        self.last_failure.insert(tool_name, now);
    }

    /// Record a success for the given tool.
    /// Resets the consecutive failure count to 0, closing the circuit.
    pub fn record_success(&mut self, tool_name: &str) {
        self.failures.insert(tool_name.to_string(), 0);
    }

    /// Returns the current circuit state for the given tool.
    ///
    /// - **Closed**: failure count is below `max_failures`
    /// - **Open**: failure count >= `max_failures` AND cooldown has not elapsed
    /// - **HalfOpen**: failure count >= `max_failures` AND cooldown has elapsed
    pub fn state(&self, tool_name: &str) -> CircuitState {
        let failures = self.failures.get(tool_name).copied().unwrap_or(0);

        if failures < self.max_failures {
            return CircuitState::Closed;
        }

        // Circuit has tripped — check cooldown
        if let Some(&last_ts) = self.last_failure.get(tool_name) {
            let now = chrono::Utc::now().timestamp();
            let elapsed = now - last_ts;
            if elapsed >= self.cooldown_secs {
                return CircuitState::HalfOpen;
            }
        }

        CircuitState::Open
    }

    /// Manually reset the circuit for a given tool, returning it to the Closed state.
    pub fn reset_tool(&mut self, tool_name: &str) {
        self.failures.insert(tool_name.to_string(), 0);
        self.last_failure.remove(tool_name);
    }

    /// Returns the configured timeout in seconds.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }
}

/// Executes a tool call with a per-call timeout.
/// Wraps `ToolHandler::execute` inside `tokio::time::timeout`.
#[derive(Debug, Clone)]
pub struct ToolTimeoutExecutor {
    timeout_secs: u64,
}

impl ToolTimeoutExecutor {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Execute the tool call with a timeout.
    /// `handler` must implement `Send + Sync + 'static` so it can be used across
    /// the `spawn_blocking` thread boundary.
    /// Returns `ToolResult::error(name, "工具执行超时")` if execution exceeds the timeout.
    pub async fn execute_with_timeout(
        &self,
        call: &ToolCall,
        handler: Arc<dyn ToolHandler>,
        work_dir: PathBuf,
    ) -> ToolResult {
        let name = call.name.clone();
        let args = call.arguments.clone();

        let result = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            tokio::task::spawn_blocking(move || handler.execute(&args, &work_dir)),
        )
        .await;

        match result {
            Ok(Ok(res)) => res,
            Ok(Err(panic_err)) => {
                // Handler panicked — treat as failure
                ToolResult::error(&name, format!("工具执行 panicked: {panic_err}"))
            }
            Err(_) => {
                // Timeout elapsed
                ToolResult::error(&name, "工具执行超时")
            }
        }
    }
}

/// Wraps `ToolCircuitBreaker` behind an `Arc<Mutex<>>` for shared mutable access
/// across async tasks.
#[derive(Debug, Clone)]
pub struct GlobalCircuitBreaker {
    inner: Arc<Mutex<ToolCircuitBreaker>>,
}

impl GlobalCircuitBreaker {
    pub fn new(timeout_secs: u64, max_failures: usize, cooldown_secs: i64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ToolCircuitBreaker::new(
                timeout_secs,
                max_failures,
                cooldown_secs,
            ))),
        }
    }

    pub async fn should_skip(&self, tool_name: &str) -> bool {
        self.inner.lock().await.should_skip(tool_name)
    }

    pub async fn record_failure(&self, tool_name: String) {
        self.inner.lock().await.record_failure(tool_name);
    }

    pub async fn record_success(&self, tool_name: &str) {
        self.inner.lock().await.record_success(tool_name);
    }

    pub async fn state(&self, tool_name: &str) -> CircuitState {
        self.inner.lock().await.state(tool_name)
    }

    pub async fn reset_tool(&self, tool_name: &str) {
        self.inner.lock().await.reset_tool(tool_name);
    }

    pub async fn timeout_secs(&self) -> u64 {
        self.inner.lock().await.timeout_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_breaker(max_failures: usize, cooldown_secs: i64) -> ToolCircuitBreaker {
        ToolCircuitBreaker::new(30, max_failures, cooldown_secs)
    }

    #[test]
    fn test_new_breaker_is_closed() {
        let cb = make_breaker(3, 60);
        assert_eq!(cb.state("foo"), CircuitState::Closed);
        assert!(!cb.should_skip("foo"));
    }

    #[test]
    fn test_failure_increments_count() {
        let mut cb = make_breaker(3, 60);
        cb.record_failure("foo".into());
        assert_eq!(cb.state("foo"), CircuitState::Closed);
        cb.record_failure("foo".into());
        assert_eq!(cb.state("foo"), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_trips_after_max_failures() {
        let mut cb = make_breaker(3, 60);
        cb.record_failure("foo".into());
        cb.record_failure("foo".into());
        assert_eq!(cb.state("foo"), CircuitState::Closed);
        cb.record_failure("foo".into()); // 3rd failure — trips the circuit
        assert_eq!(cb.state("foo"), CircuitState::Open);
        assert!(cb.should_skip("foo"));
    }

    #[test]
    fn test_success_resets_failure_count() {
        let mut cb = make_breaker(3, 60);
        cb.record_failure("foo".into());
        cb.record_failure("foo".into());
        cb.record_success("foo");
        assert_eq!(cb.failures.get("foo"), Some(&0));
        assert_eq!(cb.state("foo"), CircuitState::Closed);
    }

    #[test]
    fn test_reset_tool_clears_all_state() {
        let mut cb = make_breaker(3, 60);
        cb.record_failure("foo".into());
        cb.record_failure("foo".into());
        cb.record_failure("foo".into());
        assert_eq!(cb.state("foo"), CircuitState::Open);

        cb.reset_tool("foo");
        assert_eq!(cb.failures.get("foo"), Some(&0));
        assert!(cb.last_failure.get("foo").is_none());
        assert_eq!(cb.state("foo"), CircuitState::Closed);
        assert!(!cb.should_skip("foo"));
    }

    #[test]
    fn test_different_tools_independent() {
        let mut cb = make_breaker(2, 60);
        cb.record_failure("foo".into());
        cb.record_failure("foo".into()); // trips foo
        assert_eq!(cb.state("foo"), CircuitState::Open);
        assert_eq!(cb.state("bar"), CircuitState::Closed);
    }

    #[test]
    fn test_timeout_executor_returns_result_on_success() {
        struct OkHandler;
        impl ToolHandler for OkHandler {
            fn execute(
                &self,
                _args: &serde_json::Value,
                _work_dir: &std::path::Path,
            ) -> ToolResult {
                ToolResult::success("ok_tool", "done")
            }
            fn name(&self) -> &str {
                "ok_tool"
            }
        }

        let executor = ToolTimeoutExecutor::new(5);
        let call = ToolCall::new("ok_tool".into(), serde_json::json!({}));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor.execute_with_timeout(
            &call,
            Arc::new(OkHandler),
            PathBuf::from("/"),
        ));
        assert!(result.success);
        assert_eq!(result.name, "ok_tool");
    }

    #[test]
    fn test_timeout_executor_returns_error_on_timeout() {
        struct SlowHandler;
        impl ToolHandler for SlowHandler {
            fn execute(
                &self,
                _args: &serde_json::Value,
                _work_dir: &std::path::Path,
            ) -> ToolResult {
                std::thread::sleep(std::time::Duration::from_secs(10));
                ToolResult::success("slow_tool", "done")
            }
            fn name(&self) -> &str {
                "slow_tool"
            }
        }

        let executor = ToolTimeoutExecutor::new(1); // 1 second timeout
        let call = ToolCall::new("slow_tool".into(), serde_json::json!({}));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor.execute_with_timeout(
            &call,
            Arc::new(SlowHandler),
            PathBuf::from("/"),
        ));
        assert!(!result.success);
        assert_eq!(result.name, "slow_tool");
        assert!(result.content.contains("超时"));
    }

    #[test]
    fn test_global_circuit_breaker_wraps_inner() {
        let gcb = GlobalCircuitBreaker::new(30, 2, 60);
        assert!(!tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(gcb.should_skip("foo")));

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(gcb.record_failure("foo".into()));
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(gcb.record_failure("foo".into()));

        assert_eq!(
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(gcb.state("foo")),
            CircuitState::Open
        );
    }
}
