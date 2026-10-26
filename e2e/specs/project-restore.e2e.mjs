import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';

// The source is synthetic preparation, not native authorization evidence.
// Target/reopen never invoke a directory grant or operate its permission UI.
const PASSWORD = 'Restore-Probe-2026!';
const PROJECT_A = { id: 'restore-probe-a', name: 'Restore Probe A', sessionId: 'restore-probe-a-main' };
const PROJECT_B = { id: 'restore-probe-b', name: 'Restore Probe B', sessionId: 'restore-probe-b-main' };
const DAILY = { id: 'personal', name: 'AngelBot 日常', sessionId: 'personal-main' };
const DAILY_CANARY = 'DAILY-RESTORE-CANARY\n';
const RECEIPT = 'Synthetic read receipt retained.';

const invoke = (command, args = {}) => browser.tauri.execute(
  ({ core }, command, args) => core.invoke(command, args), command, args,
);
const workspaces = () => invoke('get_workspaces');
const messages = (sessionId) => invoke('get_messages', { sessionId });
const logicalMessages = (rows) => rows.map((row) => ({
  id: row.id, sessionId: row.sessionId, parentId: row.parentId ?? null,
  role: row.role, content: row.content,
}));

function fixture(runtime) {
  const a = { ...PROJECT_A, originalPath: join(runtime, 'old-A'), mappedPath: join(runtime, 'local-A') };
  const b = { ...PROJECT_B, originalPath: join(runtime, 'old-B') };
  const project = (item) => ({ id: item.id, name: item.name, kind: 'project', path: item.originalPath,
    active_session_id: item.sessionId, directory_confirmed: 0, created_at: 1, updated_at: 1 });
  const session = (id, projectId, leaf, parent = null) => ({ id, title: id, project_id: projectId,
    parent_id: parent, leaf_message_id: leaf, work_dir: null, additional_read_dirs: '[]',
    created_at: 1, updated_at: 1 });
  const message = (id, sessionId, role, content, parent = null) => ({ id, session_id: sessionId,
    role, content, parent_id: parent, created_at: role === 'user' ? 1 : 2, is_provisional: 0 });
  const seed = {
    version: 2, schema_version: 0, exported_at: 1, scope: 'conversations',
    projects: [
      { id: DAILY.id, name: DAILY.name, kind: 'personal', path: '', active_session_id: DAILY.sessionId,
        directory_confirmed: 1, created_at: 1, updated_at: 1 }, project(a), project(b),
    ],
    sessions: [{ ...session(DAILY.sessionId, DAILY.id, 'restore-daily-reply'), title: '' },
      session(a.sessionId, a.id, 'restore-a-reply'), session(b.sessionId, b.id, 'restore-b-reply'),
      { ...session('restore-probe-a-branch', a.id, 'restore-a-branch-reply', a.sessionId), branch_name: 'Preserved branch' }],
    messages: [
      message('restore-daily-user', DAILY.sessionId, 'user', 'restore-daily-request'),
      message('restore-daily-reply', DAILY.sessionId, 'assistant', 'restore-daily-history-visible\n\n[Daily scope](angelbot-file:scope.txt)', 'restore-daily-user'),
      message('restore-a-user', a.sessionId, 'user', 'restore-a-request'),
      { ...message('restore-a-reply', a.sessionId, 'assistant', 'restore-a-history-visible\n\n[Project scope](angelbot-file:scope.txt)', 'restore-a-user'),
        tool_calls: JSON.stringify([{ id: 'restore-a-read', name: 'read_file', arguments: JSON.stringify({ path: 'scope.txt' }) }]) },
      message('restore-a-inactive', a.sessionId, 'assistant', 'restore-a-inactive-branch', 'restore-a-user'),
      message('restore-a-branch-user', 'restore-probe-a-branch', 'user', 'restore-child-request'),
      message('restore-a-branch-reply', 'restore-probe-a-branch', 'assistant', 'restore-child-history', 'restore-a-branch-user'),
      message('restore-b-user', b.sessionId, 'user', 'restore-b-request'),
      message('restore-b-reply', b.sessionId, 'assistant', 'restore-b-history-visible', 'restore-b-user'),
    ],
    context_summaries: [],
    session_branches: [{ id: 'restore-a-branch-record', session_id: 'restore-probe-a-branch',
      parent_branch_id: null, name: 'Preserved branch', created_at: 1, updated_at: 1 }],
    task_runs: [{ id: 'restore-a-reply', session_id: a.sessionId, message_id: 'restore-a-reply',
      goal: 'Restore project history', status: 'completed', plan: '[]', confirmation_state: 'none',
      resumable: 0, step_count: 1, completed_step_count: 1, created_at: 1, updated_at: 2 }],
    task_run_facts: [{ task_run_id: 'restore-a-reply', session_id: a.sessionId, message_id: 'restore-a-reply',
      facts_json: JSON.stringify({ goal: 'Restore project history' }), created_at: 1, updated_at: 2 }],
    agent_steps: [{ id: 'restore-a-read-step', session_id: a.sessionId, tool_name: 'read_file',
      call_id: 'restore-a-read', tool_input: JSON.stringify({ path: 'scope.txt' }), tool_output: RECEIPT,
      success: 1, seq: 0, created_at: 2 }],
    agent_run_events: [],
    daily_files: [{ path: 'scope.txt', content_base64: Buffer.from(DAILY_CANARY).toString('base64') }],
  };
  const history = (ids) => ids.map((id) => {
    const row = seed.messages.find((row) => row.id === id);
    return { id: row.id, sessionId: row.session_id, parentId: row.parent_id, role: row.role, content: row.content };
  });
  const tables = ['projects', 'sessions', 'messages', 'context_summaries', 'session_branches',
    'task_runs', 'task_run_facts', 'agent_steps', 'agent_run_events'];
  const canaries = { projectA: readFileSync(join(a.mappedPath, 'scope.txt'), 'utf8'),
    oldA: readFileSync(join(a.originalPath, 'scope.txt'), 'utf8'),
    projectB: readFileSync(join(b.originalPath, 'scope.txt'), 'utf8'), daily: DAILY_CANARY };
  assert.ok(Object.values(canaries).every((value) => value.trim().length > 0));
  assert.equal(new Set(Object.values(canaries)).size, 4, 'scope canaries must be distinct');
  return { seed, manifest: {
    fixtureVersion: 2, runtime, projectA: a, projectB: b, daily: DAILY,
    canaries,
    histories: { projectA: history(['restore-a-user', 'restore-a-reply']),
      projectB: history(['restore-b-user', 'restore-b-reply']),
      daily: history(['restore-daily-user', 'restore-daily-reply']) },
    expected: Object.fromEntries(tables.map((table) => [table, seed[table]])),
  } };
}

async function verifyHistory(manifest) {
  const current = await workspaces();
  assert.deepEqual(current.map((row) => row.id).sort(), [DAILY.id, PROJECT_A.id, PROJECT_B.id].sort());
  for (const [key, item] of [['projectA', manifest.projectA], ['projectB', manifest.projectB], ['daily', manifest.daily]]) {
    assert.equal(current.find((row) => row.id === item.id)?.activeSessionId, item.sessionId);
    const rows = await messages(item.sessionId);
    assert.deepEqual(logicalMessages(rows), manifest.histories[key], 'active Main history/ancestry changed');
    if (key === 'projectA') {
      const reply = rows.find((row) => row.id === 'restore-a-reply');
      assert.equal(reply.taskRun?.id, reply.id);
      assert.equal(reply.taskRun?.status, 'completed');
      assert.equal(reply.taskRun?.resumable, false);
      assert.deepEqual((reply.toolResults ?? []).map((row) => [row.callId, row.success, row.output]),
        [['restore-a-read', true, RECEIPT]], 'restored receipt was lost or replayed');
    }
  }
  return current;
}

async function readProbe(command, args) {
  return browser.tauri.execute(async ({ core }, command, args) => {
    try { return { ok: true, value: await core.invoke(command, args) }; }
    catch (error) { return { ok: false, error: String(error) }; }
  }, command, args);
}

async function assertPending(item) {
  for (const [command, args] of [['list_work_dir', { sessionId: item.sessionId, path: '' }],
    ['read_session_file', { sessionId: item.sessionId, path: 'scope.txt' }]]) {
    const result = await readProbe(command, args);
    assert.equal(result.ok, false, 'pending project unexpectedly allowed file access');
    assert.match(result.error, /项目目录尚未确认/);
  }
}

async function chooseWorkspace(item) {
  if (await browser.execute(() => document.querySelector('.sidebar')?.getBoundingClientRect().right <= 0)) {
    await browser.$('[aria-label="打开菜单"]').click();
  }
  await browser.$(`//button[contains(@class,"session-item-select")][.//span[text()="${item.name}"]]`).click();
  await browser.waitUntil(async () =>
    await browser.$('.session-item-select.active .session-item-title').getText() === item.name,
  { timeout: 10_000, timeoutMsg: 'restored workspace did not become active' });
  await browser.waitUntil(async () => (await browser.$('.messages').getText()).includes(
    item.id === PROJECT_A.id ? 'restore-a-history-visible' : item.id === PROJECT_B.id ? 'restore-b-history-visible' : 'restore-daily-history-visible'),
  { timeout: 10_000, timeoutMsg: 'restored Main history did not appear' });
}

async function openDataSettings() {
  await browser.$('button[aria-label="设置"]').click();
  const dialog = await browser.$('[role="dialog"][aria-modal="true"]');
  await dialog.waitForDisplayed({ timeout: 10_000 });
  await dialog.$('button=数据').click();
  return dialog;
}

async function openFiles() {
  if (!await browser.$('.right-panel').isExisting()) await browser.$('[aria-label="打开工作台"]').click();
  await browser.$('.right-panel').waitForDisplayed({ timeout: 10_000 });
  const menu = await browser.$('//button[contains(@class,"workbench-menu-item")][.//span[text()="文件"]]');
  if (await menu.isExisting()) await menu.click();
}

async function previewText() {
  await browser.$('.file-preview-content').waitForDisplayed({ timeout: 10_000 });
  return browser.execute(() => document.querySelector('.file-preview-content code')?.textContent);
}

async function verifyMappedScope(manifest) {
  const current = await verifyHistory(manifest);
  const a = current.find((row) => row.id === PROJECT_A.id);
  const b = current.find((row) => row.id === PROJECT_B.id);
  assert.equal(a.directoryConfirmed, true);
  assert.equal(b.directoryConfirmed, false, 'the excluded project was also authorized');
  assert.equal(realpathSync.native(a.rootPath).toLowerCase(), realpathSync.native(manifest.projectA.mappedPath).toLowerCase());
  assert.equal(await invoke('read_session_file', { sessionId: PROJECT_A.sessionId, path: 'scope.txt' }), manifest.canaries.projectA);
  const old = await readProbe('read_session_file', { sessionId: PROJECT_A.sessionId, path: join(manifest.projectA.originalPath, 'scope.txt') });
  assert.equal(old.ok, false, 'the existing old project root remained readable after mapping');
  assert.match(old.error, /path escapes the work directory/);
  await assertPending(manifest.projectB);
}

async function verifyScopeReset(manifest) {
  await browser.$('[aria-label="在工作台中定位 scope.txt"]').click();
  assert.equal(await previewText(), manifest.canaries.projectA);
  await chooseWorkspace(manifest.daily);
  await browser.$('button[aria-label="打开 scope.txt"]').waitForDisplayed({ timeout: 10_000 });
  assert.equal(await browser.$('.file-preview').isExisting(), false, 'a prior locator selected the new scope');
  const panel = await browser.$('.right-panel').getText();
  assert.ok(!panel.includes(manifest.canaries.projectA.trim()), 'old preview content crossed scope');
  assert.ok(!panel.includes(manifest.canaries.daily.trim()), 'same-name Daily file was read without a new selection');
  await browser.$('button[aria-label="打开 scope.txt"]').click();
  assert.equal(await previewText(), manifest.canaries.daily);
  await chooseWorkspace(manifest.projectA);
  await browser.$('button[aria-label="打开 scope.txt"]').waitForDisplayed({ timeout: 10_000 });
  assert.equal(await browser.$('.file-preview').isExisting(), false);
}

describe('native project backup restore', () => {
  let report;
  let reportPath;
  let stagePath;
  afterEach(function () {
    if (!report) return;
    if (this.currentTest.state !== 'passed' || report.status !== 'passed') report.status = 'failed';
    writeFileSync(reportPath, JSON.stringify(report, null, 2));
    if (report.status === 'failed') writeFileSync(stagePath, JSON.stringify({ phase: 'failed', appPhase: report.phase, failedPhase: report.step }));
  });

  it('prepares, explicitly restores, and reopens the same Main history and user-confirmed scope', async () => {
    const phase = process.env.ANGELBOT_DESKTOP_E2E_PROJECT_RESTORE;
    const evidence = process.env.ANGELBOT_DESKTOP_E2E_RESTORE_EVIDENCE_DIR;
    const runtime = process.env.ANGELBOT_DESKTOP_E2E_RESTORE_RUNTIME;
    assert.ok(['source', 'target', 'reopen'].includes(phase));
    assert.ok(evidence && isAbsolute(evidence) && runtime && isAbsolute(runtime));
    mkdirSync(evidence, { recursive: true });
    reportPath = join(evidence, `${phase}.json`);
    stagePath = join(evidence, 'stage.json');
    report = { status: 'running', phase, step: 'startup' };
    const checkpoint = (step, detail = {}) => {
      report.step = step;
      writeFileSync(reportPath, JSON.stringify(report, null, 2));
      writeFileSync(stagePath, JSON.stringify({ phase: step, appPhase: phase, ...detail }, null, 2));
      console.log(`Project restore: ${phase}/${step}`);
    };
    checkpoint('startup'); // Invalidate stale passed evidence before any app action.
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').waitForDisplayed({ timeout: 20_000 });
    const backupPath = join(evidence, 'encrypted-backup.json');
    const manifestPath = join(evidence, 'manifest.json');
    if (phase === 'source') {
      const { seed, manifest } = fixture(runtime);
      assert.equal((await workspaces()).length, 1, 'source profile must be fresh and isolated');
      // These writes seed only the owned source profile, not the target grant.
      await invoke('import_data', { data: JSON.stringify(seed), opts: { mode: 'merge' } });
      await verifyHistory(manifest);
      const encrypted = await invoke('export_encrypted_data', { password: PASSWORD, opts: { scope: 'conversations' } });
      assert.equal(JSON.parse(encrypted).format, 'angelbot.encrypted-backup');
      writeFileSync(backupPath, encrypted);
      writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
      Object.assign(report, { backupPrepared: true, historyVerified: true });
    } else {
      assert.equal(JSON.parse(readFileSync(join(evidence, 'source.json'), 'utf8')).status, 'passed');
      const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
      assert.equal(manifest.runtime, runtime);
      assert.equal(manifest.fixtureVersion, 2);
      assert.ok(existsSync(backupPath));
      if (phase === 'target') {
        assert.equal((await workspaces()).length, 1, 'target profile must be fresh and isolated');
        const dialog = await openDataSettings();
        const section = await dialog.$('//div[contains(concat(" ",@class," ")," settings-section ")][.//h3[text()="数据备份"]]');
        const toggle = await section.$('.settings-section-header');
        if (await toggle.getAttribute('aria-expanded') !== 'true') await toggle.click();
        await section.$('input[type="password"]').setValue(PASSWORD);
        checkpoint('awaiting-backup-picker', { backupPath, instructions: 'Select this synthetic encrypted backup in the native file picker; do not open any ordinary profile files.' });
        await section.$('button*=导入数据').click();
        await browser.$('.sidebar-restore-directories').waitForDisplayed({ timeout: 180_000 });
        const pending = await verifyHistory(manifest);
        assert.equal(pending.find((row) => row.id === PROJECT_A.id).directoryConfirmed, false);
        assert.equal(pending.find((row) => row.id === PROJECT_B.id).directoryConfirmed, false);
        await assertPending(manifest.projectA);
        await assertPending(manifest.projectB);
        report.pendingDenied = true;
        await chooseWorkspace(manifest.projectA);
        assert.ok(!(await browser.$('.messages').getText()).includes('restore-a-inactive-branch'));
        await openFiles();
        await browser.$('.workbench-error').waitForDisplayed({ timeout: 10_000 });
        assert.match(await browser.$('.workbench-error').getText(), /项目目录尚未确认/);
        assert.equal(await browser.$('.file-item').isExisting(), false);
        await openDataSettings();
        checkpoint('awaiting-directory-confirmation', { projectA: manifest.projectA, projectB: manifest.projectB,
          instructions: 'Human only: select local-A for Restore Probe A, uncheck Restore Probe B, then personally click the single batch confirmation. Automation must not operate these permission controls.' });
        // Observe the actual domain state only. No permission checkbox, picker,
        // confirmation button, DOM event or grant IPC is driven by this spec.
        await browser.waitUntil(async () => {
          const current = await workspaces();
          return current.some((row) => [PROJECT_A.id, PROJECT_B.id].includes(row.id) && row.directoryConfirmed === true);
        }, { timeout: 180_000, interval: 500, timeoutMsg: 'Human directory confirmation was not completed' });
        await verifyMappedScope(manifest);
        // This panel was opened before the grant: do not refresh/reopen it.
        await browser.$('button[aria-label="打开 scope.txt"]').waitForExist({ timeout: 10_000 });
        const closeSettings = await browser.$('[role="dialog"] button[aria-label="关闭"]');
        if (await closeSettings.isExisting()) await closeSettings.click();
      } else {
        await verifyMappedScope(manifest);
        await chooseWorkspace(manifest.projectA);
        await openFiles();
        await browser.$('button[aria-label="打开 scope.txt"]').waitForDisplayed({ timeout: 10_000 });
        report.pendingDenied = true; // The excluded B remained pending on reopen.
      }
      Object.assign(report, { historyVerified: true, directoriesConfirmed: true, sameMainRefreshed: true,
        mappedRootVerified: true, oldRootDenied: true, excludedProjectDenied: true });
      await verifyScopeReset(manifest);
      report.scopeReset = true;
      await verifyHistory(manifest);
      await browser.saveScreenshot(join(evidence, `${phase}-restored.png`));
    }
    const required = phase === 'source' ? ['backupPrepared', 'historyVerified']
      : ['historyVerified', 'pendingDenied', 'directoriesConfirmed', 'sameMainRefreshed',
        'mappedRootVerified', 'oldRootDenied', 'excludedProjectDenied', 'scopeReset'];
    for (const key of required) assert.equal(report[key], true, `restore check was not completed: ${key}`);
    report.status = 'passed';
    checkpoint('done');
  });
});
