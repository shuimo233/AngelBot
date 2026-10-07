//! Durable, attributable understanding for one Main-Agent task.
//!
//! This module deliberately describes intent and uncertainty.  It is not a
//! planner and never authorizes a tool call, capability lease, or write.

use rusqlite::{params, Connection};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

const MAX_RECORDS_IN_CONTEXT: usize = 12;
const MAX_BODY_CHARS: usize = 600;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnderstandingKind {
    Goal,
    Constraint,
    EnvironmentFact,
    Assumption,
    OpenQuestion,
    AcceptanceCondition,
    Feedback,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnderstandingSource {
    User,
    Environment,
    Agent,
    Outcome,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnderstandingScope {
    Turn,
    Task,
    Workspace,
    Personal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnderstandingStatus {
    Active,
    Confirmed,
    Superseded,
    Resolved,
    Deferred,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskUnderstandingRecord {
    pub id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub kind: UnderstandingKind,
    pub source: UnderstandingSource,
    pub scope: UnderstandingScope,
    pub status: UnderstandingStatus,
    pub body: String,
    #[serde(default)]
    pub affects: Vec<String>,
    #[serde(default)]
    pub replaces_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewTaskUnderstandingRecord {
    pub kind: UnderstandingKind,
    pub source: UnderstandingSource,
    pub scope: UnderstandingScope,
    pub status: UnderstandingStatus,
    pub body: String,
    #[serde(default)]
    pub affects: Vec<String>,
    #[serde(default)]
    pub replaces_id: Option<String>,
}

/// The only task-understanding projection intended for a model turn.  It is
/// bounded and keeps provenance alongside text so an assumption cannot look
/// like a user decision after compaction.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaskUnderstandingProjection {
    pub record_ids: Vec<String>,
    pub records: Vec<TaskUnderstandingRecord>,
}

/// A non-authoritative recommendation for how the Main Agent should reduce a
/// relevant uncertainty.  The recommendation cannot run a tool or bypass a
/// confirmation; it only tells the existing loop whether user input is likely
/// necessary before the next action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InformationAcquisitionAction {
    Inspect,
    Ask,
    ActWithAssumption,
    Defer,
}

impl TaskUnderstandingProjection {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Deep persistence module for understanding records.  Callers supply a
/// workspace/session identity and a validated update; they never formulate
/// storage queries or rebuild a context projection themselves.
pub struct TaskUnderstanding;

impl TaskUnderstanding {
    pub fn apply_update(
        conn: &Connection,
        workspace_id: &str,
        session_id: &str,
        update: NewTaskUnderstandingRecord,
        now: i64,
    ) -> Result<TaskUnderstandingRecord, String> {
        validate_update(&update)?;
        let record = TaskUnderstandingRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.to_owned(),
            session_id: session_id.to_owned(),
            kind: update.kind,
            source: update.source,
            scope: update.scope,
            status: update.status,
            body: update.body.trim().to_owned(),
            affects: update.affects,
            replaces_id: update.replaces_id,
            created_at: now,
            updated_at: now,
        };
        let transaction = conn
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        if let Some(replaces_id) = &record.replaces_id {
            let changed = transaction
                .execute(
                    "UPDATE task_understanding_records
                     SET status='superseded', updated_at=?1
                     WHERE id=?2 AND workspace_id=?3 AND session_id=?4
                       AND status IN ('active', 'confirmed', 'deferred')",
                    params![now, replaces_id, workspace_id, session_id],
                )
                .map_err(|error| error.to_string())?;
            if changed != 1 {
                return Err(
                    "The understanding record to replace is not active in this task".to_owned(),
                );
            }
        }
        transaction
            .execute(
                "INSERT INTO task_understanding_records
                 (id, workspace_id, session_id, kind, source, scope, status, body, affects_json, replaces_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    record.id,
                    record.workspace_id,
                    record.session_id,
                    enum_json(&record.kind)?,
                    enum_json(&record.source)?,
                    enum_json(&record.scope)?,
                    enum_json(&record.status)?,
                    record.body,
                    serde_json::to_string(&record.affects).map_err(|error| error.to_string())?,
                    record.replaces_id,
                    record.created_at,
                    record.updated_at,
                ],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(record)
    }

    pub fn load_projection(
        conn: &Connection,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<TaskUnderstandingProjection, String> {
        let mut statement = conn
            .prepare(
                "SELECT id, workspace_id, session_id, kind, source, scope, status, body, affects_json, replaces_id, created_at, updated_at
                 FROM task_understanding_records
                 WHERE workspace_id=?1 AND session_id=?2 AND status IN ('active', 'confirmed')
                 ORDER BY CASE kind
                    WHEN 'goal' THEN 0 WHEN 'constraint' THEN 1 WHEN 'environment_fact' THEN 2
                    WHEN 'assumption' THEN 3 WHEN 'open_question' THEN 4 WHEN 'acceptance_condition' THEN 5
                    ELSE 6 END, updated_at DESC, id ASC
                 LIMIT ?3",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(
                params![workspace_id, session_id, MAX_RECORDS_IN_CONTEXT],
                read_record,
            )
            .map_err(|error| error.to_string())?;
        let records = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(TaskUnderstandingProjection {
            record_ids: records.iter().map(|record| record.id.clone()).collect(),
            records,
        })
    }

    /// Render structured data rather than prose instructions.  This keeps
    /// model-visible task data separate from system instructions.
    pub fn render_projection(
        projection: &TaskUnderstandingProjection,
    ) -> Result<Option<String>, String> {
        if projection.is_empty() {
            return Ok(None);
        }
        let records = projection
            .records
            .iter()
            .map(|record| {
                serde_json::json!({
                    "id": record.id,
                    "kind": record.kind,
                    "source": record.source,
                    "scope": record.scope,
                    "status": record.status,
                    "body": truncate(&record.body),
                    "affects": record.affects,
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&serde_json::json!({"task_understanding": records}))
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Choose the least interruptive action represented by the currently
    /// durable evidence.  Future example/experiment variants will be added
    /// only with explicit record shapes and acceptance tests.
    pub fn recommend(projection: &TaskUnderstandingProjection) -> InformationAcquisitionAction {
        if projection.records.iter().any(|record| {
            record.kind == UnderstandingKind::EnvironmentFact
                && record.status == UnderstandingStatus::Active
        }) {
            return InformationAcquisitionAction::Inspect;
        }
        if projection.records.iter().any(|record| {
            record.kind == UnderstandingKind::OpenQuestion
                && record.status == UnderstandingStatus::Active
        }) {
            return InformationAcquisitionAction::Ask;
        }
        if projection.records.iter().any(|record| {
            record.kind == UnderstandingKind::Assumption
                && record.source == UnderstandingSource::Agent
                && record.status == UnderstandingStatus::Active
        }) {
            return InformationAcquisitionAction::ActWithAssumption;
        }
        InformationAcquisitionAction::Defer
    }
}

fn validate_update(update: &NewTaskUnderstandingRecord) -> Result<(), String> {
    if update.body.trim().is_empty() {
        return Err("Task understanding body cannot be empty".to_owned());
    }
    if update.body.chars().count() > MAX_BODY_CHARS {
        return Err(format!(
            "Task understanding body exceeds {MAX_BODY_CHARS} characters"
        ));
    }
    if update.affects.len() > 8 || update.affects.iter().any(|item| item.trim().is_empty()) {
        return Err(
            "Task understanding affects must contain at most eight non-empty references".to_owned(),
        );
    }
    if update.source == UnderstandingSource::Agent
        && (!matches!(
            update.kind,
            UnderstandingKind::Assumption | UnderstandingKind::OpenQuestion
        ) || update.status == UnderstandingStatus::Confirmed)
    {
        return Err(
            "Agent-produced understanding may only be an unconfirmed assumption or open question"
                .to_owned(),
        );
    }
    Ok(())
}

fn enum_json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value)
        .map(|serialized| serialized.trim_matches('"').to_owned())
        .map_err(|error| error.to_string())
}

fn read_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskUnderstandingRecord> {
    let affects: String = row.get(8)?;
    let affects = serde_json::from_str(&affects).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(TaskUnderstandingRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        session_id: row.get(2)?,
        kind: decode_enum(row, 3)?,
        source: decode_enum(row, 4)?,
        scope: decode_enum(row, 5)?,
        status: decode_enum(row, 6)?,
        body: row.get(7)?,
        affects,
        replaces_id: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn decode_enum<T: DeserializeOwned>(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<T> {
    let value: String = row.get(index)?;
    serde_json::from_value(serde_json::Value::String(value)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn truncate(value: &str) -> String {
    let mut characters = value.chars();
    let visible: String = characters.by_ref().take(MAX_BODY_CHARS).collect();
    if characters.next().is_some() {
        format!("{visible}…")
    } else {
        visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY);
             CREATE TABLE sessions (id TEXT PRIMARY KEY);",
        )
        .unwrap();
        conn.execute_batch(include_str!("../../migrations/069_task_understanding.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO projects (id) VALUES ('w'), ('w1'), ('w2');
             INSERT INTO sessions (id) VALUES ('s'), ('s1'), ('s2');",
        )
        .unwrap();
        conn
    }

    fn update(
        kind: UnderstandingKind,
        source: UnderstandingSource,
        body: &str,
    ) -> NewTaskUnderstandingRecord {
        NewTaskUnderstandingRecord {
            kind,
            source,
            scope: UnderstandingScope::Task,
            status: UnderstandingStatus::Active,
            body: body.to_owned(),
            affects: vec![],
            replaces_id: None,
        }
    }

    #[test]
    fn agent_cannot_persist_a_confirmed_user_decision() {
        let conn = db();
        let mut candidate = update(
            UnderstandingKind::Constraint,
            UnderstandingSource::Agent,
            "never send mail",
        );
        candidate.status = UnderstandingStatus::Confirmed;
        assert!(TaskUnderstanding::apply_update(&conn, "w", "s", candidate, 1).is_err());
    }

    #[test]
    fn projection_is_task_scoped_and_preserves_provenance() {
        let conn = db();
        TaskUnderstanding::apply_update(
            &conn,
            "w1",
            "s1",
            update(
                UnderstandingKind::Goal,
                UnderstandingSource::User,
                "organize notes",
            ),
            1,
        )
        .unwrap();
        TaskUnderstanding::apply_update(
            &conn,
            "w2",
            "s2",
            update(
                UnderstandingKind::Goal,
                UnderstandingSource::User,
                "other project",
            ),
            1,
        )
        .unwrap();
        let projection = TaskUnderstanding::load_projection(&conn, "w1", "s1").unwrap();
        assert_eq!(projection.records.len(), 1);
        assert_eq!(projection.records[0].source, UnderstandingSource::User);
        assert!(TaskUnderstanding::render_projection(&projection)
            .unwrap()
            .unwrap()
            .contains("organize notes"));
    }

    #[test]
    fn replacement_supersedes_only_the_current_task_record() {
        let conn = db();
        let first = TaskUnderstanding::apply_update(
            &conn,
            "w",
            "s",
            update(
                UnderstandingKind::Assumption,
                UnderstandingSource::Agent,
                "use local storage",
            ),
            1,
        )
        .unwrap();
        let mut replacement = update(
            UnderstandingKind::Assumption,
            UnderstandingSource::Agent,
            "use encrypted local storage",
        );
        replacement.replaces_id = Some(first.id.clone());
        TaskUnderstanding::apply_update(&conn, "w", "s", replacement, 2).unwrap();
        let projection = TaskUnderstanding::load_projection(&conn, "w", "s").unwrap();
        assert_eq!(projection.records.len(), 1);
        assert_eq!(projection.records[0].body, "use encrypted local storage");
    }

    #[test]
    fn recommendation_prefers_evidence_over_a_user_interruption() {
        let conn = db();
        TaskUnderstanding::apply_update(
            &conn,
            "w",
            "s",
            update(
                UnderstandingKind::OpenQuestion,
                UnderstandingSource::Agent,
                "which reminder tone is right?",
            ),
            1,
        )
        .unwrap();
        TaskUnderstanding::apply_update(
            &conn,
            "w",
            "s",
            update(
                UnderstandingKind::EnvironmentFact,
                UnderstandingSource::Environment,
                "determine whether notifications are available",
            ),
            2,
        )
        .unwrap();
        let projection = TaskUnderstanding::load_projection(&conn, "w", "s").unwrap();
        assert_eq!(
            TaskUnderstanding::recommend(&projection),
            InformationAcquisitionAction::Inspect
        );
    }

    #[test]
    fn deferred_items_stay_durable_without_reentering_every_turn() {
        let conn = db();
        let mut deferred = update(
            UnderstandingKind::OpenQuestion,
            UnderstandingSource::Agent,
            "pick a future visual refinement",
        );
        deferred.status = UnderstandingStatus::Deferred;
        TaskUnderstanding::apply_update(&conn, "w", "s", deferred, 1).unwrap();
        assert!(TaskUnderstanding::load_projection(&conn, "w", "s")
            .unwrap()
            .is_empty());
    }
}
