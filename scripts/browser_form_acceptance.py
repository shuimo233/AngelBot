#!/usr/bin/env python3
"""Opt-in saved-model acceptance: cross-page draft, lost receipt, then reopen.

Uses a cached official Playwright MCP, isolated headless Edge and synthetic data.
Never downloads packages, touches the user's browser or submits a real business request.
Not part of the offline full profile. Requires an already authorized model.
--model-probe only checks the saved Luna binding once, without a browser or task.
"""
from __future__ import annotations

import argparse
import contextlib
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import re
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import uuid
from urllib.parse import urlsplit

from native_uia_smoke import ROOT, build_cargo_binary, isolated_environment


STYLE = """<meta charset="utf-8"><title>个人事务草稿 · 隔离验收</title>
<style>body{font:18px system-ui;max-width:760px;margin:40px auto;color:#242424;background:#fafafa}
label{display:block;margin:20px 0}input,select,textarea,button{font:inherit;padding:8px}
textarea{display:block;width:95%}nav{display:flex;gap:24px;margin-bottom:32px}
dt{font-weight:600;margin-top:18px}dd{margin:8px 0}button{margin:16px 0}</style>"""
FIELDS = ("title", "category", "location", "date", "notes", "adapter")


def page(prefix: str, body: str) -> str:
    return (f'<!doctype html><html lang="zh-CN">{STYLE}<nav>'
            f'<a href="{prefix}">首页</a><a href="{prefix}drafts">草稿箱</a></nav>{body}</html>')


def serve_fixture(owned: Path, *, saved_draft=False, attachment_upload=False, resume_from=None, continuation_url=None):
    if saved_draft and attachment_upload:
        raise ValueError('Upload acceptance must start without a saved draft')
    prefix = f'/daily/{uuid.uuid4().hex}/'
    port = 0
    if continuation_url:
        url = urlsplit(continuation_url)
        assert url.scheme == 'http' and url.hostname == '127.0.0.1'
        assert not (url.username or url.password or url.query or url.fragment)
        assert url.port and 1024 <= url.port <= 65535 and re.fullmatch(r'/daily/[0-9a-f]{32}/', url.path)
        assert resume_from is not None
        prefix, port = url.path, url.port
    state = {"drafts": [], "mutations": 0, "saveAttempts": 0, "receiptLost": False,
             "downloadAttempts": 0, "uploadAttempts": 0}
    if saved_draft:
        # Daily file delivery starts from an existing record. Draft creation
        # and lost-receipt recovery have their own unchanged live journey.
        state.update(drafts=[{'id': 1, 'title': '已有设备借用', 'category': 'equipment',
                             'location': '二楼会议室', 'date': '2026-10-23',
                             'notes': '周五会议使用，只保存草稿，不发送通知。', 'adapter': True}],
                     mutations=1, saveAttempts=1, receiptLost=True)
    if resume_from is not None:
        assert attachment_upload and not saved_draft
        state = json.loads((resume_from / 'state.json').read_text('utf-8'))
        count = 2 if continuation_url else 1
        assert state['saveAttempts'] == state['mutations'] == state['uploadAttempts'] == count
        assert state['downloadAttempts'] == 0 and state['receiptLost'] is True
        assert [draft['id'] for draft in state['drafts']] == list(range(1, count + 1))
        raw = (resume_from / 'upload-1.csv').read_bytes()
        assert 0 < len(raw) <= 4096
        for index, draft in enumerate(state['drafts'], 1):
            assert draft['attachment'] == {
                'name': 'borrow-items.csv', 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}
            assert (resume_from / f'upload-{index}.csv').read_bytes() == raw
            (owned / f'upload-{index}.csv').write_bytes(raw)
    lock = threading.Lock()

    def persist():
        pending = owned / 'state.pending'
        with pending.open('w', encoding='utf-8') as output:
            json.dump(state, output, ensure_ascii=False)
            output.flush()
            os.fsync(output.fileno())
        pending.replace(owned / 'state.json')

    persist()
    # Only the final save changes business state. Browser-local fields carry
    # ordinary navigation, but are never used as evidence of a saved draft.
    form_script = """<script>
const key='owned-form';let draft=JSON.parse(sessionStorage.getItem(key)||'{}');
for(const input of document.querySelectorAll('[name]')){
 if(draft[input.name]!==undefined){if(input.type==='checkbox')input.checked=draft[input.name];else input.value=draft[input.name];}
}
document.querySelector('form').addEventListener('submit',e=>{
 e.preventDefault();for(const input of document.querySelectorAll('[name]'))draft[input.name]=input.type==='checkbox'?input.checked:input.value;
 sessionStorage.setItem(key,JSON.stringify(draft));location.href=e.target.dataset.next;
});</script>"""

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def reply(self, code, body, content_type='text/html; charset=utf-8', disposition=None):
            encoded = body.encode('utf-8')
            self.send_response(code)
            self.send_header('Content-Type', content_type)
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(encoded)))
            if disposition:
                self.send_header('Content-Disposition', disposition)
            self.end_headers()
            self.wfile.write(encoded)

        def do_GET(self):
            if not self.path.startswith(prefix):
                return self.reply(404, 'Not found')
            route = self.path[len(prefix):]
            if route == 'api/state':
                with lock:
                    return self.reply(200, json.dumps(state, ensure_ascii=False), 'application/json; charset=utf-8')
            if route == 'download/1':
                with lock:
                    if len(state['drafts']) != 1:
                        return self.reply(404, 'Draft not found')
                    state['downloadAttempts'] += 1
                    persist()
                return self.reply(200, 'id,status\n1,draft\n', 'text/csv; charset=utf-8', 'attachment; filename="borrow-record.csv"')
            if route == '':
                body = f'<h1>个人事务草稿</h1><p>仅含合成数据，不会发送申请或通知。</p><a href="{prefix}request">新建借用申请</a>'
            elif route == 'request':
                body = (f'<h1>新建借用申请 · 基本信息</h1><form data-next="{prefix}details">'
                        '<label>申请标题 <input name="title" required maxlength="120"></label>'
                        '<label>借用类别 <select name="category"><option value="meeting">会议物料</option>'
                        '<option value="equipment">显示设备</option></select></label><button>下一步：使用信息</button></form>' + form_script)
            elif route == 'details':
                body = (f'<h1>新建借用申请 · 使用信息</h1><form data-next="{prefix}review">'
                        '<label>使用地点 <input name="location" required></label>'
                        '<label>使用日期 <input name="date" required placeholder="YYYY-MM-DD"></label>'
                        '<label>用途说明 <textarea name="notes" required></textarea></label>'
                        '<label><input name="adapter" type="checkbox">需要 HDMI 转接线</label>'
                        '<button>下一步：核对草稿</button></form>' + form_script)
            elif route == 'review':
                body = (f'<h1>核对草稿</h1><dl id="summary"></dl><button id="save">保存草稿</button>'
                        f'<p role="status" id="status"></p><a href="{prefix}drafts">前往草稿箱核对</a>'
                        """<script>const draft=JSON.parse(sessionStorage.getItem('owned-form')||'{}');
const labels={title:'申请标题',category:'借用类别',location:'使用地点',date:'使用日期',notes:'用途说明',adapter:'需要 HDMI 转接线'};
for(const [key,label] of Object.entries(labels)){const dt=document.createElement('dt'),dd=document.createElement('dd');dt.textContent=label;dd.textContent=key==='category'?(draft[key]==='equipment'?'显示设备':'会议物料'):key==='adapter'?(draft[key]?'是':'否'):draft[key];document.querySelector('#summary').append(dt,dd);}
document.querySelector('#save').addEventListener('click',async()=>{
 const status=document.querySelector('#status');status.textContent='正在保存';
 try{const response=await fetch('api/drafts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(draft)});if(!response.ok)throw new Error('Save failed');const data=await response.json();status.textContent='草稿已保存，编号 '+data.id;}
 catch{status.textContent='保存回执未收到，本次结果未知。请先从草稿箱核对，不要重复保存。';}
});</script>""").replace("fetch('api/drafts'", f"fetch('{prefix}api/drafts'")
                if attachment_upload:
                    body = body.replace('<button id="save">', '<label>附件 CSV <input id="attachment" type="file" accept=".csv"></label><button id="save">')
                    body = body.replace("try{const response=", """try{const file=document.querySelector('#attachment').files[0];
 if(!file||file.size>4096)throw new Error('Choose one bounded CSV');
 draft.attachment={name:file.name,data:btoa(String.fromCharCode(...new Uint8Array(await file.arrayBuffer())))};
 const response=""")
            elif route == 'drafts' or route.startswith('drafts/'):
                with lock:
                    drafts = json.loads(json.dumps(state['drafts']))
                if route == 'drafts':
                    body = '<h1>草稿箱</h1><p>以下记录均已保存在服务端，尚未发送。</p><ul>'
                    for draft in drafts:
                        # JSON text is inserted using textContent, never HTML.
                        body += f'<li><a href="{prefix}drafts/{draft["id"]}" data-title={json.dumps(str(draft["id"]))}>草稿 {draft["id"]}</a></li>'
                    body += '</ul><p id="count">共 ' + str(len(drafts)) + ' 份草稿</p>'
                    body += '<script>const drafts=' + json.dumps(drafts, ensure_ascii=True).replace('<', '\\u003c') + ";for(const a of document.querySelectorAll('[data-title]'))a.textContent=drafts.find(x=>String(x.id)===a.dataset.title).title;</script>"
                else:
                    identifier = route.removeprefix('drafts/')
                    draft = next((item for item in drafts if str(item['id']) == identifier), None)
                    if draft is None:
                        return self.reply(404, 'Draft not found')
                    body = '<h1>已保存的草稿</h1><p>状态：草稿，未发送</p><dl id="saved"></dl><script>const draft=' + json.dumps(draft, ensure_ascii=True).replace('<', '\\u003c') + ";for(const [key,value] of Object.entries(draft)){const dt=document.createElement('dt'),dd=document.createElement('dd');dt.textContent=key;dd.textContent=typeof value==='object'?JSON.stringify(value):String(value);document.querySelector('#saved').append(dt,dd);}</script>"
                    body += f'<a href="{prefix}download/1" download>下载草稿 CSV</a>'
            else:
                return self.reply(404, 'Not found')
            self.reply(200, page(prefix, body))

        def do_POST(self):
            if continuation_url:
                with lock:
                    state['blockedMutationAttempts'] = state.get('blockedMutationAttempts', 0) + 1
                    persist()
                return self.reply(409, 'Read-only continuation; mutation refused')
            if self.path != prefix + 'api/drafts':
                return self.reply(404, 'Not found')
            try:
                size = int(self.headers.get('Content-Length', '0'))
                if not 0 < size <= 8192:
                    raise ValueError('Invalid length')
                draft = json.loads(self.rfile.read(size))
                if (set(draft) != set(FIELDS) | ({'attachment'} if attachment_upload else set())
                        or not all(isinstance(draft[k], str) and 0 < len(draft[k]) <= 500 for k in FIELDS[:-1])
                        or not isinstance(draft['adapter'], bool)):
                    raise ValueError('Invalid fields')
                uploaded = None
                if attachment_upload:
                    attachment = draft['attachment']
                    if (not isinstance(attachment, dict) or set(attachment) != {'name', 'data'}
                            or not isinstance(attachment['name'], str) or not 0 < len(attachment['name']) <= 128
                            or any(char in attachment['name'] for char in '/\\:\x00')
                            or not attachment['name'].endswith('.csv') or not isinstance(attachment['data'], str)):
                        raise ValueError('Invalid attachment')
                    uploaded = base64.b64decode(attachment['data'], validate=True)
                    if not 0 < len(uploaded) <= 4096:
                        raise ValueError('Invalid attachment size')
                    draft['attachment'] = {'name': attachment['name'], 'bytes': len(uploaded),
                                           'sha256': hashlib.sha256(uploaded).hexdigest()}
            except (ValueError, TypeError, KeyError):
                return self.reply(400, 'Invalid draft')
            with lock:
                state['saveAttempts'] += 1
                draft['id'] = len(state['drafts']) + 1
                state['drafts'].append(draft)
                state['mutations'] += 1
                if uploaded is not None:
                    # No filename supplied by the browser is used as a path.
                    # Keep each real attempt: duplicate saves must remain observable.
                    target = owned / f'upload-{state["saveAttempts"]}.csv'
                    with target.open('xb') as output:
                        output.write(uploaded)
                        output.flush()
                        os.fsync(output.fileno())
                    state['uploadAttempts'] += 1
                lose_receipt = not state['receiptLost']
                state['receiptLost'] = True
                persist()
            if lose_receipt:
                # Commit first, then lose only the HTTP receipt. A second POST
                # really would create a duplicate, so recovery cannot be faked.
                self.close_connection = True
                with contextlib.suppress(OSError):
                    self.connection.shutdown(socket.SHUT_RDWR)
                self.connection.close()
                return
            self.reply(200, json.dumps({'id': draft['id']}), 'application/json')

    server = ThreadingHTTPServer(('127.0.0.1', port), Handler, bind_and_activate=False)
    try:
        if continuation_url and os.name == 'nt':
            server.allow_reuse_address = False
            server.socket.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        server.server_bind()
        server.server_activate()
    except BaseException:
        server.server_close()
        raise
    server.daemon_threads = True
    return server, prefix


def verify_restored_upload(owned, written, reopened, restored, business):
    """Check server bytes independently; Main's completion text is not proof."""
    assert written['sessionId'] == reopened['sessionId'] == restored['sessionId'] == 'personal-main'
    assert written['receipt'] == reopened['receipt']
    assert written['successfulWrites'] == reopened['successfulWrites'] == 1
    assert reopened['backupRestoreVerified'] is True
    assert all(restored[key] is True for key in (
        'sameMain', 'oldHistoryUnchanged', 'sourceFilesUnchanged', 'newTaskCompleted',
        'mcpStarted', 'mcpAuthorizationRenewed'))
    assert restored['successfulWrites'] == 1
    assert business['saveAttempts'] == business['mutations'] == business['uploadAttempts'] == 2
    assert business['downloadAttempts'] == 0 and business['receiptLost'] is True
    assert len(business['drafts']) == 2
    old = dict(written['receipt'])
    assert old.pop('status') == 'draft'
    assert old.pop('draft_id') == 1
    old['id'] = 1
    assert business['drafts'][0] == old
    uploaded = (owned / 'upload-1.csv').read_bytes()
    attachment = {'name': 'borrow-items.csv', 'bytes': len(uploaded),
                  'sha256': hashlib.sha256(uploaded).hexdigest()}
    assert (owned / 'upload-2.csv').read_bytes() == uploaded
    assert {p.name for p in owned.glob('upload-*.csv')} == {'upload-1.csv', 'upload-2.csv'}
    assert old['attachment'] == attachment
    expected = {**written['receipt'], 'draft_id': 2,
                'title': '恢复后复核：' + written['receipt']['title']}
    assert json.loads(restored['report']) == expected
    new = {**old, 'id': 2, 'title': expected['title']}
    assert business['drafts'][1] == new


def run_model_probe(recorded_db=None):
    """One explicit saved-plan inference, never a business-task quota probe."""
    (ROOT / 'logs').mkdir(exist_ok=True)
    recorded_db = recorded_db.resolve(strict=True) if recorded_db is not None else None
    owned = Path(tempfile.mkdtemp(prefix='inference-replay-' if recorded_db else 'model-probe-', dir=ROOT / 'logs'))
    report_path = owned / 'acceptance.json'
    report = {'status': 'building', 'scope': 'saved-luna-single-provider-call-no-tools',
              'requestCount': None, 'toolCount': 0, 'checkedAt': None}
    if recorded_db:
        report.update(scope='recorded synthetic protocol with reconstructed current host instructions; one provider invocation, no tool execution; not byte-exact replay or business acceptance',
                      sourceDatabaseSha256=hashlib.sha256(recorded_db.read_bytes()).hexdigest())
    def record():
        report_path.write_text(json.dumps(report, indent=2), encoding='utf-8')
    record()
    print(f'MODEL_PROBE_REPORT {report_path}', flush=True)
    try:
        with (owned / 'live.log').open('w', encoding='utf-8') as log:
            with contextlib.redirect_stdout(log), contextlib.redirect_stderr(log):
                binary = build_cargo_binary(library_test=True)
            env = {key: value for key, value in isolated_environment().items()
                   if not key.startswith(('ANGELBOT_', 'VITE_ANGELBOT_'))}
            env.update({'ANGELBOT_LIVE_MODEL_TEST': '1', 'ANGELBOT_LIVE_MODEL_JOURNEY': 'model-probe'})
            if recorded_db:
                env.update({'ANGELBOT_LIVE_MODEL_JOURNEY': 'recorded-inference',
                            'ANGELBOT_PRIVATE_REPLAY_DB': str(recorded_db),
                            'ANGELBOT_PRIVATE_REPLAY_OUTPUT': str(owned / 'diagnostics.db')})
            report['status'] = 'running'
            record()
            log.flush()
            result = subprocess.run([str(binary), 'commands::message::live_tests::configured_model_completes_bounded_file_task',
                                     '--exact', '--ignored', '--nocapture', '--test-threads=1'],
                                    cwd=ROOT, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                    timeout=75, creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        prefix = 'RECORDED_INFERENCE_REPORT ' if recorded_db else 'MODEL_AVAILABILITY_REPORT '
        with (owned / 'live.log').open(encoding='utf-8') as log:
            markers = [line.removeprefix(prefix).strip() for line in log if line.startswith(prefix)]
        if result.returncode or len(markers) != 1 or len(markers[0]) > (1024 if recorded_db else 256):
            raise RuntimeError('The single-request model probe did not return one terminal report')
        outcome = json.loads(markers[0])
        expected = {'status', 'requestCount', 'toolCount'}
        if recorded_db:
            expected |= {'historyCount', 'schemaCount', 'diagnosticRef', 'reconstructedNotByteExact'}
        if (not isinstance(outcome, dict) or set(outcome) != expected
                or outcome['status'] not in ('available', 'quota_exceeded', 'unavailable', 'timed_out')
                or type(outcome['requestCount']) is not int or outcome['requestCount'] not in (0, 1)
                or type(outcome['toolCount']) is not int or outcome['toolCount'] != 0
                or (outcome['status'] != 'unavailable' and outcome['requestCount'] != 1)):
            raise RuntimeError('The model probe returned an invalid bounded report')
        if recorded_db:
            if (outcome['reconstructedNotByteExact'] is not True
                    or any(type(outcome[k]) is not int or outcome[k] <= 0 for k in ('historyCount', 'schemaCount'))
                    or not (outcome['diagnosticRef'] is None or isinstance(outcome['diagnosticRef'], str)
                            and re.fullmatch(r'diag:evidence_[0-9a-f]{32}', outcome['diagnosticRef']))
                    or outcome['status'] == 'available' and outcome['diagnosticRef'] is not None):
                raise RuntimeError('The model probe returned an invalid bounded report')
            assert hashlib.sha256(recorded_db.read_bytes()).hexdigest() == report['sourceDatabaseSha256'], 'Recorded database changed during the read-only probe'
        report.update(outcome, checkedAt=time.time())
    except BaseException:
        report['status'] = 'failed'
        raise
    finally:
        record()
    print(f'MODEL_PROBE_STATUS {report["status"]}', flush=True)
    return 0 if report['status'] == 'available' else 2


def message_rows(database):
    with contextlib.closing(sqlite3.connect(database.resolve().as_uri() + '?mode=ro', uri=True)) as conn:
        conn.row_factory = sqlite3.Row
        return {row['id']: dict(row) for row in conn.execute('SELECT * FROM messages')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mcp-cli', type=Path, help='Cached official Playwright MCP 0.0.83 cli.js; required for browser tasks, never downloaded here')
    parser.add_argument('--hold-for-inspection', action='store_true', help='After success, keep the owned page alive until its stop file exists (max 10 minutes)')
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument('--workspace-report', action='store_true', help='Read synthetic project data, save one webpage draft and a local JSON receipt, then reopen both read-only')
    modes.add_argument('--daily-report', action='store_true', help='Verify Daily delivery, download, reopen, encrypted restore and a new task in the restored Main; no project')
    modes.add_argument('--upload-report', action='store_true', help='Upload a synthetic Daily CSV, reopen read-only, then verify a new upload task in the same Main after encrypted restore and explicit service reauthorization')
    modes.add_argument('--model-probe', action='store_true', help='One saved Luna plan provider call only; no browser, Main task, tools or business retry')
    modes.add_argument('--recorded-inference', type=Path, help='Read a retained synthetic restored data.db and diagnose its protocol once; never execute returned tool calls or start a browser')
    modes.add_argument('--restore-only', type=Path, help='Reuse a failed upload acceptance report and its verified retained source; import into a new isolated profile and run only its restored stage')
    modes.add_argument('--continue-delivery', type=Path, help='Continue the same retained Main after its upload/report already succeeded; no new save, upload or file write')
    args = parser.parse_args()
    if args.model_probe or args.recorded_inference:
        if args.mcp_cli or args.hold_for_inspection:
            parser.error('--model-probe does not use a browser or inspection window')
        return run_model_probe(args.recorded_inference) if args.recorded_inference else run_model_probe()
    if not args.mcp_cli:
        parser.error('--mcp-cli is required for browser tasks')
    cli = args.mcp_cli.resolve(strict=True)
    package = json.loads(cli.with_name('package.json').read_text('utf-8'))
    if package.get('name') != '@playwright/mcp' or package.get('version') != '0.0.83':
        parser.error('Use the already approved official @playwright/mcp@0.0.83 installation')
    previous = None
    retained = None
    snapshots = {}
    old_rows = {}
    continuing = args.continue_delivery is not None
    previous_path = args.continue_delivery or args.restore_only
    if continuing:
        previous_path = previous_path.resolve(strict=True)
        previous = json.loads(previous_path.read_text('utf-8'))
        assert previous['status'] == 'failed' and previous['ownedServerClosed'] is True
        assert previous['retainedSourceAndFailedTargetUnchanged'] is True
        assert Path(previous['ownedRoot']).resolve() == previous_path.parent
        uuid.UUID((previous_path.parent / 'browser-prepare.passed').read_text())
        retained = previous_path.parent / 'restored'
        old_rows = message_rows(retained / 'data.db')
        assert old_rows
        old = json.loads((retained / 'results/receipt.json').read_text('utf-8'))
        expected = {**old, 'draft_id':2, 'title':'恢复后复核：' + old['title']}
        assert json.loads((retained / 'results/restored-upload.json').read_text('utf-8')) == expected
        expected.pop('status')
        expected['id'] = expected.pop('draft_id')
        assert previous['businessState']['drafts'][1] == expected
        paths = [retained / 'results' / name for name in
                 ('request.json', 'receipt.json', 'borrow-items.csv', 'restored-upload.json')]
        paths += [previous_path, previous_path.with_name('live.log'), previous_path.with_name('state.json')]
        snapshots.update({str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths})
    if args.restore_only:
        args.restore_only = args.restore_only.resolve(strict=True)
        previous = json.loads(args.restore_only.read_text('utf-8'))
        assert previous['status'] == 'failed' and previous['ownedServerClosed'] is True
        assert Path(previous['ownedRoot']).resolve() == args.restore_only.parent
        written, reopened = previous['projectReceipts']
        assert [written['phase'], reopened['phase']] == ['source-to-web-to-report', 'reopen-read-only']
        assert written['sessionId'] == reopened['sessionId'] == 'personal-main'
        assert written['receipt'] == reopened['receipt']
        assert written['successfulWrites'] == reopened['successfulWrites'] == 1
        assert written['sourceUnchanged'] is reopened['sourceUnchanged'] is True
        assert reopened['backupRestoreVerified'] is True
        business = json.loads((args.restore_only.parent / 'state.json').read_text('utf-8'))
        assert business == previous['businessState']
        old = {**written['receipt']}
        assert old.pop('draft_id') == 1 and old.pop('status') == 'draft'
        assert business['drafts'] == [{**old, 'id': 1}]
        roots = re.findall(r'owned fixture retained at ([^\r\n]+)',
                           (args.restore_only.parent / 'live.log').read_text('utf-8'))
        assert len(roots) == 1
        retained = Path(roots[0]).resolve(strict=True)
        marker = (retained / 'browser-write.passed').read_text()
        uuid.UUID(marker)
        assert (retained / 'browser-reopen.passed').read_text() == marker
        for parent in (retained, retained / 'restored'):
            paths = [parent / 'data.db', *[parent / 'results' / name for name in
                     ('request.json', 'receipt.json', 'borrow-items.csv')]]
            wal = parent / 'data.db-wal'
            if wal.exists():
                paths.append(wal)
            snapshots.update({str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths})
            assert json.loads((parent / 'results/receipt.json').read_text('utf-8')) == written['receipt']
            assert (parent / 'results/borrow-items.csv').read_bytes() == (args.restore_only.parent / 'upload-1.csv').read_bytes()
        assert not (retained / 'restored/results/restored-upload.json').exists()
    (ROOT / 'logs').mkdir(exist_ok=True)
    owned = Path(tempfile.mkdtemp(prefix='browser-form-', dir=ROOT / 'logs'))
    upload = args.upload_report or args.restore_only is not None or continuing
    server, prefix = serve_fixture(owned, saved_draft=args.daily_report, attachment_upload=upload,
                                   resume_from=previous_path.parent if previous else None,
                                   continuation_url=previous['url'] if continuing else None)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f'http://127.0.0.1:{server.server_port}{prefix}'
    report = {'status': 'building', 'url': url, 'ownedRoot': str(owned), 'pid': os.getpid(),
              'model': 'saved model / ChatgptPlan', 'package': '@playwright/mcp@0.0.83',
              'scope': ('explicit synthetic Daily CSV → webpage attachment → Daily receipt → read-only reopen → encrypted restore → new upload task with renewed MCP authorization'
                        if upload else 'synthetic project file → webpage draft → project receipt → read-only reopen'
                        if args.workspace_report else 'Existing synthetic draft → Daily receipt + downloaded CSV → reopen → backup/restore → new-process Main creates a new report (does not create the draft)'
                        if args.daily_report else 'synthetic draft only')
                       + '; isolated headless Edge; no user browser or real submissions'}
    if continuing:
        report.update(scope='Explicit Continue on the same retained Main and existing delivery; read-only fixture, no new import/save/upload/report write',
                      reusedSourceReport=str(previous_path), continuedProfile=str(retained))
    elif previous:
        report.update(scope='Restored stage only; verified source and read-only reopen evidence reused; fresh encrypted import and restored Main task, not a rerun of the original whole journey; isolated headless Edge',
                      reusedSourceReport=str(args.restore_only), retainedFixtureRoot=str(retained))
    report_path = owned / 'acceptance.json'

    def record():
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    try:
        record()
        print(f'OWNED_WEBPAGE {url}\nOWNED_REPORT {report_path}', flush=True)
        with (owned / 'live.log').open('w', encoding='utf-8') as log:
            with contextlib.redirect_stdout(log), contextlib.redirect_stderr(log):
                binary = build_cargo_binary(library_test=True)
            log.flush()
            env = {key: value for key, value in isolated_environment().items()
                   if not key.startswith(('ANGELBOT_', 'VITE_ANGELBOT_'))}
            env.update({'ANGELBOT_LIVE_MODEL_TEST': '1', 'ANGELBOT_LIVE_MODEL_JOURNEY': 'browser-daily',
                        'ANGELBOT_LIVE_MODEL_PERMISSION': 'full_access',
                        'ANGELBOT_LIVE_BROWSER_TASK': 'upload-report' if upload else 'form-report' if args.workspace_report else 'daily-report' if args.daily_report else 'form',
                        'ANGELBOT_LIVE_BROWSER_URL': url, 'ANGELBOT_LIVE_BROWSER_CLI': str(cli)})
            if previous:
                env.update({'ANGELBOT_PRIVATE_LIVE_ROOT': str(owned),
                            'ANGELBOT_PRIVATE_LIVE_MARKER': str(uuid.uuid4()),
                            'ANGELBOT_PRIVATE_LIVE_RECEIPT': str(uuid.uuid4()),
                            'ANGELBOT_PRIVATE_CONTINUATION_PROFILE' if continuing else 'ANGELBOT_PRIVATE_RETAINED_SOURCE': str(retained)})
            report['status'] = 'running_live_model'
            record()
            phases = ('browser-continue',) if continuing else ('browser-prepare', 'browser-restored') if previous else (None,)
            for phase in phases:
                if phase:
                    env['ANGELBOT_PRIVATE_LIVE_PHASE'] = phase
                    assert not (owned / f'{phase}.passed').exists()
                result = subprocess.run([str(binary), 'commands::message::live_tests::configured_model_completes_bounded_file_task',
                                     '--exact', '--ignored', '--nocapture', '--test-threads=1'], cwd=ROOT, env=env.copy(),
                                    stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                    creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
                if result.returncode:
                    break
                if phase:
                    assert (owned / f'{phase}.passed').read_text() == env['ANGELBOT_PRIVATE_LIVE_RECEIPT']
        report.update(liveExit=result.returncode, status='awaiting_independent_page_check' if result.returncode == 0 else 'failed',
                      businessState=json.loads((owned / 'state.json').read_text('utf-8')))
        if args.workspace_report or args.daily_report or upload:
            report['projectReceipts'] = previous['projectReceipts'] if previous else [json.loads(line.removeprefix('LIVE_BROWSER_PROJECT_REPORT '))
                                         for line in (owned / 'live.log').read_text('utf-8').splitlines()
                                         if line.startswith('LIVE_BROWSER_PROJECT_REPORT ')]
            if result.returncode == 0:
                if not continuing:
                    written, reopened = report['projectReceipts']
                    assert [written['phase'], reopened['phase']] == ['source-to-web-to-report', 'reopen-read-only']
                    assert written['sessionId'] == reopened['sessionId'], 'Reopen must keep the same project Main'
                    assert written['receipt'] == reopened['receipt']
                    assert all(item['successfulWrites'] == 1 and item['sourceUnchanged'] is True
                               for item in (written, reopened))
                if upload and not continuing:
                    business = report['businessState']
                    restored, = [json.loads(line.removeprefix('LIVE_BROWSER_RESTORED_REPORT '))
                                 for line in (owned / 'live.log').read_text('utf-8').splitlines()
                                 if line.startswith('LIVE_BROWSER_RESTORED_REPORT ')]
                    verify_restored_upload(owned, written, reopened, restored, business)
                    report['restoredMain'] = restored
                    report['status'] = 'passed_with_independent_server_check'
                if continuing:
                    continued, = [json.loads(line.removeprefix('LIVE_BROWSER_CONTINUED_REPORT '))
                                  for line in (owned / 'live.log').read_text('utf-8').splitlines()
                                  if line.startswith('LIVE_BROWSER_CONTINUED_REPORT ')]
                    assert continued['sessionId'] == 'personal-main'
                    assert continued['taskCompleted'] is continued['goalUnchanged'] is True
                    assert continued['sourceMessageId'] in old_rows
                    rows = message_rows(retained / 'data.db')
                    reply = rows[continued['replyMessageId']]
                    assert reply['role'] == 'assistant' and 'angelbot-file:restored-upload.json' in reply['content']
                    with contextlib.closing(sqlite3.connect((retained / 'data.db').resolve().as_uri() + '?mode=ro', uri=True)) as conn:
                        task = conn.execute('SELECT session_id,goal,status FROM task_runs WHERE message_id=?', (continued['replyMessageId'],)).fetchone()
                        source_goal, = conn.execute('SELECT goal FROM task_runs WHERE message_id=?', (continued['sourceMessageId'],)).fetchone()
                        assert task == ('personal-main', source_goal, 'completed')
                        assert conn.execute('SELECT leaf_message_id FROM sessions WHERE id=?', ('personal-main',)).fetchone() == (continued['replyMessageId'],)
                    assert report['businessState'] == previous['businessState']
                    report.update(continuedMain=continued, status='passed_read_only_main_continuation')
                if args.daily_report:
                    assert written['sessionId'] == 'personal-main'
                    business = report['businessState']
                    assert len(business['drafts']) == 1
                    assert business['saveAttempts'] == business['mutations'] == business['downloadAttempts'] == 1
                    assert business['receiptLost'] is True
                    assert business['drafts'][0] == {
                        'id': 1, 'title': '已有设备借用', 'category': 'equipment', 'location': '二楼会议室',
                        'date': '2026-10-23', 'notes': '周五会议使用，只保存草稿，不发送通知。', 'adapter': True,
                    }
                    assert reopened['backupRestoreVerified'] is True
                    restored, = [json.loads(line.removeprefix('LIVE_BROWSER_RESTORED_REPORT '))
                                 for line in (owned / 'live.log').read_text('utf-8').splitlines()
                                 if line.startswith('LIVE_BROWSER_RESTORED_REPORT ')]
                    assert restored['sessionId'] == written['sessionId']
                    assert all(restored[key] is True for key in (
                        'sameMain', 'oldHistoryUnchanged', 'sourceFilesUnchanged', 'newTaskCompleted'))
                    assert restored['successfulWrites'] == 1 and restored['mcpStarted'] is False
                    report['restoredMain'] = restored
                    report['status'] = 'passed_with_independent_server_check'
        record()
        print(f'LIVE_TERMINAL {result.returncode}', flush=True)
        if result.returncode == 0 and args.hold_for_inspection:
            print(f'AWAITING_READ_ONLY_CHECK {url}', flush=True)
            deadline = time.monotonic() + 600
            while not (owned / 'stop').exists() and time.monotonic() < deadline:
                time.sleep(0.5)
        return result.returncode
    except BaseException:
        report['status'] = 'failed'
        raise
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        report['ownedServerClosed'] = True
        snapshot_field = 'existingDeliveryAndEvidenceFilesUnchanged' if continuing else 'retainedSourceAndFailedTargetUnchanged'
        if snapshots:
            report[snapshot_field] = all(
                Path(path).is_file() and hashlib.sha256(Path(path).read_bytes()).hexdigest() == digest
                for path, digest in snapshots.items())
            if not report[snapshot_field]:
                report['status'] = 'failed'
        if continuing:
            rows = message_rows(retained / 'data.db')
            report['oldRawHistoryUnchanged'] = all(rows.get(key) == value for key, value in old_rows.items())
            report['oldRawMessageCount'] = len(old_rows)
            if not report['oldRawHistoryUnchanged']:
                report['status'] = 'failed'
        record()
        print('OWNED_SERVER_CLOSED', flush=True)
        if continuing and not report['oldRawHistoryUnchanged']:
            raise RuntimeError('Old Main history changed during continuation')
        if snapshots and not report[snapshot_field]:
            raise RuntimeError('Retained source or original failure evidence changed')


if __name__ == '__main__':
    raise SystemExit(main())
