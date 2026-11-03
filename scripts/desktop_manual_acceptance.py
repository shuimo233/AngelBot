"""User-operated native acceptance; no WebDriver, scripted model or auto-approval.

Launched only by desktop_e2e.py --manual-native. All state except the existing
ChatGPT credential handle lives in a new ignored evidence directory. This is
not an offline test and cannot pass merely because the applications launched.
"""

from __future__ import annotations

import json
import contextlib
import os
from pathlib import Path
import sqlite3
import subprocess
import uuid

from native_uia_smoke import await_snapshot, isolated_environment, owned_fixture


# Matches the settings page's suggestion for AngelBotNativeUiaFixture.exe.
APP_ID = "angelbotnativeuiafixture"
SAFE_TOOLS = {"observe_trusted_app_window", "set_trusted_app_text", "think", "check_constraint", "verify_result"}
UNCHANGED = ("readOnlyValue", "duplicateOneValue", "duplicateTwoValue", "passwordUnchanged",
             "invocationCount", "selected", "selectionCount", "checkboxState", "checkedCount",
             "uncheckedCount", "indeterminateCount", "expansionCount", "collapseCount", "scrollCount")


def saved_plan_profile(path: Path) -> dict:
    """Copy only the stored profile/handle, never legacy keys or account labels."""
    raw = json.loads(path.read_text(encoding="utf-8-sig"))
    if (not isinstance(raw, dict) or raw.get("provider") != "openai"
            or raw.get("auth_mode") != "chatgpt_plan"
            or not isinstance(raw.get("model"), str) or not raw["model"].strip()
            or not isinstance(raw.get("credential_ref"), str) or not raw["credential_ref"].strip()):
        raise RuntimeError("请先在普通 AngelBot 中保存已授权的 ChatGPT 套餐配置；验收不切换模型或回退付费 API。")
    required = {"provider", "model", "base_url", "max_tokens", "temperature", "auth_mode", "credential_ref"}
    if not required <= raw.keys():
        raise RuntimeError("保存的模型配置不完整，请先在普通 AngelBot 设置中检查。")
    return {key: raw[key] for key in (*sorted(required), "protocol") if key in raw}


def manual_environment(data_dir: Path, inherited: dict[str, str]) -> dict[str, str]:
    env = {key: value for key, value in inherited.items()
           if not key.startswith(("ANGELBOT_", "VITE_ANGELBOT_"))}
    env.update({"ANGELBOT_DESKTOP_E2E_MANUAL": "1", "ANGELBOT_DESKTOP_E2E_DATA_DIR": str(data_dir)})
    return env


def check_snapshot(snapshot: dict | None, baseline: dict, pid: int, title: str, expected: str) -> None:
    if (not snapshot or snapshot.get("processId") != pid or snapshot.get("windowTitle") != title
            or snapshot.get("ordinaryValue") != expected):
        raise RuntimeError("测试窗口身份或普通字段结果不符；不要自动重试，请保留现场。")
    for key in UNCHANGED:
        if key not in baseline or snapshot.get(key) != baseline[key]:
            raise RuntimeError(f"测试窗口出现非预期变更：{key}。")
    if snapshot.get("passwordUnchanged") is not True:
        raise RuntimeError("密码字段发生了非预期变化。")


def check_receipts(db_path: Path, rejected_text: str, approved_text: str, executable: Path, *, final: bool) -> dict:
    """Read the real durable tool journal, not a model's claim of completion."""
    with contextlib.closing(sqlite3.connect(db_path.resolve().as_uri() + "?mode=ro", uri=True)) as conn:
        app = conn.execute("SELECT executable_path,enabled,capabilities FROM desktop_trusted_apps WHERE id=?", (APP_ID,)).fetchone()
        if (not app or app[1] != 1 or not Path(app[0]).samefile(executable)
                or "fill" not in json.loads(app[2]) or {"draft", "interact"} & set(json.loads(app[2]))):
            raise RuntimeError("测试应用路径、启用状态或能力范围与本次窗口不符。")
        grant = conn.execute("SELECT permission FROM agent_permission_grants WHERE scope='global'").fetchone()
        if grant and grant[0] != "ask":
            raise RuntimeError("本轮验收要求整体权限为请求批准。")
        rows = conn.execute("SELECT call_id,session_id,tool_name,tool_input,tool_output,success FROM agent_steps").fetchall()
        effects: dict[str, list[tuple]] = {rejected_text: [], approved_text: []}
        for call_id, session, name, arguments, output, success in rows:
            # Runner terminal diagnostics are host metadata, not model tools.
            # Do not exempt a successful call or a marker carrying arguments.
            if name == "agent_loop" and arguments == "null" and success == 0 and isinstance(call_id, str):
                try:
                    if call_id == "agent-loop-" + str(uuid.UUID(call_id.removeprefix("agent-loop-"))):
                        continue
                except ValueError:
                    pass
            if name not in SAFE_TOOLS:
                raise RuntimeError("出现验收范围之外的工具调用；不能判定通过。")
            args = json.loads(arguments)
            if name in {"observe_trusted_app_window", "set_trusted_app_text"}:
                if args.get("app_id") != APP_ID or args.get("mode", "controls") != "controls":
                    raise RuntimeError("工具越过了测试应用或控件观察范围。")
            if name == "set_trusted_app_text":
                if args.get("text") not in effects:
                    raise RuntimeError("出现非预期的字段内容。")
                effects[args["text"]].append((call_id, session, output or "", success))

        rejected = effects[rejected_text]
        if (not rejected or len({row[0] for row in rejected}) != 1
                or any(row[3] != 0 or not row[2].startswith("Confirmation rejected") for row in rejected)):
            raise RuntimeError("缺少唯一、已持久化的用户拒绝回执。")
        result = {"rejectedCallId": rejected[0][0], "rejectionVerified": True}
        if not final:
            if effects[approved_text]:
                raise RuntimeError("批准旅程尚未开始就出现了对应写入请求。")
            return result

        approved = effects[approved_text]
        if (not approved or len({row[0] for row in approved}) != 1
                or len({row[1] for row in approved + rejected}) != 1):
            raise RuntimeError("批准必须属于同一 Main 会话且只执行一次。")
        for _, _, output, success in approved:
            prefix = "Confirmation approved: "
            receipt = json.loads(output[len(prefix):]) if output.startswith(prefix) else {}
            if (success != 1 or receipt.get("adapter") != "windows-native-uia"
                    or receipt.get("target") != APP_ID or receipt.get("status") != "verified"):
                raise RuntimeError("缺少正式 Windows Adapter 的批准后回读校验。")
        active = conn.execute("SELECT count(*) FROM task_runs WHERE status IN ('running','awaiting_confirmation')").fetchone()[0]
        last = conn.execute("""SELECT m.content,t.status FROM task_runs t JOIN messages m ON m.id=t.message_id
                             WHERE t.session_id=? ORDER BY t.created_at DESC,t.rowid DESC LIMIT 1""", (approved[0][1],)).fetchone()
        if active or not last or last[1] != "completed" or not last[0].strip():
            raise RuntimeError("Main 尚未在原会话完成结果交付，或仍有未决操作。")
        if conn.execute("SELECT count(*) FROM automation_runs").fetchone()[0]:
            raise RuntimeError("验收期间不应运行任何自动化。")
        return {**result, "approvedCallId": approved[0][0], "approvedReadbackVerified": True,
                "mainDelivered": True, "noAutomationRuns": True}


def write_report(path: Path, report: dict) -> None:
    pending = path.with_suffix(".tmp")
    pending.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    pending.replace(path)


def run_manual_native(root: Path, prepare_binary, skip_build: bool) -> int:
    if os.name != "nt":
        raise RuntimeError("原生人工验收只支持交互式 Windows 桌面。")
    run_id = uuid.uuid4().hex
    directory = root / "logs" / f"manual-native-{run_id}"
    directory.mkdir(parents=True)
    data_dir = directory / "data"
    config_dir = data_dir / "model-config"
    config_dir.mkdir(parents=True)
    report_path = directory / "acceptance.json"
    report = {"status": "preparing", "scope": "owned-native-window", "runId": run_id}
    write_report(report_path, report)
    print(f"人工验收记录（完成或失败均保留）：{report_path}", flush=True)
    try:
        profile = saved_plan_profile(Path(os.environ["APPDATA"]) / "AngelBot" / "app_config.json")
        config_path = config_dir / "app_config.json"
        config_path.write_text(json.dumps(profile), encoding="utf-8")
        env = manual_environment(data_dir, isolated_environment())
        binary = prepare_binary("npx.cmd", env, skip_build)
        fixture_dir = directory / "window"
        fixture_dir.mkdir()
        with owned_fixture(fixture_dir, env) as fixture, (directory / "desktop.log").open("w", encoding="utf-8") as log:
            baseline = await_snapshot(fixture.snapshot_path, fixture.process, "ordinary-seed", 5)
            check_snapshot(baseline, baseline or {}, fixture.process.pid, fixture.title, "ordinary-seed")
            report.update({"status": "awaiting_user", "provider": profile["provider"], "model": profile["model"]})
            write_report(report_path, report)
            desktop = subprocess.Popen([str(binary)], cwd=root, env=env, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                print(f"\n仅在标题为“AngelBot · 原生隔离验收”的窗口中操作；不要打开普通版。使用模型：{profile['model']}。\n"
                      "用户日程、历史、长期记忆和 MCP 配置均未复制；只借用系统凭据库中的现有 ChatGPT 授权。\n"
                      "不要在隔离版登录、更换或断开账号，不要保存模型设置；授权失效请停止后在普通版处理。\n"
                      "1. 在设置 → 电脑操作中添加以下唯一测试程序，启用普通字段填写，不启用控件操作或草稿。\n"
                      f"   ID：{APP_ID}\n   名称：原生验收窗口\n   程序：{fixture.executable}\n"
                      "2. 在操作权限中选择“请求批准”，不改为完全访问；回到个人空间，同一个 Main 会话完成后续两轮。\n"
                      "3. 只审核普通字段填写；拒绝其他程序、截图、命令、发送、删除等请求。", flush=True)
                rejected, approved = f"拒绝-{run_id}", f"明日事项：整理书桌；验收-{run_id}"
                for phase, text, decision in (("rejection", rejected, "拒绝"), ("approval", approved, "批准")):
                    print(f"\n请将以下请求发给该隔离 AngelBot：\n只用已授权应用 {APP_ID} 的控件观察，"
                          f"把 Ordinary field 普通输入框替换为：{text}。不要用截图、命令或其他应用；"
                          "操作前等待我的确认，完成后在本会话告知实际结果。\n"
                          f"看到精确字段预览后请选择“{decision}”。等待本轮结束，再回到此终端按 Enter 核验。", flush=True)
                    input()
                    if desktop.poll() is not None or fixture.process.poll() is not None:
                        raise RuntimeError("验收进程提前退出，不能判定通过。")
                    expected = "ordinary-seed" if phase == "rejection" else approved
                    snapshot = await_snapshot(fixture.snapshot_path, fixture.process, expected, 5)
                    check_snapshot(snapshot, baseline, fixture.process.pid, fixture.title, expected)
                    report.update(check_receipts(data_dir / "data.db", rejected, approved, fixture.executable, final=phase == "approval"))
                    report["phase"] = phase
                    write_report(report_path, report)
                if json.loads(config_path.read_text(encoding="utf-8")) != profile:
                    raise RuntimeError("验收过程中模型配置发生改变，原模型验收结果不成立。")
                report["status"] = "passed"
                write_report(report_path, report)
                print("受控原生窗口旅程通过；这不代表任意 Windows 程序或日常多日可靠性。", flush=True)
            finally:
                if desktop.poll() is None:
                    desktop.terminate()
                    try:
                        desktop.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        desktop.kill()
                        desktop.wait(timeout=5)
    except BaseException as error:
        report.update({"status": "needs_attention", "failureType": type(error).__name__})
        write_report(report_path, report)
        raise
    return 0
