//! Multi-model routing and per-provider circuit breaker.
//!
//! Issue #065: Routes to healthy providers, degrades gracefully.
//!
//! Architecture:
//! - `ModelRouter` — selects the best available provider based on health weights
//! - `ProviderCircuitBreaker` — per-provider trip mechanism
//! - `RateLimitBackoff` — exponential backoff on 429 responses
//!
//! Providers are tried in order: primary → secondary → tertiary (max 2 degradations).
//! A 429 response triggers exponential backoff retry (max 3 retries).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Circuit state for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation, requests allowed.
    Closed,
    /// Circuit tripped, requests blocked.
    Open,
    /// Recovery probe — allow one request to test health.
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitState::Closed => write!(f, "closed"),
            CircuitState::Open => write!(f, "open"),
            CircuitState::HalfOpen => write!(f, "half-open"),
        }
    }
}

/// Circuit breaker for a single provider.
pub struct ProviderCircuitBreaker {
    failures: usize,
    last_failure: Option<i64>,
    state: CircuitState,
    threshold: usize,
    open_timeout_secs: i64,
}

impl ProviderCircuitBreaker {
    pub fn new(threshold: usize, open_timeout_secs: i64) -> Self {
        Self {
            failures: 0,
            last_failure: None,
            state: CircuitState::Closed,
            threshold,
            open_timeout_secs,
        }
    }

    /// Check if requests are allowed.
    pub fn is_available(&self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                // Check if cooldown has elapsed
                if let Some(last) = self.last_failure {
                    let elapsed = chrono::Utc::now().timestamp() - last;
                    if elapsed >= self.open_timeout_secs {
                        return true; // Transition to half-open handled by caller
                    }
                }
                false
            }
        }
    }

    /// Get current state.
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Record a successful request.
    pub fn record_success(&mut self) {
        self.failures = 0;
        self.last_failure = None;
        self.state = CircuitState::Closed;
    }

    /// Record a failed request.
    pub fn record_failure(&mut self) {
        self.failures += 1;
        self.last_failure = Some(chrono::Utc::now().timestamp());

        if self.failures >= self.threshold {
            self.state = CircuitState::Open;
        }
    }

    /// Transition from Open to HalfOpen (after cooldown expires).
    pub fn try_half_open(&mut self) {
        if self.state == CircuitState::Open {
            self.state = CircuitState::HalfOpen;
        }
    }
}

/// Health info for a provider.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub name: String,
    pub weight: f32,
    pub is_available: bool,
    pub circuit_state: CircuitState,
}

impl ProviderHealth {
    pub fn new(name: String) -> Self {
        Self {
            name,
            weight: 1.0,
            is_available: true,
            circuit_state: CircuitState::Closed,
        }
    }
}

/// Exponential backoff state for rate limiting.
#[derive(Debug)]
pub struct RateLimitBackoff {
    base_delay_ms: u64,
    max_delay_ms: u64,
    max_retries: usize,
    current_delay_ms: u64,
    retry_count: usize,
    last_429_at: Option<Instant>,
}

impl RateLimitBackoff {
    pub fn new() -> Self {
        Self {
            base_delay_ms: 1000,
            max_delay_ms: 30000,
            max_retries: 3,
            current_delay_ms: 0,
            retry_count: 0,
            last_429_at: None,
        }
    }

    /// Returns true if a retry is allowed.
    pub fn should_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }

    /// Record a 429 response and return the delay before retry.
    pub fn record_429(&mut self) -> Option<Duration> {
        self.last_429_at = Some(Instant::now());
        self.retry_count += 1;
        self.current_delay_ms = (self.base_delay_ms * 2u64.saturating_pow(self.retry_count as u32))
            .min(self.max_delay_ms);
        Some(Duration::from_millis(self.current_delay_ms))
    }

    /// Record a non-429 response, reset counters.
    pub fn record_success(&mut self) {
        self.retry_count = 0;
        self.current_delay_ms = 0;
    }

    /// Reset for a new request.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Get remaining retry attempts.
    pub fn remaining_retries(&self) -> usize {
        self.max_retries.saturating_sub(self.retry_count)
    }
}

/// Model router with health-weighted selection.
pub struct ModelRouter {
    providers: Vec<ProviderHealth>,
    circuit_breakers: HashMap<String, Arc<Mutex<ProviderCircuitBreaker>>>,
    backoff: RateLimitBackoff,
    rate_limit_window_secs: i64,
}

impl ModelRouter {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            circuit_breakers: HashMap::new(),
            backoff: RateLimitBackoff::new(),
            rate_limit_window_secs: 60,
        }
    }

    /// Register a provider.
    pub fn register_provider(&mut self, name: String, weight: f32) {
        self.providers.push(ProviderHealth {
            name: name.clone(),
            weight,
            is_available: true,
            circuit_state: CircuitState::Closed,
        });
        self.circuit_breakers.insert(
            name,
            Arc::new(Mutex::new(ProviderCircuitBreaker::new(3, 60))),
        );
    }

    /// Select the best available provider based on health weight.
    /// Returns `None` if all circuits are open.
    pub fn select(&self) -> Option<String> {
        self.providers
            .iter()
            .filter(|p| p.is_available)
            .filter(|p| {
                self.circuit_breakers
                    .get(&p.name)
                    .map(|cb| cb.lock().unwrap().is_available())
                    .unwrap_or(false)
            })
            .max_by(|a, b| {
                a.weight
                    .partial_cmp(&b.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|p| p.name.clone())
    }

    /// Record a successful call for a provider.
    pub fn record_success(&mut self, provider_name: &str) {
        if let Some(cb) = self.circuit_breakers.get(provider_name) {
            cb.lock().unwrap().record_success();
        }
        self.backoff.record_success();
    }

    /// Record a failed call for a provider.
    pub fn record_failure(&self, provider_name: &str, is_rate_limited: bool) {
        if let Some(cb) = self.circuit_breakers.get(provider_name) {
            let mut cb = cb.lock().unwrap();
            cb.record_failure();
            if is_rate_limited {
                cb.try_half_open(); // Rate limit → half-open sooner
            }
        }
    }

    /// Record a 429 response.
    pub fn record_429(&mut self, provider_name: &str) -> Option<Duration> {
        if let Some(cb) = self.circuit_breakers.get(provider_name) {
            cb.lock().unwrap().record_failure();
        }
        self.backoff.record_429()
    }

    /// Check if backoff allows retry.
    pub fn should_retry(&self) -> bool {
        self.backoff.should_retry()
    }

    /// Get remaining retries after 429.
    pub fn remaining_retries(&self) -> usize {
        self.backoff.remaining_retries()
    }

    /// Get circuit state for a provider.
    pub fn circuit_state(&self, provider_name: &str) -> CircuitState {
        self.circuit_breakers
            .get(provider_name)
            .map(|cb| cb.lock().unwrap().state())
            .unwrap_or(CircuitState::Closed)
    }

    /// Update provider availability.
    pub fn set_available(&mut self, name: &str, available: bool) {
        if let Some(p) = self.providers.iter_mut().find(|p| p.name == name) {
            p.is_available = available;
        }
    }

    /// Update provider health weight (can be called from health checks).
    pub fn set_weight(&mut self, name: &str, weight: f32) {
        if let Some(p) = self.providers.iter_mut().find(|p| p.name == name) {
            p.weight = weight;
        }
    }

    /// Get health info for all providers.
    pub fn provider_health(&self) -> Vec<ProviderHealth> {
        self.providers
            .iter()
            .map(|p| ProviderHealth {
                name: p.name.clone(),
                weight: p.weight,
                is_available: p.is_available,
                circuit_state: self.circuit_state(&p.name),
            })
            .collect()
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared router.
#[derive(Clone, Default)]
pub struct SharedModelRouter(Arc<Mutex<ModelRouter>>);

impl SharedModelRouter {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(ModelRouter::new())))
    }

    pub fn register_provider(&self, name: String, weight: f32) {
        self.0.lock().unwrap().register_provider(name, weight);
    }

    pub fn select(&self) -> Option<String> {
        self.0.lock().unwrap().select()
    }

    pub fn record_success(&mut self, provider: &str) {
        self.0.lock().unwrap().record_success(provider);
    }

    pub fn record_failure(&self, provider: &str, rate_limited: bool) {
        self.0
            .lock()
            .unwrap()
            .record_failure(provider, rate_limited);
    }

    pub fn record_429(&self, provider: &str) -> Option<std::time::Duration> {
        self.0.lock().unwrap().record_429(provider)
    }

    pub fn should_retry(&self) -> bool {
        self.0.lock().unwrap().should_retry()
    }

    pub fn remaining_retries(&self) -> usize {
        self.0.lock().unwrap().remaining_retries()
    }

    pub fn provider_health(&self) -> Vec<ProviderHealth> {
        self.0.lock().unwrap().provider_health()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_trips_after_threshold() {
        let mut cb = ProviderCircuitBreaker::new(3, 60);
        assert!(cb.is_available());

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_available());

        cb.record_failure();
        assert!(cb.is_available());

        cb.record_failure(); // 3rd failure → trips
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.is_available());
    }

    #[test]
    fn test_circuit_breaker_success_resets() {
        let mut cb = ProviderCircuitBreaker::new(2, 60);
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_available());
    }

    #[test]
    fn test_model_router_selects_best_available() {
        let mut router = ModelRouter::new();
        router.register_provider("fast".to_string(), 0.9);
        router.register_provider("slow".to_string(), 0.5);

        let selected = router.select();
        assert_eq!(selected, Some("fast".to_string()));
    }

    #[test]
    fn test_model_router_skips_circuit_open() {
        let mut router = ModelRouter::new();
        router.register_provider("good".to_string(), 0.9);
        router.register_provider("bad".to_string(), 0.8);

        // Trip the bad provider's circuit
        {
            let cb = router.circuit_breakers.get("bad").unwrap();
            let mut cb = cb.lock().unwrap();
            cb.record_failure();
            cb.record_failure();
            cb.record_failure();
        }

        let selected = router.select();
        assert_eq!(selected, Some("good".to_string()));
    }

    #[test]
    fn test_rate_limit_backoff() {
        let mut backoff = RateLimitBackoff::new();
        assert!(backoff.should_retry());
        assert_eq!(backoff.remaining_retries(), 3);

        let delay1 = backoff.record_429().unwrap();
        assert_eq!(delay1, Duration::from_millis(2000)); // 1000 * 2^1

        let delay2 = backoff.record_429().unwrap();
        assert_eq!(delay2, Duration::from_millis(4000)); // 1000 * 2^2

        let delay3 = backoff.record_429().unwrap();
        assert_eq!(delay3, Duration::from_millis(8000)); // 1000 * 2^3

        assert!(!backoff.should_retry());
    }

    #[test]
    fn test_backoff_reset_on_success() {
        let mut backoff = RateLimitBackoff::new();
        backoff.record_429();
        backoff.record_429();
        assert_eq!(backoff.remaining_retries(), 1);

        backoff.record_success();
        assert_eq!(backoff.remaining_retries(), 3);
    }
}
