//! Durable interaction defaults with bounded, source-backed evidence.
//!
//! This module is the seam for adaptive constraints: callers either record
//! bounded preference evidence or read the active set for a session. Persona
//! stays separate. Explicit statements reuse this store without manufacturing
//! repeated observations; neither type grants action or permission authority.

use crate::agent::config::AdaptiveConstraint;
use crate::agent::evolution::NewMemory;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

const MIN_EVIDENCE_COUNT: i32 = 2;
const MIN_CONFIDENCE: f64 = 0.7;
const LEASE_SECONDS: i64 = 90 * 24 * 60 * 60;
/// A resend, edit-and-resend, or copied message can trigger evolution more
/// than once while describing the exact same preference. It is still one
/// observation, not independent evidence.
const EVIDENCE_DEDUP_WINDOW_SECONDS: i64 = 24 * 60 * 60;

// Delayed reviews and imported history must not make an old statement look
// newer than a correction. Derive order from existing original evidence,
// including for records written before this rule; never rewrite the archive.
const PREFERENCE_SOURCES: &str = "WITH preference_sources AS (
    SELECT e.constraint_key, m.created_at, m.rowid AS source_order,
      ROW_NUMBER() OVER (PARTITION BY e.constraint_key ORDER BY m.created_at DESC,m.rowid DESC) AS rank
    FROM adaptive_constraint_evidence e JOIN messages m
      ON m.id=substr(e.source,length('explicit_user:')+1) AND e.session_id=m.session_id
    WHERE e.source LIKE 'explicit_user:%' AND m.role='user' AND m.is_provisional=0
)";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnedPreference {
    pub id: String,
    pub content: String,
    pub ready: bool,
    pub explicit: bool,
    pub updated_at: i64,
}

pub(crate) struct PreferenceLookup {
    pub records: Vec<LearnedPreference>,
    /// Candidate relevance only; this never changes the record's provenance.
    pub partial: bool,
}

/// Owner-facing recent habits, including observations that have not yet
/// accumulated enough independent evidence to influence a reply.
pub(crate) fn learned_preferences(conn: &Connection) -> Result<Vec<LearnedPreference>, String> {
    find_learned_preferences(conn, "", 50).map(|lookup| lookup.records)
}

/// Owner-requested lookup must filter before the recent-list cutoff. Stream
/// matches instead of copying the whole archive; keep Unicode matching in Rust
/// because SQLite lower() only folds ASCII without an additional extension.
pub(crate) fn find_learned_preferences(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<PreferenceLookup, String> {
    find_preferences(conn, query, limit, true)
}

/// Default recall includes user-stated preferences; inferred observations are
/// a separate opt-in. Filter before any recent-list or relevance cutoff.
pub(crate) fn find_preferences(
    conn: &Connection,
    query: &str,
    limit: usize,
    include_observed: bool,
) -> Result<PreferenceLookup, String> {
    lookup_preferences(conn, query, limit, include_observed, true)
}

fn lookup_preferences(
    conn: &Connection,
    query: &str,
    limit: usize,
    include_observed: bool,
    allow_partial: bool,
) -> Result<PreferenceLookup, String> {
    let query = query.trim().to_lowercase();
    let limit = limit.clamp(1, 50);
    let mut stmt = conn
        .prepare(&format!("{PREFERENCE_SOURCES}
         SELECT c.id, c.constraint_value, c.source='explicit_user_preference' OR (c.evidence_count >= ?1 AND c.confidence >= ?2),
           COALESCE(s.created_at,c.updated_at),c.source='explicit_user_preference'
         FROM adaptive_constraints c LEFT JOIN preference_sources s ON s.constraint_key=c.constraint_key AND s.rank=1
         WHERE c.scope='global' AND c.status='active' AND c.constraint_key LIKE 'interaction_preference:%'
           AND c.constraint_value != '' AND (c.expires_at IS NULL OR c.expires_at > ?3)
           AND (?5=1 OR c.source='explicit_user_preference')
         ORDER BY c.source='explicit_user_preference' DESC, COALESCE(s.created_at,c.updated_at) DESC,
           s.source_order DESC,c.id LIMIT ?4"))
        .map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();
    let row_limit = if query.is_empty() { limit as i64 } else { -1 };
    let parameters = params![
        MIN_EVIDENCE_COUNT,
        MIN_CONFIDENCE,
        now,
        row_limit,
        include_observed
    ];
    let decode = |r: &rusqlite::Row<'_>| {
        Ok(LearnedPreference {
            id: r.get(0)?,
            content: r.get(1)?,
            ready: r.get(2)?,
            updated_at: r.get(3)?,
            explicit: r.get(4)?,
        })
    };
    let rows = stmt
        .query_map(parameters, decode)
        .map_err(|e| e.to_string())?;
    if query.is_empty() {
        return Ok(PreferenceLookup {
            records: rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?,
            partial: false,
        });
    }
    let matcher = crate::fts::MemoryQuery::new(&query);
    let mut matched = Vec::with_capacity(limit + 1);
    let mut frequencies = std::collections::HashMap::<char, usize>::new();
    let mut total = 0;
    for row in rows {
        let preference = row.map_err(|e| e.to_string())?;
        total += 1;
        let literal = preference.content.to_lowercase().contains(&query);
        let lexical = matcher.direct_score(&preference.content);
        if literal || lexical.is_some() {
            matched.push((literal, lexical.unwrap_or(0), preference));
            // Bounded top results; stable ties keep the SQL source-time order.
            matched.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
            matched.truncate(limit);
        } else if allow_partial && matched.is_empty() {
            for character in matcher.shared_characters(&preference.content) {
                *frequencies.entry(character).or_default() += 1;
            }
        }
    }
    if !matched.is_empty() || !allow_partial {
        return Ok(PreferenceLookup {
            records: matched.into_iter().map(|(_, _, p)| p).collect(),
            partial: false,
        });
    }
    // Explicit inspection only: if direct matching found nothing, surface a
    // bounded set of partial lexical candidates, not a semantic assertion.
    // Common single characters cannot flood out an older distinctive quote.
    let mut partial = Vec::with_capacity(limit + 1);
    frequencies.retain(|_, count| *count == 1 || *count <= total / 2);
    if !frequencies.is_empty() {
        for row in stmt
            .query_map(parameters, decode)
            .map_err(|e| e.to_string())?
        {
            let preference = row.map_err(|e| e.to_string())?;
            let score: f64 = matcher
                .shared_characters(&preference.content)
                .filter_map(|c| frequencies.get(&c))
                .map(|&count| 1.0 / count as f64)
                .sum();
            if score > 0.0 {
                partial.push((score, preference));
                partial.sort_by(|a, b| b.0.total_cmp(&a.0));
                partial.truncate(limit);
            }
        }
    }
    Ok(PreferenceLookup {
        partial: !partial.is_empty(),
        records: partial.into_iter().map(|(_, p)| p).collect(),
    })
}

/// Remove a learned habit from future adaptation. Keep only its normalized
/// hash as a disabled marker so an in-flight review cannot teach it back.
/// Explicit user memories and the original conversation are not touched.
pub(crate) fn dismiss_learned_preference(conn: &Connection, id: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("请选择要停止沿用的习惯".into());
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let key: Option<String> = tx
        .query_row(
            "SELECT constraint_key FROM adaptive_constraints
         WHERE id=?1 AND scope='global' AND constraint_key LIKE 'interaction_preference:%'",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let key = key.ok_or("这条习惯记录已不存在")?;
    tx.execute(
        "UPDATE adaptive_constraints SET status='disabled', constraint_value='',
         confidence=0, evidence_count=1, source='user_dismissed', expires_at=NULL, updated_at=?2 WHERE id=?1",
        params![id, Utc::now().timestamp()],
    ).map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM adaptive_constraint_evidence WHERE constraint_key=?1",
        [key],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn record_preferences(
    conn: &Connection,
    session_id: Option<&str>,
    memories: &[NewMemory],
    source: &str,
) -> Result<usize, String> {
    let now = Utc::now().timestamp();
    let expires_at = now + LEASE_SECONDS;
    let mut stored = 0;

    // Expired interaction defaults are not permanent user memories. Bound
    // their evidence retention and prevent a fresh observation from reviving
    // an expired aggregate with its old confidence/count.
    conn.execute(
        "DELETE FROM adaptive_constraint_evidence WHERE observed_at <= ?1 AND source NOT LIKE 'explicit_user:%'",
        [now - LEASE_SECONDS],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM adaptive_constraints WHERE status != 'disabled' AND expires_at IS NOT NULL AND expires_at <= ?1",
        [now],
    )
    .map_err(|e| e.to_string())?;

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
        // Importance describes usefulness, not reliability. Even a lightweight
        // habit can be corroborated; one observation remains tentative.
        let confidence = 0.4;
        let changed = conn
            .execute(
                "INSERT INTO adaptive_constraints
             (id, scope, session_id, constraint_key, constraint_value, confidence,
              evidence_count, source, status, created_at, updated_at, expires_at)
             VALUES (?1, 'global', NULL, ?2, ?3, ?4, 1, ?5, 'active', ?6, ?6, ?7)
             ON CONFLICT DO UPDATE SET
               evidence_count = adaptive_constraints.evidence_count + 1,
               confidence = MAX(adaptive_constraints.confidence, ?8),
               source = CASE WHEN adaptive_constraints.source='explicit_user_preference' THEN adaptive_constraints.source ELSE excluded.source END,
               status = 'active',
               updated_at = CASE WHEN adaptive_constraints.source='explicit_user_preference' THEN adaptive_constraints.updated_at ELSE excluded.updated_at END,
               expires_at = CASE WHEN adaptive_constraints.source='explicit_user_preference' THEN NULL ELSE excluded.expires_at END
             WHERE adaptive_constraints.status != 'disabled'",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    key,
                    value,
                    confidence,
                    source,
                    now,
                    expires_at,
                    MIN_CONFIDENCE
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            continue;
        }
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

/// One bounded working projection, not the complete preference archive. Count
/// after scope/readiness/expiry filtering in the same query as the selected rows.
#[derive(Default)]
pub(crate) struct PreferenceProjection {
    pub records: Vec<AdaptiveConstraint>,
    pub omitted: usize,
    pub task_matches: usize,
}

/// A task view uses the same store, cutoff and authorization-neutral defaults.
/// Query matches may replace only global interaction-preference slots; local
/// agreements and other rules retain their exact positions. No semantic ruling.
pub(crate) fn project_for_task(
    conn: &Connection,
    session_id: &str,
    query: &str,
) -> Result<PreferenceProjection, String> {
    let snapshot = if conn.is_autocommit() {
        Some(conn.unchecked_transaction().map_err(|e| e.to_string())?)
    } else {
        None // Participate in the caller's existing snapshot, never nest BEGIN.
    };
    let mut view = project_for_session(conn, session_id)?;
    let replaceable = |r: &AdaptiveConstraint| {
        r.scope == "global" && r.key.starts_with("interaction_preference:")
    };
    let slots = view
        .records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| replaceable(r).then_some(i))
        .collect::<Vec<_>>();
    if view.omitted == 0 || slots.is_empty() || query.trim().is_empty() {
        return Ok(view);
    }
    // Automatic task context never consumes partial inspection candidates.
    // Skip their character counting and second scan, rather than compute/drop.
    let found = lookup_preferences(conn, query, 50, false, false)?;
    let mut candidates = Vec::new();
    for p in found.records.into_iter().filter(|p| p.ready && p.explicit) {
        // Promotion needs a still-existing verbatim user source, not just an
        // explicit label. A changed/deleted source cannot make an old default new.
        let record = conn.query_row("SELECT c.constraint_key,c.constraint_value,c.scope,c.confidence,MAX(m.created_at)
            FROM adaptive_constraints c JOIN adaptive_constraint_evidence e ON e.constraint_key=c.constraint_key
            JOIN messages m ON e.source='explicit_user:'||m.id AND e.session_id=m.session_id
            WHERE c.id=?1 AND m.role='user' AND m.is_provisional=0 AND instr(m.content,c.constraint_value)>0
            GROUP BY c.id",[p.id],|r|Ok((AdaptiveConstraint {key:r.get(0)?,value:r.get(1)?,scope:r.get(2)?,confidence:r.get(3)?,explicit:true},r.get::<_,i64>(4)?)))
            .optional().map_err(|e|format!("交流偏好来源读取失败: {e}"))?;
        if let Some(record) = record {
            candidates.push(record);
        }
    }
    let newest = candidates
        .iter()
        .enumerate()
        .max_by_key(|(i, (_, time))| (*time, std::cmp::Reverse(*i)))
        .map(|(i, _)| i);
    let mut selected = candidates
        .into_iter()
        .enumerate()
        .filter(|(i, _)| *i == 0 || Some(*i) == newest)
        .map(|(_, r)| r)
        .collect::<Vec<_>>();
    selected.sort_by(|a, b| b.1.cmp(&a.1));
    selected.truncate(slots.len());
    view.task_matches = selected.len();
    let mut records = selected.into_iter().map(|(r, _)| r).collect::<Vec<_>>();
    for r in view.records.iter().filter(|r| replaceable(r)) {
        if !records.iter().any(|existing| existing.key == r.key) {
            records.push(r.clone());
        }
    }
    for (i, r) in slots.into_iter().zip(records) {
        view.records[i] = r;
    }
    if let Some(snapshot) = snapshot {
        snapshot.commit().map_err(|e| e.to_string())?;
    }
    Ok(view)
}

pub(crate) fn project_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<PreferenceProjection, String> {
    let now = Utc::now().timestamp();
    let mut stmt = conn.prepare(&format!("{PREFERENCE_SOURCES}
         SELECT c.constraint_key, c.constraint_value, c.scope, c.confidence,c.source='explicit_user_preference',COUNT(*) OVER ()
         FROM adaptive_constraints c LEFT JOIN preference_sources s ON s.constraint_key=c.constraint_key AND s.rank=1
         WHERE c.status = 'active'
           AND (c.source='explicit_user_preference' OR (c.evidence_count >= ?1 AND c.confidence >= ?2))
           AND (c.expires_at IS NULL OR c.expires_at > ?3)
           AND (c.scope = 'global' OR (c.scope = 'session' AND c.session_id = ?4))
         ORDER BY c.source='explicit_user_preference' DESC, COALESCE(s.created_at,c.updated_at) DESC,
           s.source_order DESC,c.confidence DESC,c.constraint_key
         LIMIT 12")).map_err(|e|format!("无法读取交流偏好: {e}"))?;

    let rows = stmt
        .query_map(
            params![MIN_EVIDENCE_COUNT, MIN_CONFIDENCE, now, session_id],
            |row| {
                Ok((
                    AdaptiveConstraint {
                        key: row.get(0)?,
                        value: row.get(1)?,
                        scope: row.get(2)?,
                        confidence: row.get(3)?,
                        explicit: row.get(4)?,
                    },
                    row.get::<_, usize>(5)?,
                ))
            },
        )
        .map_err(|e| format!("无法读取交流偏好: {e}"))?;
    let mut projection = PreferenceProjection::default();
    let mut total = 0;
    for row in rows {
        let (record, count) = row.map_err(|e| format!("交流偏好记录无效: {e}"))?;
        projection.records.push(record);
        total = count;
    }
    projection.omitted = total.saturating_sub(projection.records.len());
    Ok(projection)
}

// Record-only view for existing test/benchmark fixtures. The production caller
// must carry omission metadata rather than silently treating this as an archive.
#[cfg(test)]
pub(crate) fn active_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<AdaptiveConstraint>, String> {
    project_for_session(conn, session_id).map(|view| view.records)
}

// Normalize only typography for known control/scheduling phrases, never the
// stored original wording or the model's semantic preference classification.
fn marker_text(text: &str) -> String {
    text.to_ascii_lowercase().replace(['‘', '’', '＇'], "'")
}

pub(crate) fn forbids_preference_learning(text: &str) -> bool {
    let text = marker_text(text);
    [
        "不要记住",
        "不要保存",
        "别记住",
        "不用记住",
        "不要学习",
        "别保存",
        "不需要记住",
        "do not remember",
        "don't remember",
        "do not save",
        "don't save",
        "do not store",
        "don't store",
        "do not learn",
    ]
    .iter()
    .any(|p| text.contains(p))
}

/// Conservative scheduling hint, not evidence or authority to save anything.
pub(crate) fn has_explicit_preference_cue(text: &str) -> bool {
    let text = marker_text(text);
    [
        "以后",
        "今后",
        "往后",
        "每次",
        "一直",
        "请记住",
        "我喜欢",
        "我不喜欢",
        "我偏好",
        "叫我",
        "我不想聊",
        "from now on",
        "in future",
        "i prefer",
        "i like",
        "i don't like",
        "i do not like",
        "i dislike",
        "i avoid",
        "i refuse to",
        "call me",
        "please remember",
    ]
    .iter()
    .any(|p| text.contains(p))
}

/// Scheduling only, never a semantic ruling or permission to change a record.
/// Obvious revisions of a personal default may need the existing reviewer
/// before the ordinary learning cooldown expires.
pub(crate) fn has_preference_revision_cue(text: &str) -> bool {
    let text = marker_text(text);
    [
        "偏好",
        "习惯",
        "称呼",
        "常用",
        "默认",
        "preference",
        "default",
        "call me",
    ]
    .iter()
    .any(|p| text.contains(p))
        && [
            "更正",
            "改为",
            "改成",
            "改选",
            "不再",
            "撤回",
            "取消",
            "correction",
            "instead",
            "no longer",
            "withdraw",
            "change",
        ]
        .iter()
        .any(|p| text.contains(p))
}

/// Receives a direct-statement nomination from the semantic review, not a
/// scheduling keyword match. Reuses the habit store without manufacturing a
/// second observation; the original row and scope are checked transactionally.
pub(crate) fn record_explicit_preference(
    conn: &Connection,
    session: &str,
    message: &str,
    content: &str,
    quote: &str,
) -> Result<bool, String> {
    let Some(candidate) = normalize_preference(content) else {
        return Ok(false);
    };
    let original:Option<Option<String>>=conn.query_row("SELECT CASE WHEN length(content)<=1000 THEN content END FROM messages WHERE id=?1 AND session_id=?2 AND role='user' AND is_provisional=0",params![message,session],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let Some(Some(original)) = original else {
        return Ok(false);
    };
    let quote = quote.trim();
    let Some(normalized_quote) = normalize_preference(quote) else {
        return Ok(false);
    };
    let lowered = format!("{original}\n{candidate}").to_ascii_lowercase();
    if quote.chars().count() < 4
        || quote.chars().count() > 160
        || !original.contains(quote)
        || forbids_preference_learning(&original)
        || [
            "本项目",
            "该项目",
            "这个项目",
            "当前项目",
            "本仓库",
            "这个仓库",
            "这个任务",
            "该任务",
            "本轮",
            "这次",
            "本次",
            "当前任务",
            "临时",
            "仅这",
            "for this task",
            "this project",
            "this repository",
            "for now",
            "temporar",
            "权限",
            "授权",
            "确认",
            "执行",
            "打开",
            "点击",
            "运行",
            "提醒",
            "自动",
            "联网",
            "自动化",
            "文件",
            "命令",
            "购买",
            "发送",
            "permission",
            "approval",
            "authorize",
            "execute",
            "click",
            "remind",
            "schedule",
            "ignore previous",
            "```",
            "示例",
            "举例",
            "文档说",
            "文档里",
            "转述",
            "他说",
            "她说",
            "引用",
            "example",
            "quoted",
            "密码",
            "密钥",
            "身份证",
            "病史",
            "诊断",
            "创伤",
        ]
        .iter()
        .any(|p| lowered.contains(p))
    {
        return Ok(false);
    }
    // Keep exact user wording, not a potentially lossy extractor paraphrase.
    let value = quote;
    let key = format!(
        "interaction_preference:{}",
        preference_hash(&normalized_quote)
    );
    let now = Utc::now().timestamp();
    let changed=conn.execute("INSERT INTO adaptive_constraints(id,scope,session_id,constraint_key,constraint_value,confidence,evidence_count,source,status,created_at,updated_at,expires_at)
        VALUES(?1,'global',NULL,?2,?3,1,1,'explicit_user_preference','active',?4,?4,NULL)
        ON CONFLICT DO UPDATE SET constraint_value=excluded.constraint_value,confidence=1,source=excluded.source,status='active',updated_at=excluded.updated_at,expires_at=NULL
        WHERE adaptive_constraints.status!='disabled'",params![uuid::Uuid::new_v4().to_string(),key,value,now]).map_err(|e|e.to_string())?;
    if changed == 0 {
        return Ok(false);
    };
    let source = format!("explicit_user:{message}");
    conn.execute("INSERT INTO adaptive_constraint_evidence(id,constraint_key,session_id,source,observed_at) SELECT ?1,?2,?3,?4,?5 WHERE NOT EXISTS(SELECT 1 FROM adaptive_constraint_evidence WHERE constraint_key=?2 AND session_id=?3 AND source=?4)",params![uuid::Uuid::new_v4().to_string(),key,session,source,now]).map_err(|e|e.to_string())?;
    Ok(true)
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
