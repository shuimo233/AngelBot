"""Native model-boundary recovery proof; no credentials, remote site or other app."""

from __future__ import annotations

import contextlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile

from native_uia_smoke import isolated_environment


def recovery_environment(runtime: Path, evidence: Path, phase: str) -> dict[str, str]:
    env = {key: value for key, value in isolated_environment().items()
           if not key.startswith(("ANGELBOT_", "VITE_ANGELBOT_"))}
    expected = ["native-recovery-initial", "native-recovery-next"]
    # The existing Text assertion deliberately fails after the read-only tool.
    # Failed replies do not advance the script. The next ordinary user message
    # supplies the missing marker, so the same model can recover without reload.
    script = [{"type": "text", "text": "后续消息已完成：native-recovery-ok",
               "expectUserContentIncludes": expected}]
    if phase == "first":
        script.insert(0, {"type": "tool_calls", "text": "先只读核对当前自动化。", "calls": [
            {"id": "native-recovery-list", "name": "list_automations", "arguments": {}}]})
        script.extend([
            {"type": "pending"},
            {"type": "text", "text": "停止后新任务已完成：native-stop-next-ok",
             "expectUserContentIncludes": [*expected, "native-stop-initial", "native-stop-next"]},
        ])
    elif phase == "reopen":
        script[0].update(text="重开后已接续：native-recovery-reopened",
                         expectUserContentIncludes=[*expected, "native-recovery-reopen"])
    else:
        raise ValueError(f"Unknown recovery phase: {phase}")
    env.update({
        "ANGELBOT_DESKTOP_E2E": "1",
        "VITE_ANGELBOT_DESKTOP_E2E": "1",
        "ANGELBOT_DESKTOP_E2E_DATA_DIR": str(runtime / "angelbot-data"),
        "ANGELBOT_DESKTOP_E2E_MODEL_RECOVERY": phase,
        "ANGELBOT_DESKTOP_E2E_RECOVERY_EVIDENCE_DIR": str(evidence),
        "ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT": json.dumps(script, ensure_ascii=False),
        "APPDATA": str(runtime), "LOCALAPPDATA": str(runtime),
    })
    return env


def run_model_recovery(root: Path, prepare_binary, configure_port, skip_build: bool,
                       keep_data: bool, inspect: bool) -> int:
    evidence = root / "src-tauri/target/e2e/model-recovery"
    evidence.mkdir(parents=True, exist_ok=True)
    report_path = evidence / "acceptance.json"
    report = {"status": "running", "provider": "desktop-e2e-scripted",
              "scope": "native-main-conversation-and-reopen", "phases": {}}
    report_path.write_text(json.dumps(report), encoding="utf-8")
    runtime_directory = (contextlib.nullcontext(tempfile.mkdtemp(prefix="angelbot-recovery-"))
                         if keep_data else tempfile.TemporaryDirectory(prefix="angelbot-recovery-"))
    try:
        with runtime_directory as directory:
            runtime = Path(directory)
            if keep_data:
                print(f"Isolated recovery data retained: {runtime}", flush=True)
            npx = "npx.cmd" if os.name == "nt" else "npx"
            env = recovery_environment(runtime, evidence, "first")
            prepare_binary(npx, env, skip_build)
            for phase in ("first", "reopen"):
                env = recovery_environment(runtime, evidence, phase)
                prepare_binary(npx, env, True)
                configure_port(env)
                if inspect and phase == "reopen":
                    env["ANGELBOT_DESKTOP_E2E_RECOVERY_INSPECT"] = "1"
                    (evidence / "inspection-done").unlink(missing_ok=True)
                phase_path = evidence / f"{phase}.json"
                phase_path.write_text(json.dumps({"status": "running"}), encoding="utf-8")
                subprocess.run([npx, "wdio", "run", "e2e/wdio.conf.mjs"],
                               cwd=root, env=env, check=True)
                result = json.loads(phase_path.read_text(encoding="utf-8"))
                if result.get("status") != "passed":
                    raise RuntimeError(f"Native recovery phase did not pass: {phase}")
                report["phases"][phase] = result
            with contextlib.closing(sqlite3.connect(
                    f"{(runtime / 'angelbot-data/data.db').as_uri()}?mode=ro", uri=True)) as db:
                receipts = db.execute("SELECT tool_name, success FROM agent_steps ORDER BY rowid").fetchall()
                if receipts != [("list_automations", 1), ("agent_loop", 0)]:
                    raise AssertionError(f"Read-only tool was replayed or failed: {receipts}")
                if db.execute("SELECT count(*) FROM task_runs WHERE status IN ('running','awaiting_confirmation')").fetchone()[0]:
                    raise AssertionError("Recovery left an active task")
                if db.execute("SELECT count(*) FROM automation_runs").fetchone()[0]:
                    raise AssertionError("Recovery unexpectedly ran an automation")
                if db.execute("SELECT count(*) FROM task_runs WHERE status='stopped' AND resumable=0 AND step_count=0").fetchone()[0] != 1:
                    raise AssertionError("Pending model stop was not durably recorded without a fake tool")
            report.update(status="passed", readOnlyReceiptCount=1, activeTasks=0, automationRuns=0,
                          stoppedRuns=1, nativeStopButton=True)
    except BaseException:
        report["status"] = "failed"
        raise
    finally:
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"Native recovery acceptance: {report_path}", flush=True)
    return 0
