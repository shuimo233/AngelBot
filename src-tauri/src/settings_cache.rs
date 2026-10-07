//! Shared settings cache — avoids repeated DB queries for frequently-read settings.
//!
//! On startup all settings are loaded into an RwLock<HashMap>.
//! `get_setting()` reads from cache first, falls back to DB on miss.
//! `save_setting()` writes DB then updates cache.

use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::RwLock;

/// Thread-safe settings cache.
pub type SettingsCache = RwLock<HashMap<String, String>>;

/// Create a new empty settings cache.
pub fn new_cache() -> SettingsCache {
    RwLock::new(HashMap::new())
}

/// Load all settings from DB into the cache. Called once at startup.
pub fn load_all(conn: &Connection, cache: &SettingsCache) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT key, value FROM settings")
        .map_err(|e| e.to_string())?;

    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut map = cache.write().map_err(|e| e.to_string())?;
    for (k, v) in rows {
        map.insert(k, v);
    }
    Ok(())
}

/// Read a setting value. Checks cache first, falls back to DB query.
/// If the key is not found anywhere, returns `default`.
pub fn get_setting(conn: &Connection, cache: &SettingsCache, key: &str, default: &str) -> String {
    // Try cache first
    if let Ok(map) = cache.read() {
        if let Some(v) = map.get(key) {
            return v.clone();
        }
    }

    // Fall back to DB
    let value = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| default.to_string());

    // Populate cache for next time
    if let Ok(mut map) = cache.write() {
        map.insert(key.to_string(), value.clone());
    }

    value
}

/// Write a setting to DB and update the cache.
pub fn save_setting(
    conn: &Connection,
    cache: &SettingsCache,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
        params![key, value, now],
    )
    .map_err(|e| e.to_string())?;

    // Update cache
    if let Ok(mut map) = cache.write() {
        map.insert(key.to_string(), value.to_string());
    }

    Ok(())
}

/// Read an integer setting with a default value.
pub fn get_setting_int(conn: &Connection, cache: &SettingsCache, key: &str, default: i32) -> i32 {
    get_setting(conn, cache, key, &default.to_string())
        .parse()
        .unwrap_or(default)
}
