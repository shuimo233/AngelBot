use rusqlite::{params, Connection, Result};
use std::path::Path;

/// Open a SQLite connection and apply performance PRAGMAs.
/// Enables WAL mode, foreign keys, and tunes memory/IO settings.
pub fn init(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;

    // Apply performance PRAGMAs before any other operations
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA busy_timeout = 5000;
        PRAGMA cache_size = -8000;
        PRAGMA foreign_keys = ON;
        PRAGMA mmap_size = 268435456;
        "#,
    )
    .map_err(|e| format!("Failed to apply PRAGMAs: {}", e))?;

    Ok(conn)
}

// ─── Versioned Migration System ────────────────────────────────────────────────

/// Ordered list of (version, sql) migrations.
/// Each migration runs inside a transaction; on failure the transaction is rolled back.
const MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("migrations/001_initial.sql")),
    (2, include_str!("migrations/002_memory_fields.sql")),
    (3, include_str!("migrations/003_work_dir.sql")),
    (4, include_str!("migrations/004_fts5_search.sql")),
    (5, include_str!("migrations/005_memory_dedup.sql")),
    (6, include_str!("migrations/006_vector_store.sql")),
    (7, include_str!("migrations/007_agent_steps.sql")),
    (8, include_str!("migrations/008_core_memory.sql")),
    (9, include_str!("migrations/009_embedding_cache.sql")),
    (10, include_str!("migrations/010_agent_goals.sql")),
    (11, include_str!("migrations/011_agent_events.sql")),
    (12, include_str!("migrations/012_knowledge_graph.sql")),
    (13, include_str!("migrations/013_priority_system.sql")),
    (14, include_str!("migrations/014_session_branches.sql")),
    (15, include_str!("migrations/015_usage_tracking.sql")),
    (16, include_str!("migrations/016_evolution_proposals.sql")),
    (17, include_str!("migrations/017_task_runs.sql")),
    (18, include_str!("migrations/018_automations.sql")),
    (19, include_str!("migrations/019_channels.sql")),
    (20, include_str!("migrations/020_channel_revocation.sql")),
    (
        21,
        include_str!("migrations/021_channel_proactive_policy.sql"),
    ),
    (22, include_str!("migrations/022_channel_credentials.sql")),
    (
        23,
        include_str!("migrations/023_automation_script_runner.sql"),
    ),
    (24, include_str!("migrations/024_session_read_scopes.sql")),
    (25, include_str!("migrations/025_desktop_trusted_apps.sql")),
    (
        26,
        include_str!("migrations/026_session_delete_integrity.sql"),
    ),
    (
        27,
        include_str!("migrations/027_remove_context_window_setting.sql"),
    ),
    (28, include_str!("migrations/028_task_run_continuation.sql")),
    (29, include_str!("migrations/029_task_run_facts.sql")),
    (
        30,
        include_str!("migrations/030_session_provider_binding.sql"),
    ),
    (31, include_str!("migrations/031_agent_run_events.sql")),
    (
        32,
        include_str!("migrations/032_fix_agent_goals_session_id.sql"),
    ),
    (33, include_str!("migrations/033_migration_log.sql")),
    (34, include_str!("migrations/034_inline_tool_protocol.sql")),
    (35, include_str!("migrations/035_session_tree.sql")),
    (36, include_str!("migrations/036_hard_delete_branch.sql")),
    (37, include_str!("migrations/037_agent_step_call_id.sql")),
    (38, include_str!("migrations/038_adaptive_constraints.sql")),
    (
        39,
        include_str!("migrations/039_agent_permission_grants.sql"),
    ),
    (
        40,
        include_str!("migrations/040_adaptive_constraint_evidence.sql"),
    ),
    (41, include_str!("migrations/041_delegation_runtime.sql")),
    (42, include_str!("migrations/042_attention_state.sql")),
    (
        43,
        include_str!("migrations/043_attention_cross_session_scope.sql"),
    ),
    (
        44,
        include_str!("migrations/044_work_package_confirmation.sql"),
    ),
    (
        45,
        include_str!("migrations/045_work_package_worker_policy.sql"),
    ),
    (46, include_str!("migrations/046_work_package_state.sql")),
    (
        47,
        include_str!("migrations/047_delegated_network_source_policy.sql"),
    ),
    (
        48,
        include_str!("migrations/048_provisional_assistant_runs.sql"),
    ),
    (
        49,
        include_str!("migrations/049_delegation_work_package_link.sql"),
    ),
    (
        50,
        include_str!("migrations/050_delegated_model_binding.sql"),
    ),
    (
        51,
        include_str!("migrations/051_delegation_idempotency.sql"),
    ),
    (52, include_str!("migrations/052_workspace_admissions.sql")),
    (
        53,
        include_str!("migrations/053_workspace_admission_idempotency.sql"),
    ),
    (
        54,
        include_str!("migrations/054_delegation_explorer_plans.sql"),
    ),
    (
        55,
        include_str!("migrations/055_delegation_resource_bindings.sql"),
    ),
    (
        56,
        include_str!("migrations/056_materialization_receipts.sql"),
    ),
    (
        57,
        include_str!("migrations/057_delegation_review_artifacts.sql"),
    ),
    (
        58,
        include_str!("migrations/058_delegation_review_jobs.sql"),
    ),
    (
        59,
        include_str!("migrations/059_delegation_change_handoffs.sql"),
    ),
    (
        60,
        include_str!("migrations/060_allow_review_artifact_change_set_binding.sql"),
    ),
    (
        61,
        include_str!("migrations/061_allow_change_handoff_terminal_progress.sql"),
    ),
    (62, include_str!("migrations/062_workspace_model_reset.sql")),
    (63, include_str!("migrations/063_workspace_supervision.sql")),
    (
        64,
        include_str!("migrations/064_workspace_supervisor_work_queue.sql"),
    ),
    (
        65,
        include_str!("migrations/065_workspace_supervisor_delegation_gate.sql"),
    ),
    (
        66,
        include_str!("migrations/066_automation_workspace_ownership.sql"),
    ),
    (
        67,
        include_str!("migrations/067_mcp_workspace_enablement.sql"),
    ),
    (
        68,
        include_str!("migrations/068_automation_schedule_claims.sql"),
    ),
    (69, include_str!("migrations/069_task_understanding.sql")),
    (
        70,
        include_str!("migrations/070_review_execution_identity.sql"),
    ),
    (
        71,
        include_str!("migrations/071_workspace_supervisor_retry_gates.sql"),
    ),
    (
        72,
        include_str!("migrations/072_explorer_plan_attempt_binding.sql"),
    ),
    (
        73,
        include_str!("migrations/073_delegation_retry_lineage.sql"),
    ),
    (
        74,
        include_str!("migrations/074_mcp_confirmation_schema_revision.sql"),
    ),
    (
        75,
        include_str!("migrations/075_delegated_network_scope_approvals.sql"),
    ),
    (
        76,
        include_str!("migrations/076_agent_automation_dispatch_link.sql"),
    ),
    (
        77,
        include_str!("migrations/077_delegated_delivery_follow_ups.sql"),
    ),
    (78, include_str!("migrations/078_mcp_credential_refs.sql")),
];

/// Short human-readable description for each migration version, used in
/// `migration_log`. Index 0 corresponds to version 1. Keep in sync with the
/// `MIGRATIONS` array above.
const MIGRATION_DESCRIPTIONS: &[&str] = &[
    "initial schema",
    "memory fields",
    "work directory",
    "fts5 search",
    "memory dedup",
    "vector store",
    "agent steps",
    "core memory",
    "embedding cache",
    "agent goals",
    "agent events",
    "knowledge graph",
    "priority system",
    "session branches",
    "usage tracking",
    "evolution proposals",
    "task runs",
    "automations",
    "channels",
    "channel revocation",
    "channel proactive policy",
    "channel credentials",
    "automation script runner",
    "session read scopes",
    "desktop trusted apps",
    "session delete integrity",
    "remove context window setting",
    "task run continuation",
    "task run facts",
    "session provider binding",
    "agent run events",
    "fix agent goals session id",
    "migration log journal",
    "inline tool protocol into messages",
    "session tree with parent_id",
    "hard delete branch on edit/resend",
    "separate agent step and provider call identifiers",
    "adaptive constraints",
    "expiring agent permission grants",
    "adaptive constraint evidence de-duplication",
    "delegated task runtime foundation",
    "durable compact foreground attention state",
    "cross-session attention scope",
    "work package candidate and confirmation contract",
    "work package worker capability policy",
    "work package lifecycle state",
    "delegated network source policy registry",
    "hidden provisional assistant runs",
    "delegation work package link",
    "immutable delegated model binding",
    "parent-scoped delegated tool-call idempotency",
    "workspace admission leases and writer serialization",
    "workspace admission attempt idempotency and recovery indexes",
    "durable typed explorer work plans",
    "delegation resource identities and cleanup leases",
    "canonical materialization receipts",
    "durable reviewed implementation artifacts",
    "durable semantic review jobs",
    "durable reviewed change handoffs",
    "allow one-way review artifact change-set binding",
    "allow reviewed change handoff terminal progress",
    "workspace model reset with personal home",
    "workspace supervisor input queue and activity projections",
    "workspace supervisor durable work queue",
    "workspace supervisor delegation execution gate",
    "automation workspace ownership",
    "workspace-scoped MCP enablement",
    "atomic automation schedule claims",
    "durable task understanding records",
    "independent reviewer execution identity",
    "retry-safe workspace supervisor delegation gates",
    "explorer plans bound to delegation attempts",
    "delegation retry delivery lineage",
    "MCP confirmation schema revision",
    "delegated network scope confirmation provenance",
    "agent automation foreground dispatch linkage",
    "reviewed Explorer delivery foreground follow-ups",
    "MCP environment credential references",
];

/// Read an integer setting, returning `default` if the key is missing or unparseable.
fn get_setting_int(conn: &Connection, key: &str, default: i32) -> i32 {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(default)
}

/// Return the highest migration version that successfully ran, using the
/// `migration_log` journal introduced in v33. Falls back to
/// `settings.schema_version` for databases that haven't been upgraded past v32.
///
/// The result is the **minimum** of (settings.schema_version, MAX applied in
/// migration_log). This is intentional: if `settings.schema_version` claims a
/// migration ran but `migration_log` doesn't have it (because v33 detected the
/// schema was never applied and rolled the value back), we trust the journal.
/// Once both sources exist, they should agree; if they don't, the smaller
/// value wins so migrate() can replay the missing work.
///
/// On databases that haven't been upgraded past v32 (no migration_log yet),
/// we run a one-shot self-heal probe: if `agent_goals.session_id` still has
/// its v10 foreign key, we cap the result at 31 so v32 gets replayed.
///
/// Returns `Ok(None)` if neither source has any record (fresh install).
fn get_last_applied_version(conn: &Connection) -> Result<Option<i32>, String> {
    let migration_log_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='migration_log'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("Failed to probe migration_log: {}", e))?;

    if migration_log_exists {
        let logged_max: Option<i32> = conn
            .query_row(
                "SELECT MAX(version) FROM migration_log WHERE status = 'applied'",
                [],
                |r| r.get::<_, Option<i32>>(0),
            )
            .map_err(|e| format!("Failed to read migration_log: {}", e))?;
        let settings_max = get_setting_int(conn, "schema_version", 0);
        // Conservative: trust whichever is smaller so missing schema work gets replayed.
        let candidate = match logged_max {
            Some(v) => v.min(if settings_max > 0 { settings_max } else { v }),
            None => 0,
        };
        return Ok(if candidate > 0 { Some(candidate) } else { None });
    }

    // No migration_log yet. Use settings.schema_version but apply a
    // v32-specific self-heal: if the FK still exists, cap at 31.
    let mut settings_version = get_setting_int(conn, "schema_version", 0);
    if settings_version >= 32 {
        let agent_goals_sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='agent_goals'",
                [],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        if let Some(sql) = agent_goals_sql {
            if sql.contains("REFERENCES sessions(id)") {
                settings_version = 31;
            }
        }
    }
    Ok(if settings_version > 0 {
        Some(settings_version)
    } else {
        None
    })
}

/// Schema portion of migration 034, executed directly by `migrate()` when
/// the columns are not yet present. Splitting it out keeps the `.sql` file
/// fully idempotent — it carries only the data backfill that always runs.
const MIGRATION_034_SCHEMA: &str = "\
ALTER TABLE messages ADD COLUMN tool_calls TEXT;\n\
ALTER TABLE messages ADD COLUMN tool_call_id TEXT;\n\
ALTER TABLE messages ADD COLUMN tool_name TEXT;\n\
CREATE INDEX IF NOT EXISTS idx_messages_tool_call_id\n\
    ON messages(tool_call_id);\n";

const MIGRATION_037_SCHEMA: &str = "ALTER TABLE agent_steps ADD COLUMN call_id TEXT;";

const MIGRATION_043_SCHEMA: &str = "\
ALTER TABLE attention_states ADD COLUMN scope_owner_profile_id INTEGER;\n\
ALTER TABLE attention_states ADD COLUMN scope_workspace_key TEXT;\n";
const MIGRATION_043_INDEX: &str = "\
CREATE INDEX IF NOT EXISTS idx_attention_states_scope_open\n\
    ON attention_states(scope_owner_profile_id, scope_workspace_key, updated_at DESC)\n\
    WHERE status = 'open'\n\
      AND scope_owner_profile_id IS NOT NULL\n\
      AND scope_workspace_key IS NOT NULL;\n";
const MIGRATION_045_WORKER_PROFILE: &str = "\
ALTER TABLE work_packages ADD COLUMN worker_profile TEXT NOT NULL DEFAULT 'explorer'\n\
    CHECK(worker_profile IN ('explorer', 'implementer', 'verifier'));\n";
const MIGRATION_045_POLICY_VERSION: &str = "\
ALTER TABLE work_packages ADD COLUMN worker_policy_version INTEGER NOT NULL DEFAULT 1\n\
    CHECK(worker_policy_version > 0);\n";
const MIGRATION_048_PROVISIONAL_ASSISTANT: &str = "\
ALTER TABLE messages ADD COLUMN is_provisional INTEGER NOT NULL DEFAULT 0\n\
    CHECK(is_provisional IN (0, 1));\n";

const MIGRATION_066_AUTOMATION_WORKSPACE_ID: &str =
    "ALTER TABLE automations ADD COLUMN workspace_id TEXT REFERENCES projects(id);";
const MIGRATION_068_AUTOMATION_CLAIMS_SCHEMA: &str =
    "ALTER TABLE automations ADD COLUMN schedule_claim_token TEXT;\
     ALTER TABLE automations ADD COLUMN schedule_claimed_at INTEGER;";
const MIGRATION_073_DELEGATION_RETRY_LINEAGE_SCHEMA: &str = "\
ALTER TABLE delegation_attempts
    ADD COLUMN retry_source_delivery_id TEXT
        REFERENCES delegation_deliveries(id) ON DELETE RESTRICT;\n";
const MIGRATION_074_MCP_CONFIRMATION_SCHEMA_REVISION: &str =
    "ALTER TABLE agent_steps ADD COLUMN mcp_schema_version TEXT;";
const MIGRATION_076_SUPERVISOR_INPUT_ID: &str = r#"
ALTER TABLE automation_runs ADD COLUMN supervisor_input_id TEXT
    REFERENCES workspace_supervisor_inputs(id) ON DELETE SET NULL;
"#;
const MIGRATION_076_FOREGROUND_RUN_ID: &str =
    "ALTER TABLE automation_runs ADD COLUMN foreground_run_id TEXT;";
const MIGRATION_076_INDEXES: &str = r#"
CREATE UNIQUE INDEX IF NOT EXISTS idx_automation_runs_supervisor_input
    ON automation_runs(supervisor_input_id)
    WHERE supervisor_input_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_automation_runs_foreground_run
    ON automation_runs(foreground_run_id)
    WHERE foreground_run_id IS NOT NULL;
"#;
const MIGRATION_078_ENV_REF: &str = "ALTER TABLE mcp_servers ADD COLUMN env_ref TEXT;";
const MIGRATION_078_ENV_KEYS: &str =
    "ALTER TABLE mcp_servers ADD COLUMN env_keys TEXT NOT NULL DEFAULT '[]';";
const MIGRATION_078_INDEX_AND_GC: &str = r#"
CREATE UNIQUE INDEX IF NOT EXISTS idx_mcp_servers_env_ref
    ON mcp_servers(env_ref) WHERE env_ref IS NOT NULL;
CREATE TABLE IF NOT EXISTS mcp_credential_gc (
    env_ref TEXT PRIMARY KEY
);
"#;

/// Detect whether `column` exists on the `messages` table. Used by
/// `migrate()` to decide whether migration 034's schema portion has already
/// landed (e.g. test fixtures that simulate `settings.schema_version` drift).
fn messages_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('messages') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn agent_steps_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('agent_steps') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn mcp_servers_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('mcp_servers') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn attention_states_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('attention_states') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn work_packages_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('work_packages') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn delegations_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('delegations') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn projects_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('projects') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn workspace_schema_is_present(conn: &Connection) -> bool {
    projects_has_column(conn, "kind")
        && projects_has_column(conn, "active_session_id")
        && conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM pragma_table_info('sessions') WHERE name = 'project_id')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n != 0)
            .unwrap_or(false)
}

fn sessions_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('sessions') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn automations_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('automations') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn automation_runs_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('automation_runs') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn delegation_attempts_has_column(conn: &Connection, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('delegation_attempts') WHERE name = ?1)",
        params![column],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

/// Apply all pending migrations in version order.
/// Each migration is wrapped in BEGIN/COMMIT; on error it ROLLBACKs and returns Err.
///
/// `migration_log` (introduced in v33) is the source of truth for what has been
/// applied. We still keep `settings.schema_version` in sync for compatibility
/// with older code paths and external tools that read it.
pub fn migrate(conn: &Connection) -> Result<(), String> {
    // Ensure settings table exists before querying schema_version
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL)",
    )
    .map_err(|e| format!("Failed to ensure settings table: {}", e))?;

    let current_version = get_last_applied_version(conn)?.unwrap_or(0);
    let now = chrono::Utc::now().timestamp();

    for (version, sql) in MIGRATIONS {
        if *version <= current_version {
            continue;
        }

        conn.execute_batch("BEGIN")
            .map_err(|e| format!("Migration v{} BEGIN failed: {}", *version, e))?;

        // Try to create migration_log early so we can record a failed run too.
        // It's a no-op on v33+ because the migration itself creates the table.
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS migration_log (
                version INTEGER PRIMARY KEY,
                description TEXT NOT NULL,
                checksum TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('applied','failed')),
                started_at INTEGER NOT NULL,
                finished_at INTEGER,
                error_message TEXT
            )",
        );

        // Migration 034 ships ALTER TABLE ADD COLUMN statements. SQLite has no
        // `ADD COLUMN IF NOT EXISTS`, so we run the schema portion in code
        // (probing one column first) before executing the data-backfill
        // portion that always lives in the .sql file.
        if *version == 34 {
            if !messages_has_column(conn, "tool_calls") {
                conn.execute_batch(MIGRATION_034_SCHEMA)
                    .map_err(|e| format!("Migration v{} schema failed: {}", *version, e))?;
            }
            // Otherwise the schema is already in place; the data backfill
            // in the .sql file is itself idempotent and runs below.
        }

        // Like v34, v37 extends an existing table.  Recovery migrations may
        // intentionally replay it against a schema where the column exists
        // but the journal is behind, so only the ALTER needs a column probe.
        if *version == 37 && !agent_steps_has_column(conn, "call_id") {
            conn.execute_batch(MIGRATION_037_SCHEMA)
                .map_err(|e| format!("Migration v{} schema failed: {}", *version, e))?;
        }

        // v43 is also replayed by historical recovery fixtures where the
        // schema survived but the migration journal did not. Apply its ALTERs
        // only when needed; the index remains idempotent in either case.
        let migration_sql = if *version == 43 {
            if !attention_states_has_column(conn, "scope_owner_profile_id") {
                conn.execute_batch(MIGRATION_043_SCHEMA)
                    .map_err(|e| format!("Migration v{} schema failed: {}", *version, e))?;
            }
            MIGRATION_043_INDEX.to_owned()
        } else if *version == 45 {
            // Historical recovery fixtures can retain the v45 fields while
            // losing the migration journal. SQLite cannot add columns
            // idempotently, so replay only the missing schema fragments.
            let mut schema = String::new();
            if !work_packages_has_column(conn, "worker_profile") {
                schema.push_str(MIGRATION_045_WORKER_PROFILE);
            }
            if !work_packages_has_column(conn, "worker_policy_version") {
                schema.push_str(MIGRATION_045_POLICY_VERSION);
            }
            schema
        } else if *version == 46 {
            // A historical fixture predates WorkPackage but reuses `status`.
            // Journal drift must not replay an ALTER against that survived column.
            if work_packages_has_column(conn, "status") {
                String::new()
            } else {
                (*sql).to_owned()
            }
        } else if *version == 48 {
            // Like the earlier additive-message migrations, tolerate a
            // recovered database whose schema survived while its journal did
            // not. The index in the SQL file remains safe to replay.
            if !messages_has_column(conn, "is_provisional") {
                conn.execute_batch(MIGRATION_048_PROVISIONAL_ASSISTANT)
                    .map_err(|e| format!("Migration v{} schema failed: {}", *version, e))?;
            }
            "CREATE INDEX IF NOT EXISTS idx_messages_visible_session_created ON messages(session_id, is_provisional, created_at);".to_owned()
        } else if *version == 49 {
            // Recovery can retain the link column while the migration journal
            // lags. The partial index is independently idempotent.
            if delegations_has_column(conn, "work_package_id") {
                "CREATE INDEX IF NOT EXISTS idx_delegations_work_package ON delegations(work_package_id, updated_at DESC) WHERE work_package_id IS NOT NULL;".to_owned()
            } else {
                (*sql).to_owned()
            }
        } else if *version == 51 {
            if delegations_has_column(conn, "idempotency_key") {
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_delegations_parent_idempotency ON delegations(parent_run_id, idempotency_key) WHERE idempotency_key IS NOT NULL;".to_owned()
            } else {
                (*sql).to_owned()
            }
        } else if *version == 62 {
            // The workspace reset is intentionally destructive. If a journal
            // recovery replays late migrations against a schema that already
            // owns workspaces, preserve the recovered data and only restore
            // the journal entry.
            if workspace_schema_is_present(conn) {
                String::new()
            } else if projects_has_column(conn, "kind") {
                // A pre-v62 recovery fixture can rebuild `sessions` while
                // retaining the project table. Repair only the missing owned
                // session column; re-running the destructive reset would both
                // duplicate project columns and discard recovered data.
                if sessions_has_column(conn, "project_id") {
                    String::new()
                } else {
                    "ALTER TABLE sessions ADD COLUMN project_id TEXT REFERENCES projects(id);\
                     UPDATE sessions SET project_id = 'personal' WHERE id = 'personal-main';"
                        .to_owned()
                }
            } else {
                (*sql).to_owned()
            }
        } else if *version == 66 {
            // Recovery fixtures can retain this additive field while their
            // migration journal lags. The ownership backfill and index in
            // the SQL file are safe to replay either way.
            if !automations_has_column(conn, "workspace_id") {
                conn.execute_batch(MIGRATION_066_AUTOMATION_WORKSPACE_ID)
                    .map_err(|e| format!("Migration v{} schema failed: {}", *version, e))?;
            }
            (*sql).to_owned()
        } else if *version == 68 {
            // A migration journal can lag behind the recovered schema. The
            // additive columns must not make that recovery path fail; the
            // index in the SQL file remains safe to replay.
            if !automations_has_column(conn, "schedule_claim_token") {
                conn.execute_batch(MIGRATION_068_AUTOMATION_CLAIMS_SCHEMA)
                    .map_err(|e| format!("Migration v{} schema failed: {}", *version, e))?;
            }
            "CREATE INDEX IF NOT EXISTS idx_automations_schedule_claim ON \
             automations(enabled, trigger_kind, next_run_at, schedule_claimed_at);"
                .to_owned()
        } else if *version == 73 {
            // Recovery may replay the journal against a schema where the
            // retry lineage field already exists. Apply the non-idempotent
            // ALTER only when necessary; the remaining index and trigger in
            // the SQL migration are independently idempotent.
            if !delegation_attempts_has_column(conn, "retry_source_delivery_id") {
                conn.execute_batch(MIGRATION_073_DELEGATION_RETRY_LINEAGE_SCHEMA)
                    .map_err(|e| format!("Migration v{} schema failed: {}", *version, e))?;
            }
            (*sql).to_owned()
        } else if *version == 74 {
            // An interrupted upgrade can retain this additive column while
            // losing its migration journal. Probe before replaying SQLite's
            // non-idempotent ALTER so recovery stays safe.
            if !agent_steps_has_column(conn, "mcp_schema_version") {
                conn.execute_batch(MIGRATION_074_MCP_CONFIRMATION_SCHEMA_REVISION)
                    .map_err(|error| format!("Migration v{} schema failed: {error}", *version))?;
            }
            (*sql).to_owned()
        } else if *version == 76 {
            // An interrupted upgrade or conservative journal recovery can
            // replay v76 after either additive column has already survived.
            // SQLite cannot add a column idempotently, so restore only the
            // missing fragments and always replay the independently safe
            // indexes.
            if !automation_runs_has_column(conn, "supervisor_input_id") {
                conn.execute_batch(MIGRATION_076_SUPERVISOR_INPUT_ID)
                    .map_err(|error| format!("Migration v{} schema failed: {error}", *version))?;
            }
            if !automation_runs_has_column(conn, "foreground_run_id") {
                conn.execute_batch(MIGRATION_076_FOREGROUND_RUN_ID)
                    .map_err(|error| format!("Migration v{} schema failed: {error}", *version))?;
            }
            MIGRATION_076_INDEXES.to_owned()
        } else if *version == 78 {
            // The migration journal may lag behind a recovered schema, or
            // only one of the two additive columns may have survived. Probe
            // each non-idempotent ALTER independently; the index and cleanup
            // queue can be replayed safely.
            if !mcp_servers_has_column(conn, "env_ref") {
                conn.execute_batch(MIGRATION_078_ENV_REF)
                    .map_err(|error| format!("Migration v{} schema failed: {error}", *version))?;
            }
            if !mcp_servers_has_column(conn, "env_keys") {
                conn.execute_batch(MIGRATION_078_ENV_KEYS)
                    .map_err(|error| format!("Migration v{} schema failed: {error}", *version))?;
            }
            MIGRATION_078_INDEX_AND_GC.to_owned()
        } else {
            (*sql).to_owned()
        };

        if let Err(e) = conn.execute_batch(&migration_sql) {
            // Best-effort: record the failure, then roll back.
            let _ = conn.execute(
                "INSERT OR REPLACE INTO migration_log (version, description, checksum, status, started_at, finished_at, error_message) VALUES (?1, 'unknown', '', 'failed', ?2, ?2, ?3)",
                params![version, now, e.to_string()],
            );
            conn.execute_batch("ROLLBACK").ok();
            return Err(format!(
                "Migration v{} failed: {}. Rolled back.",
                *version, e
            ));
        }

        // Record successful execution in migration_log (the journal).
        // This is the new source of truth; settings.schema_version is kept in sync
        // for backward compatibility.
        if let Err(e) = conn.execute(
            "INSERT OR REPLACE INTO migration_log (version, description, checksum, status, started_at, finished_at) VALUES (?1, ?2, '', 'applied', ?3, ?3)",
            params![version, MIGRATION_DESCRIPTIONS.get(*version as usize - 1).copied().unwrap_or(""), now],
        ) {
            conn.execute_batch("ROLLBACK").ok();
            return Err(format!(
                "Migration v{} failed to write migration_log: {}",
                *version, e
            ));
        }

        // Keep settings.schema_version in sync.
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('schema_version', ?1, ?2)",
            params![version.to_string(), now],
        )
        .map_err(|e| format!("Migration v{} failed to record version: {}", *version, e))?;

        conn.execute_batch("COMMIT")
            .map_err(|e| format!("Migration v{} COMMIT failed: {}", *version, e))?;
    }

    // Insert default settings if not present (idempotent)
    insert_default_settings(conn, now)?;
    ensure_default_profile(conn, now)?;

    Ok(())
}

/// Insert default settings that the application depends on.
/// Uses INSERT OR IGNORE — safe to call after every migrate().
fn insert_default_settings(conn: &Connection, now: i64) -> Result<(), String> {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings (key, value, updated_at) VALUES ('autostart_enabled', 'false', ?1)",
        params![now],
    );
    let default_work_dir = dirs::document_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings (key, value, updated_at) VALUES ('work_directory', ?1, ?2)",
        params![default_work_dir, now],
    );
    Ok(())
}

/// Ensure the singleton owner profile exists for databases created before the
/// delegation pipeline started depending on it. This intentionally runs after
/// every migration so already-upgraded installations are repaired as well.
fn ensure_default_profile(conn: &Connection, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO profile (id, name, preferences, habits, background, updated_at)\n         VALUES (1, NULL, NULL, NULL, NULL, ?1)",
        params![now],
    )
    .map_err(|e| format!("Failed to ensure default profile: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn setup_test_db() -> (tempfile::NamedTempFile, Connection) {
        let mut tmp = NamedTempFile::with_suffix(".db").unwrap();
        tmp.write_all(b"").unwrap();
        let conn = init(tmp.path()).unwrap();
        (tmp, conn)
    }

    // ─── PRAGMA tests ───────────────────────────────────────────────────────

    #[test]
    fn test_pragma_wal_enabled() {
        let (_tmp, conn) = setup_test_db();
        let journal_mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");
    }

    #[test]
    fn test_pragma_foreign_keys_on() {
        let (_tmp, conn) = setup_test_db();
        let fk: i32 = conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn test_pragma_busy_timeout() {
        let (_tmp, conn) = setup_test_db();
        let timeout: i32 = conn
            .pragma_query_value(None, "busy_timeout", |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
    }

    // ─── Migration versioning tests ─────────────────────────────────────────

    #[test]
    fn test_migration_full_upgrade() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        // All tables should exist
        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        for expected in [
            "agent_events",
            "adaptive_constraints",
            "adaptive_constraint_evidence",
            "agent_permission_grants",
            "agent_run_events",
            "agent_goals",
            "agent_steps",
            "automation_runs",
            "automations",
            "channel_audit_log",
            "channel_connections",
            "constraint_checks",
            "context_summaries",
            "core_memory_blocks",
            "cost_summary_cache",
            "desktop_trusted_apps",
            "delegation_attempt_events",
            "delegation_attempts",
            "delegation_capability_leases",
            "delegation_deliveries",
            "delegation_explorer_plans",
            "delegated_delivery_follow_ups",
            "delegation_resource_bindings",
            "delegated_model_bindings",
            "delegation_outbox",
            "delegations",
            "embedding_cache",
            "evolution_proposals",
            "knowledge_edges",
            "knowledge_nodes",
            "mcp_servers",
            "memories",
            "memory_history",
            "memory_index",
            "memory_vector_map",
            "messages",
            "migration_log",
            "profile",
            "projects",
            "scheduled_tasks",
            "session_branches",
            "sessions",
            "settings",
            "smart_zone_log",
            "task_run_facts",
            "task_runs",
            "tool_patterns",
            "usage_stats",
            "work_packages",
            "work_package_candidate_sets",
            "work_package_change_sets",
            "work_package_confirmations",
            "workspace_supervisor_inputs",
            "workspace_activity_projections",
            "workspace_supervisor_work",
        ] {
            assert!(
                tables.iter().any(|table| table == expected),
                "missing feature table: {expected}"
            );
        }

        let owner_profile_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM profile WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            owner_profile_count, 1,
            "the owner profile must be initialized"
        );

        // Schema version should be the latest
        let latest_schema_version = MIGRATIONS.last().map(|(version, _)| *version).unwrap();
        let version = get_setting_int(&conn, "schema_version", 0);
        assert_eq!(version, latest_schema_version);

        // migration_log should record every applied migration
        let logged_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM migration_log WHERE status = 'applied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(logged_count, MIGRATIONS.len() as i64);

        let max_logged: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM migration_log WHERE status = 'applied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(max_logged, latest_schema_version);

        let cleanup_trigger: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = 'trg_sessions_cleanup_dependents'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cleanup_trigger, 1);

        // The context limit is model-derived, not a persisted user setting.
        let obsolete_setting_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'context_window_tokens'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(obsolete_setting_count, 0);

        let foreign_key_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_violations, 0);

        let personal_workspace: (String, String) = conn
            .query_row(
                "SELECT p.kind, s.project_id FROM projects p JOIN sessions s ON s.id = p.active_session_id WHERE p.id = 'personal'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            personal_workspace,
            ("personal".to_owned(), "personal".to_owned())
        );
    }

    #[test]
    fn test_migration_registry_is_complete_and_sequential() {
        for (index, (version, _)) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                *version as usize,
                index + 1,
                "migration versions must be sequential"
            );
        }

        let migration_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/migrations");
        let sql_file_count = std::fs::read_dir(migration_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("sql")
            })
            .count();
        assert_eq!(
            MIGRATIONS.len(),
            sql_file_count,
            "every migration file must be registered exactly once"
        );
    }

    #[test]
    fn test_workspace_supervisor_retry_gate_migration_preserves_gate_and_allows_retry() {
        let (_tmp, conn) = setup_test_db();
        conn.execute_batch(
            "
            CREATE TABLE workspace_supervisor_work (id TEXT PRIMARY KEY);
            CREATE TABLE delegations (id TEXT PRIMARY KEY);
            CREATE TABLE delegation_attempts (id TEXT PRIMARY KEY);
            CREATE TABLE work_packages (id TEXT PRIMARY KEY);
            CREATE TABLE workspace_supervisor_delegations (
                supervisor_work_id TEXT PRIMARY KEY
                    REFERENCES workspace_supervisor_work(id) ON DELETE CASCADE,
                delegation_id TEXT NOT NULL UNIQUE
                    REFERENCES delegations(id) ON DELETE CASCADE,
                attempt_id TEXT NOT NULL UNIQUE
                    REFERENCES delegation_attempts(id) ON DELETE CASCADE,
                work_package_id TEXT NOT NULL UNIQUE
                    REFERENCES work_packages(id) ON DELETE CASCADE,
                authorized_at INTEGER
            );
            INSERT INTO workspace_supervisor_work (id) VALUES
                ('work_1'), ('work_2'), ('work_3');
            INSERT INTO delegations (id) VALUES ('delegation_1');
            INSERT INTO delegation_attempts (id) VALUES
                ('attempt_1'), ('attempt_2'), ('attempt_3');
            INSERT INTO work_packages (id) VALUES ('package_1');
            INSERT INTO workspace_supervisor_delegations
                (supervisor_work_id, delegation_id, attempt_id, work_package_id, authorized_at)
            VALUES ('work_1', 'delegation_1', 'attempt_1', 'package_1', 123);
            ",
        )
        .unwrap();

        let (_, retry_gate_migration) = MIGRATIONS
            .iter()
            .find(|(version, _)| *version == 71)
            .expect("retry gate migration must be registered");
        conn.execute_batch(retry_gate_migration).unwrap();

        let preserved: (String, String, i64) = conn
            .query_row(
                "SELECT delegation_id, work_package_id, authorized_at
                 FROM workspace_supervisor_delegations WHERE attempt_id = 'attempt_1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(preserved, ("delegation_1".into(), "package_1".into(), 123));

        conn.execute(
            "INSERT INTO workspace_supervisor_delegations
                (supervisor_work_id, delegation_id, attempt_id, work_package_id, authorized_at)
             VALUES ('work_2', 'delegation_1', 'attempt_2', 'package_1', NULL)",
            [],
        )
        .unwrap();

        let duplicate_attempt = conn
            .execute(
                "INSERT INTO workspace_supervisor_delegations
                    (supervisor_work_id, delegation_id, attempt_id, work_package_id, authorized_at)
                 VALUES ('work_3', 'delegation_1', 'attempt_2', 'package_1', NULL)",
                [],
            )
            .unwrap_err();
        assert!(duplicate_attempt.to_string().contains("UNIQUE"));

        let duplicate_work = conn
            .execute(
                "INSERT INTO workspace_supervisor_delegations
                    (supervisor_work_id, delegation_id, attempt_id, work_package_id, authorized_at)
                 VALUES ('work_2', 'delegation_1', 'attempt_3', 'package_1', NULL)",
                [],
            )
            .unwrap_err();
        assert!(duplicate_work.to_string().contains("UNIQUE"));

        let gate_index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index'
                   AND name = 'idx_workspace_supervisor_delegations_delegation_gate'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(gate_index, 1);
    }

    #[test]
    fn test_explorer_plan_schema_contract() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(delegation_explorer_plans)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for expected in [
            "id",
            "delegation_id",
            "attempt_id",
            "work_package_id",
            "scope_digest",
            "worker_policy_version",
            "schema_version",
            "plan_json",
            "plan_digest",
            "created_at",
        ] {
            assert!(
                columns.iter().any(|column| column == expected),
                "missing plan column: {expected}"
            );
        }
        for expected in [
            "idx_delegation_explorer_plans_package",
            "idx_delegation_explorer_plans_attempt",
            "idx_delegation_explorer_plans_delegation_attempt",
        ] {
            let present: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('index', 'table') AND name = ?1",
                    [expected],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "missing explorer plan index: {expected}");
        }
        for expected in [
            "trg_delegation_explorer_plan_binding",
            "trg_delegation_explorer_plan_immutable",
        ] {
            let present: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    [expected],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "missing explorer plan trigger: {expected}");
        }
    }

    #[test]
    fn test_resource_binding_schema_contract() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(delegation_resource_bindings)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for expected in [
            "id",
            "delegation_id",
            "work_package_id",
            "attempt_id",
            "lease_id",
            "resource_kind",
            "provider_kind",
            "resource_ref",
            "manifest_locator",
            "manifest_version",
            "manifest_nonce",
            "manifest_digest",
            "scope_digest",
            "lease_epoch",
            "admission_id",
            "admission_state_version",
            "lifecycle_state",
            "cleanup_status",
            "cleanup_step",
            "cleanup_attempts",
            "next_cleanup_at",
            "quarantine_until",
            "last_error_class",
            "created_at",
            "updated_at",
        ] {
            assert!(
                columns.iter().any(|column| column == expected),
                "missing resource binding column: {expected}"
            );
        }
        for expected in [
            "idx_delegation_resource_bindings_attempt",
            "idx_delegation_resource_bindings_cleanup",
            "idx_delegation_resource_bindings_retention",
            "trg_delegation_resource_binding_scope",
            "trg_delegation_resource_binding_immutable",
            "trg_delegation_resource_binding_admission_immutable",
        ] {
            let object_type = if expected.starts_with("idx_") {
                "index"
            } else {
                "trigger"
            };
            let present: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    rusqlite::params![object_type, expected],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "missing resource binding object: {expected}");
        }
    }

    fn seed_explorer_delegation(conn: &Connection, shape: &str, profile: &str, scope_digest: &str) {
        conn.execute(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('plan-session','plan',1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('plan-message','plan-session','user','plan',1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('plan-run','plan-session','plan-message','plan','running','[]',1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,created_at,updated_at,status) VALUES ('plan-package','plan-session',1,'workspace',?1,?2,1,?3,'approved',100,1,1,'active')",
            params![shape, profile, scope_digest],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO delegations(id,session_id,message_id,parent_run_id,work_package_id,objective,brief_json,status,state_version,created_at,updated_at) VALUES ('plan-delegation','plan-session','plan-message','plan-run','plan-package','plan','{}','queued',0,1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at) VALUES ('plan-attempt','plan-delegation',1,'queued','sandbox://plan',1)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn test_explorer_plan_binding_trigger_rejects_scope_or_profile_drift() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();
        seed_explorer_delegation(&conn, "explore", "explorer", "scope-a");

        let wrong_scope = conn.execute(
            "INSERT INTO delegation_explorer_plans(id,delegation_id,attempt_id,work_package_id,scope_digest,worker_policy_version,schema_version,plan_json,plan_digest,created_at) VALUES ('plan-bad-scope','plan-delegation','plan-attempt','plan-package','scope-b',1,1,'{}','digest-b',1)",
            [],
        );
        assert!(wrong_scope.is_err());

        conn.execute(
            "UPDATE work_packages SET task_shape = 'change', worker_profile = 'implementer' WHERE id = 'plan-package'",
            [],
        )
        .unwrap();
        let wrong_profile = conn.execute(
            "INSERT INTO delegation_explorer_plans(id,delegation_id,attempt_id,work_package_id,scope_digest,worker_policy_version,schema_version,plan_json,plan_digest,created_at) VALUES ('plan-bad-profile','plan-delegation','plan-attempt','plan-package','scope-a',1,1,'{}','digest-c',1)",
            [],
        );
        assert!(wrong_profile.is_err());
    }

    #[test]
    fn test_explorer_plan_is_immutable_and_cascades_with_delegation() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();
        seed_explorer_delegation(&conn, "explore", "explorer", "scope-a");
        conn.execute(
            "INSERT INTO delegation_explorer_plans(id,delegation_id,attempt_id,work_package_id,scope_digest,worker_policy_version,schema_version,plan_json,plan_digest,created_at) VALUES ('plan-good','plan-delegation','plan-attempt','plan-package','scope-a',1,1,'{}','digest-a',1)",
            [],
        )
        .unwrap();
        let update = conn.execute(
            "UPDATE delegation_explorer_plans SET plan_json = '{\"changed\":true}' WHERE id = 'plan-good'",
            [],
        );
        assert!(update.is_err());
        conn.execute("DELETE FROM delegations WHERE id = 'plan-delegation'", [])
            .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM delegation_explorer_plans",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn test_migration_idempotent() {
        let (_tmp, conn) = setup_test_db();

        // Run migrations twice — second run should be a no-op
        migrate(&conn).unwrap();
        let version_after_first = get_setting_int(&conn, "schema_version", 0);

        migrate(&conn).unwrap();
        let version_after_second = get_setting_int(&conn, "schema_version", 0);

        assert_eq!(version_after_first, version_after_second);
    }

    #[test]
    fn mcp_confirmation_schema_migration_recovers_when_the_journal_lags() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();
        assert!(agent_steps_has_column(&conn, "mcp_schema_version"));

        conn.execute("DELETE FROM migration_log WHERE version = 74", [])
            .unwrap();
        conn.execute(
            "UPDATE settings SET value = '73' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert!(agent_steps_has_column(&conn, "mcp_schema_version"));
        let latest_schema_version = MIGRATIONS
            .last()
            .map(|(version, _)| *version)
            .expect("at least one migration");
        assert_eq!(
            get_setting_int(&conn, "schema_version", 0),
            latest_schema_version
        );
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM migration_log WHERE version = 74 AND status = 'applied'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1);
    }

    #[test]
    fn mcp_credential_ref_migration_recovers_when_the_journal_lags() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();
        assert!(mcp_servers_has_column(&conn, "env_ref"));
        assert!(mcp_servers_has_column(&conn, "env_keys"));

        conn.execute("DELETE FROM migration_log WHERE version = 78", [])
            .unwrap();
        conn.execute(
            "UPDATE settings SET value = '77' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        migrate(&conn).unwrap();

        assert!(mcp_servers_has_column(&conn, "env_ref"));
        assert!(mcp_servers_has_column(&conn, "env_keys"));
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM migration_log WHERE version = 78 AND status = 'applied'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1);
        assert_eq!(get_setting_int(&conn, "schema_version", 0), 78);
    }

    #[test]
    fn mcp_credential_ref_migration_repairs_a_partially_surviving_schema() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO mcp_servers (id, name, command, args, env, enabled, created_at)
                VALUES ('server-a', 'A', 'cmd', '', 'TOKEN=preserved', 1, 0);
             ALTER TABLE mcp_servers DROP COLUMN env_keys;
             DELETE FROM migration_log WHERE version = 78;
             UPDATE settings SET value = '77' WHERE key = 'schema_version';",
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert!(mcp_servers_has_column(&conn, "env_ref"));
        assert!(mcp_servers_has_column(&conn, "env_keys"));
        let (legacy, keys): (String, String) = conn
            .query_row(
                "SELECT env, env_keys FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(legacy, "TOKEN=preserved");
        assert_eq!(keys, "[]");
        let queue_exists: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'mcp_credential_gc')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queue_exists, 1);
    }

    #[test]
    fn migration_survives_a_real_database_reopen() {
        let (tmp, conn) = setup_test_db();
        let path = tmp.path().to_path_buf();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('restart_probe', 'preserved', 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let reopened = init(&path).unwrap();
        migrate(&reopened).unwrap();
        let value: String = reopened
            .query_row(
                "SELECT value FROM settings WHERE key = 'restart_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(value, "preserved");
        assert_eq!(
            get_setting_int(&reopened, "schema_version", 0),
            MIGRATIONS.len() as i32
        );
    }

    /// Regression test for the production incident on 2026-07-24:
    /// `settings.schema_version` had been advanced to 32 by an external
    /// writer, but the v32 migration's actual schema changes (DROP/CREATE
    /// `agent_goals`, recreate trigger) never ran. Without the journal, the
    /// next `migrate()` call would skip v32 and leave the FK constraint in
    /// place. With `migration_log` we detect the inconsistency and replay.
    #[test]
    fn test_migration_recovers_when_schema_version_lies() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        // Simulate the bad state: schema_version=33 but migration_log is empty
        // and the v32 schema changes are missing. We force a partial state by
        // dropping migration_log, putting schema_version=33, and reverting
        // agent_goals to its v10 schema.
        conn.execute("DROP TABLE IF EXISTS migration_log", [])
            .unwrap();
        conn.execute(
            "UPDATE settings SET value = '33' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();

        // Revert agent_goals to v10 shape so v32 actually has work to do.
        conn.execute("ALTER TABLE agent_goals RENAME TO agent_goals_post32", [])
            .unwrap();
        conn.execute(
            "CREATE TABLE agent_goals (
                id TEXT PRIMARY KEY,
                session_id TEXT DEFAULT '' REFERENCES sessions(id) ON DELETE SET DEFAULT,
                goal_text TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','active','done','failed','cancelled')),
                progress_pct INTEGER DEFAULT 0,
                parent_goal_id TEXT REFERENCES agent_goals(id),
                summary TEXT,
                created_at INTEGER NOT NULL,
                completed_at INTEGER
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO agent_goals SELECT id, session_id, goal_text, status, progress_pct, parent_goal_id, summary, created_at, completed_at FROM agent_goals_post32",
            [],
        )
        .unwrap();
        conn.execute("DROP TABLE agent_goals_post32", []).unwrap();

        // v32's trigger references the new (post-v32) agent_goals and would
        // be valid either way; drop it so v32's CREATE TRIGGER re-runs.
        conn.execute("DROP TRIGGER IF EXISTS trg_sessions_cleanup_dependents", [])
            .unwrap();

        // Drop task_runs (FK'd to messages) before rebuilding messages.
        // The recovery state claims v33, so restore its v28-compatible shape
        // after rebuilding messages: v17/v28 will not be replayed below.
        conn.execute("DROP TABLE task_runs", []).unwrap();

        // v35 added columns to messages/sessions; drop them so re-running
        // migrate() can re-apply v34 and v35 without "duplicate column" errors.
        // SQLite has no DROP COLUMN IF EXISTS, so we use table rebuilds.
        //
        // sessions.parent_id (v14) is a self-referential FK (REFERENCES sessions(id)).
        // sessions.parent_id VALUES in the backup reference the sessions table itself,
        // NOT messages. When re-enabling FK enforcement with non-NULL parent_id values,
        // SQLite cannot validate them → "FK mismatch sessions referencing messages".
        //
        // SOLUTION:
        // 1. PRAGMA foreign_keys = OFF
        // 2. UPDATE sessions SET parent_id = NULL (breaks the self-ref FK chain)
        // 3. UPDATE messages SET parent_id = NULL (removes FK reference before backup)
        // 4. BACKUP sessions (no parent_id column), BACKUP messages (no parent_id)
        // 5. DROP sessions, DROP messages
        // 6. CREATE sessions (pre-v35, with parent_id FK to sessions(id))
        // 7. CREATE messages (pre-v35, no parent_id)
        // 8. RESTORE data (sessions.parent_id = NULL → self-ref FK satisfied)
        // 9. PRAGMA foreign_keys = ON
        // 10. migrate() re-adds all columns idempotently
        conn.execute("PRAGMA foreign_keys = OFF", []).unwrap();

        // Break the sessions.parent_id self-ref FK chain before backing up.
        conn.execute("UPDATE sessions SET parent_id = NULL", [])
            .unwrap();
        conn.execute("UPDATE messages SET parent_id = NULL", [])
            .unwrap();

        // Back up sessions WITHOUT parent_id.
        conn.execute(
            "CREATE TABLE sessions_v35 AS
             SELECT id, title, created_at, updated_at, context_version,
                    last_compressed_at, metadata, work_dir,
                    branch_name, branch_created_at, additional_read_dirs,
                    agent_provider, agent_model
             FROM sessions",
            [],
        )
        .unwrap();

        // Back up messages WITHOUT v34/v35 columns.
        conn.execute(
            "CREATE TABLE messages_v35 AS
             SELECT id, session_id, role, content, metadata, created_at
             FROM messages",
            [],
        )
        .unwrap();

        // Drop and recreate sessions with pre-v35 schema (no v35 columns).
        conn.execute("DROP TABLE IF EXISTS sessions", []).unwrap();
        conn.execute(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                context_version INTEGER NOT NULL DEFAULT 0,
                last_compressed_at INTEGER,
                metadata TEXT,
                work_dir TEXT,
                parent_id TEXT REFERENCES sessions(id),
                branch_name TEXT,
                branch_created_at INTEGER,
                additional_read_dirs TEXT NOT NULL DEFAULT '[]',
                agent_provider TEXT,
                agent_model TEXT
            )",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO sessions SELECT id, title, created_at, updated_at, context_version, last_compressed_at, metadata, work_dir, NULL AS parent_id, branch_name, branch_created_at, additional_read_dirs, agent_provider, agent_model FROM sessions_v35", []).unwrap();
        conn.execute("DROP TABLE sessions_v35", []).unwrap();

        // Drop and recreate messages with pre-v34 schema (no v34/v35 columns).
        conn.execute("DROP INDEX IF EXISTS idx_messages_parent", [])
            .unwrap();
        conn.execute("DROP INDEX IF EXISTS idx_messages_tool_call_id", [])
            .unwrap();
        conn.execute("DROP TABLE IF EXISTS messages", []).unwrap();
        conn.execute(
            "CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT,
                created_at INTEGER NOT NULL
            )",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO messages SELECT id, session_id, role, content, metadata, created_at FROM messages_v35", []).unwrap();
        conn.execute("DROP TABLE messages_v35", []).unwrap();

        conn.execute_batch(
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                goal TEXT NOT NULL,
                status TEXT NOT NULL,
                plan TEXT NOT NULL,
                confirmation_state TEXT NOT NULL DEFAULT 'none',
                resumable INTEGER NOT NULL DEFAULT 0,
                step_count INTEGER NOT NULL DEFAULT 0,
                completed_step_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                continuation_context TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX idx_task_runs_session ON task_runs(session_id);
            CREATE INDEX idx_task_runs_message ON task_runs(message_id);",
        )
        .unwrap();

        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();

        // Now migrate() should detect the inconsistency, run v32, then v33.
        migrate(&conn).unwrap();

        // After recovery, agent_goals must allow empty session_id.
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('s1', 't', 0, 0)",
            [],
        )
        .unwrap();
        let res = conn.execute(
            "INSERT INTO agent_goals (id, session_id, goal_text, status, created_at) VALUES ('g1', '', 'empty-sid', 'pending', 0)",
            [],
        );
        assert!(
            res.is_ok(),
            "empty session_id must be insertable after recovery: {:?}",
            res
        );

        // Both v32 and v33 must be recorded as applied.
        let v32: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM migration_log WHERE version = 32 AND status = 'applied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v32, 1, "v32 must be re-applied");

        let v33: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM migration_log WHERE version = 33 AND status = 'applied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v33, 1, "v33 must be applied");
    }

    /// Verify that migration 032 is idempotent on its own — running it twice
    /// in a row (without our migrate() wrapper) must not fail.
    #[test]
    fn test_migration_032_is_idempotent() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        // Re-run v32 in isolation; must succeed.
        let sql = include_str!("migrations/032_fix_agent_goals_session_id.sql");
        conn.execute_batch(sql).expect("v32 must be safe to re-run");

        // Empty session_id still works.
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('s1', 't', 0, 0)",
            [],
        )
        .unwrap();
        let res = conn.execute(
            "INSERT INTO agent_goals (id, session_id, goal_text, status, created_at) VALUES ('g1', '', 'empty-sid', 'pending', 0)",
            [],
        );
        assert!(
            res.is_ok(),
            "empty session_id must still be insertable: {:?}",
            res
        );
    }

    #[test]
    fn context_window_setting_is_removed_when_upgrading() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('context_window_tokens', '8192', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE settings SET value = '26' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        // Recreate the v26 schema before replaying later migrations.
        // The normal upgrade path never downgrades schema_version; this fixture
        // deliberately does so to exercise migration 027.
        conn.execute("ALTER TABLE task_runs DROP COLUMN continuation_context", [])
            .unwrap();
        conn.execute("DROP TABLE task_run_facts", []).unwrap();
        conn.execute("ALTER TABLE sessions DROP COLUMN agent_provider", [])
            .unwrap();
        conn.execute("ALTER TABLE sessions DROP COLUMN agent_model", [])
            .unwrap();

        // Drop the columns added by v34 (tool protocol) and v35 (session tree)
        // so that re-running migrate() can replay them without "duplicate column"
        // errors. SQLite has no DROP COLUMN IF EXISTS, so we use table rebuilds.
        //
        // sessions.parent_id (v14) is a self-referential FK (REFERENCES sessions(id)).
        // sessions.parent_id VALUES in the backup reference the sessions table itself.
        // When re-enabling FK enforcement with non-NULL parent_id values, SQLite
        // cannot validate them → "FK mismatch sessions referencing messages".
        //
        // SOLUTION: same as fixture 1, plus DROP task_runs (orphan FK to messages).
        conn.execute("PRAGMA foreign_keys = OFF", []).unwrap();

        // Break the sessions.parent_id self-ref FK chain before backing up.
        conn.execute("UPDATE sessions SET parent_id = NULL", [])
            .unwrap();
        conn.execute("UPDATE messages SET parent_id = NULL", [])
            .unwrap();

        // Back up task_runs WITHOUT v28's continuation_context column.
        // task_runs has FKs to sessions(id) and messages(id) -- with FK=OFF, no validation needed.
        // Later migrations (v28, v29, v31, v34) need task_runs to exist.
        conn.execute(
            "CREATE TABLE task_runs_v27 AS
             SELECT id, session_id, message_id, goal, status, plan,
                    confirmation_state, resumable, step_count, completed_step_count,
                    created_at, updated_at
             FROM task_runs",
            [],
        )
        .unwrap();

        // task_runs FKs to sessions and messages -- DROP it before rebuilding sessions/messages.
        conn.execute("DROP TABLE task_runs", []).unwrap();

        // Back up sessions WITHOUT parent_id.
        conn.execute(
            "CREATE TABLE sessions_v34 AS
             SELECT id, title, created_at, updated_at, context_version,
                    last_compressed_at, metadata, work_dir,
                    branch_name, branch_created_at, additional_read_dirs
             FROM sessions",
            [],
        )
        .unwrap();

        // Back up messages WITHOUT v34/v35 columns.
        conn.execute(
            "CREATE TABLE messages_v34 AS
             SELECT id, session_id, role, content, metadata, created_at
             FROM messages",
            [],
        )
        .unwrap();

        // Drop and recreate sessions with pre-v35 schema (no v35 columns, no agent_provider/model).
        conn.execute("DROP TABLE IF EXISTS sessions", []).unwrap();
        conn.execute(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                context_version INTEGER NOT NULL DEFAULT 0,
                last_compressed_at INTEGER,
                metadata TEXT,
                work_dir TEXT,
                parent_id TEXT REFERENCES sessions(id),
                branch_name TEXT,
                branch_created_at INTEGER,
                additional_read_dirs TEXT NOT NULL DEFAULT '[]'
            )",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO sessions SELECT id, title, created_at, updated_at, context_version, last_compressed_at, metadata, work_dir, NULL AS parent_id, branch_name, branch_created_at, additional_read_dirs FROM sessions_v34", []).unwrap();
        conn.execute("DROP TABLE sessions_v34", []).unwrap();

        // Drop and recreate messages with pre-v34 schema (no v34/v35 columns).
        conn.execute("DROP INDEX IF EXISTS idx_messages_parent", [])
            .unwrap();
        conn.execute("DROP INDEX IF EXISTS idx_messages_tool_call_id", [])
            .unwrap();
        conn.execute("DROP TABLE IF EXISTS messages", []).unwrap();
        conn.execute(
            "CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT,
                created_at INTEGER NOT NULL
            )",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO messages SELECT id, session_id, role, content, metadata, created_at FROM messages_v34", []).unwrap();
        conn.execute("DROP TABLE messages_v34", []).unwrap();

        // Restore task_runs to pre-v28 state (no continuation_context).
        // v28 will ADD COLUMN continuation_context idempotently.
        conn.execute(
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                message_id TEXT NOT NULL REFERENCES messages(id),
                goal TEXT NOT NULL,
                status TEXT NOT NULL,
                plan TEXT NOT NULL,
                confirmation_state TEXT NOT NULL DEFAULT 'none',
                resumable INTEGER NOT NULL DEFAULT 0,
                step_count INTEGER NOT NULL DEFAULT 0,
                completed_step_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO task_runs SELECT id, session_id, message_id, goal, status, plan, confirmation_state, resumable, step_count, completed_step_count, created_at, updated_at FROM task_runs_v27", []).unwrap();
        conn.execute("DROP TABLE task_runs_v27", []).unwrap();

        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();

        migrate(&conn).unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'context_window_tokens'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn test_migration_rollback_on_error() {
        let (_tmp, conn) = setup_test_db();

        // Ensure settings table exists first
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL)",
        )
        .unwrap();

        // Run full migration first to get all tables created
        migrate(&conn).unwrap();
        let latest_schema_version = MIGRATIONS.last().map(|(version, _)| *version).unwrap();
        let version = get_setting_int(&conn, "schema_version", 0);
        assert_eq!(version, latest_schema_version);

        // Test that SQLite-level rollback works: execute a bad statement in a transaction
        conn.execute_batch("BEGIN").unwrap();
        let result = conn.execute_batch("THIS IS NOT VALID SQL");
        assert!(result.is_err(), "Bad SQL should fail");
        // Rollback should succeed
        conn.execute_batch("ROLLBACK").unwrap();

        // Database should still be in a consistent state
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(tables > 0, "Tables should still exist after rollback");

        // Running migrate again should be safe (idempotent)
        migrate(&conn).unwrap();
        assert_eq!(
            get_setting_int(&conn, "schema_version", 0),
            latest_schema_version
        );
    }

    #[test]
    fn test_task_runs_migration_columns_and_indexes() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(task_runs)").unwrap();
            stmt.query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        for expected in [
            "id",
            "session_id",
            "message_id",
            "goal",
            "status",
            "plan",
            "confirmation_state",
            "resumable",
            "step_count",
            "completed_step_count",
            "continuation_context",
            "created_at",
            "updated_at",
        ] {
            assert!(
                columns.contains(&expected.to_string()),
                "missing column {}",
                expected
            );
        }

        let indexes: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='task_runs'",
                )
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert!(indexes.contains(&"idx_task_runs_session".to_string()));
        assert!(indexes.contains(&"idx_task_runs_message".to_string()));
    }

    #[test]
    fn test_task_run_facts_migration_columns_and_index() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(task_run_facts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();
        for expected in [
            "task_run_id",
            "session_id",
            "message_id",
            "facts_json",
            "created_at",
            "updated_at",
        ] {
            assert!(
                columns.contains(&expected.to_string()),
                "missing column {expected}"
            );
        }

        let index_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_task_run_facts_session_message'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_exists, 1);
    }

    #[test]
    fn test_automation_script_runner_migration_columns_and_index() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let automation_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(automations)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for expected in [
            "executor_kind",
            "script_path",
            "script_args",
            "working_dir",
            "timeout_seconds",
            "workspace_id",
            "schedule_claim_token",
            "schedule_claimed_at",
        ] {
            assert!(
                automation_columns.contains(&expected.to_string()),
                "missing column {}",
                expected
            );
        }

        let run_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(automation_runs)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(run_columns.contains(&"exit_code".to_string()));
        assert!(run_columns.contains(&"output".to_string()));

        for index in [
            "idx_automations_executor_kind",
            "idx_automations_workspace_id",
            "idx_automations_schedule_claim",
        ] {
            let index_exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [index],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(index_exists, 1, "missing index {index}");
        }
    }

    #[test]
    fn automation_workspace_ownership_backfills_legacy_agent_definitions() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO automations (id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,script_args,timeout_seconds,created_at,updated_at) VALUES ('legacy-agent','Legacy','summarize notes','schedule','每天 09:00',1,'ask','agent','[]',300,1,1)",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM migration_log WHERE version = 66", [])
            .unwrap();
        conn.execute(
            "UPDATE settings SET value = '65' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let owner: Option<String> = conn
            .query_row(
                "SELECT workspace_id FROM automations WHERE id = 'legacy-agent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner.as_deref(), Some("personal"));
    }

    #[test]
    fn agent_automation_dispatch_migration_recovers_when_journal_lags() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();
        conn.execute("DELETE FROM migration_log WHERE version = 76", [])
            .unwrap();
        conn.execute(
            "UPDATE settings SET value = '75' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert!(automation_runs_has_column(&conn, "supervisor_input_id"));
        assert!(automation_runs_has_column(&conn, "foreground_run_id"));
        for index in [
            "idx_automation_runs_supervisor_input",
            "idx_automation_runs_foreground_run",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing index {index}");
        }
    }

    #[test]
    fn test_migration_records_timestamp() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let updated_at: i64 = conn
            .query_row(
                "SELECT updated_at FROM settings WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(updated_at > 0);
    }

    #[test]
    fn test_memory_frequency_decay_fields() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let now = chrono::Utc::now().timestamp();

        // Verify the memories table has all the columns from migration 002
        conn.execute(
            r#"INSERT INTO memories (id, scope, category, content, importance, source,
                frequency, last_mentioned, is_permanent, decay_factor, forget_stage,
                created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
            params![
                "test-memory-1",
                "global",
                "fact",
                "Test content",
                5,
                "test",
                3,
                now,
                1,
                0.8,
                "active",
                now,
                now
            ],
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE id = 'test-memory-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_permanent_memory_not_forgotten() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let now = chrono::Utc::now().timestamp();

        conn.execute(
            r#"INSERT INTO memories (id, scope, category, content, importance, source,
                frequency, last_mentioned, is_permanent, decay_factor, forget_stage,
                created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
            params![
                "permanent-memory",
                "global",
                "personality",
                "Core personality trait",
                10,
                "system",
                0,
                now - 10000000,
                1,
                0.1,
                "active",
                now,
                now
            ],
        )
        .unwrap();

        let is_perm: i32 = conn
            .query_row(
                "SELECT is_permanent FROM memories WHERE id = 'permanent-memory'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(is_perm, 1);
    }

    #[test]
    fn test_forget_stage_transitions() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        let now = chrono::Utc::now().timestamp();

        let stages = vec!["active", "summarized", "archived", "deleted"];
        for (i, stage) in stages.iter().enumerate() {
            conn.execute(
                r#"INSERT INTO memories (id, scope, category, content, importance, source,
                    frequency, last_mentioned, is_permanent, decay_factor, forget_stage,
                    created_at, updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
                params![
                    format!("memory-{}", i),
                    "global",
                    "fact",
                    format!("Content {}", i),
                    5,
                    "test",
                    0,
                    now,
                    0,
                    1.0,
                    stage,
                    now,
                    now
                ],
            )
            .unwrap();
        }

        let active_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE forget_stage = 'active'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active_count, 1);

        let archived_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE forget_stage = 'archived'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(archived_count, 1);
    }

    #[test]
    fn test_decay_factor_calculation() {
        let base_decay_rate: f64 = 0.9;
        let days_elapsed: f64 = 7.0;
        let expected_decay = base_decay_rate.powf(days_elapsed);
        assert!((expected_decay - 0.4783).abs() < 0.01);

        let decay_14_days = base_decay_rate.powf(14.0);
        assert!((decay_14_days - 0.2288).abs() < 0.01);

        let decay_30_days = base_decay_rate.powf(30.0);
        assert!((decay_30_days - 0.0424).abs() < 0.01);
    }

    #[test]
    fn test_forget_threshold() {
        let threshold = 0.5;
        let memory1_score = 5.0 * 0.9;
        assert!(memory1_score > threshold);
        let memory2_score = 1.0 * 0.1;
        assert!(memory2_score < threshold);
        let memory3_score = 2.0 * 0.2;
        assert!(memory3_score < threshold);
    }

    #[test]
    fn test_work_dir_column_exists() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        // Verify sessions has work_dir column (migration 003)
        let has_column: bool = conn
            .prepare("SELECT work_dir FROM sessions LIMIT 0")
            .is_ok();
        assert!(has_column);
    }

    #[test]
    fn test_session_provider_binding_migration_columns_exist() {
        let (_tmp, conn) = setup_test_db();
        migrate(&conn).unwrap();

        for column in ["agent_provider", "agent_model"] {
            assert!(
                conn.prepare(&format!("SELECT {column} FROM sessions LIMIT 0"))
                    .is_ok(),
                "sessions.{column} should exist after migration"
            );
        }
    }
}
