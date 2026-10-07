//! Durable, compact attention state for the foreground Main Agent.
//!
//! This module deliberately stores only controlled identifiers and enums. Raw
//! provider/tool errors remain in their existing audit stores and can only be
//! fetched through their own access-controlled evidence references.

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

pub const ATTENTION_CARD_VERSION: u8 = 1;
pub const MAX_CONTEXT_CARDS: usize = 3;
pub const MAX_CONTEXT_BYTES: usize = 512;
const VISIBLE_ATTENTION_MESSAGE: &str =
    "有工作需要继续处理，进度已安全保留。请回到主对话说明下一步。";

/// The only Attention representation intended for the workbench.
///
/// It deliberately excludes goal, recovery, and evidence references. Those
/// references remain useful to the Main Agent's bounded context assembler, but
/// they are not a user-facing diagnostic or control surface.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VisibleAttentionSummary {
    pub open_count: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionCard {
    #[serde(rename = "v")]
    pub version: u8,
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "g")]
    pub goal_ref: String,
    #[serde(rename = "s")]
    pub status: AttentionStatus,
    #[serde(rename = "c")]
    pub category: AttentionCategory,
    #[serde(rename = "i")]
    pub impact: AttentionImpact,
    #[serde(rename = "a")]
    pub next_action: AttentionAction,
    #[serde(rename = "p")]
    pub progress: Vec<String>,
    #[serde(rename = "r", skip_serializing_if = "Option::is_none")]
    pub recovery_ref: Option<String>,
    #[serde(rename = "e")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionStatus {
    Open,
    Resolved,
    Superseded,
    Expired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionCategory {
    RuntimeFailure,
    PermissionDenied,
    ConfirmationNeeded,
    VerificationFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionImpact {
    Informational,
    Recoverable,
    Blocking,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionAction {
    Retry,
    Resume,
    AskUser,
    Inspect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CrossSessionScope {
    owner_profile_id: i64,
    workspace_key: String,
}

impl AttentionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Superseded => "superseded",
            Self::Expired => "expired",
        }
    }
}
impl AttentionCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeFailure => "runtime_failure",
            Self::PermissionDenied => "permission_denied",
            Self::ConfirmationNeeded => "confirmation_needed",
            Self::VerificationFailed => "verification_failed",
        }
    }
}
impl AttentionImpact {
    fn as_str(self) -> &'static str {
        match self {
            Self::Informational => "informational",
            Self::Recoverable => "recoverable",
            Self::Blocking => "blocking",
        }
    }
}
impl AttentionAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Resume => "resume",
            Self::AskUser => "ask_user",
            Self::Inspect => "inspect",
        }
    }
}

impl AttentionCard {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != ATTENTION_CARD_VERSION {
            return Err("unsupported attention card version".into());
        }
        if self.id.len() > 80
            || self.goal_ref.len() > 96
            || !is_short_ref(&self.id)
            || !is_short_ref(&self.goal_ref)
        {
            return Err("attention card contains an invalid identifier".into());
        }
        if self.progress.len() > 3 || self.evidence_refs.len() > 2 {
            return Err("attention card exceeds collection limits".into());
        }
        if self.progress.iter().any(|v| !is_short_ref(v))
            || self.evidence_refs.iter().any(|v| !is_short_ref(v))
            || self.recovery_ref.as_ref().is_some_and(|v| !is_short_ref(v))
        {
            return Err("attention card contains untrusted detail".into());
        }
        Ok(())
    }

    pub fn context_json(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
}

/// A deterministic, versioned visible projection. No arbitrary error text is
/// persisted: callers render this only after a durable card exists.
pub fn render_visible_projection(card: &AttentionCard) -> &'static str {
    match (card.category, card.impact, card.next_action) {
        (
            AttentionCategory::RuntimeFailure,
            AttentionImpact::Recoverable,
            AttentionAction::Retry,
        ) => "刚才的执行需要继续处理，进度已安全保留；我可以从上一步重试。",
        (_, AttentionImpact::Blocking, AttentionAction::AskUser) => {
            "这一步需要你的决定后才能继续。"
        }
        (_, _, AttentionAction::Resume) => "此前的工作可以继续，我会从已保留的进度恢复。",
        _ => "有一项工作需要关注；我会根据已保留的状态继续处理。",
    }
}

/// Loads the compact, safe Attention summary for one Workspace conversation.
///
/// This intentionally does not reuse `load_context_cards`: the latter is a
/// bounded model-context admission path, whereas the workbench must report all
/// still-open items in the permitted Workspace scope. No diagnostic evidence or
/// opaque recovery reference is read or returned here.
pub fn load_visible_summary(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<VisibleAttentionSummary>, String> {
    let scope = cross_session_scope_for_session(conn, session_id)?;
    let open_count: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM attention_states
             WHERE status = 'open'
               AND ((?1 IS NOT NULL AND scope_owner_profile_id = ?1 AND scope_workspace_key = ?2)
                 OR (?1 IS NULL AND session_id = ?3))",
            params![
                scope.as_ref().map(|scope| scope.owner_profile_id),
                scope.as_ref().map(|scope| scope.workspace_key.as_str()),
                session_id,
            ],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let open_count = usize::try_from(open_count)
        .map_err(|_| "attention state count is outside the supported range".to_string())?;
    Ok((open_count > 0).then(|| VisibleAttentionSummary {
        open_count,
        message: VISIBLE_ATTENTION_MESSAGE.to_string(),
    }))
}

pub fn record_recoverable_runtime_failure(
    conn: &Connection,
    session_id: &str,
    goal_ref: &str,
    run_id: &str,
    diagnostic_error: &str,
) -> Result<AttentionCard, String> {
    let scope = cross_session_scope_for_session(conn, session_id)?;
    let evidence_id = format!("evidence_{}", Uuid::new_v4().simple());
    let card = AttentionCard {
        version: ATTENTION_CARD_VERSION,
        id: format!("attn_{}", Uuid::new_v4().simple()),
        goal_ref: goal_ref.to_string(),
        status: AttentionStatus::Open,
        category: AttentionCategory::RuntimeFailure,
        impact: AttentionImpact::Recoverable,
        next_action: AttentionAction::Retry,
        progress: Vec::new(),
        recovery_ref: Some(format!("run:{run_id}")),
        evidence_refs: vec![format!("diag:{evidence_id}")],
    };
    card.validate()?;
    let now = Utc::now().timestamp();
    // There is intentionally no public loader for this table. The current
    // code only creates the evidence; a later authorized diagnostic surface
    // must verify both session ownership and the explicit evidence reference.
    conn.execute(
        "INSERT INTO attention_diagnostic_evidence (id, attention_id, session_id, access_scope, payload, created_at)
         VALUES (?1, ?2, ?3, 'diagnostic_evidence', ?4, ?5)",
        params![evidence_id, card.id, session_id, diagnostic_error, now],
    ).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO attention_states (id, session_id, goal_ref, status, category, impact, next_action, progress_json, recovery_ref, evidence_refs_json, scope_owner_profile_id, scope_workspace_key, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        params![
            card.id, session_id, card.goal_ref, card.status.as_str(),
            card.category.as_str(), card.impact.as_str(),
            card.next_action.as_str(), serde_json::to_string(&card.progress).unwrap(),
            card.recovery_ref, serde_json::to_string(&card.evidence_refs).unwrap(),
            scope.as_ref().map(|scope| scope.owner_profile_id),
            scope.as_ref().map(|scope| scope.workspace_key.as_str()), now
        ],
    ).map_err(|e| e.to_string())?;
    Ok(card)
}

/// Loads only cards that remain actionable and admits them in deterministic
/// recency order under the total byte budget.
pub fn load_context_cards(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<AttentionCard>, String> {
    let scope = cross_session_scope_for_session(conn, session_id)?;
    let mut stmt = conn.prepare(
        "SELECT id, goal_ref, status, category, impact, next_action, progress_json, recovery_ref, evidence_refs_json
         FROM attention_states
         WHERE status = 'open'
           AND ((?1 IS NOT NULL AND scope_owner_profile_id = ?1 AND scope_workspace_key = ?2)
             OR (?1 IS NULL AND session_id = ?3))
         ORDER BY updated_at DESC, id DESC LIMIT 16"
    ).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            params![
                scope.as_ref().map(|scope| scope.owner_profile_id),
                scope.as_ref().map(|scope| scope.workspace_key.as_str()),
                session_id,
            ],
            |row| {
                Ok(AttentionCard {
                    version: ATTENTION_CARD_VERSION,
                    id: row.get(0)?,
                    goal_ref: row.get(1)?,
                    status: serde_json::from_str::<AttentionStatus>(&format!(
                        "\"{}\"",
                        row.get::<_, String>(2)?
                    ))
                    .map_err(json_error)?,
                    category: serde_json::from_str::<AttentionCategory>(&format!(
                        "\"{}\"",
                        row.get::<_, String>(3)?
                    ))
                    .map_err(json_error)?,
                    impact: serde_json::from_str::<AttentionImpact>(&format!(
                        "\"{}\"",
                        row.get::<_, String>(4)?
                    ))
                    .map_err(json_error)?,
                    next_action: serde_json::from_str::<AttentionAction>(&format!(
                        "\"{}\"",
                        row.get::<_, String>(5)?
                    ))
                    .map_err(json_error)?,
                    progress: serde_json::from_str(&row.get::<_, String>(6)?)
                        .map_err(json_error)?,
                    recovery_ref: row.get(7)?,
                    evidence_refs: serde_json::from_str(&row.get::<_, String>(8)?)
                        .map_err(json_error)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    let mut accepted = Vec::new();
    let mut total = 0usize;
    for row in rows {
        let card = row.map_err(|e| e.to_string())?;
        let encoded = match card.context_json() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if accepted.len() == MAX_CONTEXT_CARDS || total + encoded.len() > MAX_CONTEXT_BYTES {
            continue;
        }
        total += encoded.len();
        accepted.push(card);
    }
    Ok(accepted)
}

/// Cross-session injection is allowed only when both the local profile and an
/// existing canonical workspace are known. Missing identity/workspace data is
/// deliberately a session-only fallback, never a global scope.
fn cross_session_scope_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<CrossSessionScope>, String> {
    let owner_profile_id = conn
        .query_row("SELECT id FROM profile WHERE id = 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .ok();
    let work_dir = conn
        .query_row(
            "SELECT work_dir FROM sessions WHERE id = ?1",
            params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(|error| error.to_string())?
        .filter(|value| !value.trim().is_empty());
    let workspace_key = work_dir.and_then(|value| canonical_workspace_key(&value));
    Ok(owner_profile_id
        .zip(workspace_key)
        .map(|(owner_profile_id, workspace_key)| CrossSessionScope {
            owner_profile_id,
            workspace_key,
        }))
}

fn canonical_workspace_key(work_dir: &str) -> Option<String> {
    let canonical = std::fs::canonicalize(Path::new(work_dir)).ok()?;
    if !canonical.is_dir() {
        return None;
    }
    let key = canonical.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let key = key.to_ascii_lowercase();
    Some(key)
}

pub fn render_context_cards(cards: &[AttentionCard]) -> Result<String, String> {
    let selected: Vec<String> = cards
        .iter()
        .take(MAX_CONTEXT_CARDS)
        .map(AttentionCard::context_json)
        .collect::<Result<_, _>>()?;
    let rendered = format!(
        "Open attention cards (structured only; follow refs only when authorized): [{}]",
        selected.join(",")
    );
    if rendered.len() > MAX_CONTEXT_BYTES + 100 {
        return Err("attention context exceeds budget".into());
    }
    Ok(rendered)
}

fn is_short_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.'))
}
fn json_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seed_profile(conn: &Connection) {
        conn.execute(
            "INSERT OR IGNORE INTO profile (id, updated_at) VALUES (1, 0)",
            [],
        )
        .unwrap();
    }
    fn seed_session(conn: &Connection, id: &str, work_dir: Option<&Path>) {
        conn.execute("INSERT INTO sessions (id, title, created_at, updated_at, work_dir) VALUES (?1, 'test', 0, 0, ?2)", params![id, work_dir.map(|path| path.to_string_lossy().to_string())]).unwrap();
    }
    #[test]
    fn context_caps_and_sensitive_detail_are_excluded() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        seed_profile(&conn);
        seed_session(&conn, "session-test", Some(workspace.path()));
        for i in 0..5 {
            record_recoverable_runtime_failure(
                &conn,
                "session-test",
                &format!("goal-{i}"),
                &format!("run-{i}"),
                "stack secret C:\\work",
            )
            .unwrap();
        }
        let cards = load_context_cards(&conn, "session-test").unwrap();
        assert!(cards.len() <= MAX_CONTEXT_CARDS);
        assert!(cards
            .iter()
            .all(|card| !card.context_json().unwrap().contains("stack")));
        assert!(AttentionCard {
            progress: vec!["C:\\secret".into()],
            ..cards[0].clone()
        }
        .validate()
        .is_err());
    }
    #[test]
    fn template_is_deterministic_and_never_uses_raw_error() {
        let card = AttentionCard {
            version: 1,
            id: "attn_x".into(),
            goal_ref: "goal_x".into(),
            status: AttentionStatus::Open,
            category: AttentionCategory::RuntimeFailure,
            impact: AttentionImpact::Recoverable,
            next_action: AttentionAction::Retry,
            progress: vec![],
            recovery_ref: Some("run:x".into()),
            evidence_refs: vec!["audit:x".into()],
        };
        assert_eq!(
            render_visible_projection(&card),
            render_visible_projection(&card)
        );
        assert!(!render_visible_projection(&card).contains("secret"));
    }

    #[test]
    fn visible_summary_reports_all_open_cards_without_diagnostic_or_control_refs() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        seed_profile(&conn);
        seed_session(&conn, "session-visible", Some(workspace.path()));
        for index in 0..4 {
            record_recoverable_runtime_failure(
                &conn,
                "session-visible",
                &format!("message-{index}"),
                &format!("run-{index}"),
                "provider diagnostic secret",
            )
            .unwrap();
        }

        let summary = load_visible_summary(&conn, "session-visible")
            .unwrap()
            .expect("open cards should be visible to the workbench");

        assert_eq!(summary.open_count, 4);
        assert_eq!(summary.message, VISIBLE_ATTENTION_MESSAGE);
        let encoded = serde_json::to_string(&summary).unwrap();
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("message-"));
        assert!(!encoded.contains("run-"));
    }

    #[test]
    fn durable_failure_creates_open_card_without_terminal_completion() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        seed_profile(&conn);
        seed_session(&conn, "session-failure", Some(workspace.path()));
        let card = record_recoverable_runtime_failure(
            &conn,
            "session-failure",
            "message-1",
            "run-1",
            "provider error secret",
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attention_states WHERE id = ?1 AND status = 'open'",
                params![card.id],
                |row| row.get(0),
            )
            .unwrap();
        let terminal_events: i64 = conn.query_row("SELECT COUNT(*) FROM agent_run_events WHERE run_id = 'run-1' AND event_type IN ('agent_end', 'turn_end')", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
        assert_eq!(terminal_events, 0);
        let diagnostic: String = conn
            .query_row(
                "SELECT payload FROM attention_diagnostic_evidence WHERE attention_id = ?1",
                params![card.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(diagnostic, "provider error secret");
    }

    #[test]
    fn same_profile_and_workspace_cards_cross_session_but_never_cross_scope() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let other_workspace = tempfile::tempdir().unwrap();
        seed_profile(&conn);
        seed_session(&conn, "same-a", Some(workspace.path()));
        seed_session(&conn, "same-b", Some(workspace.path()));
        seed_session(&conn, "other-workspace", Some(other_workspace.path()));
        let included =
            record_recoverable_runtime_failure(&conn, "same-a", "goal-a", "run-a", "secret-a")
                .unwrap();
        let excluded_workspace = record_recoverable_runtime_failure(
            &conn,
            "other-workspace",
            "goal-b",
            "run-b",
            "secret-b",
        )
        .unwrap();
        let superseded =
            record_recoverable_runtime_failure(&conn, "same-a", "goal-c", "run-c", "secret-c")
                .unwrap();
        conn.execute(
            "UPDATE attention_states SET status = 'resolved' WHERE id = ?1",
            params![excluded_workspace.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE attention_states SET status = 'superseded' WHERE id = ?1",
            params![superseded.id],
        )
        .unwrap();
        let cards = load_context_cards(&conn, "same-b").unwrap();
        assert!(cards.iter().any(|card| card.id == included.id));
        assert!(cards.iter().all(|card| card.id != excluded_workspace.id));
        assert!(cards.iter().all(|card| card.id != superseded.id));
        assert!(cards
            .iter()
            .all(|card| !card.context_json().unwrap().contains("secret")));
        assert_eq!(
            load_visible_summary(&conn, "same-b")
                .unwrap()
                .expect("same Workspace open card should be summarized")
                .open_count,
            1
        );
    }

    #[test]
    fn different_profile_and_legacy_scope_do_not_cross_session() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        seed_profile(&conn);
        seed_session(&conn, "origin", Some(workspace.path()));
        seed_session(&conn, "other", Some(workspace.path()));
        let different_profile =
            record_recoverable_runtime_failure(&conn, "origin", "goal-a", "run-a", "secret-a")
                .unwrap();
        conn.execute(
            "UPDATE attention_states SET scope_owner_profile_id = 2 WHERE id = ?1",
            params![different_profile.id],
        )
        .unwrap();
        let cards = load_context_cards(&conn, "other").unwrap();
        assert!(cards.iter().all(|card| card.id != different_profile.id));

        conn.execute("DELETE FROM profile WHERE id = 1", [])
            .unwrap();
        let legacy =
            record_recoverable_runtime_failure(&conn, "origin", "goal-b", "run-b", "secret-b")
                .unwrap();
        let cards = load_context_cards(&conn, "other").unwrap();
        assert!(cards.iter().all(|card| card.id != legacy.id));
        let origin_cards = load_context_cards(&conn, "origin").unwrap();
        assert!(origin_cards.iter().any(|card| card.id == legacy.id));
    }
}
