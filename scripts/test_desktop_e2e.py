"""Offline regressions for canonical verification and isolated desktop artifacts."""

import json
import base64
import hashlib
import http.client
import contextlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import sqlite3
import sys
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import desktop_e2e
import desktop_manual_acceptance as manual
import desktop_model_recovery as recovery
import desktop_project_restore as restore
import desktop_restore_assertions as restore_assertions
import browser_form_acceptance as browser_acceptance
import native_uia_smoke as native
import verify


REPO_ROOT = desktop_e2e.ROOT


class DesktopE2eArtifactTests(unittest.TestCase):
    def test_canonical_checks_isolate_credentials_and_clean_owned_config(self):
        for fail in (False, True):
            roots = []

            def run(_command, **kwargs):
                env = kwargs.get('env')
                self.assertIsNotNone(env, 'canonical checks must supply an isolated environment')
                for key in ('OPENAI_API_KEY', 'AZURE_OPENAI_KEY', 'AWS_ACCESS_KEY_ID',
                            'ANGELBOT_LIVE_MODEL_TEST', 'VITE_ANGELBOT_E2E'):
                    self.assertTrue(key not in env, f'{key} must not be inherited')
                self.assertEqual(env['CARGO_NET_OFFLINE'], 'true')
                self.assertEqual(env['NPM_CONFIG_OFFLINE'], 'true')
                self.assertEqual(env['COMPILER_SYSTEM_MARKER'], 'keep')
                root = Path(env['ANGELBOT_PRIVATE_VERIFY_CONFIG_DIR'])
                self.assertTrue(root.is_absolute() and root.is_dir())
                roots.append(root)
                if fail:
                    raise OSError('owned subprocess failure')
                return SimpleNamespace(returncode=0)

            with patch.dict(os.environ, {'OPENAI_API_KEY': 'fixture-secret',
                                        'AZURE_OPENAI_KEY': 'fixture-azure-secret',
                                        'AWS_ACCESS_KEY_ID': 'fixture-access-id',
                                        'COMPILER_SYSTEM_MARKER': 'keep',
                                        'ANGELBOT_LIVE_MODEL_TEST': '1', 'VITE_ANGELBOT_E2E': '1'}), \
                    patch.object(verify, 'executable_available', return_value=True), \
                    patch.object(verify, 'platform_command', side_effect=lambda command: command), \
                    patch.object(verify.subprocess, 'run', side_effect=run), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(verify.run_check(verify.Check('owned check', ('owned',)), False), not fail)
            self.assertEqual(len(roots), 1)
            self.assertFalse(roots[0].exists(), 'owned config must be cleaned on success and failure')

    def test_native_storage_is_explicit_and_never_passes_by_skipping_on_other_systems(self):
        native_checks = verify.PROFILES['native-storage']
        self.assertEqual(len(native_checks), 1)
        self.assertIn('--exact', native_checks[0].command)
        self.assertIn('--ignored', native_checks[0].command)
        self.assertIn('llm::chatgpt_auth::tests::native_profile_cutover_keeps_windows_vault_binding',
                      native_checks[0].command)
        for profile in ('quick', 'frontend', 'backend', 'full'):
            self.assertNotIn(native_checks[0], verify.PROFILES[profile])
        with patch.object(sys, 'argv', ['verify.py', 'native-storage']), \
                patch.object(verify, 'os', SimpleNamespace(name='posix')), \
                patch.object(verify, 'run_check') as check, \
                contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                verify.main()
            self.assertEqual(error.exception.code, 2)
            check.assert_not_called()
        with patch.object(sys, 'argv', ['verify.py', 'native-storage']), \
                patch.object(verify, 'os', SimpleNamespace(name='nt')), \
                patch.object(verify, 'run_check', return_value=True) as check, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(verify.main(), 0)
            check.assert_called_once_with(native_checks[0], False)

    def test_native_controls_only_never_builds_normal_binary_or_requests_capture(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(sys, "argv", ["native_uia_smoke.py", "--controls-only"]), \
                patch.object(native, "os", SimpleNamespace(name="nt")), \
                patch.object(native, "isolated_environment", return_value={}), \
                patch.object(native, "build_cargo_binary", return_value=Path(directory) / "owned-test.exe") as build, \
                patch.object(native.subprocess, "CREATE_NO_WINDOW", 0, create=True), \
                patch.object(native.subprocess, "run", return_value=SimpleNamespace(stdout=f"{native.TEST_NAME}: test\n")), \
                patch.object(native, "owned_fixture", side_effect=RuntimeError("owned fixture checkpoint")), \
                self.assertRaisesRegex(RuntimeError, "owned fixture checkpoint"):
            native.main()
        build.assert_called_once_with(library_test=True)
        with patch.object(sys, "argv", ["native_uia_smoke.py", "--controls-only", "--capture-only"]), \
                patch.object(native, "build_cargo_binary") as build, \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as exited:
            native.main()
        self.assertEqual(exited.exception.code, 2)
        build.assert_not_called()

        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            artifact=root / "src-tauri/target/native-capture-helper/debug/angelbot.exe"
            output=json.dumps({"reason":"compiler-artifact","target":{"name":"angelbot","kind":["bin"]},"profile":{"test":False},"executable":str(artifact)})
            with patch.object(native,"ROOT",root), patch.object(native,"isolated_environment",return_value={}), \
                    patch.object(native.subprocess,"run",return_value=SimpleNamespace(stdout=output,stderr="",returncode=0)) as build, \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(native.build_cargo_binary(library_test=False),artifact.resolve())
                self.assertEqual(build.call_args.kwargs["env"]["CARGO_TARGET_DIR"],str(root / "src-tauri/target/native-capture-helper"))

    def test_model_recovery_isolates_environment_and_preserves_next_message_gate(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
                "PRIVATE_API_KEY": "PRIVATE_CANARY", "ANGELBOT_NATIVE_UIA_SMOKE": "1",
                "ANGELBOT_DESKTOP_E2E_MANUAL": "1", "VITE_ANGELBOT_LIVE_TEST": "1",
                "ANGELBOT_DESKTOP_E2E_MODEL_RESPONSE": "PRIVATE_CANARY"}, clear=True):
            runtime = Path(directory)
            first = recovery.recovery_environment(runtime, runtime / "evidence", "first")
            self.assertNotIn("PRIVATE_CANARY", json.dumps(first))
            self.assertNotIn("ANGELBOT_DESKTOP_E2E_MANUAL", first)
            self.assertNotIn("ANGELBOT_NATIVE_UIA_SMOKE", first)
            self.assertNotIn("VITE_ANGELBOT_LIVE_TEST", first)
            self.assertEqual(first["APPDATA"], directory)
            self.assertEqual(first["LOCALAPPDATA"], directory)
            self.assertEqual(first["ANGELBOT_DESKTOP_E2E_DATA_DIR"], str(runtime / "angelbot-data"))
            script = json.loads(first["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"])
            self.assertEqual(script[0]["calls"], [{"id": "native-recovery-list", "name": "list_automations", "arguments": {}}])
            self.assertEqual(script[1]["expectUserContentIncludes"], ["native-recovery-initial", "native-recovery-next"])
            reopened = recovery.recovery_environment(runtime, runtime / "evidence", "reopen")
            script = json.loads(reopened["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"])
            self.assertEqual(len(script), 1)
            self.assertEqual(script[0]["type"], "text")
            self.assertEqual(script[0]["expectUserContentIncludes"],
                             ["native-recovery-initial", "native-recovery-next", "native-recovery-reopen"])
            with self.assertRaises(ValueError):
                recovery.recovery_environment(runtime, runtime / "evidence", "typo")

    def test_project_restore_isolates_profiles_and_never_reuses_stale_passes(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
                "PRIVATE_API_KEY": "PRIVATE_CANARY", "ANGELBOT_DESKTOP_E2E_MANUAL": "1",
                "VITE_ANGELBOT_LIVE_TEST": "1", "ANGELBOT_DESKTOP_E2E_MODEL_RESPONSE": "PRIVATE_CANARY"}, clear=True):
            runtime = Path(directory)
            first = restore.restore_environment(runtime, runtime / "evidence", "source")
            target = restore.restore_environment(runtime, runtime / "evidence", "target")
            reopened = restore.restore_environment(runtime, runtime / "evidence", "reopen")
            self.assertNotIn("PRIVATE_CANARY", json.dumps(first))
            self.assertNotIn("ANGELBOT_DESKTOP_E2E_MANUAL", first)
            self.assertNotIn("VITE_ANGELBOT_LIVE_TEST", first)
            self.assertEqual(first["APPDATA"], str(runtime / "source"))
            self.assertEqual(target["APPDATA"], str(runtime / "target"))
            self.assertEqual(target["ANGELBOT_DESKTOP_E2E_DATA_DIR"], reopened["ANGELBOT_DESKTOP_E2E_DATA_DIR"])
            self.assertNotEqual(first["ANGELBOT_DESKTOP_E2E_DATA_DIR"], target["ANGELBOT_DESKTOP_E2E_DATA_DIR"])
            with self.assertRaises(ValueError):
                restore.restore_environment(runtime, runtime / "evidence", "typo")

        for failure in ("build", "driver", "incomplete"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                evidence = root / "src-tauri/target/e2e/project-restore"
                evidence.mkdir(parents=True)
                for name in ("acceptance", *restore.REQUIRED):
                    (evidence / f"{name}.json").write_text('{"status":"passed"}')
                error = subprocess.CalledProcessError(2, "PRIVATE_STARTUP_ERROR")
                def driver(*_args, **_kwargs):
                    if failure == "driver":
                        raise error
                def prepare(*_args):
                    if failure == "build":
                        raise error

                with patch.object(restore.subprocess, "run", side_effect=driver), \
                        contextlib.redirect_stdout(io.StringIO()), \
                        self.assertRaises(RuntimeError if failure == "incomplete" else subprocess.CalledProcessError):
                    restore.run_project_restore(root, prepare, lambda _env: None, False, False)
                report = json.loads((evidence / "acceptance.json").read_text())
                self.assertEqual(report["status"], "failed")
                self.assertFalse(report["phases"])
                self.assertNotIn("PRIVATE_STARTUP_ERROR", json.dumps(report))
                self.assertEqual(json.loads((evidence / "stage.json").read_text())["phase"], "failed")
                for name in ("target", "reopen"):
                    self.assertEqual(json.loads((evidence / f"{name}.json").read_text())["status"], "not_started")

    def test_restore_host_requires_exact_history_scope_and_no_replay(self):
        expected = {table: [] for table in restore_assertions.TABLES}
        for identity in ("personal", "a", "b"):
            expected["projects"].append({"id": identity, "kind": "personal" if identity == "personal" else "project",
                "path": None if identity == "personal" else f"/old-{identity}",
                "directory_confirmed": int(identity == "personal"), "active_session_id": f"{identity}-main"})
            expected["sessions"].append({"id": f"{identity}-main", "project_id": identity,
                "parent_id": None, "leaf_message_id": "reply" if identity == "a" else None,
                "work_dir": None, "additional_read_dirs": "[]",
                "title": "" if identity == "personal" else f"{identity} title"})
        expected["messages"] = [{"id": "reply", "session_id": "a-main", "parent_id": None,
            "role": "assistant", "content": "fixed synthetic history", "is_provisional": 0},
            {"id": "daily-user", "session_id": "personal-main", "parent_id": None,
             "role": "user", "content": "daily title canary", "is_provisional": 0}]
        expected["task_runs"] = [{"id": "reply", "session_id": "a-main", "message_id": "reply",
            "status": "completed", "confirmation_state": "none", "resumable": 0}]
        expected["task_run_facts"] = [{"task_run_id": "reply", "session_id": "a-main", "message_id": "reply",
            "facts_json": json.dumps({"goal": "fixed synthetic goal"})}]
        completed_facts = {"goal": "fixed synthetic goal", "plan": [], "completed_steps": [],
            "failed_steps": [], "modified_files": [], "verification_evidence": [],
            "pending_confirmation": None, "repair_attempts": 0, "no_progress_count": 0,
            "context_summary": None, "provider_attempts": 0, "provider_binding": None,
            "terminal_reason": None}
        manifest = {"expected": expected,
            "projectA": {"id": "a", "sessionId": "a-main", "mappedPath": "/mapped-a"},
            "projectB": {"id": "b", "sessionId": "b-main", "originalPath": "/old-b"}}
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "target.db"
            source = Path(directory) / "source.db"
            with contextlib.closing(sqlite3.connect(target)) as db:
                for table, rows in expected.items():
                    columns = list(rows[0]) if rows else ["task_run_id" if table == "task_run_facts" else "id"]
                    db.execute(f'CREATE TABLE "{table}" ({",".join(columns)})')
                    for row in rows:
                        observed = dict(row)
                        if table == "projects" and row["id"] == "a":
                            observed.update(path="/mapped-a", directory_confirmed=1)
                        if table == "sessions" and row["project_id"] == "a":
                            observed["work_dir"] = "/mapped-a"
                        if table == "sessions" and row["id"] == "personal-main":
                            observed["title"] = "daily title canary"
                        if table == "task_run_facts":
                            observed["facts_json"] = json.dumps(completed_facts)
                        db.execute(f'INSERT INTO "{table}" VALUES ({",".join("?" for _ in columns)})', list(observed.values()))
                for table in restore_assertions.EMPTY_QUEUES:
                    db.execute(f'CREATE TABLE "{table}" (id TEXT)')
                for table in ("automations", "scheduled_tasks"):
                    db.execute(f'CREATE TABLE "{table}" (id TEXT, enabled INTEGER)')
                db.commit()
                self.assertTrue(restore_assertions.check_restored_database(target, source, manifest)["noReplay"])
                for sql in ("UPDATE projects SET directory_confirmed=1 WHERE id='b'",
                            "UPDATE messages SET content='altered'",
                            "UPDATE sessions SET title='wrong title' WHERE id='personal-main'",
                            "UPDATE sessions SET title='wrong title' WHERE id='a-main'",
                            f"UPDATE task_run_facts SET facts_json='{json.dumps({**completed_facts, 'terminal_reason': 'needs_attention'})}'",
                            "INSERT INTO agent_steps(id) VALUES('unexpected')",
                            "INSERT INTO automation_runs(id) VALUES('replay')",
                            "INSERT INTO automations(id,enabled) VALUES('timer',1)",
                            "DROP TABLE workspace_supervisor_work"):
                    with self.subTest(sql=sql):
                        db.execute(sql)
                        # A second read-only connection must observe the change.
                        db.commit()
                        with self.assertRaises(AssertionError):
                            restore_assertions.check_restored_database(target, source, manifest)
                        if sql.startswith("DROP"):
                            db.execute("CREATE TABLE workspace_supervisor_work (id TEXT)")
                        else:
                            db.execute("UPDATE projects SET directory_confirmed=0 WHERE id='b'")
                            db.executemany("UPDATE messages SET content=? WHERE id=?",
                                           [(row["content"], row["id"]) for row in expected["messages"]])
                            db.execute("UPDATE sessions SET title=CASE id WHEN 'personal-main' THEN 'daily title canary' WHEN 'a-main' THEN 'a title' ELSE title END")
                            db.execute("UPDATE task_run_facts SET facts_json=?", (json.dumps(completed_facts),))
                            db.execute("DELETE FROM agent_steps")
                            db.execute("DELETE FROM automation_runs")
                            db.execute("DELETE FROM automations")
                        db.commit()
                with self.assertRaisesRegex(AssertionError, "paths must differ"):
                    restore_assertions.check_restored_database(target, target, manifest)
                expected["sessions"][0]["title"] = "existing daily title"
                with self.assertRaisesRegex(AssertionError, "exact sessions.title for personal-main"):
                    restore_assertions.check_restored_database(target, source, manifest)
                db.execute("UPDATE sessions SET title='existing daily title' WHERE id='personal-main'")
                db.commit()
                self.assertTrue(restore_assertions.check_restored_database(target, source, manifest)["noReplay"])
                for source_status, source_reason in (("running", None), ("completed", "awaiting_confirmation")):
                    with self.subTest(source_status=source_status, source_reason=source_reason):
                        expected["task_runs"][0]["status"] = source_status
                        expected["task_runs"][0]["resumable"] = 1
                        expected["task_run_facts"][0]["facts_json"] = json.dumps({
                            "goal": "fixed synthetic goal", "terminal_reason": source_reason})
                        db.execute("UPDATE task_runs SET status=?, resumable=0",
                                   ("needs_attention" if source_status == "running" else "completed",))
                        db.execute("UPDATE task_run_facts SET facts_json=?", (json.dumps({
                            **completed_facts, "terminal_reason": "needs_attention"}),))
                        db.commit()
                        self.assertTrue(restore_assertions.check_restored_database(target, source, manifest)["noReplay"])
                        db.execute("UPDATE task_run_facts SET facts_json=?", (json.dumps(completed_facts),))
                        db.commit()
                        with self.assertRaisesRegex(AssertionError, "exact task_run_facts.facts_json"):
                            restore_assertions.check_restored_database(target, source, manifest)

    def test_saved_model_probe_is_single_request_and_never_starts_browser_tasks(self):
        for status, count, extra, exit_code in (("available", 1, {}, 0), ("quota_exceeded", 1, {}, 2),
                ("unavailable", 0, {}, 2), ("unavailable", 1, {}, 2), ("timed_out", 1, {}, 2),
                ("available", 0, {}, None), ("available", 1, {"diagnostic": "PRIVATE_CANARY"}, None)):
            with self.subTest(status=status, count=count, extra=extra), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                def driver(command, **kwargs):
                    self.assertEqual(command[1], 'commands::message::live_tests::configured_model_completes_bounded_file_task')
                    self.assertEqual(kwargs['env']['ANGELBOT_LIVE_MODEL_JOURNEY'], 'model-probe')
                    self.assertNotIn('PRIVATE_CANARY', json.dumps(kwargs['env']))
                    self.assertNotIn('ANGELBOT_PRIVATE_LIVE_PHASE', kwargs['env'])
                    self.assertNotIn('ANGELBOT_LIVE_BROWSER_CLI', kwargs['env'])
                    self.assertEqual(kwargs['timeout'], 75)
                    kwargs['stdout'].write('MODEL_AVAILABILITY_REPORT ' + json.dumps({
                        'status': status, 'requestCount': count, 'toolCount': 0, **extra}) + '\n')
                    return SimpleNamespace(returncode=0)
                with patch.object(browser_acceptance, 'ROOT', root), \
                        patch.object(browser_acceptance, 'build_cargo_binary', return_value=root / 'owned-test.exe') as build, \
                        patch.object(browser_acceptance.subprocess, 'run', side_effect=driver) as run, \
                        patch.object(browser_acceptance, 'serve_fixture') as server, \
                        patch.dict(os.environ, {'PRIVATE_API_KEY': 'PRIVATE_CANARY',
                            'ANGELBOT_PRIVATE_LIVE_PHASE': 'browser-write',
                            'ANGELBOT_LIVE_BROWSER_CLI': 'PRIVATE_CANARY'}), \
                        contextlib.redirect_stdout(io.StringIO()) as output:
                    if exit_code is None:
                        with self.assertRaisesRegex(RuntimeError, 'invalid bounded report'):
                            browser_acceptance.run_model_probe()
                    else:
                        self.assertEqual(browser_acceptance.run_model_probe(), exit_code)
                build.assert_called_once_with(library_test=True)
                run.assert_called_once()
                server.assert_not_called()
                report, = (root / 'logs').glob('model-probe-*/acceptance.json')
                body = json.loads(report.read_text())
                self.assertEqual(body['status'], status if exit_code is not None else 'failed')
                self.assertNotIn('PRIVATE_CANARY', report.read_text() + output.getvalue())
        with patch.object(sys, 'argv', ['browser_form_acceptance.py', '--model-probe']), \
                patch.object(browser_acceptance, 'run_model_probe', return_value=7) as start:
            self.assertEqual(browser_acceptance.main(), 7)
            start.assert_called_once()
        for arguments in ([], ['--model-probe', '--daily-report'], ['--model-probe', '--hold-for-inspection']):
            with self.subTest(arguments=arguments), patch.object(sys, 'argv', ['browser_form_acceptance.py', *arguments]), \
                    patch.object(browser_acceptance, 'run_model_probe') as start, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                browser_acceptance.main()
            start.assert_not_called()

    def test_recorded_inference_probe_never_starts_browser_or_accepts_execution(self):
        for tool_count, diagnostic in ((0, None), (1, None), (0, 'diag:PRIVATE_CANARY')):
            with self.subTest(tool_count=tool_count, diagnostic=diagnostic), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / 'data.db'
                source.write_bytes(b'owned synthetic database')
                def driver(command, **kwargs):
                    self.assertEqual(kwargs['env']['ANGELBOT_LIVE_MODEL_JOURNEY'], 'recorded-inference')
                    self.assertEqual(kwargs['env']['ANGELBOT_PRIVATE_REPLAY_DB'], str(source.resolve()))
                    self.assertNotIn('ANGELBOT_LIVE_BROWSER_CLI', kwargs['env'])
                    self.assertNotIn('PRIVATE_API_KEY', kwargs['env'])
                    kwargs['stdout'].write('RECORDED_INFERENCE_REPORT ' + json.dumps({
                        'status':'available', 'requestCount':1, 'toolCount':tool_count,
                        'historyCount':3, 'schemaCount':2, 'diagnosticRef':diagnostic,
                        'reconstructedNotByteExact':True}) + '\n')
                    return SimpleNamespace(returncode=0)
                with patch.object(browser_acceptance, 'ROOT', root), \
                        patch.object(browser_acceptance, 'build_cargo_binary', return_value=root / 'owned-test.exe'), \
                        patch.object(browser_acceptance.subprocess, 'run', side_effect=driver) as run, \
                        patch.object(browser_acceptance, 'serve_fixture') as server, \
                        patch.dict(os.environ, {'PRIVATE_API_KEY':'PRIVATE_CANARY'}), \
                        contextlib.redirect_stdout(io.StringIO()) as output:
                    if tool_count or diagnostic:
                        with self.assertRaisesRegex(RuntimeError, 'invalid bounded report'):
                            browser_acceptance.run_model_probe(source)
                    else:
                        self.assertEqual(browser_acceptance.run_model_probe(source), 0)
                run.assert_called_once()
                server.assert_not_called()
                report, = (root / 'logs').glob('inference-replay-*/acceptance.json')
                self.assertNotIn('PRIVATE_CANARY', report.read_text() + output.getvalue())
                self.assertEqual(source.read_bytes(), b'owned synthetic database')

    def test_restore_only_reuses_source_evidence_without_running_old_phases(self):
        for alter_original in (False, True):
            with self.subTest(alter_original=alter_original), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                previous = root / 'old-report'
                previous.mkdir()
                retained = root / 'retained'
                marker = str(browser_acceptance.uuid.uuid4())
                raw = b'item,count\ncable,1\n'
                attachment = dict(name='borrow-items.csv', bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
                draft = dict(id=1, title='Synthetic', category='equipment', location='Room',
                             date='2026-10-23', notes='Draft only', adapter=True, attachment=attachment)
                business = dict(drafts=[draft], mutations=1, saveAttempts=1, receiptLost=True,
                                downloadAttempts=0, uploadAttempts=1)
                receipt = {k: v for k, v in draft.items() if k != 'id'}
                receipt.update(draft_id=1, status='draft')
                (previous / 'state.json').write_text(json.dumps(business))
                (previous / 'upload-1.csv').write_bytes(raw)
                (previous / 'live.log').write_text(f'owned fixture retained at {retained}\n')
                for parent in (retained, retained / 'restored'):
                    (parent / 'results').mkdir(parents=True)
                    (parent / 'data.db').write_bytes(b'original database evidence')
                    for name in ('request.json', 'receipt.json', 'borrow-items.csv'):
                        content = json.dumps(receipt).encode() if name == 'receipt.json' else raw
                        (parent / 'results' / name).write_bytes(content)
                for name in ('browser-write.passed', 'browser-reopen.passed'):
                    (retained / name).write_text(marker)
                written = dict(phase='source-to-web-to-report', sessionId='personal-main',
                               successfulWrites=1, receipt=receipt, sourceUnchanged=True)
                reopened = {**written, 'phase':'reopen-read-only', 'backupRestoreVerified':True}
                source_report = dict(status='failed', ownedRoot=str(previous), ownedServerClosed=True,
                                     projectReceipts=[written, reopened], businessState=business)
                report_path = previous / 'acceptance.json'
                report_path.write_text(json.dumps(source_report))
                cli = root / 'cli.js'
                cli.touch()
                (root / 'package.json').write_text(json.dumps({'name':'@playwright/mcp', 'version':'0.0.83'}))
                def driver(_command, **kwargs):
                    env = kwargs['env']
                    phase = env['ANGELBOT_PRIVATE_LIVE_PHASE']
                    self.assertIn(phase, ('browser-prepare', 'browser-restored'))
                    self.assertEqual(env['ANGELBOT_PRIVATE_RETAINED_SOURCE'], str(retained.resolve()))
                    self.assertNotIn('PRIVATE_API_KEY', env)
                    owned = Path(env['ANGELBOT_PRIVATE_LIVE_ROOT'])
                    (owned / f'{phase}.passed').write_text(env['ANGELBOT_PRIVATE_LIVE_RECEIPT'])
                    if phase == 'browser-prepare':
                        return SimpleNamespace(returncode=0)
                    self.assertEqual(json.loads((owned / 'state.json').read_text()), business)
                    new = {**draft, 'id':2, 'title':'恢复后复核：' + draft['title']}
                    state = {**business, 'drafts':[draft, new], 'saveAttempts':2, 'mutations':2, 'uploadAttempts':2}
                    (owned / 'state.json').write_text(json.dumps(state))
                    (owned / 'upload-2.csv').write_bytes(raw)
                    restored = dict(sessionId='personal-main', successfulWrites=1,
                                    report=json.dumps({**receipt, 'draft_id':2, 'title':new['title']}),
                                    **dict.fromkeys(('sameMain', 'oldHistoryUnchanged', 'sourceFilesUnchanged',
                                                     'newTaskCompleted', 'mcpStarted', 'mcpAuthorizationRenewed'), True))
                    kwargs['stdout'].write('LIVE_BROWSER_RESTORED_REPORT ' + json.dumps(restored) + '\n')
                    if alter_original:
                        (retained / 'restored' / 'data.db').write_bytes(b'changed old failure evidence')
                    return SimpleNamespace(returncode=0)
                with patch.object(browser_acceptance, 'ROOT', root), \
                        patch.object(browser_acceptance, 'build_cargo_binary', return_value=root / 'test.exe'), \
                        patch.object(browser_acceptance.subprocess, 'run', side_effect=driver) as run, \
                        patch.dict(os.environ, {'PRIVATE_API_KEY':'PRIVATE_CANARY'}), \
                        patch.object(sys, 'argv', ['browser_form_acceptance.py', '--restore-only', str(report_path), '--mcp-cli', str(cli)]), \
                        contextlib.redirect_stdout(io.StringIO()):
                    if alter_original:
                        with self.assertRaisesRegex(RuntimeError, 'original failure evidence changed'):
                            browser_acceptance.main()
                    else:
                        self.assertEqual(browser_acceptance.main(), 0)
                self.assertEqual(run.call_count, 2)
                self.assertEqual([call.kwargs['env']['ANGELBOT_PRIVATE_LIVE_PHASE'] for call in run.call_args_list],
                                 ['browser-prepare', 'browser-restored'])
                result, = (root / 'logs').glob('browser-form-*/acceptance.json')
                result = json.loads(result.read_text(encoding='utf-8'))
                self.assertEqual(result['retainedSourceAndFailedTargetUnchanged'], not alter_original)
                self.assertEqual(result['status'], 'failed' if alter_original else 'passed_with_independent_server_check')
                self.assertEqual(json.loads(report_path.read_text()), source_report)
                self.assertEqual(json.loads((previous / 'state.json').read_text()), business)

    def test_continue_delivery_retains_main_history_and_refuses_replay(self):
        for failure in (None, 'old-history', 'replay'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                previous = root / 'prior'
                results = previous / 'restored/results'
                results.mkdir(parents=True)
                (previous / 'browser-prepare.passed').write_text(str(browser_acceptance.uuid.uuid4()))
                raw = b'item,count\ncable,1\n'
                receipt = dict(draft_id=1, status='draft', title='Synthetic', category='equipment', location='Room',
                               date='2026-10-23', notes='Draft only', adapter=True,
                               attachment=dict(name='borrow-items.csv', bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest()))
                delivered = {**receipt, 'draft_id':2, 'title':'恢复后复核：Synthetic'}
                for name, data in (('request.json', b'{}'), ('receipt.json', json.dumps(receipt).encode()),
                                   ('borrow-items.csv', raw), ('restored-upload.json', json.dumps(delivered).encode())):
                    (results / name).write_bytes(data)
                drafts = [{**{k:v for k,v in value.items() if k not in ('draft_id','status')}, 'id':value['draft_id']}
                          for value in (receipt, delivered)]
                business = dict(drafts=drafts, mutations=2, saveAttempts=2, uploadAttempts=2,
                                receiptLost=True, downloadAttempts=0)
                with browser_acceptance.socket.socket() as reserve:
                    reserve.bind(('127.0.0.1', 0))
                    port = reserve.getsockname()[1]
                url = f'http://127.0.0.1:{port}/daily/{browser_acceptance.uuid.uuid4().hex}/'
                source_report = dict(status='failed', ownedRoot=str(previous), ownedServerClosed=True,
                                     retainedSourceAndFailedTargetUnchanged=True, businessState=business,
                                     projectReceipts=[], url=url)
                # Wrapper-only fixture: actual backend tests use the full migrated schema.
                database = results.parent / 'data.db'
                with contextlib.closing(sqlite3.connect(database)) as conn:
                    conn.executescript("CREATE TABLE messages(id TEXT,role TEXT,content TEXT);"
                        "CREATE TABLE sessions(id TEXT,leaf_message_id TEXT);"
                        "CREATE TABLE task_runs(message_id TEXT,session_id TEXT,goal TEXT,status TEXT);"
                        "INSERT INTO messages VALUES('old','assistant','original reply');"
                        "INSERT INTO sessions VALUES('personal-main','old');"
                        "INSERT INTO task_runs VALUES('old','personal-main','original goal','needs_attention');")
                    conn.commit()
                report_path = previous / 'acceptance.json'
                report_path.write_text(json.dumps(source_report))
                (previous / 'state.json').write_text(json.dumps(business))
                (previous / 'live.log').write_text('original failed attempt')
                for index in (1, 2):
                    (previous / f'upload-{index}.csv').write_bytes(raw)
                cli = root / 'cli.js'
                cli.touch()
                (root / 'package.json').write_text(json.dumps({'name':'@playwright/mcp', 'version':'0.0.83'}))
                def driver(_command, **kwargs):
                    env = kwargs['env']
                    self.assertEqual(env['ANGELBOT_PRIVATE_LIVE_PHASE'], 'browser-continue')
                    self.assertEqual(Path(env['ANGELBOT_PRIVATE_CONTINUATION_PROFILE']), results.parent)
                    owned = Path(env['ANGELBOT_PRIVATE_LIVE_ROOT'])
                    (owned / 'browser-continue.passed').write_text(env['ANGELBOT_PRIVATE_LIVE_RECEIPT'])
                    with contextlib.closing(sqlite3.connect(database)) as conn:
                        conn.execute("INSERT INTO messages VALUES('new','assistant','Report: angelbot-file:restored-upload.json')")
                        conn.execute("INSERT INTO task_runs VALUES('new','personal-main','original goal','completed')")
                        conn.execute("UPDATE sessions SET leaf_message_id='new'")
                        if failure == 'old-history':
                            conn.execute("UPDATE messages SET content='changed' WHERE id='old'")
                        conn.commit()
                    if failure == 'replay':
                        parsed = browser_acceptance.urlsplit(url)
                        connection = http.client.HTTPConnection('127.0.0.1', parsed.port, timeout=2)
                        try:
                            connection.request('POST', parsed.path + 'api/drafts', b'{}')
                            response = connection.getresponse()
                            self.assertEqual(response.status, 409)
                            response.read()
                        finally:
                            connection.close()
                    kwargs['stdout'].write('LIVE_BROWSER_CONTINUED_REPORT ' + json.dumps(dict(
                        sessionId='personal-main', sourceMessageId='old', replyMessageId='new',
                        taskCompleted=True, goalUnchanged=True, calls=0, mcpStarted=False)) + '\n')
                    return SimpleNamespace(returncode=0)
                with patch.object(browser_acceptance, 'ROOT', root), \
                        patch.object(browser_acceptance, 'build_cargo_binary', return_value=root / 'test.exe'), \
                        patch.object(browser_acceptance.subprocess, 'run', side_effect=driver) as run, \
                        patch.object(sys, 'argv', ['browser_form_acceptance.py', '--continue-delivery', str(report_path), '--mcp-cli', str(cli)]), \
                        contextlib.redirect_stdout(io.StringIO()):
                    if failure == 'old-history':
                        with self.assertRaisesRegex(RuntimeError, 'Old Main history changed'):
                            browser_acceptance.main()
                    elif failure == 'replay':
                        with self.assertRaises(AssertionError):
                            browser_acceptance.main()
                    else:
                        self.assertEqual(browser_acceptance.main(), 0)
                run.assert_called_once()
                outcome, = (root / 'logs').glob('browser-form-*/acceptance.json')
                outcome = json.loads(outcome.read_text(encoding='utf-8'))
                self.assertEqual(outcome['status'], 'failed' if failure else 'passed_read_only_main_continuation')
                self.assertTrue(outcome['ownedServerClosed'])
                self.assertTrue(outcome['existingDeliveryAndEvidenceFilesUnchanged'])
                self.assertEqual(json.loads(report_path.read_text()), source_report)

    def test_upload_fixture_checks_real_bytes_and_exposes_duplicate_saves(self):
        with tempfile.TemporaryDirectory() as directory:
            owned = Path(directory)
            server, prefix = browser_acceptance.serve_fixture(owned, attachment_upload=True)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            def request(method, route, payload=None):
                connection = http.client.HTTPConnection('127.0.0.1', server.server_port, timeout=2)
                try:
                    data = json.dumps(payload).encode() if payload is not None else None
                    connection.request(method, prefix + route, data, {'Content-Type': 'application/json'})
                    response = connection.getresponse()
                    return response.status, response.read().decode()
                finally:
                    connection.close()
            try:
                code, review = request('GET', 'review')
                self.assertEqual(code, 200)
                self.assertIn('type="file"', review)
                # Compile fixture JS without a DOM, browser, model or package download.
                import re
                scripts = re.findall(r'<script>(.*?)</script>', review, re.S)
                self.assertTrue(scripts)
                subprocess.run([shutil.which('node'), '-e',
                    'for(const source of JSON.parse(require("fs").readFileSync(0,"utf8"))) new (require("vm").Script)(source)'],
                    input=json.dumps(scripts), text=True, encoding='utf-8', capture_output=True, check=True,
                    timeout=10, env=verify.isolated_environment(),
                    creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
                draft = dict(title='合成设备申请', category='equipment', location='二楼会议室',
                             date='2026-10-23', notes='只保存草稿', adapter=True)
                raw = '物品,数量\nHDMI 转接线,1\n'.encode()
                attachment = {'name': 'borrow-items.csv', 'data': base64.b64encode(raw).decode()}
                for extra in (None, {**attachment, 'name': '../escape.csv'},
                              {**attachment, 'data': '!invalid!'},
                              {**attachment, 'data': base64.b64encode(b'x' * 4097).decode()}):
                    payload = draft if extra is None else {**draft, 'attachment': extra}
                    self.assertEqual(request('POST', 'api/drafts', payload)[0], 400)
                self.assertEqual(json.loads((owned / 'state.json').read_text(encoding='utf-8'))['mutations'], 0)
                payload = {**draft, 'attachment': attachment}
                with self.assertRaises((http.client.RemoteDisconnected, ConnectionResetError)):
                    request('POST', 'api/drafts', payload)
                state = json.loads((owned / 'state.json').read_text(encoding='utf-8'))
                self.assertEqual(state['uploadAttempts'], 1)
                self.assertEqual(state['saveAttempts'], 1)
                self.assertTrue(state['receiptLost'])
                expected = {'name': 'borrow-items.csv', 'bytes': len(raw),
                            'sha256': hashlib.sha256(raw).hexdigest()}
                self.assertEqual(state['drafts'][0]['attachment'], expected)
                self.assertEqual((owned / 'upload-1.csv').read_bytes(), raw)
                self.assertIn(expected['sha256'], request('GET', 'drafts/1')[1])
                # Do not add fixture-side idempotency that would hide an Agent replay.
                self.assertEqual(request('POST', 'api/drafts', payload)[0], 200)
                state = json.loads((owned / 'state.json').read_text(encoding='utf-8'))
                self.assertEqual(state['saveAttempts'], 2)
                self.assertEqual(state['uploadAttempts'], 2)
                self.assertEqual(state['mutations'], 2)
                self.assertEqual(len(state['drafts']), 2)
                self.assertEqual((owned / 'upload-2.csv').read_bytes(), raw)
                # The restored journey permits one explicitly new draft, not
                # a replay of the old one. Reject misleading completion proof.
                receipt = {**state['drafts'][0], 'draft_id': 1, 'status': 'draft'}
                receipt.pop('id')
                written = {'sessionId': 'personal-main', 'receipt': receipt, 'successfulWrites': 1}
                reopened = {**written, 'backupRestoreVerified': True}
                new_receipt = {**receipt, 'draft_id': 2, 'title': '恢复后复核：' + receipt['title']}
                restored = {'sessionId': 'personal-main', 'successfulWrites': 1,
                            'report': json.dumps(new_receipt),
                            **dict.fromkeys(('sameMain', 'oldHistoryUnchanged', 'sourceFilesUnchanged',
                                             'newTaskCompleted', 'mcpStarted', 'mcpAuthorizationRenewed'), True)}
                business = json.loads(json.dumps(state))
                business['drafts'][1]['title'] = new_receipt['title']
                browser_acceptance.verify_restored_upload(owned, written, reopened, restored, business)
                for field, value in (('sameMain', False), ('mcpAuthorizationRenewed', False),
                                     ('sourceFilesUnchanged', False), ('successfulWrites', 0),
                                     ('report', json.dumps(receipt))):
                    with self.subTest(restored_field=field), self.assertRaises(AssertionError):
                        browser_acceptance.verify_restored_upload(
                            owned, written, reopened, {**restored, field: value}, business)
                for field, value in (('saveAttempts', 3), ('downloadAttempts', 1), ('uploadAttempts', 1)):
                    with self.subTest(business_field=field), self.assertRaises(AssertionError):
                        browser_acceptance.verify_restored_upload(
                            owned, written, reopened, restored, {**business, field: value})
                (owned / 'upload-2.csv').write_bytes(raw + b'changed')
                with self.assertRaises(AssertionError):
                    browser_acceptance.verify_restored_upload(owned, written, reopened, restored, business)
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=5)
            self.assertFalse(thread.is_alive())
            with self.assertRaises(ValueError):
                browser_acceptance.serve_fixture(owned, saved_draft=True, attachment_upload=True)

    def test_manual_native_profile_and_environment_never_copy_secrets_or_scripts(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "profile.json"
            profile = {"provider": "openai", "model": "saved-test-model", "base_url": "https://example.invalid",
                       "max_tokens": 4096, "temperature": 0.7, "auth_mode": "chatgpt_plan",
                       "credential_ref": "opaque-handle", "protocol": "openai_responses",
                       "api_key": "PRIVATE_CANARY", "plan_account_label": "PRIVATE_CANARY"}
            source.write_text(json.dumps(profile), encoding="utf-8")
            saved = manual.saved_plan_profile(source)
            self.assertEqual(saved["credential_ref"], "opaque-handle")
            self.assertNotIn("PRIVATE_CANARY", json.dumps(saved))
            for field, value in (("auth_mode", "api_key"), ("credential_ref", ""), ("provider", "custom")):
                source.write_text(json.dumps({**profile, field: value}), encoding="utf-8")
                with self.assertRaises(RuntimeError):
                    manual.saved_plan_profile(source)
            with patch.dict(os.environ, {"PRIVATE_API_KEY": "PRIVATE_CANARY", "APPDATA": "original",
                    "ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT": "script", "ANGELBOT_DESKTOP_E2E_MODEL_RESPONSE": "fake",
                    "ANGELBOT_NATIVE_UIA_SMOKE": "1", "VITE_ANGELBOT_DESKTOP_E2E": "1"}, clear=True):
                env = manual.manual_environment(Path(directory), manual.isolated_environment())
            self.assertNotIn("PRIVATE_CANARY", json.dumps(env))
            self.assertEqual(env["APPDATA"], "original")
            self.assertEqual({key for key in env if key.startswith(("ANGELBOT_", "VITE_ANGELBOT_"))},
                             {"ANGELBOT_DESKTOP_E2E_MANUAL", "ANGELBOT_DESKTOP_E2E_DATA_DIR"})

    def test_manual_native_requires_durable_approval_actual_readback_and_main_completion(self):
        baseline = {key: 0 for key in manual.UNCHANGED}
        baseline.update({"passwordUnchanged": True, "processId": 12, "windowTitle": "owned", "ordinaryValue": "ordinary-seed"})
        manual.check_snapshot(baseline, baseline, 12, "owned", "ordinary-seed")
        for key, value in (("processId", 99), ("ordinaryValue", "wrong"), ("passwordUnchanged", False), ("invocationCount", 1)):
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                manual.check_snapshot({**baseline, key: value}, baseline, 12, "owned", "ordinary-seed")
        with tempfile.TemporaryDirectory() as directory:
            db = Path(directory) / "data.db"
            executable = Path(directory) / "fixture.exe"
            executable.write_bytes(b"not an executable; identity only")
            with contextlib.closing(sqlite3.connect(db)) as conn:
                conn.executescript("""CREATE TABLE agent_steps(call_id TEXT,session_id TEXT,tool_name TEXT,tool_input TEXT,tool_output TEXT,success INTEGER);
                    CREATE TABLE task_runs(message_id TEXT,session_id TEXT,status TEXT,created_at INTEGER);
                    CREATE TABLE messages(id TEXT,content TEXT);
                    CREATE TABLE automation_runs(id TEXT);
                    CREATE TABLE desktop_trusted_apps(id TEXT,executable_path TEXT,enabled INTEGER,capabilities TEXT);
                    CREATE TABLE agent_permission_grants(scope TEXT,permission TEXT);
                    INSERT INTO messages VALUES('reply','已填写并回读核验');
                    INSERT INTO task_runs VALUES('reply','main','completed',1);""")
                conn.execute("INSERT INTO desktop_trusted_apps VALUES(?,?,1,?)", (manual.APP_ID, str(executable), '["observe","fill"]'))
                conn.execute("INSERT INTO agent_steps VALUES('reject','main','set_trusted_app_text',?,'Confirmation rejected',0)",
                             (json.dumps({"app_id": manual.APP_ID, "text": "rejected"}),))
                conn.commit()
                self.assertTrue(manual.check_receipts(db, "rejected", "approved", executable, final=False)["rejectionVerified"])
                receipt = {"adapter": "windows-native-uia", "target": manual.APP_ID, "status": "verified"}
                approved = "Confirmation approved: " + json.dumps(receipt)
                conn.execute("INSERT INTO agent_steps VALUES('approve','main','set_trusted_app_text',?,?,1)",
                             (json.dumps({"app_id": manual.APP_ID, "text": "approved"}), approved))
                conn.execute("INSERT INTO agent_steps VALUES('agent-loop-00000000-0000-4000-8000-000000000001','main','agent_loop','null','host terminal diagnostic',0)")
                conn.commit()
                self.assertTrue(manual.check_receipts(db, "rejected", "approved", executable, final=True)["mainDelivered"])
                for broken in ({**receipt, "adapter": "mock"}, {**receipt, "status": "dispatched"},
                               {**receipt, "target": "another-app"}):
                    conn.execute("UPDATE agent_steps SET tool_output=? WHERE call_id='approve'",
                                 ("Confirmation approved: " + json.dumps(broken),))
                    conn.commit()
                    with self.assertRaises(RuntimeError):
                        manual.check_receipts(db, "rejected", "approved", executable, final=True)
                conn.execute("UPDATE agent_steps SET tool_output=? WHERE call_id='approve'", (approved,))
                for query in ("UPDATE task_runs SET status='awaiting_confirmation'",
                              "INSERT INTO automation_runs VALUES('unexpected')",
                              "INSERT INTO agent_steps SELECT 'replay',session_id,tool_name,tool_input,tool_output,success FROM agent_steps WHERE call_id='approve'",
                              "INSERT INTO agent_permission_grants VALUES('global','full_access')",
                              "UPDATE agent_steps SET tool_input='{}' WHERE tool_name='agent_loop'",
                              "UPDATE agent_steps SET success=1 WHERE tool_name='agent_loop'",
                              "UPDATE agent_steps SET call_id='forged-call' WHERE tool_name='agent_loop'",
                              "UPDATE desktop_trusted_apps SET capabilities='[\"observe\",\"fill\",\"interact\"]'"):
                    conn.execute(query)
                    conn.commit()
                    with self.assertRaises(RuntimeError):
                        manual.check_receipts(db, "rejected", "approved", executable, final=True)
                    conn.execute("UPDATE task_runs SET status='completed'")
                    conn.execute("DELETE FROM automation_runs")
                    conn.execute("DELETE FROM agent_steps WHERE call_id='replay'")
                    conn.execute("DELETE FROM agent_permission_grants")
                    conn.execute("UPDATE agent_steps SET call_id='agent-loop-00000000-0000-4000-8000-000000000001',tool_input='null',success=0 WHERE tool_name='agent_loop'")
                    conn.execute("UPDATE desktop_trusted_apps SET capabilities='[\"observe\",\"fill\"]'")
                    conn.commit()

    def test_desktop_cli_help_and_invalid_modes_never_launch_or_build(self):
        for arguments, expected in ((["--help"], 0), (["--reply-export", "--manual-native"], 2),
                                    (["--model-recovery", "--reply-export"], 2),
                                    (["--project-restore", "--manual-native"], 2),
                                    (["--project-restore", "--inspect"], 2), (["--inspect"], 2), (["--typo"], 2)):
            with self.subTest(arguments=arguments), patch.object(sys, "argv", ["desktop_e2e.py", *arguments]), \
                    patch.object(desktop_e2e, "prepare_desktop_binary") as build, \
                    patch.object(manual, "run_manual_native") as start, \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()), \
                    self.assertRaises(SystemExit) as exited:
                desktop_e2e.main()
            self.assertEqual(exited.exception.code, expected)
            build.assert_not_called()
            start.assert_not_called()

    def test_daily_attachment_script_allows_results_but_never_project_commands(self):
        captured = []

        def capture_script(_npx, env, _skip_build):
            captured.extend(json.loads(env["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"]))
            raise RuntimeError("owned script checkpoint")

        # Capture the real emitted fixture before any build, window or driver.
        with patch.object(sys, "argv", ["desktop_e2e.py", "--skip-build"]), \
                patch.object(desktop_e2e, "prepare_desktop_binary", side_effect=capture_script), \
                patch.object(desktop_e2e, "owned_fixture") as fixture, \
                patch.object(desktop_e2e, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "owned script checkpoint"):
                desktop_e2e.main()
            fixture.assert_not_called()
            run.assert_not_called()
        replies = [reply for reply in captured
                   if "daily-actions.csv" in reply.get("expectUserContentIncludes", [])]
        self.assertEqual(len(replies), 3, "initial, continued and edited snapshots must all be checked")
        for reply in replies:
            self.assertIn("attachment-e2e-marker", reply["expectUserContentIncludes"])
            self.assertEqual(reply["forbiddenToolNames"], ["run_project_command"],
                             "Personal owns result-file tools, not project command authority")

    def test_build_and_cache_never_use_normal_artifacts(self):
        startup = (REPO_ROOT / "src-tauri/src/lib.rs").read_text(encoding="utf-8")
        loader = startup.split("fn load_deployment_environment()", 1)[1].split("fn start_dev_http_server()", 1)[0]
        self.assertIn('#[cfg(not(any(test, feature = "desktop-e2e")))]', loader)
        self.assertEqual(startup.count("dotenvy::from_filename"), 1)
        self.assertEqual(startup.count("load_deployment_environment();"), 2)
        message_source = (REPO_ROOT / "src-tauri/src/commands/message.rs").read_text(encoding="utf-8")
        self.assertNotIn("generate_title_from_content", message_source)
        normal_config = json.loads((REPO_ROOT / "src-tauri/tauri.conf.json").read_text())
        # Tauri's native drop interception prevents HTML File-based drops on Windows.
        # Keep explicit user-selected bytes on the same bounded attachment path.
        self.assertIs(normal_config["app"]["windows"][0].get("dragDropEnabled"), False)
        config = json.loads((REPO_ROOT / "src-tauri/tauri.desktop-e2e.conf.json").read_text())
        self.assertEqual(config["build"], {
            "frontendDist": "target/desktop-e2e/frontend",
            "beforeBuildCommand": "npm run build -- --outDir src-tauri/target/desktop-e2e/frontend",
        })
        for mode in ("build", "cached", "missing", "failed"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                normal = root / "src-tauri/target/debug/angelbot.exe"
                normal.parent.mkdir(parents=True)
                normal.write_bytes(b"daily desktop")
                normal_frontend = root / "dist/index.html"
                normal_frontend.parent.mkdir()
                normal_frontend.write_text("daily frontend")
                isolated = root / "src-tauri/target/desktop-e2e/debug/angelbot.exe"
                if mode == "cached":
                    isolated.parent.mkdir(parents=True)
                    isolated.write_bytes(b"cached desktop e2e")
                env = {
                    "CARGO_TARGET_DIR": str(normal.parent),
                    "ANGELBOT_DESKTOP_E2E_BINARY_PATH": str(normal),
                }

                def build(command, child_env):
                    self.assertEqual(child_env["CARGO_TARGET_DIR"], str(isolated.parents[1]))
                    self.assertEqual(child_env["ANGELBOT_DESKTOP_E2E_BINARY_PATH"], str(isolated))
                    self.assertEqual(command, [
                        "npx", "tauri", "build", "--debug", "--no-bundle",
                        "--features", "desktop-e2e", "--config",
                        "src-tauri/tauri.desktop-e2e.conf.json",
                    ])
                    if mode == "failed":
                        raise subprocess.CalledProcessError(2, command)
                    isolated.parent.mkdir(parents=True)
                    isolated.write_bytes(b"new desktop e2e")

                with patch.object(desktop_e2e, "ROOT", root), patch.object(desktop_e2e, "run", side_effect=build) as run:
                    if mode == "missing":
                        with self.assertRaisesRegex(RuntimeError, "without --skip-build"):
                            desktop_e2e.prepare_desktop_binary("npx", env, True)
                    elif mode == "failed":
                        with self.assertRaises(subprocess.CalledProcessError):
                            desktop_e2e.prepare_desktop_binary("npx", env, False)
                    else:
                        self.assertEqual(
                            desktop_e2e.prepare_desktop_binary("npx", env, mode == "cached"),
                            isolated,
                        )
                    self.assertEqual(run.call_count, int(mode in {"build", "failed"}))
                self.assertEqual(env["CARGO_TARGET_DIR"], str(isolated.parents[1]))
                self.assertEqual(env["ANGELBOT_DESKTOP_E2E_BINARY_PATH"], str(isolated))
                self.assertEqual(normal.read_bytes(), b"daily desktop")
                self.assertEqual(normal_frontend.read_text(), "daily frontend")

        with tempfile.TemporaryDirectory() as directory:
            env = {"ANGELBOT_DESKTOP_E2E_SEED_NETWORK_APPROVAL": "1", "ANGELBOT_DESKTOP_E2E_EXPLORER_SCRIPT": "unused"}
            desktop_e2e.configure_reply_export_probe(env, Path(directory))
            self.assertEqual(env["ANGELBOT_DESKTOP_E2E_REPLY_EXPORT"], "1")
            self.assertEqual(Path(env["ANGELBOT_DESKTOP_E2E_EXPORT_DIR"]), Path(directory) / "reply-exports")
            self.assertEqual((Path(directory) / "angelbot-data/results/download.bin").read_bytes(), bytes([0, 255, 1, 128]))
            self.assertEqual((Path(directory) / "angelbot-data/results/验收结果.csv").read_text(encoding="utf-8"), "事项,状态\n本机验收,完成\n")
            self.assertEqual(json.loads(env["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"])[0]["type"], "text")
            self.assertNotIn("ANGELBOT_DESKTOP_E2E_SEED_NETWORK_APPROVAL", env)
            self.assertNotIn("ANGELBOT_DESKTOP_E2E_EXPLORER_SCRIPT", env)

        # Windows may reserve the driver's default 4445 (for example inside
        # 4404..4503). Both launch and polling must use one OS-selected port;
        # no OS exclusion/security settings are changed. Mock loopback here so
        # canonical full remains network-free.
        env = {"TAURI_WEBDRIVER_PORT": "4445", "UNCHANGED": "value"}
        with patch("socket.socket") as socket_factory:
            probe = socket_factory.return_value.__enter__.return_value
            probe.getsockname.return_value = ("127.0.0.1", 9254)
            desktop_e2e.configure_webdriver_port(env)
            probe.bind.assert_called_once_with(("127.0.0.1", 0))
            self.assertEqual(env, {"TAURI_WEBDRIVER_PORT": "9254", "UNCHANGED": "value"})
            self.assertTrue(socket_factory.return_value.__exit__.called)

    def test_wdio_requires_and_reuses_the_explicit_artifact(self):
        node = shutil.which("node")
        self.assertIsNotNone(node, "Node is required by the existing desktop/frontend toolchain")
        script = """
          const {config} = await import(process.argv[1]);
          console.log(JSON.stringify({paths: [
            config.capabilities[0]['tauri:options'].application,
            config.capabilities[0]['wdio:tauriServiceOptions'].appBinaryPath,
            config.services[0][1].appBinaryPath,
          ], specs: config.specs, timeout: config.mochaOpts.timeout}));
        """
        binary = REPO_ROOT / "src-tauri/target/desktop-e2e/debug/angelbot.exe"
        for supplied, probe in ((False, None), (True, None), (True, "reply-export"),
                                (True, "model-recovery"), (True, "project-restore")):
            with self.subTest(supplied=supplied, probe=probe):
                env = {name: os.environ[name] for name in ("PATH", "SYSTEMROOT") if name in os.environ}
                if supplied:
                    env["ANGELBOT_DESKTOP_E2E_BINARY_PATH"] = str(binary)
                if probe:
                    flag = {"reply-export": "REPLY_EXPORT", "model-recovery": "MODEL_RECOVERY",
                            "project-restore": "PROJECT_RESTORE"}[probe]
                    env[f"ANGELBOT_DESKTOP_E2E_{flag}"] = "source" if probe == "project-restore" else "1"
                result = subprocess.run(
                    [node, "--input-type=module", "-e", script, (REPO_ROOT / "e2e/wdio.conf.mjs").as_uri()],
                    env=env, capture_output=True, text=True, timeout=10,
                )
                if supplied:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    configuration = json.loads(result.stdout)
                    self.assertEqual(configuration["paths"], [str(binary)] * 3)
                    self.assertEqual(configuration["specs"], [f"./specs/{probe or 'first-run'}.e2e.mjs"])
                    self.assertEqual(configuration["timeout"], 600_000 if probe == "project-restore" else 300_000 if probe else 90_000)
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("isolated binary path is required", result.stderr)

        # Execute the real spec's failure lifecycle without a desktop. It must
        # invalidate old success evidence and seal the checkpoint, not emit raw
        # diagnostics or fabricate a passed native-picker journey.
        with tempfile.TemporaryDirectory(prefix="angelbot-desktop-e2e-") as directory:
            root = Path(directory)
            evidence = root / "src-tauri/target/e2e"
            evidence.mkdir(parents=True)
            (evidence / "reply-export-desktop.json").write_text('{"status":"passed"}')
            exports = root / "reply-exports"
            exports.mkdir()
            script = """
              import {readFileSync} from 'node:fs';
              import {join} from 'node:path';
              process.chdir(process.argv[2]);
              process.env.ANGELBOT_DESKTOP_E2E_EXPORT_DIR = join(process.cwd(), 'reply-exports');
              let body, cleanup;
              globalThis.describe = (_, fn) => fn();
              globalThis.it = (_, fn) => { body = fn; };
              globalThis.afterEach = fn => { cleanup = fn; };
              globalThis.browser = {tauri: {execute: async () => { throw new Error('PRIVATE_NATIVE_ERROR'); }}};
              await import(process.argv[1]);
              try { await body(); throw new Error('expected failure'); }
              catch (error) { if (error.message !== 'PRIVATE_NATIVE_ERROR') throw error; }
              cleanup.call({currentTest: {state: 'failed'}});
              console.log(readFileSync('src-tauri/target/e2e/reply-export-desktop.json', 'utf8'));
            """
            result = subprocess.run([node, "--input-type=module", "-e", script,
                (REPO_ROOT / "e2e/specs/reply-export.e2e.mjs").as_uri(), str(root)],
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads(result.stdout)
            self.assertEqual(report["status"], "failed")
            self.assertEqual(report["phase"], "startup")
            self.assertFalse(any(report[key] for key in ("nativeCancellation", "markdownExact", "textExact", "transcriptUnchanged")))
            self.assertNotIn("PRIVATE_NATIVE_ERROR", result.stdout)
            self.assertEqual(json.loads((exports / "stage.json").read_text()), {"phase": "failed", "failedPhase": "startup"})

        # A build/driver failure occurs before the JS spec's afterEach hook.
        # It must also invalidate a previous pass, without logging diagnostics
        # into the acceptance report.
        for failure in ("build", "driver", "incomplete", "none"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                report_path = root / "src-tauri/target/e2e/reply-export-desktop.json"
                report_path.parent.mkdir(parents=True)
                report_path.write_text('{"status":"passed"}')
                error = subprocess.CalledProcessError(2, "PRIVATE_STARTUP_ERROR")
                def driver(command, env):
                    if failure == "driver":
                        raise error
                    if failure == "none":
                        report_path.write_text(json.dumps({"status": "passed", "phase": "done",
                            "nativeCancellation": True, "markdownExact": True,
                            "textExact": True, "transcriptUnchanged": True, "dailyPreviewExact": True,
                            "dailyNativeCancellation": True, "binaryExportExact": True}))

                with patch.object(desktop_e2e, "ROOT", root), \
                        patch.object(desktop_e2e, "prepare_desktop_binary", side_effect=error if failure == "build" else None), \
                        patch.object(desktop_e2e, "configure_webdriver_port"), \
                        patch.object(desktop_e2e, "run", side_effect=driver):
                    if failure == "none":
                        desktop_e2e.run_reply_export_probe("npx", {}, root, False)
                    else:
                        with self.assertRaises(RuntimeError if failure == "incomplete" else subprocess.CalledProcessError):
                            desktop_e2e.run_reply_export_probe("npx", {}, root, False)
                report = json.loads(report_path.read_text())
                self.assertEqual(report["status"], "passed" if failure == "none" else "failed")
                self.assertEqual(all(report[key] for key in ("nativeCancellation", "markdownExact", "textExact", "transcriptUnchanged")), failure == "none")
                self.assertNotIn("PRIVATE_STARTUP_ERROR", report_path.read_text())


if __name__ == "__main__":
    unittest.main()
