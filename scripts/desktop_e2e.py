#!/usr/bin/env python3
"""Build and smoke-test the real desktop shell in an isolated runtime."""

from __future__ import annotations

import contextlib
import argparse
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import sqlite3
import re
import socket
import subprocess
import sys
import tempfile
import zipfile

from native_uia_smoke import await_snapshot, owned_fixture


ROOT = Path(__file__).resolve().parents[1]
E2E_EXPLORER_OBJECTIVE = "检索 AngelBot 的公开测试资料并返回可核验结论。"
MCP_E2E_SERVER_ID = "desktop-e2e-mcp"
MCP_E2E_REMOTE_TOOL = "angelbot_e2e_ping"
MCP_E2E_TOOL_NAME = (
    f"mcp_desktop_e2e_mcp__{MCP_E2E_REMOTE_TOOL}_"
    f"{hashlib.sha256(f'{MCP_E2E_SERVER_ID}{chr(0)}{MCP_E2E_REMOTE_TOOL}'.encode()).hexdigest()[:8]}"
)
DESKTOP_OBSERVATION_STATE_KEYS = (
    "ordinaryValue", "invocationCount", "selectionCount", "expansionCount",
    "collapseCount", "checkboxState", "checkedCount", "uncheckedCount",
    "indeterminateCount", "scrollCount", "denseMode", "denseSwapped",
    "denseFieldValues", "denseFieldChangeCounts", "denseChecked",
    "denseCheckedCounts", "denseUncheckedCounts", "denseButtonInvocationCounts",
)


def office_attachment_fixture(parts: dict[str, str]) -> str:
    """A standard-library-only fixture; never read a user's document."""
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, content in parts.items():
            archive.writestr(name, content)
    return base64.b64encode(buffer.getvalue()).decode("ascii")


def docx_attachment_fixture() -> str:
    return office_attachment_fixture({
        "[Content_Types].xml": '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>',
        "_rels/.rels": '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>',
        "word/document.xml": '<d:document xmlns:d="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><d:body><d:p><d:r><d:t>今天核对安排 &amp; 本地提醒</d:t></d:r></d:p><d:tbl><d:tr><d:tc><d:p><d:r><d:t>事项</d:t></d:r></d:p></d:tc><d:tc><d:p><d:r><d:t>核对今天的安排</d:t></d:r></d:p></d:tc></d:tr></d:tbl></d:body></d:document>',
        "word/header1.xml": "private-header-not-in-body",
    })


def xlsx_attachment_fixture(*, formula: bool = False) -> str:
    cell = '<c r="A1" t="s"><v>0</v></c>' if not formula else '<c r="A1"><f>1+1</f><v>2</v></c>'
    return office_attachment_fixture({
        "[Content_Types].xml": '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>',
        "_rels/.rels": '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>',
        "xl/workbook.xml": '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="本地事项" sheetId="1" r:id="s1"/></sheets></workbook>',
        "xl/_rels/workbook.xml.rels": '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="s1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>',
        "xl/worksheets/sheet1.xml": f'<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1">{cell}<c r="B1"><v>0</v></c></row></sheetData><mergeCells><mergeCell ref="A1:A2"/></mergeCells></worksheet>',
        "xl/sharedStrings.xml": '<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>native-xlsx-marker</t></si></sst>',
        "xl/comments1.xml": "private-comment-not-in-values",
    })


def run(command: list[str], env: dict[str, str]) -> None:
    subprocess.run(command, cwd=ROOT, env=env, check=True)


def prepare_desktop_binary(npx: str, env: dict[str, str], skip_build: bool) -> Path:
    """Keep test-feature artifacts separate from the normal desktop build."""
    target_dir = ROOT / "src-tauri" / "target" / "desktop-e2e"
    binary = target_dir / "debug" / "angelbot.exe"
    # Override inherited Cargo/output settings; WDIO consumes this same path.
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["ANGELBOT_DESKTOP_E2E_BINARY_PATH"] = str(binary)
    if not skip_build:
        run([
            npx, "tauri", "build", "--debug", "--no-bundle",
            "--features", "desktop-e2e", "--config",
            "src-tauri/tauri.desktop-e2e.conf.json",
        ], env)
    if not binary.is_file():
        raise RuntimeError(
            f"Desktop E2E binary is missing: {binary}. "
            "Run python scripts/desktop_e2e.py without --skip-build first."
        )
    return binary


def configure_webdriver_port(env: dict[str, str]) -> None:
    """Avoid occupied/Windows-reserved fixed ports without changing the OS."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        env["TAURI_WEBDRIVER_PORT"] = str(probe.getsockname()[1])


def configure_reply_export_probe(env: dict[str, str], runtime_dir: Path) -> None:
    """Real native pickers; synthetic reply/files, never a GUI mock."""
    export_dir = runtime_dir / "reply-exports"
    export_dir.mkdir()
    results_dir = runtime_dir / "angelbot-data" / "results"
    results_dir.mkdir(parents=True)
    (results_dir / "验收结果.csv").write_bytes("事项,状态\n本机验收,完成\n".encode("utf-8"))
    (results_dir / "download.bin").write_bytes(bytes([0, 255, 1, 128]))
    env["ANGELBOT_DESKTOP_E2E_REPLY_EXPORT"] = "1"
    env["ANGELBOT_DESKTOP_E2E_EXPORT_DIR"] = str(export_dir)
    env["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"] = json.dumps([
        {"type": "text", "text": "# 保存回复验收\n\n- 中文事项一\n- item two\n\n仅用于本机测试。\n\n[验收结果.csv](angelbot-file:验收结果.csv)"},
    ], ensure_ascii=False)
    env.pop("ANGELBOT_DESKTOP_E2E_SEED_NETWORK_APPROVAL", None)
    env.pop("ANGELBOT_DESKTOP_E2E_EXPLORER_SCRIPT", None)
    print(f"Native reply-export checkpoint: {export_dir / 'stage.json'}", flush=True)


def run_reply_export_probe(npx: str, env: dict[str, str], runtime_dir: Path, skip_build: bool) -> None:
    """Seal startup failures too; the JS spec only owns post-startup evidence."""
    report_path = ROOT / "src-tauri/target/e2e/reply-export-desktop.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report = {"status": "running", "phase": "preparing", "nativeCancellation": False,
              "markdownExact": False, "textExact": False, "transcriptUnchanged": False,
              "dailyPreviewExact": False, "dailyNativeCancellation": False, "binaryExportExact": False}
    report_path.write_text(json.dumps(report), encoding="utf-8")
    try:
        configure_reply_export_probe(env, runtime_dir)
        prepare_desktop_binary(npx, env, skip_build)
        configure_webdriver_port(env)
        run([npx, "wdio", "run", "e2e/wdio.conf.mjs"], env)
        completed = json.loads(report_path.read_text(encoding="utf-8"))
        if completed.get("status") != "passed" or not all(completed.get(key) is True for key in (
                "nativeCancellation", "markdownExact", "textExact", "transcriptUnchanged",
                "dailyPreviewExact", "dailyNativeCancellation", "binaryExportExact")):
            raise RuntimeError("Native reply-export acceptance did not seal all required checks.")
    except BaseException:
        try:
            report = json.loads(report_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            pass
        report["status"] = "failed"
        report_path.write_text(json.dumps(report), encoding="utf-8")
        raise


def owned_desktop_control_replies() -> list[dict]:
    """Drive two approved effects using only references returned by real UIA."""
    replies = []
    for phase, action in (("before", "check"), ("after-check", "uncheck"), ("after-uncheck", None)):
        root_id, region_id = f"cu-root-{phase}", f"cu-region-{phase}"
        replies.extend([
            {
                "type": "tool_calls",
                "text": "我会重新观察授权窗口的 A 区；每次改变控件前都先请你确认。",
                "calls": [{
                    "id": root_id,
                    "name": "observe_trusted_app_window",
                    "arguments": {"app_id": "desktop-e2e-window", "mode": "controls"},
                }],
            },
            {
                "type": "desktop_observed_region", "callId": region_id,
                "sourceCallId": root_id, "appId": "desktop-e2e-window",
                "regionName": "Fixture region A",
            },
        ])
        if action:
            replies.append({
                "type": "desktop_observed_control", "callId": f"cu-{action}",
                "sourceCallId": region_id, "rootCallId": root_id,
                "appId": "desktop-e2e-window", "regionName": "Fixture region A",
                "controlName": "Region checkbox", "action": action,
            })
    replies.append({
        "type": "text",
        "text": "已按两次确认将 A 区复选框勾选并恢复为未选中；控件状态由操作回读确认，未提交其他操作。",
        "expectCurrentDesktopObservation": {
            "callId": "cu-region-after-uncheck", "sourceCallId": "cu-root-after-uncheck",
            "appId": "desktop-e2e-window", "regionName": "Fixture region A",
        },
    })
    return replies


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
        # Inspect only this runner's isolated database, including private
        # transcript/event columns that the public message projection omits.
        tables = connection.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' "
            "AND COALESCE(sql, '') NOT LIKE 'CREATE VIRTUAL TABLE%'"
        ).fetchall()
        # Python's SQLite has no vec0 module. Its ordinary backing/shadow tables
        # are still scanned; do not query the unavailable virtual-table view.
        for table in tables:
            quoted = '"' + table[0].replace('"', '""') + '"'
            for row in connection.execute(f"SELECT * FROM {quoted}"):
                if any(
                    isinstance(value, str)
                    and ("data:image/png;base64," in value or "iVBORw0KGgo" in value)
                    or isinstance(value, bytes) and b"\x89PNG\r\n\x1a\n" in value
                    for value in row
                ):
                    raise AssertionError("Transient window image leaked into durable E2E data")
    finally:
        connection.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--keep-data", action="store_true")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--reply-export", action="store_true")
    modes.add_argument("--manual-native", action="store_true",
                       help="Real saved ChatGPT model + owned native window; the user handles approvals.")
    modes.add_argument("--model-recovery", action="store_true",
                       help="Isolated native conversation: model failure, next message and application reopen.")
    modes.add_argument("--project-restore", action="store_true",
                       help="Isolated encrypted restore; a person must choose and approve project directories.")
    parser.add_argument("--inspect", action="store_true",
                        help="With --model-recovery, hold the final window for read-only native inspection (up to 120s).")
    args = parser.parse_args()
    if args.inspect and not args.model_recovery:
        parser.error("--inspect requires --model-recovery")
    skip_build, keep_data, reply_export = args.skip_build, args.keep_data, args.reply_export
    if args.project_restore:
        from desktop_project_restore import run_project_restore
        return run_project_restore(ROOT, prepare_desktop_binary, configure_webdriver_port,
                                   skip_build, keep_data)
    if args.model_recovery:
        from desktop_model_recovery import run_model_recovery
        return run_model_recovery(ROOT, prepare_desktop_binary, configure_webdriver_port,
                                  skip_build, keep_data, args.inspect)
    if args.manual_native:
        from desktop_manual_acceptance import run_manual_native
        return run_manual_native(ROOT, prepare_desktop_binary, skip_build)
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
                            "expectUserContentIncludes": ["今日安排.docx", "DOCX 正文文字快照", "今天核对安排 & 本地提醒", "本地事项.xlsx", "XLSX 单元格值快照", "native-xlsx-marker", "Windows.log", "native-utf16-marker"],
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
                            "type": "tool_calls",
                            "calls": [{"id": "main-stop-list-rejected", "name": "list_automations",
                                       "arguments": {"query": "停止本次执行检查"}}],
                        },
                        {
                            "type": "automation_stop", "callId": "main-stop-rejected",
                            "sourceCallId": "main-stop-list-rejected", "title": "停止本次执行检查",
                        },
                        {
                            "type": "tool_calls",
                            "calls": [{"id": "main-stop-list-approved", "name": "list_automations",
                                       "arguments": {"query": "停止本次执行检查"}}],
                        },
                        {
                            "type": "automation_stop", "callId": "main-stop-approved",
                            "sourceCallId": "main-stop-list-approved", "title": "停止本次执行检查",
                        },
                        {
                            "type": "text",
                            "text": "停止请求已接受，清理结果请查看当前对话的事项回看；已发生的操作不会撤回。",
                        },
                        {
                            "type": "text",
                            "text": "已读取你选择的日常行动清单；这是文件快照，不会修改原文件。",
                            "expectUserContentIncludes": ["daily-actions.csv", "attachment-e2e-marker"],
                            "forbiddenToolNames": ["run_project_command"],
                        },
                        {
                            "type": "text",
                            "text": "上一轮的文件快照仍在当前日常会话中，可以继续整理。",
                            "expectUserContentIncludes": ["daily-actions.csv", "attachment-e2e-marker"],
                            "forbiddenToolNames": ["run_project_command"],
                        },
                        {
                            "type": "text",
                            "text": "已按编辑后的要求重新整理，保留了原先选择的文件快照。",
                            "expectUserContentIncludes": ["daily-actions.csv", "attachment-e2e-marker"],
                            "forbiddenToolNames": ["run_project_command"],
                        },
                        {
                            "type": "tool_calls",
                            "text": "请先核对 MAA 自动化范围；本次测试只查看确认卡，不运行游戏。",
                            "calls": [{
                                "id": "desktop-e2e-maa-consent",
                                "name": "create_automation",
                                "arguments": {
                                    "title": "MAA 确认测试",
                                    "schedule": "每天 18:00",
                                    "executorKind": "maa",
                                    "appId": "desktop-e2e-maa-not-executed",
                                    "launchEmulator": True,
                                    "timeoutSeconds": 1800,
                                },
                            }],
                        },
                        {
                            "type": "tool_calls",
                            "text": "只有授权仍然有效才会打开测试应用。",
                            "calls": [{
                                "id": "desktop-e2e-revoked-app",
                                "name": "open_trusted_app",
                                "arguments": {"app_id": "notepad"},
                            }],
                        },
                        {
                            "type": "tool_calls",
                            "text": "请确认是否只查看临时测试窗口的图像。",
                            "calls": [{
                                "id": "desktop-e2e-rejected-image",
                                "name": "observe_trusted_app_window",
                                "arguments": {"app_id": "desktop-e2e-window", "mode": "image"},
                            }],
                        },
                        {
                            "type": "tool_calls",
                            "text": "我会只观察临时测试窗口，不点击或填写。",
                            "calls": [{
                                "id": "desktop-e2e-window-image",
                                "name": "observe_trusted_app_window",
                                "arguments": {"app_id": "desktop-e2e-window", "mode": "image"},
                            }],
                        },
                        {
                            "type": "text",
                            "text": "已看到测试窗口中的两组色块；这是只读观察，没有点击或填写。",
                            "expectToolImageCallIds": ["desktop-e2e-window-image"],
                        },
                        {
                            "type": "text",
                            "text": "上一轮图像已释放，当前只保留文字结果。",
                        },
                        *owned_desktop_control_replies(),
                        {
                            "type": "text",
                            "text": "今日安排已整理到这一步。",
                            "finishReason": "length",
                        },
                        {
                            "type": "text",
                            "text": "已核对现有回复；本轮只补充说明，没有重复任何操作。",
                            "expectUserContentIncludes": ["先核对现有结果，只补充说明，不重复操作。"],
                        },
                    ],
                    ensure_ascii=False,
                ),
                "VITE_ANGELBOT_DESKTOP_E2E": "1",
                "ANGELBOT_DESKTOP_E2E_DATA_DIR": str(data_dir),
                "ANGELBOT_DESKTOP_E2E_DOCX_BASE64": docx_attachment_fixture(),
                "ANGELBOT_DESKTOP_E2E_XLSX_BASE64": xlsx_attachment_fixture(),
                "ANGELBOT_DESKTOP_E2E_FORMULA_XLSX_BASE64": xlsx_attachment_fixture(formula=True),
                "ANGELBOT_DESKTOP_E2E_UTF16_BASE64": base64.b64encode(b"\xff\xfe" + "Windows 日志\r\nnative-utf16-marker".encode("utf-16-le")).decode("ascii"),
                "APPDATA": runtime_dir,
                "LOCALAPPDATA": runtime_dir,
            }
        )
        npx = "npx.cmd" if os.name == "nt" else "npx"
        if reply_export:
            run_reply_export_probe(npx, env, Path(runtime_dir), skip_build)
            return 0
        prepare_desktop_binary(npx, env, skip_build)
        configure_webdriver_port(env)
        # Start only after the potentially long desktop build. Always remove
        # this owned window, including on WDIO failure or --keep-data runs.
        with tempfile.TemporaryDirectory(prefix="angelbot-e2e-window-") as fixture_dir:
            with owned_fixture(Path(fixture_dir), env, dense=True) as fixture:
                env["ANGELBOT_DESKTOP_E2E_WINDOW_EXE"] = str(fixture.executable)
                before = await_snapshot(fixture.snapshot_path, fixture.process, "ordinary-seed", 5)
                # Only the Node test runner reads this owned fixture snapshot.
                # Check the read-only image phase before later intentional UIA
                # effects, so those effects cannot conceal an earlier mutation.
                env["ANGELBOT_DESKTOP_E2E_WINDOW_SNAPSHOT"] = str(fixture.snapshot_path)
                env["ANGELBOT_DESKTOP_E2E_WINDOW_BASELINE"] = json.dumps({
                    key: before.get(key)
                    for key in (*DESKTOP_OBSERVATION_STATE_KEYS, "processId", "windowTitle")
                })
                run([npx, "wdio", "run", "e2e/wdio.conf.mjs"], env)
                after = await_snapshot(fixture.snapshot_path, fixture.process, "ordinary-seed", 5)
                for snapshot in (before, after):
                    if (
                        snapshot.get("processId") != fixture.process.pid
                        or snapshot.get("windowTitle") != fixture.title
                        or snapshot.get("denseMode") is not True
                    ):
                        raise AssertionError("Owned observation fixture identity was lost")
                for key in DESKTOP_OBSERVATION_STATE_KEYS:
                    if key in {"denseCheckedCounts", "denseUncheckedCounts"}:
                        continue
                    if before.get(key) != after.get(key):
                        raise AssertionError(f"Owned desktop task changed unrelated fixture state: {key}")
                for key in ("denseCheckedCounts", "denseUncheckedCounts"):
                    if before.get(key) != [0, 0, 0, 0] or after.get(key) != [1, 0, 0, 0]:
                        raise AssertionError(f"Owned desktop task did not act exactly once in region A: {key}")
        assert_terminal_e2e_cleanup(data_dir)
    return 0


if __name__ == "__main__":
    sys.exit(main())
