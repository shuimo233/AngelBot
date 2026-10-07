#!/usr/bin/env python3
"""Build and smoke-test the real desktop shell in an isolated runtime."""

from __future__ import annotations

import contextlib
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import re
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
E2E_EXPLORER_OBJECTIVE = "检索 AngelBot 的公开测试资料并返回可核验结论。"
MCP_E2E_SERVER_ID = "desktop-e2e-mcp"
MCP_E2E_REMOTE_TOOL = "angelbot_e2e_ping"
MCP_E2E_TOOL_NAME = (
    f"mcp_desktop_e2e_mcp__{MCP_E2E_REMOTE_TOOL}_"
    f"{hashlib.sha256(f'{MCP_E2E_SERVER_ID}{chr(0)}{MCP_E2E_REMOTE_TOOL}'.encode()).hexdigest()[:8]}"
)


def run(command: list[str], env: dict[str, str]) -> None:
    subprocess.run(command, cwd=ROOT, env=env, check=True)


def assert_terminal_e2e_cleanup(data_dir: Path) -> None:
    """Verify the delegated Explorer left no live capability behind.

    WDIO covers the user-visible review and summary.  This test-only check
    reads the isolated runtime database after WDIO succeeds so a successful
    UI journey cannot conceal a leaked attempt, lease, admission, or sandbox.
    """

    database_path = data_dir / "data.db"
    if not database_path.is_file():
        raise AssertionError(f"Desktop E2E runtime database was not created: {database_path}")

    connection = sqlite3.connect(f"{database_path.as_uri()}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    try:
        delegations = connection.execute(
            "SELECT id, status FROM delegations WHERE objective = ?",
            (E2E_EXPLORER_OBJECTIVE,),
        ).fetchall()
        if len(delegations) != 1:
            raise AssertionError(
                "expected exactly one delegated Explorer for the E2E objective; "
                f"found {len(delegations)}"
            )
        delegation = delegations[0]
        if delegation["status"] != "completed":
            raise AssertionError(
                "expected the delegated Explorer to complete; "
                f"found status={delegation['status']!r}"
            )

        attempts = connection.execute(
            "SELECT id, status FROM delegation_attempts WHERE delegation_id = ?",
            (delegation["id"],),
        ).fetchall()
        if len(attempts) != 1:
            raise AssertionError(
                "expected exactly one Explorer attempt for the E2E delegation; "
                f"found {len(attempts)}"
            )
        attempt = attempts[0]
        if attempt["status"] != "sealed":
            raise AssertionError(
                "expected the Explorer attempt to be sealed; "
                f"found status={attempt['status']!r}"
            )

        leases = connection.execute(
            "SELECT status, revoked_at FROM delegation_capability_leases WHERE attempt_id = ?",
            (attempt["id"],),
        ).fetchall()
        if len(leases) != 1 or leases[0]["status"] != "revoked" or leases[0]["revoked_at"] is None:
            raise AssertionError(
                "expected one revoked Explorer capability lease with a revocation timestamp"
            )

        admissions = connection.execute(
            "SELECT status, released_at FROM workspace_admissions WHERE attempt_id = ?",
            (attempt["id"],),
        ).fetchall()
        active_admissions = connection.execute(
            "SELECT COUNT(*) FROM workspace_admissions WHERE attempt_id = ? AND status = 'active'",
            (attempt["id"],),
        ).fetchone()[0]
        if (
            len(admissions) != 1
            or admissions[0]["status"] != "released"
            or admissions[0]["released_at"] is None
            or active_admissions != 0
        ):
            raise AssertionError(
                "expected one released Explorer workspace admission and no active admission"
            )

        sandboxes = connection.execute(
            """
            SELECT lifecycle_state, cleanup_status, sealed_at, revoked_at, cleaned_at
            FROM delegation_resource_bindings
            WHERE attempt_id = ? AND resource_kind = 'sandbox'
            """,
            (attempt["id"],),
        ).fetchall()
        if len(sandboxes) != 1:
            raise AssertionError(
                "expected exactly one sandbox resource binding for the Explorer attempt; "
                f"found {len(sandboxes)}"
            )
        sandbox = sandboxes[0]
        if (
            sandbox["lifecycle_state"] != "quarantined"
            or sandbox["cleanup_status"] != "cleaned"
            or any(sandbox[timestamp] is None for timestamp in ("sealed_at", "revoked_at", "cleaned_at"))
        ):
            raise AssertionError(
                "expected the Explorer sandbox to be quarantined and cleaned with terminal timestamps"
            )
    finally:
        connection.close()


def main() -> int:
    skip_build = "--skip-build" in sys.argv[1:]
    keep_data = "--keep-data" in sys.argv[1:]
    runtime_directory = (
        contextlib.nullcontext(tempfile.mkdtemp(prefix="angelbot-desktop-e2e-"))
        if keep_data
        else tempfile.TemporaryDirectory(prefix="angelbot-desktop-e2e-")
    )
    with runtime_directory as runtime_dir:
        data_dir = Path(runtime_dir) / "angelbot-data"
        if keep_data:
            print(f"Desktop E2E runtime retained at: {runtime_dir}", flush=True)
        # Match the native smoke's child-environment hygiene. The fixture uses
        # scripted providers and must not inherit a developer's live secrets.
        sensitive = re.compile(r"TOKEN|API.?KEY|SECRET|PASSWORD|CREDENTIAL", re.IGNORECASE)
        env = {name: value for name, value in os.environ.items() if not sensitive.search(name)}
        env.update({"CARGO_NET_OFFLINE": "true", "NPM_CONFIG_OFFLINE": "true"})
        env.update(
            {
                "ANGELBOT_DESKTOP_E2E": "1",
                "ANGELBOT_DESKTOP_E2E_SEED_NETWORK_APPROVAL": "1",
                "ANGELBOT_DESKTOP_E2E_EXPLORER_SCRIPT": json.dumps(
                    {
                        "exchanges": [
                            {
                                "host": "docs.example.test",
                                "method": "GET",
                                "path_and_query": "/search?q=angelbot-e2e",
                                "status": 200,
                                "content_type": "application/json",
                                "body": json.dumps(
                                    {
                                        "results": [
                                            {
                                                "title": "AngelBot E2E documentation",
                                                "url": "https://docs.example.test/angelbot-e2e",
                                                "snippet": "Deterministic public Explorer evidence.",
                                            }
                                        ]
                                    }
                                ),
                            }
                        ]
                    },
                    ensure_ascii=False,
                ),
                "ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT": json.dumps(
                    [
                        {
                            "type": "tool_calls",
                            "text": "我会设置一个不调用模型的本地提醒。",
                            "calls": [
                                {
                                    "id": "desktop-e2e-reminder",
                                    "name": "schedule_reminder",
                                    "arguments": {
                                        "title": "核对今天的安排",
                                        "body": "回看今天的安排，不执行外部操作。",
                                        "when": (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat(),
                                    },
                                }
                            ],
                        },
                        {
                            "type": "text",
                            "text": "本地提醒已设置，你可以在当前对话中回看。",
                        },
                        {
                            "type": "tool_calls",
                            "text": "我会把这项重复工作交给当前工作区的自动化处理。",
                            "calls": [
                                {
                                    "id": "desktop-e2e-create-automation",
                                    "name": "create_automation",
                                    "arguments": {
                                        "title": "每日邮件行动清单",
                                        "prompt": "检查待处理邮件并生成行动清单；只整理和起草，不要发送邮件。",
                                        "schedule": "每天 09:00",
                                    },
                                }
                            ],
                        },
                        {
                            "type": "text",
                            "text": "自动化已创建，并已绑定到当前工作区。",
                        },
                        {
                            "type": "tool_calls",
                            "text": "需要你允许后才会打开 Windows 声音设置。",
                            "calls": [
                                {
                                    "id": "desktop-e2e-open-sound-settings",
                                    "name": "open_windows_setting",
                                    "arguments": {"page": "sound"},
                                }
                            ],
                        },
                        {
                            "type": "text",
                            "text": "已取消打开 Windows 声音设置，并先说明如何调整声音。",
                        },
                        {
                            "type": "text",
                            "text": "自动化已执行：已整理今日待处理邮件，未发送任何邮件。",
                        },
                        {
                            "type": "tool_calls",
                            "text": "我会调用当前项目已启用的本地 MCP 工具；执行前需要你确认。",
                            "calls": [
                                {
                                    "id": "desktop-e2e-mcp-call",
                                    "name": MCP_E2E_TOOL_NAME,
                                    "arguments": {},
                                }
                            ],
                        },
                        {
                            "type": "text",
                            "text": "本地 MCP 工具已返回校验结果。",
                        },
                        {
                            "type": "tool_calls",
                            "text": "需要你允许后才会在当前项目中写入测试文件。",
                            "calls": [
                                {
                                    "id": "desktop-e2e-rejected-write",
                                    "name": "write_file",
                                    "arguments": {
                                        "path": "rejected-by-e2e.txt",
                                        "content": "this must never be written",
                                    },
                                }
                            ],
                        },
                        {
                            "type": "tool_calls",
                            "text": "需要你允许后才会写入已批准的测试文件。",
                            "calls": [
                                {
                                    "id": "desktop-e2e-approved-write",
                                    "name": "write_file",
                                    "arguments": {
                                        "path": "approved-by-e2e.txt",
                                        "content": "approved desktop E2E file content\n",
                                    },
                                }
                            ],
                        },
                        {
                            "type": "text",
                            "text": "文件已写入：[查看 E2E 文件](angelbot-file:approved-by-e2e.txt)",
                        },
                        {
                            "type": "tool_calls",
                            "text": "需要你确认后，才能委派一次受限的公开资料检索。",
                            "calls": [
                                {
                                    "id": "desktop-e2e-network-delegation",
                                    "name": "delegate_network_exploration",
                                    "arguments": {
                                        "goal": "检索 AngelBot 的公开测试资料并返回可核验结论。",
                                        "explorer_operations": [
                                            {
                                                "kind": "search",
                                                "provider_host": "docs.example.test",
                                                "query": "angelbot-e2e",
                                            }
                                        ],
                                    },
                                }
                            ],
                        },
                        {
                            "type": "text",
                            "text": "已委派受限公开资料检索；完成后我会汇总可核验的结论。",
                        },
                        {
                            "type": "text",
                            "text": "已读取你选择的日常行动清单；这是文件快照，不会修改原文件。",
                            "expectUserContentIncludes": ["daily-actions.csv", "attachment-e2e-marker"],
                            "forbiddenToolNames": ["read_file", "write_file"],
                        },
                        {
                            "type": "text",
                            "text": "上一轮的文件快照仍在当前日常会话中，可以继续整理。",
                            "expectUserContentIncludes": ["daily-actions.csv", "attachment-e2e-marker"],
                            "forbiddenToolNames": ["read_file", "write_file"],
                        },
                        {
                            "type": "text",
                            "text": "已按编辑后的要求重新整理，保留了原先选择的文件快照。",
                            "expectUserContentIncludes": ["daily-actions.csv", "attachment-e2e-marker"],
                            "forbiddenToolNames": ["read_file", "write_file"],
                        },
                    ],
                    ensure_ascii=False,
                ),
                "VITE_ANGELBOT_DESKTOP_E2E": "1",
                "ANGELBOT_DESKTOP_E2E_DATA_DIR": str(data_dir),
                "APPDATA": runtime_dir,
                "LOCALAPPDATA": runtime_dir,
            }
        )
        npx = "npx.cmd" if os.name == "nt" else "npx"
        if not skip_build:
            run(
                [
                    npx,
                    "tauri",
                    "build",
                    "--debug",
                    "--no-bundle",
                    "--features",
                    "desktop-e2e",
                    "--config",
                    "src-tauri/tauri.desktop-e2e.conf.json",
                ],
                env,
            )
        run([npx, "wdio", "run", "e2e/wdio.conf.mjs"], env)
        assert_terminal_e2e_cleanup(data_dir)
    return 0


if __name__ == "__main__":
    sys.exit(main())
