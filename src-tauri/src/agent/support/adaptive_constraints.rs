//! Durable, low-priority interaction defaults learned from repeated evidence.
//!
//! This module is the seam for adaptive constraints: callers either record
//! bounded preference evidence or read the active set for a session. Persona
//! and explicit user preferences stay outside this module by design.

use crate::agent::config::AdaptiveConstraint;
use crate::agent::evolution::NewMemory;
use chrono::Utc;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

const MIN_EVIDENCE_COUNT: i32 = 2;
const MIN_CONFIDENCE: f64 = 0.7;
const LEASE_SECONDS: i64 = 90 * 24 * 60 * 60;
/// A resend, edit-and-resend, or copied message can trigger evolution more
/// than once while describing the exact same preference. It is still one
/// observation, not independent evidence.
const EVIDENCE_DEDUP_WINDOW_SECONDS: i64 = 24 * 60 * 60;

pub(crate) fn record_preferences(
    conn: &Connection,
    session_id: Option<&str>,
    memories: &[NewMemory],
    source: &str,
) -> Result<usize, String> {
    let now = Utc::now().timestamp();
    let expires_at = now + LEASE_SECONDS;
    let mut stored = 0;

    for memory in memories {
        if !memory.category.eq_ignore_ascii_case("preference") {
            continue;
        }
        let Some(value) = normalize_preference(&memory.content) else {
            continue;
        };
        let key = format!("interaction_preference:{}", preference_hash(&value));
        if recently_observed_in_session(conn, &key, session_id, now)? {
            continue;
        }
        let confidence = (memory.importance.clamp(1, 10) as f64 / 10.0).max(0.4);
        conn.execute(
            "INSERT INTO adaptive_constraints
             (id, scope, session_id, constraint_key, constraint_value, confidence,
              evidence_count, source, status, created_at, updated_at, expires_at)
             VALUES (?1, 'global', NULL, ?2, ?3, ?4, 1, ?5, 'active', ?6, ?6, ?7)
             ON CONFLICT DO UPDATE SET
               evidence_count = adaptive_constraints.evidence_count + 1,
               confidence = MAX(adaptive_constraints.confidence, excluded.confidence),
               source = excluded.source,
               status = 'active',
               updated_at = excluded.updated_at,
               expires_at = excluded.expires_at",
            params![
                uuid::Uuid::new_v4().to_string(),
                key,
                value,
                confidence,
                source,
                now,
                expires_at
            ],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO adaptive_constraint_evidence
             (id, constraint_key, session_id, source, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                uuid::Uuid::new_v4().to_string(),
                key,
                session_id,
                source,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        stored += 1;
    }
    Ok(stored)
}

/// Suppress duplicate evidence only within one conversation and a bounded
/// time window. The same preference in a later conversation, or after the
/// window has elapsed, remains an independent observation.
fn recently_observed_in_session(
    conn: &Connection,
    constraint_key: &str,
    session_id: Option<&str>,
    now: i64,
) -> Result<bool, String> {
    let Some(session_id) = session_id else {
        return Ok(false);
    };
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM adaptive_constraint_evidence
           WHERE constraint_key = ?1 AND session_id = ?2
             AND observed_at > ?3
         )",
        params![
            constraint_key,
            session_id,
            now - EVIDENCE_DEDUP_WINDOW_SECONDS
        ],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

pub(crate) fn active_for_session(conn: &Connection, session_id: &str) -> Vec<AdaptiveConstraint> {
    let now = Utc::now().timestamp();
    let mut stmt = match conn.prepare(
        "SELECT constraint_key, constraint_value, scope, confidence
         FROM adaptive_constraints
         WHERE status = 'active'
           AND evidence_count >= ?1
           AND confidence >= ?2
           AND (expires_at IS NULL OR expires_at > ?3)
           AND (scope = 'global' OR (scope = 'session' AND session_id = ?4))
         ORDER BY scope = 'session' DESC, confidence DESC, updated_at DESC
         LIMIT 12",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };

    stmt.query_map(
        params![MIN_EVIDENCE_COUNT, MIN_CONFIDENCE, now, session_id],
        |row| {
            Ok(AdaptiveConstraint {
                key: row.get(0)?,
                value: row.get(1)?,
                scope: row.get(2)?,
                confidence: row.get(3)?,
            })
        },
    )
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub(crate) fn normalize_preference(content: &str) -> Option<String> {
    let value = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() || value.chars().count() > 160 {
        return None;
    }
    let lowered = value.to_ascii_lowercase();
    [
        "ignore previous",
        "system prompt",
        "developer message",
        "tool call",
    ]
    .iter()
    .all(|needle| !lowered.contains(needle))
    .then_some(value)
}

fn preference_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"adaptive:preference:");
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}
