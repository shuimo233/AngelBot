//! Durable, fact-based state for one bounded foreground agent task.
//!
//! `TaskFacts` deliberately records observable task facts rather than an
//! executable workflow graph.  Callers derive a task phase from the facts and
//! must never use loading these records as authority to replay a tool call.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    Pending,
    Completed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStepFact {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub status: PlanStepStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationEvidence {
    pub command: String,
    pub exit_code: i32,
    pub summary: String,
    pub verified_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingConfirmationFact {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub requested_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderBinding {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskTerminalReason {
    Completed,
    AwaitingConfirmation,
    NeedsAttention,
    Stopped,
}

/// The stable JSON shape stored in `task_run_facts.facts_json`.
///
/// The table keeps task/session/message identifiers as columns because they
/// are queried directly. The remaining fields evolve together as a versioned
/// serde shape, with defaults preserving compatibility for future additions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskFacts {
    pub goal: String,
    #[serde(default)]
    pub plan: Vec<PlanStepFact>,
    #[serde(default)]
    pub completed_steps: Vec<String>,
    #[serde(default)]
    pub failed_steps: Vec<String>,
    #[serde(default)]
    pub modified_files: Vec<String>,
    #[serde(default)]
    pub verification_evidence: Vec<VerificationEvidence>,
    #[serde(default)]
    pub pending_confirmation: Option<PendingConfirmationFact>,
    #[serde(default)]
    pub repair_attempts: u32,
    #[serde(default)]
    pub no_progress_count: u32,
    #[serde(default)]
    pub context_summary: Option<String>,
    #[serde(default)]
    pub provider_attempts: u32,
    #[serde(default)]
    pub provider_binding: Option<ProviderBinding>,
    #[serde(default)]
    pub terminal_reason: Option<TaskTerminalReason>,
}

impl TaskFacts {
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            plan: Vec::new(),
            completed_steps: Vec::new(),
            failed_steps: Vec::new(),
            modified_files: Vec::new(),
            verification_evidence: Vec::new(),
            pending_confirmation: None,
            repair_attempts: 0,
            no_progress_count: 0,
            context_summary: None,
            provider_attempts: 0,
            provider_binding: None,
            terminal_reason: None,
        }
    }

    pub fn has_unfinished_plan_steps(&self) -> bool {
        self.plan
            .iter()
            .any(|step| step.status == PlanStepStatus::Pending)
    }

    pub fn has_unhandled_failures(&self) -> bool {
        !self.failed_steps.is_empty()
            || self
                .plan
                .iter()
                .any(|step| step.status == PlanStepStatus::Failed)
    }

    pub fn has_successful_verification(&self) -> bool {
        self.verification_evidence
            .iter()
            .any(|evidence| evidence.exit_code == 0)
    }

    /// Project mutations invalidate all prior verification evidence.
    pub fn record_project_mutation(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !self.modified_files.iter().any(|existing| existing == &path) {
            self.modified_files.push(path);
        }
        self.verification_evidence.clear();
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DerivedTaskPhase {
    Running,
    AwaitingConfirmation,
    NeedsAttention,
    Stopped,
    Completed,
}

/// Derive a user-visible phase without creating an executable state machine.
pub fn derive_task_phase(facts: &TaskFacts, verification_required: bool) -> DerivedTaskPhase {
    match facts.terminal_reason {
        Some(TaskTerminalReason::Stopped) => return DerivedTaskPhase::Stopped,
        Some(TaskTerminalReason::NeedsAttention) => return DerivedTaskPhase::NeedsAttention,
        Some(TaskTerminalReason::AwaitingConfirmation) => {
            return DerivedTaskPhase::AwaitingConfirmation
        }
        Some(TaskTerminalReason::Completed) => return DerivedTaskPhase::Completed,
        None => {}
    }

    if facts.pending_confirmation.is_some() {
        return DerivedTaskPhase::AwaitingConfirmation;
    }
    if facts.has_unhandled_failures() {
        return DerivedTaskPhase::NeedsAttention;
    }
    if facts.has_unfinished_plan_steps() {
        return DerivedTaskPhase::Running;
    }
    if verification_required
        && !facts.modified_files.is_empty()
        && !facts.has_successful_verification()
    {
        return DerivedTaskPhase::NeedsAttention;
    }
    DerivedTaskPhase::Completed
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredTaskFacts {
    pub task_run_id: String,
    pub session_id: String,
    pub message_id: String,
    pub facts: TaskFacts,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn persist_task_facts(
    conn: &Connection,
    task_run_id: &str,
    session_id: &str,
    message_id: &str,
    facts: &TaskFacts,
    now: i64,
) -> Result<(), String> {
    let facts_json = serde_json::to_string(facts).map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO task_run_facts (
            task_run_id, session_id, message_id, facts_json, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        ON CONFLICT(task_run_id) DO UPDATE SET
            session_id = excluded.session_id,
            message_id = excluded.message_id,
            facts_json = excluded.facts_json,
            updated_at = excluded.updated_at",
        params![task_run_id, session_id, message_id, facts_json, now],
    )
    .map_err(|error| format!("Failed to persist task facts: {error}"))?;
    Ok(())
}

pub fn load_task_facts(
    conn: &Connection,
    task_run_id: &str,
) -> Result<Option<StoredTaskFacts>, String> {
    conn.query_row(
        "SELECT task_run_id, session_id, message_id, facts_json, created_at, updated_at
         FROM task_run_facts WHERE task_run_id = ?1",
        params![task_run_id],
        |row| {
            let facts_json: String = row.get(3)?;
            let facts = serde_json::from_str(&facts_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(StoredTaskFacts {
                task_run_id: row.get(0)?,
                session_id: row.get(1)?,
                message_id: row.get(2)?,
                facts,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("Failed to load task facts: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_mutation_invalidates_prior_verification() {
        let mut facts = TaskFacts::new("update project");
        facts.verification_evidence.push(VerificationEvidence {
            command: "cargo test --lib".to_string(),
            exit_code: 0,
            summary: "passed".to_string(),
            verified_at: 1,
        });

        facts.record_project_mutation("src/main.rs");

        assert_eq!(facts.modified_files, vec!["src/main.rs"]);
        assert!(facts.verification_evidence.is_empty());
        assert_eq!(
            derive_task_phase(&facts, true),
            DerivedTaskPhase::NeedsAttention
        );
    }

    #[test]
    fn phase_is_derived_from_facts() {
        let mut facts = TaskFacts::new("inspect files");
        facts.pending_confirmation = Some(PendingConfirmationFact {
            call_id: "call-1".to_string(),
            tool_name: "write_file".to_string(),
            arguments: serde_json::json!({"path":"note.txt"}),
            requested_at: 1,
        });
        assert_eq!(
            derive_task_phase(&facts, false),
            DerivedTaskPhase::AwaitingConfirmation
        );

        facts.pending_confirmation = None;
        facts.failed_steps.push("call-1".to_string());
        assert_eq!(
            derive_task_phase(&facts, false),
            DerivedTaskPhase::NeedsAttention
        );
    }

    #[test]
    fn facts_round_trip_without_rewriting_creation_time() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE task_runs (id TEXT PRIMARY KEY NOT NULL);
             CREATE TABLE task_run_facts (
                task_run_id TEXT PRIMARY KEY NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                facts_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );",
        )
        .unwrap();
        conn.execute("INSERT INTO task_runs (id) VALUES ('run-1')", [])
            .unwrap();

        let mut facts = TaskFacts::new("update task facts");
        facts.record_project_mutation("src/lib.rs");
        persist_task_facts(&conn, "run-1", "session-1", "message-1", &facts, 10).unwrap();

        facts.context_summary = Some("file modified; verification pending".to_string());
        persist_task_facts(&conn, "run-1", "session-1", "message-1", &facts, 20).unwrap();

        let stored = load_task_facts(&conn, "run-1").unwrap().unwrap();
        assert_eq!(stored.created_at, 10);
        assert_eq!(stored.updated_at, 20);
        assert_eq!(stored.facts, facts);
    }
}
