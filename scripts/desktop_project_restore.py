"""Opt-in native encrypted restore; directory permission is granted only by a person."""

from __future__ import annotations

import contextlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from native_uia_smoke import isolated_environment
from desktop_restore_assertions import check_restored_database


REQUIRED = {
    "source": ("backupPrepared", "historyVerified"),
    "target": ("historyVerified", "pendingDenied", "directoriesConfirmed",
               "sameMainRefreshed", "mappedRootVerified", "oldRootDenied",
               "excludedProjectDenied", "scopeReset"),
    "reopen": ("historyVerified", "pendingDenied", "directoriesConfirmed",
               "sameMainRefreshed", "mappedRootVerified", "oldRootDenied",
               "excludedProjectDenied", "scopeReset"),
}


def restore_environment(runtime: Path, evidence: Path, phase: str) -> dict[str, str]:
    if phase not in REQUIRED:
        raise ValueError(f"Unknown restore phase: {phase}")
    profile = runtime / ("source" if phase == "source" else "target")
    env = {key: value for key, value in isolated_environment().items()
           if not key.startswith(("ANGELBOT_", "VITE_ANGELBOT_"))}
    env.update({
        "ANGELBOT_DESKTOP_E2E": "1",
        "VITE_ANGELBOT_DESKTOP_E2E": "1",
        "ANGELBOT_DESKTOP_E2E_DATA_DIR": str(profile / "angelbot-data"),
        "ANGELBOT_DESKTOP_E2E_PROJECT_RESTORE": phase,
        "ANGELBOT_DESKTOP_E2E_RESTORE_RUNTIME": str(runtime),
        "ANGELBOT_DESKTOP_E2E_RESTORE_EVIDENCE_DIR": str(evidence),
        # A scripted fallback prevents credential use. The acceptance requires
        # no new task/receipt, so an accidental conversation cannot pass.
        "ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT": json.dumps([
            {"type": "text", "text": "restore-fixture-no-model"}]),
        "APPDATA": str(profile), "LOCALAPPDATA": str(profile),
    })
    return env


def run_project_restore(root: Path, prepare_binary, configure_port, skip_build: bool,
                        keep_data: bool) -> int:
    evidence = root / "src-tauri/target/e2e/project-restore"
    evidence.mkdir(parents=True, exist_ok=True)
    report_path = evidence / "acceptance.json"
    report = {"status": "running", "provider": "synthetic-history-no-model",
              "scope": "native-encrypted-restore-human-directory-grant", "phases": {}}
    report_path.write_text(json.dumps(report), encoding="utf-8")
    # Invalidate all previous checkpoints before build/driver startup can fail.
    for phase in REQUIRED:
        (evidence / f"{phase}.json").write_text('{"status":"not_started"}', encoding="utf-8")
    (evidence / "stage.json").write_text('{"phase":"preparing"}', encoding="utf-8")
    runtime_directory = (contextlib.nullcontext(tempfile.mkdtemp(prefix="angelbot-restore-"))
                         if keep_data else tempfile.TemporaryDirectory(prefix="angelbot-restore-"))
    try:
        with runtime_directory as directory:
            # Match native directory canonicalization, including TEMP junctions
            # and Windows short-name aliases, before declaring fixture paths.
            runtime = Path(directory).resolve()
            report["retainedRuntime"] = str(runtime) if keep_data else None
            for folder, text in (("old-A", "OLD-A-DENIED\n"),
                                 ("local-A", "A-CANARY\n"), ("old-B", "B-CANARY\n")):
                destination = runtime / folder
                destination.mkdir()
                (destination / "scope.txt").write_text(text, encoding="utf-8")
            for profile in ("source", "target"):
                (runtime / profile).mkdir()
            print(f"Isolated restore runtime: {runtime}", flush=True)
            print("Native import uses encrypted-backup.json and synthetic password Restore-Probe-2026!; "
                  "directory selection/exclusion/final grant are human-only.", flush=True)
            npx = "npx.cmd" if os.name == "nt" else "npx"
            env = restore_environment(runtime, evidence, "source")
            prepare_binary(npx, env, skip_build)
            for phase, required in REQUIRED.items():
                report["phase"] = phase
                report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
                env = restore_environment(runtime, evidence, phase)
                prepare_binary(npx, env, True)
                configure_port(env)
                phase_path = evidence / f"{phase}.json"
                phase_path.write_text('{"status":"running"}', encoding="utf-8")
                subprocess.run([npx, "wdio", "run", "e2e/wdio.conf.mjs"],
                               cwd=root, env=env, check=True, timeout=1080)
                result = json.loads(phase_path.read_text(encoding="utf-8"))
                if result.get("status") != "passed" or not all(result.get(key) is True for key in required):
                    raise RuntimeError(f"Native project restore phase incomplete: {phase}")
                report["phases"][phase] = result
                if phase != "source":
                    manifest = json.loads((evidence / "manifest.json").read_text(encoding="utf-8"))
                    report["phases"][phase]["host"] = check_restored_database(
                        runtime / "target/angelbot-data/data.db",
                        runtime / "source/angelbot-data/data.db", manifest)
            report.update(status="passed", phase="done")
    except BaseException:
        report["status"] = "failed"
        (evidence / "stage.json").write_text(json.dumps({
            "phase": "failed", "failedPhase": report.get("phase", "preparing")}), encoding="utf-8")
        raise
    finally:
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"Native project restore acceptance: {report_path}", flush=True)
    return 0
