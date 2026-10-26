"""Read-only, target-owned database evidence for the native restore probe."""

from __future__ import annotations

import contextlib
import json
from pathlib import Path
import sqlite3


TABLES = ("projects", "sessions", "messages", "context_summaries", "session_branches",
          "task_runs", "task_run_facts", "agent_steps", "agent_run_events")
EMPTY_QUEUES = ("automation_runs", "delegations", "workspace_supervisor_inputs",
                "workspace_supervisor_work", "delegated_delivery_follow_ups", "agent_events")
FACT_DEFAULTS = {
    "plan": [], "completed_steps": [], "failed_steps": [], "modified_files": [],
    "verification_evidence": [], "pending_confirmation": None, "repair_attempts": 0,
    "no_progress_count": 0, "context_summary": None, "provider_attempts": 0,
    "provider_binding": None, "terminal_reason": None,
}


def require(condition: bool, detail: str) -> None:
    if not condition:
        raise AssertionError(f"Native restore database check failed: {detail}")


def check_restored_database(target_db: Path, source_db: Path, manifest: dict) -> dict:
    target, source = target_db.resolve(), source_db.resolve()
    require(target != source, "source and target database paths must differ")
    require(target.is_file(), "explicit target database is missing")
    expected = manifest["expected"]
    require(set(expected) == set(TABLES), "nine declared fixture tables are required")
    terminal_run_ids = {row["id"] for row in expected["task_runs"]
                        if row["status"] in ("completed", "failed", "stopped", "cancelled")}
    project_a, project_b = manifest["projectA"], manifest["projectB"]
    counts = {}
    with contextlib.closing(sqlite3.connect(target.as_uri() + "?mode=ro", uri=True)) as db:
        db.row_factory = sqlite3.Row
        tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        require(set((*TABLES, *EMPTY_QUEUES, "automations", "scheduled_tasks")) <= tables,
                "required restore or execution-queue schema is missing")
        require(not db.execute("PRAGMA foreign_key_check").fetchall(), "foreign key violation")
        actual = {}
        for table in TABLES:
            key = "task_run_id" if table == "task_run_facts" else "id"
            rows = [dict(row) for row in db.execute(f'SELECT * FROM "{table}"')]
            actual[table] = {row[key]: row for row in rows}
            declared = {row[key]: row for row in expected[table]}
            require(len(declared) == len(expected[table]), f"duplicate fixture identity in {table}")
            require(set(actual[table]) == set(declared), f"exact {table} identity set")
            counts[table] = len(rows)
            for identity, declared_row in declared.items():
                wanted = dict(declared_row)
                row = actual[table][identity]
                if (table == "projects" and identity == "personal") or (
                        table == "sessions" and identity == "personal-main"):
                    for field in ("created_at", "updated_at"):
                        wanted.pop(field, None)
                if table == "projects":
                    wanted["directory_confirmed"] = int(identity in ("personal", project_a["id"]))
                    if identity == project_a["id"]:
                        wanted["path"] = project_a["mappedPath"]
                        # The real user's confirmation advances this timestamp.
                        wanted.pop("updated_at", None)
                elif table == "sessions":
                    wanted["work_dir"] = (project_a["mappedPath"]
                                          if row["project_id"] == project_a["id"] else None)
                    wanted["additional_read_dirs"] = "[]"
                    if identity == "personal-main" and wanted.get("title") == "":
                        # Loading this fixture auto-titles the empty daily Main.
                        # Keep the canary within the UI's unchanged-title range,
                        # rather than duplicating its general title formatter.
                        users = [message for message in expected["messages"]
                                 if message["session_id"] == identity and message["role"] == "user"]
                        require(len(users) == 1, "daily title fixture requires exactly one user message")
                        title = users[0]["content"]
                        require(bool(title) and title.isascii() and len(title) <= 40
                                and title == " ".join(title.split()), "plain daily title canary is required")
                        wanted["title"] = title
                elif table == "messages":
                    wanted["is_provisional"] = 0
                elif table == "task_runs":
                    wanted["resumable"] = 0
                    if wanted["status"] not in ("completed", "failed", "stopped", "cancelled"):
                        wanted.update(status="needs_attention", confirmation_state="none")
                for field, value in wanted.items():
                    require(field in row, f"unknown fixture column {table}.{field}")
                    if table == "task_run_facts" and field == "facts_json":
                        value = {**FACT_DEFAULTS, **json.loads(value), "pending_confirmation": None}
                        terminal_default = value["terminal_reason"] is None and identity in terminal_run_ids
                        if value["terminal_reason"] not in ("completed", "stopped") and not terminal_default:
                            value["terminal_reason"] = "needs_attention"
                        observed = json.loads(row[field])
                    else:
                        observed = row[field]
                    require(observed == value, f"exact {table}.{field} for {identity}")
        projects, sessions = actual["projects"], actual["sessions"]
        for project in (project_a, project_b):
            require(projects[project["id"]]["active_session_id"] == project["sessionId"],
                    "restored Main identity changed")
        require(projects[project_b["id"]]["path"] == project_b["originalPath"]
                and projects[project_b["id"]]["directory_confirmed"] == 0,
                "excluded project must remain an unconfirmed path hint")
        require(all(row["work_dir"] is None for row in sessions.values()
                    if row["project_id"] == project_b["id"]), "excluded session acquired a root")
        for table in EMPTY_QUEUES:
            require(db.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0] == 0,
                    f"unexpected execution or replay in {table}")
        for table in ("automations", "scheduled_tasks"):
            require(db.execute(f'SELECT COUNT(*) FROM "{table}" WHERE enabled=1').fetchone()[0] == 0,
                    f"enabled trigger in {table}")
    return {"databaseVerified": True, "foreignKeysClean": True, "noReplay": True,
            "restoredRows": counts}
