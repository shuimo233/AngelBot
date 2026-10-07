//! Tool result cache.
//!
//! The cache is retained as an extension point for idempotent tool calls. Context
//! compression lives in `agent::compaction` and is intentionally not implemented
//! here.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::agent::tool::ToolResult;

/// Cache key for tool results: unique per tool name and argument hash.
#[derive(Clone, Debug)]
pub struct CacheKey {
    pub tool_name: String,
    pub args_hash: u64,
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.tool_name == other.tool_name && self.args_hash == other.args_hash
    }
}

impl Eq for CacheKey {}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.tool_name.hash(state);
        self.args_hash.hash(state);
    }
}

impl CacheKey {
    /// Compute a deterministic key from a tool name and JSON arguments.
    pub fn compute(tool_name: &str, args: &serde_json::Value) -> Self {
        let args_str = serde_json::to_string(args).unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        args_str.hash(&mut hasher);
        Self {
            tool_name: tool_name.to_string(),
            args_hash: hasher.finish(),
        }
    }
}

/// Cached tool result with its insertion timestamp.
#[derive(Clone)]
pub struct CachedResult {
    pub result: ToolResult,
    cached_at: Instant,
}

impl CachedResult {
    pub fn new(result: ToolResult) -> Self {
        Self {
            result,
            cached_at: Instant::now(),
        }
    }

    fn is_expired(&self, ttl_nanos: u64) -> bool {
        ttl_nanos != 0 && self.cached_at.elapsed().as_nanos() > u128::from(ttl_nanos)
    }
}

/// Thread-safe cache for idempotent tool results.
///
/// Entries are addressed by `(tool_name, hash(args))`; the least-recently-added
/// entry is evicted when the configured capacity is reached.
#[derive(Clone)]
pub struct ToolResultCache {
    cache: Arc<Mutex<HashMap<CacheKey, CachedResult>>>,
    max_size: usize,
    ttl_nanos: u64,
    hits: Arc<Mutex<usize>>,
    misses: Arc<Mutex<usize>>,
    order: Arc<Mutex<Vec<CacheKey>>>,
}

impl ToolResultCache {
    /// `ttl_nanos = 0` means the entry lasts for the current session.
    pub fn new(max_size: usize, ttl_nanos: u64) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            max_size,
            ttl_nanos,
            hits: Arc::new(Mutex::new(0)),
            misses: Arc::new(Mutex::new(0)),
            order: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn get(&self, tool_name: &str, args: &serde_json::Value) -> Option<ToolResult> {
        let key = CacheKey::compute(tool_name, args);
        let cache = self.cache.lock().unwrap();
        if let Some(cached) = cache.get(&key) {
            if cached.is_expired(self.ttl_nanos) {
                drop(cache);
                self.evict(&key);
                *self.misses.lock().unwrap() += 1;
                return None;
            }
            *self.hits.lock().unwrap() += 1;
            return Some(cached.result.clone());
        }
        *self.misses.lock().unwrap() += 1;
        None
    }

    pub fn put(&self, tool_name: String, args: serde_json::Value, result: ToolResult) {
        // Images are observations of a moment, never reusable cached evidence.
        if !result.images.is_empty() {
            return;
        }
        let key = CacheKey::compute(&tool_name, &args);
        let mut cache = self.cache.lock().unwrap();
        let mut order = self.order.lock().unwrap();

        if cache.len() >= self.max_size && !cache.contains_key(&key) {
            if let Some(oldest) = order.first().cloned() {
                cache.remove(&oldest);
                order.remove(0);
            }
        }

        cache.insert(key.clone(), CachedResult::new(result));
        if let Some(position) = order.iter().position(|existing| *existing == key) {
            order.remove(position);
        }
        order.push(key);
    }

    pub fn clear(&self) {
        self.cache.lock().unwrap().clear();
        self.order.lock().unwrap().clear();
        *self.hits.lock().unwrap() = 0;
        *self.misses.lock().unwrap() = 0;
    }

    pub fn hit_rate(&self) -> (usize, usize) {
        (*self.hits.lock().unwrap(), *self.misses.lock().unwrap())
    }

    fn evict(&self, key: &CacheKey) {
        self.cache.lock().unwrap().remove(key);
        self.order
            .lock()
            .unwrap()
            .retain(|existing| existing != key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_args() -> serde_json::Value {
        serde_json::json!({ "path": "test.txt" })
    }

    #[test]
    fn cache_key_is_deterministic() {
        let args = make_args();
        let first = CacheKey::compute("read_file", &args);
        let second = CacheKey::compute("read_file", &args);
        assert_eq!(first.tool_name, second.tool_name);
        assert_eq!(first.args_hash, second.args_hash);
    }

    #[test]
    fn cache_key_changes_with_arguments() {
        let first = CacheKey::compute("read_file", &serde_json::json!({ "path": "a.txt" }));
        let second = CacheKey::compute("read_file", &serde_json::json!({ "path": "b.txt" }));
        assert_ne!(first.args_hash, second.args_hash);
    }

    #[test]
    fn cache_returns_stored_value() {
        let cache = ToolResultCache::new(10, 0);
        let args = make_args();
        cache.put(
            "read_file".into(),
            args.clone(),
            ToolResult::success("read_file", "contents"),
        );
        assert_eq!(cache.get("read_file", &args).unwrap().content, "contents");
    }

    #[test]
    fn cache_records_hits_and_misses() {
        let cache = ToolResultCache::new(10, 0);
        let args = make_args();
        assert!(cache.get("read_file", &args).is_none());
        cache.put(
            "read_file".into(),
            args.clone(),
            ToolResult::success("read_file", "contents"),
        );
        assert!(cache.get("read_file", &args).is_some());
        assert_eq!(cache.hit_rate(), (1, 1));
    }

    #[test]
    fn cache_clear_removes_entries_and_statistics() {
        let cache = ToolResultCache::new(10, 0);
        let args = make_args();
        cache.put(
            "read_file".into(),
            args.clone(),
            ToolResult::success("read_file", "contents"),
        );
        assert!(cache.get("read_file", &args).is_some());
        cache.clear();
        assert!(cache.get("read_file", &args).is_none());
        assert_eq!(cache.hit_rate(), (0, 1));
    }

    #[test]
    fn cache_evicts_oldest_entry_at_capacity() {
        let cache = ToolResultCache::new(2, 0);
        let first = serde_json::json!({ "path": "a.txt" });
        let second = serde_json::json!({ "path": "b.txt" });
        let third = serde_json::json!({ "path": "c.txt" });
        for args in [&first, &second, &third] {
            cache.put(
                "read_file".into(),
                args.clone(),
                ToolResult::success("read_file", "value"),
            );
        }
        assert!(cache.get("read_file", &first).is_none());
        assert!(cache.get("read_file", &second).is_some());
        assert!(cache.get("read_file", &third).is_some());
    }

    #[test]
    fn cache_replacing_a_key_does_not_evict() {
        let cache = ToolResultCache::new(2, 0);
        let args = make_args();
        cache.put(
            "read_file".into(),
            args.clone(),
            ToolResult::success("read_file", "first"),
        );
        cache.put(
            "read_file".into(),
            args.clone(),
            ToolResult::success("read_file", "second"),
        );
        assert_eq!(cache.get("read_file", &args).unwrap().content, "second");
    }

    #[test]
    fn zero_ttl_does_not_expire() {
        let cached = CachedResult::new(ToolResult::success("test", "data"));
        assert!(!cached.is_expired(0));
    }
}
