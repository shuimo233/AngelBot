import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { waitForVisible } from '../helpers/visibility.mjs';

// Windows temp directories may use an 8.3 alias; match the backend's canonical root.
const projectRoot = realpathSync.native(mkdtempSync(join(tmpdir(), 'angelbot-e2e-project-')));
writeFileSync(join(projectRoot, 'README.md'), '# Desktop E2E Project\n\nThis file verifies the project workbench preview.\n');
const ownedStopScript = join(projectRoot, 'owned-stop.cjs');
const ownedStopStarted = join(projectRoot, 'owned-stop-started.txt');
const mainStopEvidenceDir = mkdtempSync(join(tmpdir(), 'angelbot-main-stop-evidence-'));
writeFileSync(ownedStopScript, "require('node:fs').writeFileSync(require('node:path').join(__dirname, 'owned-stop-started.txt'), String(process.pid));\nsetInterval(() => {}, 1000);\n");
const notepadPath = join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'notepad.exe');
const mcpFixturePath = fileURLToPath(new URL('../fixtures/mcp-stdio.cjs', import.meta.url));

async function navigatePrimary(label) {
  const menuHidden = await browser.execute(() => document.querySelector('.sidebar')?.getBoundingClientRect().right <= 0);
  if (menuHidden) {
    await browser.$('[aria-label="打开菜单"]').click();
    await browser.waitUntil(() => browser.execute(() => document.querySelector('.sidebar')?.getBoundingClientRect().left >= 0), { timeout: 5_000 });
  }
  await browser.$('.sidebar-primary-nav').$(`button=${label}`).click();
}

// Cross-confirmation evidence belongs to the durable ledger, not a compacted
// model prompt. Parent links order slices; call ordinals order this sequential
// script within a slice. Result-array order carries no chronology.
function assertOwnedDesktopControlJourney(messages, task) {
  const byId = new Map(messages.map((message) => [message.id, message]));
  assert.equal(byId.size, messages.length, 'duplicate durable message identity');
  const users = messages.filter((message) => message.role === 'user' && message.content === task);
  assert.equal(users.length, 1, 'owned control task must have one user origin');
  const descendsFrom = (message, ancestorId) => {
    const seen = new Set();
    while (message?.parentId) {
      assert.ok(!seen.has(message.id), 'cycle in durable task ancestry');
      seen.add(message.id);
      if (message.parentId === ancestorId) return true;
      message = byId.get(message.parentId);
      assert.ok(message, 'missing durable task ancestor');
    }
    return false;
  };
  const taskMessages = messages.filter((message) => descendsFrom(message, users[0].id));
  for (const message of taskMessages.filter((item) => item.toolCalls?.some((call) => call.id.startsWith('cu-')))) {
    assert.ok(message.taskRun?.goal === task, 'continuation must retain the user goal, not nest its own prompt');
    assert.ok(message.taskFacts?.goal === task, 'continuation facts must retain the same user goal');
  }
  const calls = taskMessages.flatMap((message) => (message.toolCalls ?? [])
    .map((call, ordinal) => ({ call, message, ordinal })));
  const results = taskMessages.flatMap((message) => message.toolResults ?? []);
  const phases = ['before', 'after-check', 'after-uncheck'];
  const actionIds = ['cu-check', 'cu-uncheck'];
  const expectedIds = phases.flatMap((phase, index) => [
    `cu-root-${phase}`, `cu-region-${phase}`, ...(index < 2 ? [actionIds[index]] : []),
  ]);
  const isCompaction = (id, name) => /^context-compaction-\d+$/.test(id) && name === 'context_compaction';
  assert.deepEqual(calls.filter(({ call }) => !isCompaction(call.id, call.name))
    .map(({ call }) => call.id).sort(), [...expectedIds].sort(),
  'owned task must contain exactly the expected desktop calls, apart from compression metadata');
  assert.deepEqual(results.filter((result) => !isCompaction(result.callId, result.toolName))
    .map((result) => result.callId).sort(), [...expectedIds].sort(),
  'owned task must contain exactly the paired results, apart from compression metadata');
  const pair = (id, name) => {
    const matchingCalls = calls.filter(({ call }) => call.id === id);
    const matchingResults = results.filter((result) => result.callId === id);
    assert.equal(matchingCalls.length, 1, 'desktop call must be unique');
    assert.equal(matchingResults.length, 1, 'desktop result must pair uniquely');
    const entry = matchingCalls[0];
    const result = matchingResults[0];
    assert.equal(entry.message.role, 'assistant');
    assert.equal(entry.call.name, name);
    assert.equal(result.toolName, name);
    assert.equal(result.success, true, 'desktop result must succeed');
    assert.ok(!result.confirmationRequired, 'desktop result must be settled');
    try {
      return { ...entry, result, args: JSON.parse(entry.call.arguments),
        output: JSON.parse(result.output.replace(/^Result: /, '').replace(/^Confirmation approved: /, '')) };
    } catch {
      assert.fail('desktop evidence must contain structured arguments and output');
    }
  };
  const ordered = (left, right) => left.message.id === right.message.id
    ? left.ordinal < right.ordinal : descendsFrom(right.message, left.message.id);
  const validRef = (reference) => typeof reference === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(reference);
  const exactArgs = (args, expected) => Object.keys(args).length === Object.keys(expected).length
    && Object.entries(expected).every(([key, value]) => args[key] === value);
  const appId = 'desktop-e2e-window';
  const regionRefs = new Set();
  const controlRefs = new Set();
  let previousAction;
  for (const [index, phase] of phases.entries()) {
    const root = pair(`cu-root-${phase}`, 'observe_trusted_app_window');
    const scoped = pair(`cu-region-${phase}`, 'observe_trusted_app_window');
    assert.ok(exactArgs(root.args, { app_id: appId, mode: 'controls' }), 'root observation scope mismatch');
    assert.equal(root.output.appId, appId);
    assert.ok(!root.output.regionRef, 'root observation must not be scoped');
    const regions = (root.output.regions ?? []).filter((region) => region.name === 'Fixture region A');
    assert.equal(regions.length, 1, 'root must identify one exact region');
    assert.equal(regions[0].role, 'group');
    const regionRef = regions[0].regionRef;
    assert.ok(validRef(regionRef) && !regionRefs.has(regionRef), 'region reference must be fresh');
    regionRefs.add(regionRef);
    assert.ok(exactArgs(scoped.args, { app_id: appId, mode: 'controls', region_ref: regionRef }),
      'scoped observation must use the exact root reference');
    assert.equal(scoped.output.appId, appId);
    assert.ok(scoped.output.regionRef === regionRef, 'scoped result reference mismatch');
    assert.equal(scoped.output.truncated, false);
    assert.equal((scoped.output.diagnostics ?? []).length, 0);
    const controls = (scoped.output.controls ?? []).filter((control) => control.name === 'Region checkbox');
    assert.equal(controls.length, 1, 'scope must identify one exact checkbox');
    assert.equal(controls[0].role, 'checkBox');
    assert.equal(controls[0].enabled, true);
    assert.ok(controls[0].capabilities.includes('toggle'));
    assert.deepEqual(controls[0].state, { toggle: index === 1 ? 'on' : 'off' },
      'fresh observation must independently read the expected checkbox state');
    const controlRef = controls[0].controlRef;
    assert.ok(validRef(controlRef) && !controlRefs.has(controlRef), 'control reference must be fresh');
    controlRefs.add(controlRef);
    assert.ok(ordered(root, scoped), 'root must precede scoped observation');
    if (previousAction) assert.ok(ordered(previousAction, root), 'new observation must follow the previous effect');
    if (index < 2) {
      const operation = ['check', 'uncheck'][index];
      const action = pair(`cu-${operation}`, 'operate_trusted_app_control');
      assert.ok(exactArgs(action.args, { app_id: appId, control_ref: controlRef, action: operation }),
        'effect must target the exact current observed control');
      assert.equal(action.result.confirmationStatus, 'approved');
      assert.match(action.result.output.replace(/^Result: /, ''), /^Confirmation approved: /);
      assert.equal(action.output.adapter, 'windows-native-uia');
      assert.equal(action.output.status, 'verified');
      assert.equal(action.output.target, appId);
      assert.equal(action.output.action, operation);
      assert.equal(typeof action.output.detail, 'string');
      assert.ok(ordered(scoped, action), 'scoped observation must precede its effect');
      previousAction = action;
    }
  }
}

after(async () => {
  // A failed assertion may leave the owned script using this directory. Stop
  // that exact run before removing its files, rather than racing app teardown.
  const active = await browser.tauri.execute(async ({ core }) => {
    const id = window.__angelbotE2eOwnedStopId;
    if (!id) return false;
    const runs = await core.invoke('get_automation_runs', { automationId: id });
    const run = runs.find(item => item.status === 'running' && item.stopControl);
    if (!run) return false;
    await core.invoke('stop_automation_run', { id, runId: run.id });
    return true;
  });
  if (active) await browser.waitUntil(() => browser.tauri.execute(() => window.__angelbotE2eOwnedStopResult === 'settled'), { timeout: 10_000 });
  rmSync(projectRoot, { recursive: true, force: true });
});

describe('first-run desktop experience', () => {
  it('guides the first task without blocking or sending on the user\'s behalf', async () => {
    const location = await browser.tauri.execute(() => window.location.href);
    assert.match(location, /^https?:\/\/tauri\.localhost\//);
    const screenshotDir = join(process.cwd(), 'src-tauri', 'target', 'e2e');
    mkdirSync(screenshotDir, { recursive: true });
    console.log(`Main stop evidence: ${mainStopEvidenceDir}`);

    const initialWorkspaces = await browser.tauri.execute(({ core }) => core.invoke('get_workspaces'));
    assert.deepEqual(initialWorkspaces.map((workspace) => workspace.kind), ['personal']);
    assert.equal(initialWorkspaces[0].rootPath, undefined);
    await browser.execute((sessionId) => {
      window.__angelbotE2ePersonalSessionId = sessionId;
    }, initialWorkspaces[0].activeSessionId);
    await browser.execute((id) => { window.__angelbotE2ePersonalWorkspaceId = id; }, initialWorkspaces[0].id);

    const runtimeHealth = await browser.tauri.execute(({ core }) => core.invoke('get_runtime_health'));
    assert.equal(runtimeHealth.status, 'ready');
    assert.equal(runtimeHealth.delegationAvailable, true);
    assert.deepEqual(runtimeHealth.issues, []);

    await browser.tauri.execute(({ core }) => core.invoke('set_agent_execution_permission', {
      permission: 'full_access',
    }));

    const composer = await browser.$('textarea[placeholder="给 AngelBot 发消息…"]');
    await composer.waitForDisplayed({ timeout: 20_000 });

    const personalStarter = await browser.$('//button[contains(., "整理今天的安排")]');
    await personalStarter.waitForDisplayed({ timeout: 20_000 });
    await personalStarter.click();
    assert.equal(
      await composer.getValue(),
      '帮我把今天要处理的事情整理成按优先级排序的行动清单。先问我缺少的关键信息。',
    );
    assert.equal((await browser.$$('.message-row.user')).length, 0, 'starter must only prepare a draft');
    await browser.$('//button[contains(., "整理今天的安排")]').waitForExist({ reverse: true, timeout: 5_000 });
    await composer.clearValue();
    await browser.$('//button[contains(., "整理今天的安排")]').waitForDisplayed({ timeout: 5_000 });

    const sendButton = await browser.$('button[title="发送"]');
    // Real Tauri IPC extraction, not a browser/model mock. A rejected batch
    // must leave no chip, draft snapshot or phantom user turn.
    for (const [name, encoded, expected] of [
      ['broken.docx', btoa('broken zip'), 'DOCX'],
      ['broken.xlsx', btoa('broken zip'), 'XLSX'],
      ['formula.xlsx', process.env.ANGELBOT_DESKTOP_E2E_FORMULA_XLSX_BASE64, '公式'],
    ]) {
      assert.ok(encoded, 'owned invalid document fixture is required');
      await browser.execute((name, encoded) => {
        const input = document.querySelector('[aria-label="选择文本附件"]');
        const transfer = new DataTransfer();
        transfer.items.add(new File([Uint8Array.from(atob(encoded), (char) => char.charCodeAt(0))], name));
        input.files = transfer.files;
        input.dispatchEvent(new Event('change', { bubbles: true }));
      }, name, encoded);
      await browser.$(`//*[contains(@class,"composer-attachment-error") and contains(.,"${expected}")]`).waitForDisplayed({ timeout: 5_000 });
      assert.equal((await browser.$$('.composer-file-chip')).length, 0);
      assert.equal((await browser.$$('.message-row.user')).length, 0);
      assert.equal(await sendButton.isEnabled(), false);
    }
    assert.ok(process.env.ANGELBOT_DESKTOP_E2E_DOCX_BASE64, 'owned DOCX fixture is required');
    assert.ok(process.env.ANGELBOT_DESKTOP_E2E_XLSX_BASE64 && process.env.ANGELBOT_DESKTOP_E2E_UTF16_BASE64, 'owned XLSX/UTF-16 fixtures are required');
    const outsideDrop = await browser.execute(() => {
      const transfer = new DataTransfer();
      transfer.items.add(new File(['not an attachment'], 'ignored.txt'));
      const target = document.querySelector('.sidebar');
      const over = new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: transfer });
      const drop = new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: transfer });
      const location = window.location.href;
      target.dispatchEvent(over);
      target.dispatchEvent(drop);
      return { over: over.defaultPrevented, drop: drop.defaultPrevented, stayed: location === window.location.href };
    });
    assert.deepEqual(outsideDrop, { over: true, drop: true, stayed: true });
    assert.equal((await browser.$$('.composer-file-chip')).length, 0, 'outside drops must never read files');
    assert.equal((await browser.$$('.message-row.user')).length, 0);
    const dropAccepted = await browser.execute((docx, xlsx, utf16) => {
      const target = document.querySelector('.composer-input-root');
      const transfer = new DataTransfer();
      for (const [name, encoded] of [['今日安排.docx', docx], ['本地事项.xlsx', xlsx], ['Windows.log', utf16]]) {
        const data = Uint8Array.from(atob(encoded), (char) => char.charCodeAt(0));
        transfer.items.add(new File([data], name));
      }
      const over = new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: transfer });
      const drop = new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: transfer });
      target.dispatchEvent(over);
      target.dispatchEvent(drop);
      return { over: over.defaultPrevented, drop: drop.defaultPrevented };
    }, process.env.ANGELBOT_DESKTOP_E2E_DOCX_BASE64, process.env.ANGELBOT_DESKTOP_E2E_XLSX_BASE64, process.env.ANGELBOT_DESKTOP_E2E_UTF16_BASE64);
    assert.deepEqual(dropAccepted, { over: true, drop: true });
    await browser.$('.composer-file-chip').waitForDisplayed({ timeout: 5_000 });
    assert.match(await browser.$('.composer-attachment-note').getText(), /DOCX 只读取正文和表格文字/);
    assert.match(await browser.$('.composer-attachment-note').getText(), /XLSX 读取保存的原始单元格值/);
    assert.equal((await browser.$$('.composer-file-chip')).length, 3);
    assert.equal((await browser.$$('.composer-attachment-error')).length, 0);
    await browser.saveScreenshot(join(screenshotDir, 'docx-draft.png'));
    const draftSheet = await browser.$('.composer-file-chip:nth-child(2) details');
    await draftSheet.$('summary').click();
    await draftSheet.$('pre').waitForDisplayed({ timeout: 5_000 });
    assert.equal(await browser.execute(() => document.activeElement.tagName), 'SUMMARY');
    await draftSheet.$('summary').click();
    await browser.waitUntil(async () => !(await draftSheet.$('pre').isDisplayed()), { timeout: 5_000 });
    await draftSheet.$('summary').click();
    await draftSheet.$('pre').waitForDisplayed({ timeout: 5_000 });
    const draftSheetText = await browser.execute(() => document.querySelector('.composer-file-chip:nth-child(2) pre').textContent);
    assert.match(draftSheetText, /A1 = "native-xlsx-marker"/);
    assert.match(draftSheetText, /数字未按日期、百分比或货币格式换算/);
    assert.equal((await browser.$$('.message-row.user')).length, 0, 'preview must not send a turn');
    const previewPreferredDark = await browser.execute(() => {
      const wasDark = document.documentElement.classList.contains('dark');
      document.documentElement.classList.remove('dark');
      return wasDark;
    });
    await browser.pause(200);
    await browser.saveScreenshot(join(screenshotDir, 'attachment-preview.png'));
    await browser.execute(() => document.documentElement.classList.add('dark'));
    await browser.pause(200);
    await browser.saveScreenshot(join(screenshotDir, 'attachment-preview-dark.png'));
    const previousViewport = await browser.execute(() => ({ width: innerWidth, height: innerHeight }));
    await browser.setWindowSize(1000, 540);
    let compactDraft;
    await browser.waitUntil(async () => {
      compactDraft = await browser.execute((previous) => {
        const send = document.querySelector('button[title="发送"]').getBoundingClientRect();
        const input = document.querySelector('.composer-bar textarea').getBoundingClientRect();
        const strip = document.querySelector('.composer-files-strip').getBoundingClientRect();
        return { resized: innerWidth < previous.width && innerHeight < previous.height,
          compact: innerWidth <= 1000 && innerHeight <= 540,
          usable: [send, input].every((rect) => rect.top >= 0 && rect.bottom <= innerHeight && rect.height > 0),
          bounded: strip.height <= innerHeight * 0.3 + 5 };
      }, previousViewport);
      return compactDraft.resized && compactDraft.compact && compactDraft.usable && compactDraft.bounded;
    }, {
      timeout: 5_000,
      timeoutMsg: 'compact attachment layout did not settle after the native window resize',
    }).catch(async (error) => {
      const geometry = await browser.execute(() => ({
        width: innerWidth, height: innerHeight, scale: devicePixelRatio,
        send: document.querySelector('button[title="发送"]')?.getBoundingClientRect().toJSON(),
        input: document.querySelector('.composer-bar textarea')?.getBoundingClientRect().toJSON(),
        strip: document.querySelector('.composer-files-strip')?.getBoundingClientRect().toJSON(),
      })).catch(() => ({ diagnosticUnavailable: true }));
      console.error('Compact attachment resize layout:', JSON.stringify(geometry));
      throw error;
    });
    assert.deepEqual(compactDraft, { resized: true, compact: true, usable: true, bounded: true });
    await browser.saveScreenshot(join(screenshotDir, 'attachment-preview-compact.png'));
    await browser.setWindowSize(1200, 800);
    await browser.execute((restoreDark) => document.documentElement.classList.toggle('dark', restoreDark), previewPreferredDark);
    await draftSheet.$('summary').click();
    await composer.setValue('帮我设置一个本地提醒，稍后核对今天的安排。');
    await sendButton.waitForEnabled({ timeout: 5_000 });
    await sendButton.click();
    await waitForVisible(browser, '//*[contains(text(), "本地提醒已设置，你可以在当前对话中回看")]', { timeout: 20_000 });
    const attachment = await browser.$('.message-text-attachments details');
    await attachment.$('summary').click();
    assert.match(await attachment.getText(), /DOCX 正文文字/);
    assert.match(await attachment.$('pre').getText(), /今天核对安排 & 本地提醒/);
    assert.match(await attachment.$('pre').getText(), /事项\s+核对今天的安排/);
    assert.doesNotMatch(await attachment.$('pre').getText(), /private-header/);
    await browser.saveScreenshot(join(screenshotDir, 'docx-sent.png'));
    const attachmentMessages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
      sessionId: window.__angelbotE2ePersonalSessionId,
    }));
    const savedAttachment = attachmentMessages.find((message) => message.role === 'user').textAttachments;
    assert.deepEqual(attachmentMessages.map((message) => message.role), ['user', 'assistant']);
    assert.equal(savedAttachment.length, 3);
    assert.equal(savedAttachment[0].name, '今日安排.docx');
    assert.match(savedAttachment[0].text, /^【DOCX 正文文字快照】/);
    assert.match(savedAttachment[0].text, /事项\t核对今天的安排/);
    assert.deepEqual(Object.keys(savedAttachment[0]).sort(), ['name', 'text']);
    assert.equal(savedAttachment[1].name, '本地事项.xlsx');
    assert.match(savedAttachment[1].text, /^【XLSX 单元格值快照】/);
    assert.equal(savedAttachment[1].text, draftSheetText, 'preview and durable snapshot must be identical');
    assert.match(savedAttachment[1].text, /A1 = "native-xlsx-marker"\nB1 = "0"\n合并范围 A1:A2/);
    assert.doesNotMatch(savedAttachment[1].text, /private-comment/);
    assert.deepEqual(savedAttachment[2], { name: 'Windows.log', text: 'Windows 日志\r\nnative-utf16-marker' });
    assert.equal((await browser.tauri.execute(({ core }) => core.invoke('get_workspaces')))[0].rootPath, undefined);
    await attachment.$('summary').click();
    const sheetAttachment = (await browser.$$('.message-text-attachments details'))[1];
    await sheetAttachment.$('summary').click();
    assert.match(await sheetAttachment.getText(), /XLSX 单元格值/);
    assert.match(await sheetAttachment.$('pre').getText(), /数字未按日期、百分比或货币格式换算/);
    await browser.saveScreenshot(join(screenshotDir, 'xlsx-sent.png'));
    await sheetAttachment.$('summary').click();
    const reminderRecall = await browser.$('.workspace-reminders');
    await reminderRecall.waitForDisplayed({ timeout: 5_000 });
    await reminderRecall.$('summary').click();
    assert.match(await reminderRecall.getText(), /核对今天的安排/);
    assert.match(await reminderRecall.getText(), /待执行 1 · 今日已触发 0/);
    const reminder = (await browser.tauri.execute(({ core }) => core.invoke('get_automations')))
      .find((item) => item.executorKind === 'notification');
    assert.ok(reminder);
    assert.equal(reminder.workspaceId, initialWorkspaces[0].id);
    await browser.execute((id) => { window.__angelbotE2eReminderId = id; }, reminder.id);

    // Exercise the persisted local receipt without waiting an hour or sending
    // an OS notification. Returning focus must show the fresh durable state.
    const notificationSettings = await browser.tauri.execute(({ core }) => core.invoke('get_notification_settings'));
    await browser.tauri.execute(async ({ core }) => {
      const settings = await core.invoke('get_notification_settings');
      await core.invoke('save_notification_settings', { settings: { ...settings, reminder: false } });
      await core.invoke('run_automation_now', { id: window.__angelbotE2eReminderId });
    });
    await browser.execute(() => window.dispatchEvent(new Event('focus')));
    await browser.waitUntil(async () => /待执行 0 · 今日已触发 1/.test(await reminderRecall.getText()), {
      timeout: 5_000, timeoutMsg: 'local reminder receipt did not return to the existing Main conversation',
    });
    assert.match(await reminderRecall.getText(), /不代表任务已完成或系统通知已送达/);
    const reminderRuns = await browser.tauri.execute(({ core }) => core.invoke('get_automation_runs', {
      automationId: window.__angelbotE2eReminderId,
    }));
    assert.equal(reminderRuns.length, 1);
    // Muting consumes the occurrence without claiming notification delivery.
    assert.equal(reminderRuns[0].status, 'skipped');
    assert.equal(reminderRuns[0].summary, '通知已关闭，本次提醒未发送；不会自动补发');
    await browser.execute((settings) => { window.__angelbotE2eNotificationSettings = settings; }, notificationSettings);
    await browser.tauri.execute(({ core }) => core.invoke('save_notification_settings', {
      settings: window.__angelbotE2eNotificationSettings,
    }));
    await browser.saveScreenshot(join(screenshotDir, 'daily-reminder-recall.png'));
    // An expanded recall must leave a usable conversation and composer on
    // small laptops, even when both sections contain several items.
    await browser.tauri.execute(async ({ core }) => {
      window.__angelbotE2eLayoutReminderIds = [];
      for (let index = 0; index < 6; index++) {
        const item = await core.invoke('create_automation', {
          title: `布局检查提醒 ${index + 1}`, prompt: 'Only an isolated layout fixture.',
          triggerKind: 'once', triggerValue: new Date(Date.now() + 3_600_000).toISOString(),
          permissionSummary: 'Local fixture', executorKind: 'notification',
          workspaceId: window.__angelbotE2ePersonalWorkspaceId,
        });
        window.__angelbotE2eLayoutReminderIds.push(item.id);
        if (index < 3) await core.invoke('run_automation_now', { id: item.id });
      }
    });
    await browser.execute(() => window.dispatchEvent(new Event('focus')));
    await browser.waitUntil(async () => /待执行 3 · 今日已触发 4/.test(await reminderRecall.getText()), { timeout: 5_000 });
    await browser.setWindowSize(1200, 800);
    const reminderLayout = await browser.execute(() => ({
      messagesHeight: document.querySelector('.messages')?.getBoundingClientRect().height,
      composerBottom: document.querySelector('.composer')?.getBoundingClientRect().bottom,
      viewportHeight: window.innerHeight,
    }));
    assert.ok(reminderLayout.messagesHeight >= 120, `expanded recall must leave reading space: ${JSON.stringify(reminderLayout)}`);
    assert.ok(reminderLayout.composerBottom <= reminderLayout.viewportHeight + 1, `composer is clipped: ${JSON.stringify(reminderLayout)}`);
    await browser.saveScreenshot(join(screenshotDir, 'compact-reminder-recall.png'));
    await browser.setWindowSize(1200, 800);
    await browser.tauri.execute(async ({ core }) => {
      for (const id of window.__angelbotE2eLayoutReminderIds) await core.invoke('delete_automation', { id });
    });
    await browser.execute(() => window.dispatchEvent(new Event('focus')));

    await composer.setValue('每天上午九点检查待处理邮件，整理成行动清单。只整理和起草，不要发送邮件。');
    // Streaming replaces Send with Stop; the first turn's Send handle no
    // longer represents the enabled control after the completed reply.
    const automationSendButton = await browser.$('button[title="发送"]');
    await automationSendButton.waitForEnabled({ timeout: 5_000 });
    await automationSendButton.click();
    const assistantReply = await browser.$('//*[contains(text(), "自动化已创建，并已绑定到当前工作区")]');
    await assistantReply.waitForDisplayed({ timeout: 20_000 });

    const automations = (await browser.tauri.execute(({ core }) => core.invoke('get_automations')))
      .filter((item) => item.executorKind === 'agent');
    assert.equal(automations.length, 1);
    assert.equal(automations[0].title, '每日邮件行动清单');
    assert.equal(automations[0].executorKind, 'agent');
    assert.equal(automations[0].workspaceId, initialWorkspaces[0].id);
    assert.equal(automations[0].triggerValue, '每天 09:00');
    assert.match(automations[0].prompt, /不要发送邮件/);

    // Request-mode side effects must stay visible and rejectable in the main
    // conversation. Reject the prompt so the E2E run never opens host UI.
    await browser.tauri.execute(({ core }) => core.invoke('set_agent_execution_permission', {
      permission: 'ask',
    }));
    await composer.setValue('打开 Windows 声音设置。');
    const confirmationSendButton = await browser.$('button[title="发送"]');
    await confirmationSendButton.waitForEnabled({ timeout: 5_000 });
    await confirmationSendButton.click();
    await browser.pause(1_000);
    const confirmationTimeline = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
      sessionId: window.__angelbotE2ePersonalSessionId,
    }));
    const confirmationMessage = confirmationTimeline.findLast((message) =>
      message.toolCalls?.some((call) => call.name === 'open_windows_setting'));
    assert.ok(
      confirmationMessage,
      `the second foreground turn must consume the Windows-settings script: ${JSON.stringify(confirmationTimeline)}`,
    );
    assert.equal(
      confirmationMessage.taskFacts?.pending_confirmation?.tool_name,
      'open_windows_setting',
      `request mode must keep the settings action pending: ${JSON.stringify(confirmationMessage)}`,
    );
    const confirmationSummary = await browser.$('//*[contains(text(), "打开 Windows 设置：声音")]');
    await confirmationSummary.waitForDisplayed({ timeout: 20_000 });
    const allowOnceButton = await browser.$('button=允许一次');
    await allowOnceButton.waitForDisplayed({ timeout: 5_000 });
    const visibleRejectButtons = await browser.$$('button=拒绝');
    assert.equal(visibleRejectButtons.length, 1, 'one pending action must expose exactly one rejection control');
    // A new user intent is a redirect, never an implicit approval of an older
    // action. The old system-settings request must become terminal before the
    // new foreground turn completes; this test deliberately never invokes the
    // approval IPC, so the desktop shell cannot open a host application.
    await composer.setValue('先不要打开 Windows 设置，改为告诉我如何调整声音。');
    const redirectSendButton = await browser.$('button[title="发送"]');
    await redirectSendButton.waitForEnabled({ timeout: 5_000 });
    await redirectSendButton.click();
    const redirectReply = await browser.$('//*[contains(text(), "已取消打开 Windows 声音设置，并先说明如何调整声音")]');
    await redirectReply.waitForDisplayed({ timeout: 20_000 });

    let redirectedTimeline;
    await browser.waitUntil(async () => {
      redirectedTimeline = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2ePersonalSessionId,
      }));
      const originalAction = redirectedTimeline.find((message) => message.id === confirmationMessage.id);
      const originalResult = originalAction?.toolResults?.find((result) => result.toolName === 'open_windows_setting');
      return originalAction?.taskRun?.confirmationState !== 'pending'
        && originalAction?.taskRun?.resumable === false
        && originalAction?.taskFacts?.pending_confirmation == null
        && originalResult?.confirmationStatus !== 'pending'
        && originalResult?.success === false;
    }, {
      timeout: 10_000,
      interval: 200,
      timeoutMsg: 'a redirected confirmation was not made terminal before the new turn completed',
    });
    const originalAction = redirectedTimeline.find((message) => message.id === confirmationMessage.id);
    const originalResult = originalAction?.toolResults?.find((result) => result.toolName === 'open_windows_setting');
    assert.notEqual(originalAction?.taskRun?.confirmationState, 'pending', JSON.stringify(originalAction));
    assert.equal(originalAction?.taskRun?.resumable, false, JSON.stringify(originalAction));
    assert.equal(originalAction?.taskFacts?.pending_confirmation ?? null, null, JSON.stringify(originalAction));
    assert.notEqual(originalResult?.confirmationStatus, 'pending', JSON.stringify(originalAction));
    assert.equal(originalResult?.success, false, JSON.stringify(originalAction));
    const remainingApprovalButtons = await browser.$$('button=允许一次');
    const remainingRejectButtons = await browser.$$('button=拒绝');
    assert.equal(remainingApprovalButtons.length, 0, 'a redirected action must not remain approvable');
    assert.equal(remainingRejectButtons.length, 0, 'a redirected action must not remain actionable');
    const conversationText = await browser.$('.messages').getText();
    const expectedOrder = [
      '每天上午九点检查待处理邮件',
      '自动化已创建，并已绑定到当前工作区',
      '打开 Windows 声音设置',
      '需要你允许后才会打开 Windows 声音设置',
      '先不要打开 Windows 设置，改为告诉我如何调整声音',
      '已取消打开 Windows 声音设置，并先说明如何调整声音',
    ];
    const positions = expectedOrder.map((text) => conversationText.indexOf(text));
    assert.ok(
      positions.every((position) => position >= 0)
        && positions.every((position, index) => index === 0 || positions[index - 1] < position),
      `rapid turns must stay in chronological order: ${JSON.stringify({ positions, conversationText })}`,
    );
    assert.doesNotMatch(conversationText, /0\/1 已完成/);
    assert.doesNotMatch(conversationText, /动作已完成/);

    await browser.execute((id) => {
      window.__angelbotE2eAutomationId = id;
    }, automations[0].id);
    const beforeAutomationTimeline = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
      sessionId: window.__angelbotE2ePersonalSessionId,
    }));
    await browser.tauri.execute(({ core }) => core.invoke('run_automation_now', {
      id: window.__angelbotE2eAutomationId,
    }));
    let completedAutomation;
    await browser.waitUntil(async () => {
      const [runs, messages] = await Promise.all([
        browser.tauri.execute(({ core }) => core.invoke('get_automation_runs', {
          automationId: window.__angelbotE2eAutomationId,
        })),
        browser.tauri.execute(({ core }) => core.invoke('get_messages', {
          sessionId: window.__angelbotE2ePersonalSessionId,
        })),
      ]);
      completedAutomation = { runs, messages };
      return completedAutomation.runs.length === 1
        && completedAutomation.runs[0].status === 'completed'
        && completedAutomation.messages.some((message) =>
          message.role === 'assistant'
          && message.content.includes('自动化已执行：已整理今日待处理邮件，未发送任何邮件。'));
    }, {
      timeout: 20_000,
      interval: 200,
      timeoutMsg: 'agent automation did not complete through the owning Main-Agent session',
    });
    assert.equal(completedAutomation.runs.length, 1);
    assert.equal(completedAutomation.runs[0].status, 'completed');
    const automationOverview = await browser.tauri.execute(({ core }) =>
      core.invoke('get_workspace_automation_overview', {
        workspaceId: window.__angelbotE2ePersonalWorkspaceId,
      }));
    const recalledAutomation = automationOverview.items.find((item) => item.id === automations[0].id);
    assert.ok(recalledAutomation, 'completed automation must remain visible in the owning workspace');
    assert.equal(recalledAutomation.latestRun.id, completedAutomation.runs[0].id);
    assert.equal(recalledAutomation.latestRun.status, 'completed');
    assert.equal(automationOverview.truncated, false);
    for (const field of ['prompt', 'scriptPath', 'scriptArgs', 'workingDir']) {
      assert.equal(field in recalledAutomation, false, `recall must not expose ${field}`);
    }
    assert.equal('output' in recalledAutomation.latestRun, false, 'recall must not expose raw process output');
    await browser.tauri.execute(() => window.dispatchEvent(new Event('focus')));
    await browser.waitUntil(async () => /主 Agent 已结束/.test(await reminderRecall.getText()), {
      timeout: 5_000, timeoutMsg: 'automation result did not reach the existing Main conversation recall',
    });
    await browser.saveScreenshot(join(screenshotDir, 'daily-automation-result.png'));
    assert.equal(
      completedAutomation.messages.length,
      beforeAutomationTimeline.length + 1,
      'automation must add one Main-Agent reply without persisting a synthetic user message',
    );
    assert.equal(
      completedAutomation.messages.filter((message) =>
        message.content.includes('自动化已执行：已整理今日待处理邮件，未发送任何邮件。')).length,
      1,
      'automation must not replay its Main-Agent turn',
    );

    // A daily definition remains scheduled after one completed occurrence.
    // Viewing that result returns to its existing Main conversation, not a rerun.
    await navigatePrimary('自动化');
    const dailyAutomationCard = await browser.$('//article[contains(@class,"automation-card")][.//h3[text()="每日邮件行动清单"]]');
    await dailyAutomationCard.waitForDisplayed({ timeout: 10_000 });
    assert.match(await dailyAutomationCard.$('.automation-status').getText(), /已完成/);
    assert.match(await dailyAutomationCard.getText(), /下次执行/);
    assert.doesNotMatch(await dailyAutomationCard.getText(), /已暂停/);
    await dailyAutomationCard.$('button*=查看结果').click();
    await browser.$('.chat-area').waitForDisplayed({ timeout: 10_000 });
    await browser.waitUntil(async () =>
      (await browser.$('.messages').getText()).includes('自动化已执行：已整理今日待处理邮件，未发送任何邮件。'), {
      timeout: 10_000, timeoutMsg: 'the daily result link did not return to its owning Main conversation',
    });
    const afterResultOpen = await browser.tauri.execute(async ({ core }) => ({
      runs: await core.invoke('get_automation_runs', { automationId: window.__angelbotE2eAutomationId }),
      messages: await core.invoke('get_messages', { sessionId: window.__angelbotE2ePersonalSessionId }),
      definition: (await core.invoke('get_automations')).find((item) => item.id === window.__angelbotE2eAutomationId),
    }));
    assert.deepEqual(afterResultOpen.runs, completedAutomation.runs, 'viewing a result must not start another occurrence');
    assert.equal(afterResultOpen.messages.length, completedAutomation.messages.length, 'viewing a result must not add a new turn');
    assert.equal(afterResultOpen.definition.enabled, true, 'a completed daily occurrence must not disable its definition');
    await browser.saveScreenshot(join(screenshotDir, 'daily-automation-result-link.png'));
    assert.equal(
      await browser.$('body').getText().then((text) => text.includes('AngelBot 日常工作区不具有项目文件访问权限。')),
      false,
      'personal chat must not require a project filesystem root',
    );

    await browser.execute((path) => {
      window.__angelbotE2eProjectRoot = path;
    }, projectRoot);
    const createdProject = await browser.tauri.execute(({ core }) => core.invoke('create_project_workspace', {
      path: window.__angelbotE2eProjectRoot,
      name: 'Desktop E2E Project',
      agent_provider: null,
      agent_model: null,
    }));
    assert.equal(createdProject.workspace.kind, 'project');
    await browser.execute((id) => { window.__angelbotE2eOverviewWorkspaceId = id; }, createdProject.workspace.id);
    const projectAutomationOverview = await browser.tauri.execute(({ core }) =>
      core.invoke('get_workspace_automation_overview', { workspaceId: window.__angelbotE2eOverviewWorkspaceId }));
    assert.deepEqual(projectAutomationOverview.items, [], 'personal automation results must not cross into a new project');
    assert.equal(createdProject.workspace.activeSessionId, createdProject.session.id);
    await browser.execute((id) => {
      window.__angelbotE2eProjectSessionId = id;
    }, createdProject.session.id);

    const workspaces = await browser.tauri.execute(({ core }) => core.invoke('get_workspaces'));
    assert.deepEqual(workspaces.map((workspace) => workspace.kind), ['personal', 'project']);
    assert.equal(workspaces[1].activeSessionId, createdProject.session.id);
    assert.equal(workspaces[1].rootPath, projectRoot);
    const sessions = await browser.tauri.execute(({ core }) => core.invoke('get_sessions'));
    assert.equal(sessions.length, 2, 'a fresh personal/project pair exposes one session per workspace');
    await browser.execute((id) => {
      window.__angelbotE2eWorkspaceId = id;
    }, createdProject.workspace.id);
    const reopenedProject = await browser.tauri.execute(({ core }) => core.invoke('open_workspace', {
      id: window.__angelbotE2eWorkspaceId,
    }));
    assert.equal(reopenedProject.session.id, createdProject.session.id);
    const seededNetworkApprovals = await browser.tauri.execute(({ core }) => core.invoke('get_project_network_approvals', {
      workspaceId: window.__angelbotE2eWorkspaceId,
    }));
    assert.equal(seededNetworkApprovals.approvals.length, 1);
    assert.deepEqual(seededNetworkApprovals.approvals[0].hosts, ['docs.example.test']);

    const modelNotice = await browser.$('.model-config-notice');
    await modelNotice.waitForDisplayed({ timeout: 20_000 });
    assert.match(await modelNotice.getText(), /连接模型后即可开始对话/);

    const preferredDark = await browser.execute(() => document.documentElement.classList.contains('dark'));
    await browser.execute(() => document.documentElement.classList.remove('dark'));
    await browser.pause(200);
    await browser.saveScreenshot(join(screenshotDir, 'first-run.png'));
    await browser.saveScreenshot(join(screenshotDir, 'daily-workspace.png'));
    await browser.execute(() => document.documentElement.classList.add('dark'));
    await browser.pause(200);
    await browser.saveScreenshot(join(screenshotDir, 'first-run-dark.png'));
    await browser.execute((restoreDark) => document.documentElement.classList.toggle('dark', restoreDark), preferredDark);

    await modelNotice.$('button=模型设置').click();
    const settingsDialog = await browser.$('[role="dialog"][aria-modal="true"]');
    await settingsDialog.waitForDisplayed({ timeout: 5_000 });
    await browser.waitUntil(
      async () => browser.execute(() => document.activeElement?.getAttribute('aria-label') === '关闭'),
      { timeout: 5_000, timeoutMsg: 'settings dialog did not receive keyboard focus' },
    );
    const settingsNavigationLayout = await browser.execute(() => {
      const item = document.querySelector('.settings-nav-item');
      const itemLabel = document.querySelector('.settings-nav-label');
      const itemIcon = document.querySelector('.settings-nav-icon');
      const groupLabel = document.querySelector('.settings-nav-group-label');
      if (!(item instanceof HTMLElement) || !(itemLabel instanceof HTMLElement) || !(itemIcon instanceof HTMLElement) || !(groupLabel instanceof HTMLElement)) return null;
      const groupStyle = getComputedStyle(groupLabel);
      return {
        justifyContent: getComputedStyle(item).justifyContent,
        itemLabelLeft: Math.round(itemLabel.getBoundingClientRect().left),
        itemIconLeft: Math.round(itemIcon.getBoundingClientRect().left),
        expectedLabelLeft: Math.round(itemIcon.getBoundingClientRect().right + Number.parseFloat(getComputedStyle(item).columnGap)),
        groupTextLeft: Math.round(groupLabel.getBoundingClientRect().left + Number.parseFloat(groupStyle.paddingLeft)),
        itemFontSize: getComputedStyle(itemLabel).fontSize,
        groupFontSize: groupStyle.fontSize,
      };
    });
    assert.deepEqual(settingsNavigationLayout, {
      justifyContent: 'flex-start',
      // Group headings align with the icon column. Labels follow the shared
      // icon+gap, not the old icon-free navigation's heading coordinate.
      itemLabelLeft: settingsNavigationLayout?.expectedLabelLeft,
      itemIconLeft: settingsNavigationLayout?.groupTextLeft,
      expectedLabelLeft: settingsNavigationLayout?.expectedLabelLeft,
      groupTextLeft: settingsNavigationLayout?.groupTextLeft,
      itemFontSize: '13px',
      groupFontSize: '12px',
    });
    assert.equal(await settingsDialog.$('.settings-page-title').getText(), '模型与 API');
    assert.equal(await settingsDialog.$('button=模型 API').getAttribute('aria-current'), 'page');
    assert.match(await settingsDialog.getText(), /服务商与模型/);
    assert.match(await settingsDialog.getText(), /连接信息/);
    await settingsDialog.$('.model-field select').waitForDisplayed({ timeout: 5_000 });
    await browser.execute(() => document.documentElement.classList.remove('dark'));
    await browser.pause(200);
    await browser.saveScreenshot(join(screenshotDir, 'model-settings.png'));
    await browser.execute(() => document.documentElement.classList.add('dark'));
    await browser.pause(200);
    await browser.saveScreenshot(join(screenshotDir, 'model-settings-dark.png'));
    await browser.execute((restoreDark) => document.documentElement.classList.toggle('dark', restoreDark), preferredDark);

    // The real local stdio child exercises settings, Tauri lifecycle commands,
    // and fresh tool discovery without a network service or model credential.
    assert.ok(existsSync(mcpFixturePath), `MCP E2E fixture is missing: ${mcpFixturePath}`);
    const mcpFixtureArgs = `"${mcpFixturePath}"`;
    await settingsDialog.$('button=MCP').click();
    assert.equal(await settingsDialog.$('.settings-page-title').getText(), 'MCP 服务');
    await settingsDialog.$('button=添加服务').click();
    const mcpForm = await settingsDialog.$('.add-server-form');
    await mcpForm.$('input[placeholder="服务名称"]').setValue('Desktop E2E MCP');
    await mcpForm.$('input[placeholder="npx.cmd"]').setValue(process.execPath);
    await mcpForm.$('input[placeholder^="-y @modelcontextprotocol/"]').setValue(mcpFixtureArgs);
    await mcpForm.$('button=添加').click();
    const mcpItem = await settingsDialog.$('.server-item');
    await mcpItem.waitForDisplayed({ timeout: 5_000 });
    assert.equal(await mcpItem.$('.server-name').getText(), 'Desktop E2E MCP');
    assert.match(await mcpItem.getText(), /未连接/);
    const mcpSecretCanary = 'angelbot-e2e-mcp-secret-canary';
    const newlyAddedMcp = (await browser.tauri.execute(({ core }) => core.invoke('get_mcp_servers')))
      .find((server) => server.name === 'Desktop E2E MCP');
    assert.ok(newlyAddedMcp, 'new MCP server was not returned by the desktop command');
    await browser.execute((id) => { window.__angelbotE2eMcpId = id; }, newlyAddedMcp.id);
    await browser.tauri.execute(({ core }) => core.invoke('set_mcp_env_var', {
      serverId: window.__angelbotE2eMcpId,
      key: 'ANGELBOT_E2E_TOKEN',
      value: 'angelbot-e2e-mcp-secret-canary',
    }));
    const credentialMcp = (await browser.tauri.execute(({ core }) => core.invoke('get_mcp_servers')))
      .find((server) => server.id === newlyAddedMcp.id);
    assert.deepEqual(credentialMcp.envKeys, ['ANGELBOT_E2E_TOKEN']);
    assert.equal(credentialMcp.env, '');
    assert.equal(credentialMcp.envUnavailable, false);
    assert.ok(!JSON.stringify(credentialMcp).includes(mcpSecretCanary));
    const plainMcpBackup = await browser.tauri.execute(({ core }) => core.invoke('export_data'));
    assert.ok(!plainMcpBackup.includes(mcpSecretCanary));
    assert.ok(!plainMcpBackup.includes('env_ref'));
    assert.ok(!plainMcpBackup.includes('mcp_credentials'));
    const encryptedMcpBackup = await browser.tauri.execute(({ core }) => core.invoke('export_encrypted_data', {
      password: 'angelbot-e2e-backup-password',
    }));
    assert.ok(!encryptedMcpBackup.includes(mcpSecretCanary));
    await browser.execute((data) => { window.__angelbotE2eMcpBackup = data; }, encryptedMcpBackup);
    const mcpAccess = await mcpItem.$('.mcp-workspace-access');
    await mcpAccess.$('.//label[contains(., "Desktop E2E Project")]/input').click();
    let configuredMcp;
    await browser.waitUntil(async () => {
      const servers = await browser.tauri.execute(({ core }) => core.invoke('get_mcp_servers'));
      configuredMcp = servers.find((server) => server.name === 'Desktop E2E MCP');
      return configuredMcp?.enabledWorkspaceIds?.includes(createdProject.workspace.id);
    }, { timeout: 5_000, timeoutMsg: 'MCP workspace permission did not persist' });
    assert.equal(configuredMcp.command, process.execPath);
    assert.equal(configuredMcp.args, mcpFixtureArgs);
    assert.deepEqual(configuredMcp.enabledWorkspaceIds, [createdProject.workspace.id]);
    assert.equal(configuredMcp.id, newlyAddedMcp.id);

    const connectMcp = await mcpItem.$('button=连接并检查');
    await connectMcp.waitForEnabled({ timeout: 5_000 });
    await connectMcp.click();
    const mcpSummary = await mcpItem.$('.mcp-runtime-summary');
    await mcpSummary.waitForDisplayed({ timeout: 10_000 });
    assert.match(await mcpSummary.getText(), /已发现 1 个工具/);
    assert.match(await mcpSummary.getText(), /angelbot_e2e_ping/);
    await browser.saveScreenshot(join(screenshotDir, 'mcp-settings.png'));
    const connectedMcp = await browser.tauri.execute(({ core }) => core.invoke('get_mcp_server_status', {
      serverId: window.__angelbotE2eMcpId,
    }));
    assert.equal(connectedMcp.status, 'running');
    assert.equal(connectedMcp.tools_count, 1);
    await mcpItem.$('button[aria-label="编辑 Desktop E2E MCP"]').click();
    const editMcpForm = await mcpItem.$('.add-server-form');
    const argumentsLabel = await editMcpForm.$('label=参数');
    const argumentsFieldId = await argumentsLabel.getAttribute('for');
    assert.ok(argumentsFieldId, 'the MCP arguments label must identify its editable input');
    await editMcpForm.$(`[id="${argumentsFieldId}"]`).setValue(`${mcpFixtureArgs} --e2e-edited`);
    await editMcpForm.$('button=保存修改').click();
    await browser.waitUntil(async () => {
      const servers = await browser.tauri.execute(({ core }) => core.invoke('get_mcp_servers'));
      const updated = servers.find((server) => server.id === configuredMcp.id);
      return updated?.args.endsWith('--e2e-edited') && updated.enabledWorkspaceIds.length === 0;
    }, { timeout: 5_000, timeoutMsg: 'editing MCP arguments did not revoke workspace access' });
    assert.equal((await browser.tauri.execute(({ core }) => core.invoke('get_mcp_server_status', {
      serverId: window.__angelbotE2eMcpId,
    }))).status, 'stopped');
    const editedMcpAccess = await mcpItem.$('.mcp-workspace-access');
    await editedMcpAccess.$('.//label[contains(., "Desktop E2E Project")]/input').click();
    const reconnectMcp = await mcpItem.$('button=连接并检查');
    await reconnectMcp.waitForEnabled({ timeout: 5_000 });
    await reconnectMcp.click();
    await mcpItem.$('.mcp-runtime-summary').waitForDisplayed({ timeout: 10_000 });
    await mcpItem.$('button=停止').click();
    await browser.waitUntil(
      async () => (await mcpItem.$('.status-text').getText()) === '按需连接 · 目录已准备',
      { timeout: 5_000, timeoutMsg: 'stopped MCP did not retain its prepared discovery' },
    );
    const stoppedMcp = await browser.tauri.execute(({ core }) => core.invoke('get_mcp_server_status', {
      serverId: window.__angelbotE2eMcpId,
    }));
    assert.equal(stoppedMcp.status, 'stopped');
    const preparedMcp = (await browser.tauri.execute(({ core }) => core.invoke('get_mcp_servers')))
      .find((server) => server.id === newlyAddedMcp.id);
    assert.equal(preparedMcp.catalogReady, true);
    await mcpItem.$('button[aria-label="删除 Desktop E2E MCP"]').click();
    await mcpItem.waitForExist({ reverse: true, timeout: 5_000 });
    await browser.tauri.execute(({ core }) => core.invoke('import_encrypted_data', {
      data: window.__angelbotE2eMcpBackup,
      password: 'angelbot-e2e-backup-password',
      opts: { mode: 'merge' },
    }));
    const restoredMcp = (await browser.tauri.execute(({ core }) => core.invoke('get_mcp_servers')))
      .find((server) => server.id === newlyAddedMcp.id);
    assert.ok(restoredMcp, 'encrypted backup did not restore the MCP service');
    assert.deepEqual(restoredMcp.envKeys, ['ANGELBOT_E2E_TOKEN']);
    assert.equal(restoredMcp.env, '');
    assert.equal(restoredMcp.envUnavailable, false);
    assert.ok(!JSON.stringify(restoredMcp).includes(mcpSecretCanary));
    await browser.tauri.execute(({ core }) => core.invoke('delete_mcp_server', {
      id: window.__angelbotE2eMcpId,
    }));

    // This stays entirely local: it verifies the configuration guardrails
    // without entering a key, saving a provider, or issuing a search request.
    await settingsDialog.$('button=联网搜索').click();
    assert.equal(await settingsDialog.$('.settings-page-title').getText(), '联网搜索');
    assert.equal(await settingsDialog.$('button=联网搜索').getAttribute('aria-current'), 'page');
    const hostedEndpoint = await settingsDialog.$('#web-search-trusted-endpoint');
    await hostedEndpoint.waitForDisplayed({ timeout: 5_000 });
    assert.equal(await hostedEndpoint.getValue(), 'https://api.tavily.com/search');
    assert.equal(await browser.execute(() => {
      const endpoint = document.querySelector('#web-search-trusted-endpoint');
      return endpoint instanceof HTMLInputElement && endpoint.readOnly;
    }), true, 'hosted search endpoints must remain read-only');
    assert.equal(await settingsDialog.$('#web-search-api-key').getValue(), '');
    const saveWebSearchConfig = await settingsDialog.$('button=保存搜索配置');
    assert.equal(await saveWebSearchConfig.isEnabled(), false, 'an empty hosted API key must not be saveable');
    const webSearchLabel = await settingsDialog.$('label=允许使用联网搜索');
    const webSearchToggleId = await webSearchLabel.getAttribute('for');
    assert.ok(webSearchToggleId, 'the search permission label must identify its switch');
    const webSearchToggle = await settingsDialog.$(`[id="${webSearchToggleId}"]`);
    assert.equal(await webSearchToggle.isEnabled(), false);
    assert.match(await settingsDialog.getText(), /请先保存一个搜索服务商。/);

    const webSearchProvider = await settingsDialog.$('#web-search-provider');
    // Edge WebDriver's option click does not emit React's change event inside
    // the Tauri WebView. Update the rendered select through its native setter
    // and dispatch the normal bubbling events instead.
    await browser.execute(() => {
      const select = document.querySelector('#web-search-provider');
      if (!(select instanceof HTMLSelectElement)) throw new Error('web search provider select is missing');
      const ownSetter = Object.getOwnPropertyDescriptor(select, 'value')?.set;
      const nativeSetter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set;
      (nativeSetter && ownSetter !== nativeSetter ? nativeSetter : ownSetter)?.call(select, 'searxng');
      select.dispatchEvent(new Event('input', { bubbles: true }));
      select.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await browser.waitUntil(
      async () => (await webSearchProvider.getValue()) === 'searxng',
      { timeout: 5_000, timeoutMsg: 'SearXNG provider selection did not apply' },
    );
    const searxngEndpoint = await settingsDialog.$('#web-search-searxng-endpoint');
    await searxngEndpoint.waitForDisplayed({ timeout: 5_000 });
    await settingsDialog.$('#web-search-api-key').waitForExist({ reverse: true, timeout: 5_000 });
    assert.match(await settingsDialog.getText(), /不需要 API Key/);

    await browser.execute((path) => {
      window.__angelbotE2eTrustedAppPath = path;
    }, notepadPath);
    await browser.tauri.execute(({ core }) => core.invoke('save_desktop_trusted_app', {
      app: {
        id: 'notepad',
        displayName: 'Windows Notepad',
        executablePath: window.__angelbotE2eTrustedAppPath,
        capabilities: ['launch'],
        draftSelector: null,
        enabled: true,
        createdAt: 0,
        updatedAt: 0,
      },
    }));
    const desktopStatuses = await browser.tauri.execute(({ core }) => core.invoke('get_desktop_trusted_app_statuses'));
    assert.deepEqual(desktopStatuses, [{ id: 'notepad', status: 'available' }]);
    await settingsDialog.$('button=电脑操作').click();
    assert.equal(await settingsDialog.$('.settings-page-title').getText(), '电脑操作');
    assert.equal(await settingsDialog.$('button=电脑操作').getAttribute('aria-current'), 'page');
    await settingsDialog.$('//*[contains(text(), "Windows Notepad")]').waitForDisplayed({ timeout: 5_000 });
    assert.match(await settingsDialog.getText(), /可供 AngelBot 使用/);
    await browser.saveScreenshot(join(screenshotDir, 'desktop-control-settings.png'));

    await settingsDialog.$('button=文件访问').click();
    assert.equal(await settingsDialog.$('.settings-page-title').getText(), '文件访问');
    assert.equal(await settingsDialog.$('button=文件访问').getAttribute('aria-current'), 'page');
    assert.match(await settingsDialog.getText(), /执行权限/);
    await browser.saveScreenshot(join(screenshotDir, 'file-access-settings.png'));
    await settingsDialog.$('button[aria-label="关闭"]').click();
    await settingsDialog.waitForDisplayed({ reverse: true, timeout: 5_000 });

    await browser.refresh();
    await browser.$('.model-config-notice').waitForDisplayed({ timeout: 20_000 });
    // Tauri's direct-evaluation bridge runs inside the WebView and therefore
    // loses its test-only value on refresh. Restore the opaque id before the
    // final persistence readback below; product state is never stored here.
    await browser.execute((id) => {
      window.__angelbotE2eWorkspaceId = id;
    }, createdProject.workspace.id);
    const quickSwitchButton = await browser.$('[aria-label="快速切换"]');
    await quickSwitchButton.waitForDisplayed({ timeout: 20_000 });
    await quickSwitchButton.click();
    const commandPalette = await browser.$('.command-palette');
    await commandPalette.waitForDisplayed({ timeout: 5_000 });
    assert.match(await commandPalette.getText(), /AngelBot 日常/);
    assert.match(await commandPalette.getText(), /Desktop E2E Project/);
    assert.doesNotMatch(await commandPalette.getText(), /personal-main|project-main/);
    await browser.saveScreenshot(join(screenshotDir, 'quick-switch.png'));
    await browser.keys(['Escape']);
    await commandPalette.waitForDisplayed({ reverse: true, timeout: 5_000 });

    // Reopen through the visible control because WebDriver's synthetic Ctrl+K
    // is intercepted outside the WebView; the desktop journey still verifies
    // search, keyboard selection and the resulting workspace switch.
    await quickSwitchButton.click();
    const reopenedCommandPalette = await browser.$('.command-palette');
    await reopenedCommandPalette.waitForDisplayed({ timeout: 5_000 });
    const commandSearch = await reopenedCommandPalette.$('#command-palette-input');
    await commandSearch.setValue('Desktop E2E Project');
    await browser.keys(['Enter']);
    await reopenedCommandPalette.waitForDisplayed({ reverse: true, timeout: 5_000 });
    await browser.waitUntil(
      async () => (await browser.$('.session-item-select.active .session-item-title').getText()) === 'Desktop E2E Project',
      { timeout: 10_000, timeoutMsg: 'project workspace did not become active after quick switch' },
    );
    await browser.$('.workspace-reminders').waitForExist({ reverse: true, timeout: 5_000 });
    await browser.execute((id) => {
      window.__angelbotE2eProjectSessionId = id;
    }, createdProject.session.id);

    // Project mutations are always request-mode side effects. Exercise both
    // outcomes against the real scoped file service before opening the Files
    // workbench, so its first tree load observes the approved disk state.
    await browser.tauri.execute(({ core }) => core.invoke('set_agent_execution_permission', {
      permission: 'ask',
    }));
    const rejectedFilePath = join(projectRoot, 'rejected-by-e2e.txt');
    const approvedFilePath = join(projectRoot, 'approved-by-e2e.txt');
    const approvedFileContent = 'approved desktop E2E file content\n';
    // Switching workspaces remounts the composer, so reacquire its live node
    // instead of writing through the personal-workspace element handle.
    const projectComposer = await browser.$('textarea');
    await projectComposer.waitForDisplayed({ timeout: 10_000 });

    // Keep the service ID stable so the offline model script can name the
    // project-scoped MCP tool without a production-only test hook.
    await browser.execute((command, args) => {
      window.__angelbotE2eMcpCommand = command;
      window.__angelbotE2eMcpArgs = args;
    }, process.execPath, mcpFixtureArgs);
    await browser.tauri.execute(async ({ core }) => {
      await core.invoke('save_mcp_server', {
        server: {
          id: 'desktop-e2e-mcp',
          name: 'Desktop E2E MCP call',
          command: window.__angelbotE2eMcpCommand,
          args: window.__angelbotE2eMcpArgs,
          env: '',
          enabled: true,
          enabledWorkspaceIds: [],
        },
      });
      await core.invoke('set_mcp_env_var', {
        serverId: 'desktop-e2e-mcp',
        key: 'ANGELBOT_E2E_TOKEN',
        value: 'angelbot-e2e-mcp-secret-canary',
      });
      const server = (await core.invoke('get_mcp_servers'))
        .find((candidate) => candidate.id === 'desktop-e2e-mcp');
      await core.invoke('save_mcp_server', {
        server: { ...server, enabledWorkspaceIds: [window.__angelbotE2eWorkspaceId] },
      });
      await core.invoke('start_mcp_server', { serverId: 'desktop-e2e-mcp' });
    });
    const callTools = await browser.tauri.execute(({ core }) => core.invoke('refresh_mcp_tools', {
      serverId: 'desktop-e2e-mcp',
    }));
    assert.deepEqual(callTools.map((tool) => tool.name), ['angelbot_e2e_ping']);

    await projectComposer.setValue('请使用当前项目已启用的本地 MCP 工具完成一次检查。');
    await browser.$('button[title="发送"]').click();
    await browser.waitUntil(async () => {
      const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2eProjectSessionId,
      }));
      const message = messages.findLast((candidate) =>
        candidate.toolCalls?.some((call) => call.id === 'desktop-e2e-mcp-call'));
      const result = message?.toolResults?.find((candidate) => candidate.callId === 'desktop-e2e-mcp-call');
      return message?.taskFacts?.pending_confirmation?.call_id === 'desktop-e2e-mcp-call'
        || result?.confirmationStatus === 'pending';
    }, {
      timeout: 20_000,
      interval: 200,
      timeoutMsg: 'project MCP call never reached the confirmation boundary',
    });
    const mcpApprovalButtons = await browser.$$('button=允许一次');
    assert.equal(mcpApprovalButtons.length, 1, 'only the project MCP call should be approvable');
    await mcpApprovalButtons[0].click();
    await browser.waitUntil(async () => {
      const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2eProjectSessionId,
      }));
      const message = messages.findLast((candidate) =>
        candidate.toolCalls?.some((call) => call.id === 'desktop-e2e-mcp-call'));
      const result = message?.toolResults?.find((candidate) => candidate.callId === 'desktop-e2e-mcp-call');
      return result?.confirmationStatus === 'approved'
        && result.success === true
        && result.output.includes('angelbot-e2e-mcp-call-ok');
    }, {
      timeout: 20_000,
      interval: 200,
      timeoutMsg: 'approved MCP call did not return its local fixture result',
    });
    await browser.$('//*[contains(text(), "本地 MCP 工具已返回校验结果")]')
      .waitForDisplayed({ timeout: 20_000 });
    const stoppedCallMcp = await browser.tauri.execute(({ core }) => core.invoke('stop_mcp_server', {
      serverId: 'desktop-e2e-mcp',
    }));
    assert.equal(stoppedCallMcp.status, 'stopped');
    await browser.tauri.execute(({ core }) => core.invoke('delete_mcp_server', {
      id: 'desktop-e2e-mcp',
    }));

    await projectComposer.setValue('请在当前项目中新建 rejected-by-e2e.txt，内容为测试文本。');
    const rejectedWriteSend = await browser.$('button[title="发送"]');
    await rejectedWriteSend.waitForEnabled({ timeout: 5_000 });
    await rejectedWriteSend.click();

    let rejectedWriteTimeline;
    await browser.waitUntil(async () => {
      rejectedWriteTimeline = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2eProjectSessionId,
      }));
      const message = rejectedWriteTimeline.findLast((candidate) =>
        candidate.toolCalls?.some((call) => call.id === 'desktop-e2e-rejected-write'));
      const result = message?.toolResults?.find((candidate) => candidate.callId === 'desktop-e2e-rejected-write');
      return message?.taskFacts?.pending_confirmation?.call_id === 'desktop-e2e-rejected-write'
        || result?.confirmationStatus === 'pending';
    }, {
      timeout: 20_000,
      interval: 200,
      timeoutMsg: 'rejected project write never reached the confirmation boundary',
    });
    const rejectedButtons = await browser.$$('button=拒绝');
    assert.equal(rejectedButtons.length, 1, 'only the pending project write should be rejectable');
    await rejectedButtons[0].click();
    await browser.waitUntil(async () => {
      const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2eProjectSessionId,
      }));
      const message = messages.findLast((candidate) =>
        candidate.toolCalls?.some((call) => call.id === 'desktop-e2e-rejected-write'));
      const result = message?.toolResults?.find((candidate) => candidate.callId === 'desktop-e2e-rejected-write');
      return result?.confirmationStatus === 'rejected' && result.success === false;
    }, {
      timeout: 10_000,
      interval: 200,
      timeoutMsg: 'rejected project write was not made terminal',
    });
    assert.equal(existsSync(rejectedFilePath), false, 'rejecting a project write must not create a file');

    await projectComposer.setValue('请在当前项目中新建 approved-by-e2e.txt，内容为 approved desktop E2E file content。');
    const approvedWriteSend = await browser.$('button[title="发送"]');
    await approvedWriteSend.waitForEnabled({ timeout: 5_000 });
    await approvedWriteSend.click();
    await browser.waitUntil(async () => {
      const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2eProjectSessionId,
      }));
      const message = messages.findLast((candidate) =>
        candidate.toolCalls?.some((call) => call.id === 'desktop-e2e-approved-write'));
      const result = message?.toolResults?.find((candidate) => candidate.callId === 'desktop-e2e-approved-write');
      return message?.taskFacts?.pending_confirmation?.call_id === 'desktop-e2e-approved-write'
        || result?.confirmationStatus === 'pending';
    }, {
      timeout: 20_000,
      interval: 200,
      timeoutMsg: 'approved project write never reached the confirmation boundary',
    });
    const approvedButtons = await browser.$$('button=允许一次');
    assert.equal(approvedButtons.length, 1, 'only the pending project write should be approvable');
    await approvedButtons[0].click();
    await browser.waitUntil(async () => {
      const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2eProjectSessionId,
      }));
      const message = messages.findLast((candidate) =>
        candidate.toolCalls?.some((call) => call.id === 'desktop-e2e-approved-write'));
      const result = message?.toolResults?.find((candidate) => candidate.callId === 'desktop-e2e-approved-write');
      return result?.confirmationStatus === 'approved'
        && result.success === true
        && existsSync(approvedFilePath);
    }, {
      timeout: 20_000,
      interval: 200,
      timeoutMsg: 'approved project write did not reach the scoped filesystem',
    });
    assert.equal(readFileSync(approvedFilePath, 'utf8'), approvedFileContent);

    const approvedFileSelector = '[aria-label="在工作台中定位 approved-by-e2e.txt"]';
    try {
      // A successful tool receipt precedes the final message. Streaming and
      // persisted citations use different nodes, so resolve the final visible
      // citation on each poll rather than holding an intermediate element.
      await browser.waitUntil(async () => {
        if (!await browser.execute(() =>
          document.querySelector('.message-bubble--content-stream') === null)) return false;
        return (await browser.$(approvedFileSelector)).isDisplayed();
      }, {
        timeout: 20_000,
        interval: 200,
        timeoutMsg: 'the final approved file citation was not displayed',
      });
    } catch (error) {
      const replyState = await browser.tauri.execute(async ({ core }) => {
        const messages = await core.invoke('get_messages', {
          sessionId: window.__angelbotE2eProjectSessionId,
        });
        const writeTurn = messages.findLast((message) =>
          message.toolCalls?.some((call) => call.id === 'desktop-e2e-approved-write'));
        return {
          writeStatus: writeTurn?.taskRun?.status,
          citationPersisted: messages.some((message) =>
            message.role === 'assistant' && message.content.includes('angelbot-file:approved-by-e2e.txt')),
        };
      }).catch(() => ({ unavailable: true }));
      const displayState = await browser.execute(() => ({
        fileLinks: [...document.querySelectorAll('.message-file-link')].map((link) => link.getAttribute('aria-label')),
        liveBubbles: document.querySelectorAll('.message-bubble--content-stream').length,
        liveCitation: [...document.querySelectorAll('.content-block-text')].some((block) =>
          block.textContent.includes('查看 E2E 文件')),
        locatorAncestors: (() => {
          const ancestors = [];
          let node = document.querySelector('[aria-label="在工作台中定位 approved-by-e2e.txt"]');
          while (node && ancestors.length < 10) {
            const style = getComputedStyle(node);
            const bounds = node.getBoundingClientRect();
            ancestors.push({ tag: node.tagName, className: node.className,
              open: node instanceof HTMLDetailsElement ? node.open : undefined,
              display: style.display, visibility: style.visibility,
              height: bounds.height, top: bounds.top, width: bounds.width, left: bounds.left,
              opacity: style.opacity, contentVisibility: style.contentVisibility,
              checkVisibility: node.checkVisibility?.({ contentVisibilityAuto: true,
                opacityProperty: true, visibilityProperty: true }) });
            node = node.parentElement;
          }
          return ancestors;
        })(),
      })).catch(() => ({ unavailable: true }));
      await browser.saveScreenshot(join(screenshotDir, 'file-locator-failure.png'));
      throw new Error(`${error.message}; ${JSON.stringify({ replyState, displayState })}`, { cause: error });
    }
    const approvedFileLocator = await browser.$(approvedFileSelector);
    await approvedFileLocator.click();
    const approvedFileEntry = await browser.$('//button[contains(@class,"file-item")][contains(.,"approved-by-e2e.txt")]');
    await approvedFileEntry.waitForDisplayed({ timeout: 20_000 });
    await browser.waitUntil(
      async () => (await approvedFileEntry.getAttribute('aria-current')) === 'true',
      { timeout: 10_000, timeoutMsg: 'file locator did not select the approved project file' },
    );
    const approvedPreview = await browser.$('.file-preview');
    await approvedPreview.waitForDisplayed({ timeout: 10_000 });
    assert.match(await approvedPreview.getText(), /approved desktop E2E file content/);
    assert.equal(await browser.$$('//button[contains(@class,"file-item")][contains(.,"rejected-by-e2e.txt")]').length, 0);
    // Return from the locator's file preview to the workbench root so the
    // following surface-navigation assertions start from their intended state.
    const filePreviewBack = await browser.$('.right-panel-back');
    await filePreviewBack.waitForDisplayed({ timeout: 5_000 });
    await filePreviewBack.click();
    await browser.$('.workbench-menu-item').waitForDisplayed({ timeout: 5_000 });

    // Exercise the visible permission-management path against the real IPC and
    // SQLite manager. The fixture is feature-gated inside the desktop E2E build;
    // no browser-facing test command can mint an approval.
    await browser.$('button[aria-label="设置"]').click();
    const projectSettingsDialog = await browser.$('[role="dialog"][aria-modal="true"]');
    await projectSettingsDialog.waitForDisplayed({ timeout: 5_000 });
    await projectSettingsDialog.$('button=项目联网权限').click();
    assert.equal(await projectSettingsDialog.$('.settings-page-title').getText(), '项目联网权限');
    await projectSettingsDialog.$('//*[contains(text(), "docs.example.test")]').waitForDisplayed({ timeout: 5_000 });
    await projectSettingsDialog.$('button[aria-label="撤销 docs.example.test 的联网授权"]').click();
    await projectSettingsDialog.$('//*[contains(text(), "尚无联网授权")]').waitForDisplayed({ timeout: 5_000 });
    const revokedNetworkApprovals = await browser.tauri.execute(({ core }) => core.invoke('get_project_network_approvals', {
      workspaceId: window.__angelbotE2eWorkspaceId,
    }));
    assert.deepEqual(revokedNetworkApprovals.approvals, []);
    await projectSettingsDialog.$('button[aria-label="关闭"]').click();
    await projectSettingsDialog.waitForDisplayed({ reverse: true, timeout: 5_000 });

    // A network Explorer is still one Main-Agent conversation: the user
    // approves the bounded public scope once and sees only a compact work
    // status. The physical HTTP/DNS adapter is deterministic, but the
    // approval, plan, lease, worker, delivery, independent-review, and
    // constrained Main-Agent summary paths are the production graph.
    const networkDelegationComposer = await browser.$('textarea');
    await networkDelegationComposer.waitForDisplayed({ timeout: 10_000 });
    await networkDelegationComposer.setValue('请检索 AngelBot 的公开测试资料，并告诉我结论。');
    const networkDelegationSend = await browser.$('button[title="发送"]');
    await networkDelegationSend.waitForEnabled({ timeout: 5_000 });
    await networkDelegationSend.click();

    await browser.waitUntil(async () => {
      const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2eProjectSessionId,
      }));
      const message = messages.findLast((candidate) =>
        candidate.toolCalls?.some((call) => call.id === 'desktop-e2e-network-delegation'));
      const result = message?.toolResults?.find((candidate) => candidate.callId === 'desktop-e2e-network-delegation');
      return message?.taskFacts?.pending_confirmation?.call_id === 'desktop-e2e-network-delegation'
        || result?.confirmationStatus === 'pending';
    }, {
      timeout: 20_000,
      interval: 200,
      timeoutMsg: 'network delegation never reached its exact confirmation boundary',
    });
    const networkApprovalButtons = await browser.$$('button=允许一次');
    assert.equal(networkApprovalButtons.length, 1, 'only the bounded network delegation should be approvable');
    await networkApprovalButtons[0].click();

    let delegatedActivity;
    let lastDelegationLifecycleState;
    try {
      await browser.waitUntil(async () => {
      const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
        sessionId: window.__angelbotE2eProjectSessionId,
      }));
      const message = messages.findLast((candidate) =>
        candidate.toolCalls?.some((call) => call.id === 'desktop-e2e-network-delegation'));
      const result = message?.toolResults?.find((candidate) => candidate.callId === 'desktop-e2e-network-delegation');
      const activity = await browser.tauri.execute(({ core }) => core.invoke('get_workspace_activity_projection', {
        workspaceId: window.__angelbotE2eWorkspaceId,
      }));
      delegatedActivity = activity.delegations.find((candidate) =>
        candidate.goal.includes('AngelBot 的公开测试资料'));
      const hasMainAgentSummary = messages.some((candidate) =>
        candidate.role === 'assistant'
        && candidate.content.includes('资料检索已完成：AngelBot E2E documentation'));
      lastDelegationLifecycleState = {
        confirmation_status: result?.confirmationStatus ?? null,
        tool_result_success: result?.success ?? null,
        delegation_status: delegatedActivity?.status ?? null,
        evidence_count: delegatedActivity?.evidence_count ?? null,
        reviewer_status: delegatedActivity?.reviewer_status ?? null,
        has_main_agent_summary: hasMainAgentSummary,
        recent_assistant_turns: messages
          .filter((candidate) => candidate.role === 'assistant')
          .slice(-3)
          .map((candidate) => ({
            content: candidate.content,
            tool_calls: candidate.toolCalls?.map((call) => call.name) ?? [],
            tool_results: candidate.toolResults?.map((toolResult) => ({
              call_id: toolResult.callId,
              success: toolResult.success,
              output: toolResult.output,
            })) ?? [],
          })),
      };
      return result?.confirmationStatus === 'approved'
        && result.success === true
        && delegatedActivity?.status === 'completed'
        && delegatedActivity?.evidence_count === 1
        && delegatedActivity?.reviewer_status === 'passed'
        && hasMainAgentSummary;
      }, {
        timeout: 30_000,
        interval: 250,
        timeoutMsg: 'approved network delegation did not complete through the independent review loop',
      });
    } catch (error) {
      throw new Error(
        `approved network delegation did not complete through the independent review loop; last state: ${JSON.stringify(lastDelegationLifecycleState)}`,
        { cause: error },
      );
    }
    assert.equal(delegatedActivity.status, 'completed');
    assert.equal(delegatedActivity.evidence_count, 1);
    assert.equal(delegatedActivity.reviewer_status, 'passed');

    // The three primary surfaces must remain one navigable application, not
    // visually isolated routes that only work in unit tests.
    await navigatePrimary('自动化');
    const automationPage = await browser.$('.automation-page');
    await automationPage.waitForDisplayed({ timeout: 10_000 });
    assert.match(await automationPage.getText(), /每日邮件行动清单/);
    await browser.saveScreenshot(join(screenshotDir, 'automation-workspace.png'));
    const closeStopWorkbench = await browser.$('[aria-label="关闭工作侧栏"]');
    if (await closeStopWorkbench.isDisplayed()) await closeStopWorkbench.click();

    // Exercise the real signal/cleanup path on one owned temporary process,
    // not a user's game helper or a stale persisted running receipt.
    const ownedStopDefinition = await browser.tauri.execute(async ({ core }, { scriptPath, rootPath }) => {
      window.__angelbotE2ePreviousScriptRoot = await core.invoke('get_work_directory');
      await core.invoke('set_work_directory', { path: rootPath });
      const item = await core.invoke('create_automation', {
        title: '停止本次执行检查', prompt: 'Only an isolated owned process fixture.',
        triggerKind: 'once', triggerValue: new Date(Date.now() + 86_400_000).toISOString(),
        permissionSummary: 'Local fixture', executorKind: 'script', scriptPath,
        scriptArgs: [], timeoutSeconds: 60, workspaceId: window.__angelbotE2eWorkspaceId,
      });
      window.__angelbotE2eOwnedStopId = item.id;
      window.__angelbotE2eOwnedStopResult = 'pending';
      void core.invoke('run_automation_now', { id: item.id }).then(
        () => { window.__angelbotE2eOwnedStopResult = 'settled'; },
        () => { window.__angelbotE2eOwnedStopResult = 'failed'; },
      );
      return item;
    }, { scriptPath: ownedStopScript, rootPath: projectRoot });
    await browser.waitUntil(() => existsSync(ownedStopStarted), {
      timeout: 10_000, interval: 100, timeoutMsg: 'owned stop fixture did not actually start',
    });
    await automationPage.$('button=刷新').click();
    const ownedStopCard = await browser.$('//article[contains(@class,"automation-card")][.//h3[text()="停止本次执行检查"]]');
    const stopButton = await ownedStopCard.$('button=停止本次执行');
    await stopButton.waitForEnabled({ timeout: 10_000 });
    const ownedStopPid = Number(readFileSync(ownedStopStarted, 'utf8'));
    assert.ok(Number.isInteger(ownedStopPid) && ownedStopPid > 0);
    process.kill(ownedStopPid, 0);
    await navigatePrimary('对话');
    await browser.$('.chat-area').waitForDisplayed({ timeout: 10_000 });
    for (const decision of ['rejected', 'approved']) {
      const callId = `main-stop-${decision}`;
      const task = decision === 'rejected' ? '请停止本次执行检查，但先让我确认。' : '现在停止本次执行检查。';
      const stopComposer = await browser.$('textarea');
      await stopComposer.setValue(task);
      const send = await browser.$('button[title="发送"]');
      await send.waitForEnabled({ timeout: 5_000 });
      await send.click();
      await browser.waitUntil(async () => {
        const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
          sessionId: window.__angelbotE2eProjectSessionId,
        }));
        const message = messages.findLast(item => item.toolCalls?.some(call => call.id === callId));
        return message?.taskFacts?.pending_confirmation?.call_id === callId
          || message?.toolResults?.some(result => result.callId === callId && result.confirmationStatus === 'pending');
      }, { timeout: 10_000, interval: 100, timeoutMsg: 'Main stop did not reach confirmation' });
      const beforeDecision = await browser.tauri.execute(({ core }) => core.invoke('get_automation_runs', {
        automationId: window.__angelbotE2eOwnedStopId,
      }));
      assert.equal(beforeDecision.length, 1);
      assert.equal(beforeDecision[0].status, 'running');
      assert.equal(beforeDecision[0].stopControl, 'available', 'no stop signal before consent');
      process.kill(ownedStopPid, 0);
      const confirmationSummary = await browser.$('.agent-run-confirmation-summary, .content-block-tool-confirmation-summary');
      await browser.execute(() => document.querySelector('.agent-run-confirmation-summary, .content-block-tool-confirmation-summary')?.scrollIntoView({ block: 'center', behavior: 'instant' }));
      await browser.pause(250);
      const cardText = await confirmationSummary.getText();
      try {
        await browser.waitUntil(() => browser.execute(() => {
          const summary = document.querySelector('.agent-run-confirmation-summary, .content-block-tool-confirmation-summary');
          const host = summary?.closest('.messages');
          if (!summary || !host) return false;
          const bounds = summary.getBoundingClientRect();
          const viewport = host.getBoundingClientRect();
          return bounds.top >= viewport.top && bounds.bottom <= viewport.bottom;
        }), { timeout: 2_000, interval: 100, timeoutMsg: 'stop consent scope is outside the reading viewport' });
      } catch (error) {
        await browser.saveScreenshot(join(mainStopEvidenceDir, 'main-automation-stop-visibility-failure.png'));
        throw error;
      }
      assert.match(cardText, /停止当前工作区中这次受控执行/);
      assert.match(cardText, /不会撤回/);
      assert.match(cardText, /等待清理结果/);
      if (decision === 'approved') await browser.saveScreenshot(join(mainStopEvidenceDir, 'main-automation-stop-consent.png'));
      const buttons = await browser.$$(decision === 'approved' ? 'button=允许一次' : 'button=拒绝');
      assert.equal(buttons.length, 1, 'only the exact owned stop should require consent');
      await buttons[0].click();
      await browser.waitUntil(async () => {
        const messages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
          sessionId: window.__angelbotE2eProjectSessionId,
        }));
        const result = messages.flatMap(item => item.toolResults ?? []).find(item => item.callId === callId);
        return result?.confirmationStatus === decision && result.success === (decision === 'approved');
      }, { timeout: 10_000, interval: 100, timeoutMsg: 'Main stop decision was not durably settled' });
      if (decision === 'rejected') {
        const runs = await browser.tauri.execute(({ core }) => core.invoke('get_automation_runs', {
          automationId: window.__angelbotE2eOwnedStopId,
        }));
        assert.equal(runs[0].status, 'running');
        assert.equal(runs[0].stopControl, 'available', 'rejection must not stop the fixture');
        process.kill(ownedStopPid, 0);
      }
    }
    await browser.waitUntil(async () => {
      const { runs, settled } = await browser.tauri.execute(async ({ core }) => ({
        runs: await core.invoke('get_automation_runs', { automationId: window.__angelbotE2eOwnedStopId }),
        settled: window.__angelbotE2eOwnedStopResult === 'settled',
      }));
      return settled && runs.length === 1 && runs[0].status === 'cancelled' && runs[0].stopControl === undefined;
    }, { timeout: 10_000, timeoutMsg: 'owned stop did not finalize after process cleanup' });
    assert.throws(() => process.kill(ownedStopPid, 0), 'owned process must have exited before final receipt');
    const recall = await browser.$('.workspace-reminders');
    await recall.waitForExist({ timeout: 5_000 });
    const recallWasOpen = await recall.getAttribute('open') !== null;
    if (!recallWasOpen) await recall.$('summary').click();
    await browser.waitUntil(async () => (await recall.getText()).includes('本次已停止'), {
      timeout: 5_000, timeoutMsg: 'Main conversation did not recall the final stop result',
    });
    const stoppedRecall = await recall.$('.//ul[contains(@class,"workspace-reminders-results")]/li[.//span[text()="停止本次执行检查"]]');
    assert.doesNotMatch(await stoppedRecall.getText(), /退出码/, 'an intentional stop must not look like a failed exit');
    await browser.waitUntil(async () => (await browser.$('.messages')).getText().then(text => text.includes('停止请求已接受')), {
      timeout: 10_000, timeoutMsg: 'Main did not explain the accepted stop request',
    });
    const ownedStopOutcome = await browser.tauri.execute(async ({ core }) => ({
      item: (await core.invoke('get_automations')).find((item) => item.id === window.__angelbotE2eOwnedStopId),
      result: window.__angelbotE2eOwnedStopResult,
    }));
    assert.equal(ownedStopOutcome.result, 'settled');
    assert.equal(ownedStopOutcome.item.enabled, true, 'stop must not pause the future schedule');
    assert.equal(ownedStopOutcome.item.nextRunAt, ownedStopDefinition.nextRunAt);
    const stopMessages = await browser.tauri.execute(({ core }) => core.invoke('get_messages', {
      sessionId: window.__angelbotE2eProjectSessionId,
    }));
    const stopCall = stopMessages.flatMap(message => message.toolCalls ?? []).find(call => call.id === 'main-stop-approved');
    const stopResult = stopMessages.flatMap(message => message.toolResults ?? []).find(result => result.callId === 'main-stop-approved');
    const stopArgs = JSON.parse(stopCall.arguments);
    assert.equal(stopArgs.automation_id, ownedStopDefinition.id);
    const stopOutput = JSON.parse(stopResult.output.replace(/^Result: /, '').replace(/^Confirmation approved: /, ''));
    assert.equal(stopOutput.stopControl, 'requested', 'tool response must not fabricate terminal completion');
    assert.equal(stopOutput.runId, stopArgs.run_id);
    assert.equal(stopOutput.id, ownedStopDefinition.id);
    assert.ok(!Object.hasOwn(stopOutput, 'status'));
    assert.equal((await browser.$$('vite-error-overlay')).length, 0);
    await browser.saveScreenshot(join(mainStopEvidenceDir, 'main-automation-stop-result.png'));
    if (!recallWasOpen) await recall.$('summary').click();
    await navigatePrimary('自动化');
    await browser.$('.automation-page').waitForDisplayed({ timeout: 10_000 });
    await browser.waitUntil(async () => (await browser.$('//article[contains(@class,"automation-card")][.//h3[text()="停止本次执行检查"]]').getText()).includes('已停止'), { timeout: 5_000 });
    await browser.saveScreenshot(join(screenshotDir, 'automation-owned-stop.png'));
    await browser.tauri.execute(async ({ core }) => {
      await core.invoke('delete_automation', { id: window.__angelbotE2eOwnedStopId });
      await core.invoke('set_work_directory', { path: window.__angelbotE2ePreviousScriptRoot });
    });

    await navigatePrimary('资料库');
    const libraryPage = await browser.$('.library-page');
    await libraryPage.waitForDisplayed({ timeout: 10_000 });
    assert.match(await libraryPage.getText(), /资料、记忆和已添加的技能/);
    await libraryPage.$('button=记忆').click();
    await libraryPage.$('.memory-page').waitForDisplayed({ timeout: 10_000 });
    await browser.saveScreenshot(join(screenshotDir, 'library-memory.png'));

    await navigatePrimary('对话');
    await browser.$('.chat-area').waitForDisplayed({ timeout: 10_000 });
    const workbenchButton = await browser.$('[aria-label="打开工作台"]');
    await workbenchButton.waitForDisplayed({ timeout: 20_000 });
    const workbenchAlreadyOpen = await browser.$('.right-panel').isDisplayed();
    if (!workbenchAlreadyOpen) {
      await workbenchButton.click();
    }
    await browser.$('.right-panel').waitForDisplayed({ timeout: 10_000 });
    const workbenchBack = await browser.$('.right-panel-back');
    if (await workbenchBack.isDisplayed()) await workbenchBack.click();
    const workbenchState = await browser.execute(() => ({
      shellClass: document.querySelector('.app-shell')?.className,
      panelCount: document.querySelectorAll('.right-panel').length,
      toggleCount: document.querySelectorAll('[aria-label="打开工作台"]').length,
      projectLabels: Array.from(document.querySelectorAll('.session-item-title')).map((node) => node.textContent),
      workbenchEntries: Array.from(document.querySelectorAll('.workbench-menu-item span')).map((node) => node.textContent),
    }));
    assert.equal(workbenchState.panelCount, 1, JSON.stringify(workbenchState));
    assert.deepEqual(workbenchState.workbenchEntries, ['任务与委派', '浏览器', '文件']);
    await browser.$('.right-panel').waitForDisplayed({ timeout: 20_000 });
    await browser.$('.workbench-menu-item').click();
    await browser.$('.workspace-activity-panel').waitForDisplayed({ timeout: 20_000 });
    const completedDelegation = await browser.$('.workspace-activity-panel__list');
    await completedDelegation.waitForDisplayed({ timeout: 10_000 });
    assert.match(await completedDelegation.getText(), /AngelBot 的公开测试资料/);
    assert.match(await completedDelegation.getText(), /已完成/);
    await browser.saveScreenshot(join(screenshotDir, 'project-workbench.png'));

    await browser.$('.right-panel-back').click();
    const filesEntry = await browser.$('//button[contains(@class,"workbench-menu-item")][.//span[text()="文件"]]');
    await filesEntry.waitForDisplayed({ timeout: 5_000 });
    await filesEntry.click();
    const readmeEntry = await browser.$('//button[contains(@class,"file-item")][contains(.,"README.md")]');
    await readmeEntry.waitForDisplayed({ timeout: 10_000 });
    await readmeEntry.click();
    const preview = await browser.$('.file-preview');
    await preview.waitForDisplayed({ timeout: 10_000 });
    assert.match(await preview.getText(), /Desktop E2E Project/);
    assert.equal(await readmeEntry.getAttribute('aria-current'), 'true');
    await browser.saveScreenshot(join(screenshotDir, 'project-files.png'));

    // Compact desktop windows keep the conversation usable and promote the
    // workbench to an inspector drawer instead of squeezing three columns.
    await browser.setWindowSize(1000, 760);
    await browser.pause(250);
    const compactWorkbenchState = await browser.execute(() => {
      const panel = document.querySelector('.right-panel');
      const chat = document.querySelector('.chat-area');
      const main = document.querySelector('.main-with-nav');
      if (!(panel instanceof HTMLElement) || !(chat instanceof HTMLElement)) return null;
      return {
        panelPosition: getComputedStyle(panel).position,
        panelRight: getComputedStyle(panel).right,
        chatWidth: Math.round(chat.getBoundingClientRect().width),
        mainWidth: main instanceof HTMLElement ? Math.round(main.getBoundingClientRect().width) : 0,
        viewportWidth: window.innerWidth,
      };
    });
    assert.equal(compactWorkbenchState?.panelPosition, 'fixed');
    assert.equal(compactWorkbenchState?.panelRight, '0px');
    const compactMinimum = Math.min(650, compactWorkbenchState?.viewportWidth ?? 0);
    assert.ok((compactWorkbenchState?.mainWidth ?? 0) >= compactMinimum, JSON.stringify(compactWorkbenchState));
    // Windows reserves up to a scrollbar-width inside the scrolling main
    // surface. The chat must occupy the rest of that surface; a fixed CSS
    // width would be DPI-dependent in the real desktop shell.
    assert.ok(
      (compactWorkbenchState?.chatWidth ?? 0) >= (compactWorkbenchState?.mainWidth ?? 0) - 16,
      JSON.stringify(compactWorkbenchState),
    );
    await browser.saveScreenshot(join(screenshotDir, 'compact-workbench.png'));
    await browser.setWindowSize(1200, 800);
    await browser.$('[aria-label="关闭工作侧栏"]').click();
    await browser.$('.right-panel').waitForExist({ reverse: true, timeout: 5_000 });

    // User-selected file snapshots must reach the same Main conversation and
    // survive replay/edit without granting Personal a project root. Its owned
    // results tools remain available, but this snapshot turn must not use them. A real
    // File/change event exercises the production reader; no IPC-only shortcut.
    await browser.$('//button[contains(@class,"session-item-select")][.//span[text()="AngelBot 日常"]]').click();
    await browser.waitUntil(async () => (await browser.$('.session-item-select.active .session-item-title').getText()) === 'AngelBot 日常', {
      timeout: 10_000, timeoutMsg: 'Personal did not become active for the explicit file journey',
    });
    const invalidAttachment = await browser.tauri.execute(async ({ core }) => {
      // The earlier reload clears window globals. Resolve the live identity
      // through the same workspace contract instead of a stale test variable.
      const personal = (await core.invoke('get_workspaces')).find((item) => item.kind === 'personal');
      if (!personal?.activeSessionId) throw new Error('Personal Main conversation identity missing');
      const sessionId = personal.activeSessionId;
      const before = await core.invoke('get_messages', { sessionId });
      let rejected = false;
      try {
        await core.invoke('send_message', { req: {
          sessionId, role: 'user', content: 'This invalid attachment must not create a turn.',
          textAttachments: [{ name: 'unsupported.pdf', text: 'not a supported snapshot' }],
        } });
      } catch { rejected = true; }
      const after = await core.invoke('get_messages', { sessionId });
      return { rejected, before: before.map((item) => item.id), after: after.map((item) => item.id) };
    });
    assert.ok(invalidAttachment.rejected, 'backend must reject unsupported files before reserving a turn');
    assert.deepEqual(invalidAttachment.after, invalidAttachment.before, 'invalid attachment changed the durable conversation');
    await browser.execute(() => {
      const input = document.querySelector('.composer input[type="file"]');
      if (!(input instanceof HTMLInputElement)) throw new Error('Composer file input missing');
      const files = new DataTransfer();
      files.items.add(new File(['unsupported image fixture'], 'not-readable.png', { type: 'image/png' }));
      input.files = files.files;
      input.dispatchEvent(new Event('change', { bubbles: true }));
    });
    const attachmentError = await browser.$('.composer-attachment-error[role="alert"]');
    await attachmentError.waitForDisplayed({ timeout: 5_000 });
    assert.match(await attachmentError.getText(), /图片.*暂不支持/);
    assert.equal((await browser.$$('.composer-file-chip')).length, 0, 'unsupported file must not become a misleading preview');
    const attachmentText = 'task,priority\nattachment-e2e-marker,high\n';
    await browser.execute((text) => {
      const input = document.querySelector('.composer input[type="file"]');
      if (!(input instanceof HTMLInputElement)) throw new Error('Composer file input missing');
      const files = new DataTransfer();
      files.items.add(new File([text], 'daily-actions.csv', { type: 'text/csv' }));
      for (let index = 1; index < 4; index++) {
        files.items.add(new File(['bounded layout fixture'], `${'long-name-'.repeat(14)}${index}.txt`, { type: 'text/plain' }));
      }
      input.files = files.files;
      input.dispatchEvent(new Event('change', { bubbles: true }));
    }, attachmentText);
    await browser.waitUntil(async () => (await browser.$$('.composer-file-chip')).length === 4, { timeout: 5_000 });
    assert.equal(await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').getValue(), '');
    const fileDraftLayout = await browser.execute(() => ({
      messagesHeight: document.querySelector('.messages')?.getBoundingClientRect().height,
      composerBottom: document.querySelector('.composer')?.getBoundingClientRect().bottom,
      viewportHeight: window.innerHeight,
      regions: Array.from(document.querySelector('.chat-area')?.children ?? []).map((item) => ({
        className: item.className, height: Math.round(item.getBoundingClientRect().height),
      })),
      composerHeight: document.querySelector('.composer')?.getBoundingClientRect().height,
    }));
    await browser.saveScreenshot(join(screenshotDir, 'daily-file-draft.png'));
    assert.ok(fileDraftLayout.messagesHeight >= 120, `file draft must leave reading space: ${JSON.stringify(fileDraftLayout)}`);
    assert.ok(fileDraftLayout.composerBottom <= fileDraftLayout.viewportHeight + 1, `file draft clips Composer: ${JSON.stringify(fileDraftLayout)}`);
    const removeButtons = await browser.$$('.composer-file-chip button');
    for (let index = removeButtons.length - 1; index >= 1; index--) await removeButtons[index].click();
    await browser.waitUntil(async () => (await browser.$$('.composer-file-chip')).length === 1, { timeout: 5_000 });
    await browser.$('button[title="发送"]').waitForEnabled({ timeout: 5_000 });
    await browser.$('button[title="发送"]').click();
    await waitForVisible(browser, '//*[contains(text(), "已读取你选择的日常行动清单")]', { timeout: 20_000 });
    const personalMessages = await browser.tauri.execute(async ({ core }) => {
      const personal = (await core.invoke('get_workspaces')).find((item) => item.kind === 'personal');
      if (!personal?.activeSessionId) throw new Error('Personal Main conversation identity missing');
      return core.invoke('get_messages', { sessionId: personal.activeSessionId });
    });
    const attachmentMessage = personalMessages.find((item) => item.role === 'user' && item.textAttachments?.[0]?.name === 'daily-actions.csv');
    assert.ok(attachmentMessage, 'explicit file-only turn was not persisted');
    assert.equal(attachmentMessage.content, '', 'file snapshot must not be flattened into the visible prompt');
    assert.deepEqual(attachmentMessage.textAttachments, [{ name: 'daily-actions.csv', text: attachmentText }]);
    const snapshotReply = personalMessages.find((item) => item.role === 'assistant'
      && item.parentId === attachmentMessage.id && item.content.includes('已读取你选择的日常行动清单'));
    assert.ok(snapshotReply, 'snapshot reply must belong to the selected file turn');
    const filesystemCalls = ['read_file', 'write_file', 'run_project_command'];
    assert.deepEqual((snapshotReply.toolCalls ?? []).filter((call) => filesystemCalls.includes(call.name)), []);
    assert.deepEqual((snapshotReply.toolResults ?? []).filter((result) => filesystemCalls.includes(result.toolName)), []);
    await browser.saveScreenshot(join(screenshotDir, 'daily-file-snapshot.png'));

    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('继续整理上一份清单。');
    await browser.$('button[title="发送"]').waitForEnabled({ timeout: 5_000 });
    await browser.$('button[title="发送"]').click();
    await waitForVisible(browser, '//*[contains(text(), "上一轮的文件快照仍在当前日常会话中")]', { timeout: 20_000 });
    // Switching away/back must render the persisted attachment, not a local
    // preview left behind in Composer state.
    await browser.$('//button[contains(@class,"session-item-select")][.//span[text()="Desktop E2E Project"]]').click();
    await browser.waitUntil(async () => (await browser.$('.session-item-select.active .session-item-title').getText()) === 'Desktop E2E Project', { timeout: 10_000 });
    await browser.$('//button[contains(@class,"session-item-select")][.//span[text()="AngelBot 日常"]]').click();
    await browser.waitUntil(async () => (await browser.$('.session-item-select.active .session-item-title').getText()) === 'AngelBot 日常', { timeout: 10_000 });
    const attachedRow = await browser.$('//div[contains(@class,"message-row") and contains(@class,"user")][contains(.,"daily-actions.csv")]');
    await attachedRow.waitForDisplayed({ timeout: 10_000 });
    await attachedRow.$('button[title="编辑消息"]').click();
    await attachedRow.$('textarea[aria-label="编辑后的消息内容"]').setValue('按优先级重新整理这份附件。');
    await attachedRow.$('button=重新发送').click();
    await waitForVisible(browser, '//*[contains(text(), "已按编辑后的要求重新整理")]', { timeout: 20_000 });
    const editedMessages = await browser.tauri.execute(async ({ core }) => {
      const personal = (await core.invoke('get_workspaces')).find((item) => item.kind === 'personal');
      if (!personal?.activeSessionId) throw new Error('Personal Main conversation identity missing');
      return core.invoke('get_messages', { sessionId: personal.activeSessionId });
    });
    const editedAttachmentMessage = editedMessages.find((item) => item.id === attachmentMessage.id);
    assert.equal(editedAttachmentMessage?.content, '按优先级重新整理这份附件。');
    assert.deepEqual(editedAttachmentMessage?.textAttachments, attachmentMessage.textAttachments);
    assert.ok(!editedMessages.some((item) => item.content === '继续整理上一份清单。'), 'resend must replace the old future branch');

    // Consent rendering only: reject before invoking MAA or creating a timer.
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('查看 MAA 自动化确认范围，先不要执行。');
    await browser.$('button[title="发送"]').click();
    const maaScopeSelector = '//*[contains(text(), "以后按计划无人值守运行 MAA 当前 GUI 勾选队列")]';
    await browser.waitUntil(async () => (await browser.$(maaScopeSelector)).isDisplayed(), { timeout: 20_000 });
    const maaScope = await browser.$(maaScopeSelector);
    const maaScopeText = await maaScope.getText();
    for (const expected of ['每天 18:00', '启动已配置的模拟器', '1800 秒',
      '按现有消耗设置执行', '仅回收本次启动的进程', '不会修改游戏任务或消耗设置']) {
      assert.ok(maaScopeText.includes(expected), `missing MAA consent scope: ${expected}`);
    }
    await browser.saveScreenshot(join(screenshotDir, 'maa-consent-scope.png'));
    const maaRejectButtons = await browser.$$('button=拒绝');
    assert.equal(maaRejectButtons.length, 1);
    await maaRejectButtons[0].click();
    await browser.waitUntil(async () => {
      const messages = await browser.tauri.execute(async ({ core }) => {
        const personal = (await core.invoke('get_workspaces')).find((item) => item.kind === 'personal');
        return core.invoke('get_messages', { sessionId: personal.activeSessionId });
      });
      return messages.flatMap((message) => message.toolResults ?? []).some((result) =>
        result.callId === 'desktop-e2e-maa-consent'
        && result.confirmationStatus === 'rejected' && result.success === false);
    }, { timeout: 10_000, interval: 200, timeoutMsg: 'MAA rejection was not persisted' });
    const afterMaaRejection = await browser.tauri.execute(({ core }) => core.invoke('get_automations'));
    assert.ok(!afterMaaRejection.some((item) => item.title === 'MAA 确认测试'),
      'rejected MAA consent must not create a timer');

    // Revoke the isolated fixture before approval so no real app can launch.
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('打开测试记事本。');
    await browser.$('button[title="发送"]').click();
    await waitForVisible(browser, '//*[contains(text(), "打开已信任应用：notepad")]', { timeout: 20_000 });
    await browser.tauri.execute(async ({ core }) => {
      const apps = await core.invoke('get_desktop_trusted_apps');
      const app = apps.find((item) => item.id === 'notepad');
      if (!app) throw new Error('isolated Notepad fixture missing');
      await core.invoke('save_desktop_trusted_app', { app: { ...app, enabled: false } });
    });
    const revokedAppStatuses = await browser.tauri.execute(({ core }) => core.invoke('get_desktop_trusted_app_statuses'));
    assert.deepEqual(revokedAppStatuses, [{ id: 'notepad', status: 'disabled' }]);
    const revokedAppApprovals = await browser.$$('button=允许一次');
    assert.equal(revokedAppApprovals.length, 1);
    await revokedAppApprovals[0].click();
    await browser.waitUntil(async () =>
      (await browser.$('[aria-label="操作结果"]')).isDisplayed(),
    { timeout: 10_000, timeoutMsg: 'revoked desktop action did not produce a visible operation receipt' });
    const failedDesktopReceipt = await browser.$('[aria-label="操作结果"]');
    assert.match(await failedDesktopReceipt.getText(), /操作未完成/);
    assert.match(await failedDesktopReceipt.getText(), /重试前请先核对/);
    const failedReceiptMessages = await browser.tauri.execute(async ({ core }) => {
      const personal = (await core.invoke('get_workspaces')).find((item) => item.kind === 'personal');
      return core.invoke('get_messages', { sessionId: personal.activeSessionId });
    });
    const failedAppMessage = failedReceiptMessages.find((message) =>
      message.toolCalls?.some((call) => call.id === 'desktop-e2e-revoked-app'));
    const failedAppResult = failedAppMessage?.toolResults?.find((result) => result.callId === 'desktop-e2e-revoked-app');
    assert.equal(failedAppResult?.success, false, 'revoked launch must remain a failure in durable history');
    assert.equal(failedAppResult?.confirmationRequired, false, 'failed approval must be settled, not pending again');
    assert.equal(failedAppMessage?.taskFacts?.pending_confirmation ?? null, null);
    await browser.saveScreenshot(join(screenshotDir, 'desktop-failed-result.png'));

    // A second, owned process is configured through the ordinary application
    // allowlist. The real desktop parent must launch its own capture helper;
    // no helper path or image payload enters the frontend or this request.
    const captureFixturePath = process.env.ANGELBOT_DESKTOP_E2E_WINDOW_EXE;
    assert.ok(captureFixturePath && existsSync(captureFixturePath), 'owned window fixture missing');
    await browser.execute((path) => { window.__angelbotE2eWindowPath = path; }, captureFixturePath);
    await browser.tauri.execute(({ core }) => core.invoke('save_desktop_trusted_app', {
      app: {
        id: 'desktop-e2e-window', displayName: 'Owned window observation fixture',
        executablePath: window.__angelbotE2eWindowPath,
        capabilities: ['observe', 'observeImage'], draftSelector: null,
        enabled: true, createdAt: 0, updatedAt: 0,
      },
    }));
    const readImageJourneyMessages = () => browser.tauri.execute(async ({ core }) => {
      const personal = (await core.invoke('get_workspaces')).find((item) => item.kind === 'personal');
      return core.invoke('get_messages', { sessionId: personal.activeSessionId });
    });
    const imageApproveSelector = 'button=允许发送图像一次';
    const waitForOwnedDesktopConsent = async (selector, expectedLabels, failureScreenshot) => {
      let lastConsentState = {};
      try {
        await browser.waitUntil(async () => {
          const visibleButtons = [];
          for (const button of await browser.$$(selector)) {
            if (await button.isDisplayed()) visibleButtons.push(button);
          }
          lastConsentState = { singleCard: visibleButtons.length === 1 };
          if (!lastConsentState.singleCard) return false;
          const button = visibleButtons[0];
          lastConsentState = {
            ...lastConsentState, enabled: await button.isEnabled(),
          };
          if (!lastConsentState.enabled) return false;
          const actions = await button.$('..');
          const card = await actions.$('..');
          const text = await card.getText();
          lastConsentState = {
            ...lastConsentState,
            appName: text.includes('Owned window observation fixture'),
            executableName: text.includes(basename(captureFixturePath)),
            noExecutablePath: !text.includes(captureFixturePath),
            ...Object.fromEntries(expectedLabels.map((label, index) => [`scope${index}`, text.includes(label)])),
          };
          return Object.values(lastConsentState).every(Boolean);
        }, { timeout: 20_000, timeoutMsg: 'desktop consent did not identify the exact owned app and operation scope' });
      } catch (error) {
        await browser.saveScreenshot(join(screenshotDir, failureScreenshot));
        console.error('Owned window desktop consent failure:', JSON.stringify(lastConsentState));
        const taskMessages = await readImageJourneyMessages();
        const observations = taskMessages
          .flatMap((message) => message.toolResults ?? [])
          .filter((result) => result.callId.startsWith('cu-')).slice(-8).map((result) => {
            let output;
            try { output = JSON.parse(result.output); } catch { output = null; }
            return {
              callId: result.callId, success: result.success,
              confirmationRequired: result.confirmationRequired,
              confirmationStatus: result.confirmationStatus,
              outputKeys: output && typeof output === 'object' ? Object.keys(output) : [],
              errorCode: output?.code, appMatches: output?.appId === 'desktop-e2e-window',
              hasRegionRef: typeof output?.regionRef === 'string', truncated: output?.truncated,
              regionCount: output?.regions?.length, controlCount: output?.controls?.length,
              diagnostics: output?.diagnostics,
            };
          });
        console.error('Owned window task observation diagnostics:', JSON.stringify(observations));
        console.error('Owned window task call order:', JSON.stringify(taskMessages
          .flatMap((message) => (message.toolCalls ?? []).filter((call) => call.id.startsWith('cu-'))
            .map((call) => ({ callId: call.id, name: call.name })))));
        throw error;
      }
    };
    const waitForImageConsent = () => waitForOwnedDesktopConsent(imageApproveSelector,
      ['已核对授权应用', '当前配置的模型', '窗口可能包含私人内容'],
      'desktop-window-image-consent-failure.png');
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('先展示测试窗口图像的确认范围，我会拒绝。');
    await browser.$('button[title="发送"]').click();
    await waitForImageConsent();
    await browser.saveScreenshot(join(screenshotDir, 'desktop-window-image-consent.png'));
    await browser.$('button=拒绝').click();
    await browser.waitUntil(async () => (await readImageJourneyMessages())
      .flatMap((message) => message.toolResults ?? []).some((result) =>
        result.callId === 'desktop-e2e-rejected-image'
        && result.confirmationStatus === 'rejected' && result.success === false),
    { timeout: 10_000, timeoutMsg: 'image rejection was not made terminal' });

    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('只观察临时测试窗口，确认后告诉我看到了什么。');
    await browser.$('button[title="发送"]').click();
    await waitForImageConsent();
    const allowImageOnce = await browser.$(imageApproveSelector);
    await allowImageOnce.click();
    const imageReply = '已看到测试窗口中的两组色块；这是只读观察，没有点击或填写。';
    try {
      await waitForVisible(browser, `//*[contains(text(), "${imageReply}")]`, { timeout: 20_000 });
    } catch (error) {
      await browser.saveScreenshot(join(screenshotDir, 'desktop-window-image-failure.png'));
      const recent = (await readImageJourneyMessages()).slice(-4).map((message) => ({
        role: message.role,
        content: message.content?.slice(0, 1200),
        results: message.toolResults?.map((result) => ({
          callId: result.callId, success: result.success, confirmationStatus: result.confirmationStatus,
          output: result.output?.slice(0, 1200),
        })),
      }));
      console.error('Owned window image handoff failure:', JSON.stringify(recent));
      throw error;
    }
    await browser.waitUntil(async () => !(await browser.$('button[title="停止运行"]').isExisting()),
      { timeout: 10_000, timeoutMsg: 'composed image continuation did not settle' });
    const imageMessages = await readImageJourneyMessages();
    const imageResults = imageMessages.flatMap((message) => message.toolResults ?? [])
      .filter((result) => result.callId === 'desktop-e2e-window-image');
    assert.equal(imageResults.length, 1, 'capture must have one durable receipt');
    assert.equal(imageResults[0].success, true, imageResults[0].output);
    assert.equal(imageResults[0].confirmationStatus, 'approved');
    const lastImageReply = imageMessages.filter((message) => message.role === 'assistant').at(-1);
    assert.equal(lastImageReply?.content, imageReply, 'frontend must not repeat backend continuation');
    assert.ok(!JSON.stringify(imageMessages).includes('data:image/png;base64,'));
    assert.ok(!JSON.stringify(imageMessages).includes('iVBORw0KGgo'));

    // The scripted provider also asserts zero tool images on this new turn;
    // serializing history must not retain or resend the prior screenshot.
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('这一轮不要重新观察窗口。');
    await browser.$('button[title="发送"]').click();
    await browser.waitUntil(async () => {
      const reply = (await readImageJourneyMessages()).filter((message) => message.role === 'assistant').at(-1);
      return reply?.id !== lastImageReply.id && reply?.content === '上一轮图像已释放，当前只保留文字结果。';
    }, { timeout: 20_000, timeoutMsg: 'next turn retained images or consumed an extra scripted reply' });
    await browser.saveScreenshot(join(screenshotDir, 'desktop-window-image-result.png'));

    // This independent read happens before any intentional UIA mutation.
    // Fixture paths and values stay in the Node runner, not the WebView/model.
    const fixtureSnapshotPath = process.env.ANGELBOT_DESKTOP_E2E_WINDOW_SNAPSHOT;
    const fixtureBaseline = JSON.parse(process.env.ANGELBOT_DESKTOP_E2E_WINDOW_BASELINE ?? 'null');
    assert.ok(fixtureSnapshotPath && existsSync(fixtureSnapshotPath) && fixtureBaseline,
      'owned image-phase snapshot baseline missing');
    const imagePhaseSnapshot = JSON.parse(readFileSync(fixtureSnapshotPath, 'utf8'));
    for (const [key, value] of Object.entries(fixtureBaseline)) {
      assert.deepEqual(imagePhaseSnapshot[key], value, `read-only image phase changed fixture state: ${key}`);
    }

    // Grant only the already implemented UIA control capability to this owned
    // app. Model replies must obtain fresh opaque references after each effect.
    await browser.tauri.execute(({ core }) => core.invoke('save_desktop_trusted_app', {
      app: {
        id: 'desktop-e2e-window', displayName: 'Owned window observation fixture',
        executablePath: window.__angelbotE2eWindowPath,
        capabilities: ['observe', 'observeImage', 'interact'], draftSelector: null,
        enabled: true, createdAt: 0, updatedAt: 0,
      },
    }));
    const beforeControlTask = await readImageJourneyMessages();
    const controlTask = '请只操作测试窗口 A 区的复选框：先勾选，再重新观察并取消勾选；每次操作都先让我确认，不提交其他操作。';
    const waitForOwnedCheckboxState = (checked, checkedCount, uncheckedCount) => browser.waitUntil(() => {
      try {
        const snapshot = JSON.parse(readFileSync(fixtureSnapshotPath, 'utf8'));
        return snapshot.processId === fixtureBaseline.processId
          && snapshot.windowTitle === fixtureBaseline.windowTitle
          && snapshot.denseMode === true
          && JSON.stringify(snapshot.denseChecked) === JSON.stringify([checked, false, false, false])
          && JSON.stringify(snapshot.denseCheckedCounts) === JSON.stringify([checkedCount, 0, 0, 0])
          && JSON.stringify(snapshot.denseUncheckedCounts) === JSON.stringify([uncheckedCount, 0, 0, 0]);
      } catch { return false; } // Retry a transient partial fixture write, not an action.
    }, { timeout: 5_000, timeoutMsg: 'owned UIA task state/counters did not match the exact approved effects' });

    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue(controlTask);
    await browser.$('button[title="发送"]').click();
    const checkSelector = 'button=允许勾选复选框一次';
    const uncheckSelector = 'button=允许取消勾选复选框一次';
    const controlScope = ['已核对目标', 'Fixture region A', 'Region checkbox'];
    await waitForOwnedDesktopConsent(checkSelector, controlScope, 'desktop-window-task-consent-failure.png');
    await waitForOwnedCheckboxState(false, 0, 0);
    await browser.$(checkSelector).click();
    await waitForOwnedDesktopConsent(uncheckSelector, controlScope, 'desktop-window-task-consent-failure.png');
    await waitForOwnedCheckboxState(true, 1, 0);
    await browser.saveScreenshot(join(screenshotDir, 'desktop-window-task-second-consent.png'));
    await browser.$(uncheckSelector).click();
    // Check the owned application's state before waiting for model narration,
    // so a late observation mismatch cannot conceal a wrong/repeated effect.
    await waitForOwnedCheckboxState(false, 1, 1);
    const controlTaskReply = '已按两次确认将 A 区复选框勾选并恢复为未选中；控件状态由操作回读确认，未提交其他操作。';
    await waitForVisible(browser, `//*[contains(text(), "${controlTaskReply}")]`, { timeout: 20_000 });
    await browser.waitUntil(async () => !(await browser.$('button[title="停止运行"]').isExisting()),
      { timeout: 10_000, timeoutMsg: 'approved desktop control task did not settle' });
    await waitForOwnedCheckboxState(false, 1, 1);
    const controlTaskMessages = await readImageJourneyMessages();
    assert.equal(controlTaskMessages.filter((message) => message.role === 'user').length,
      beforeControlTask.filter((message) => message.role === 'user').length + 1,
      'both approvals must continue the same user task, not append another user turn');
    assert.equal(controlTaskMessages.filter((message) => message.role === 'assistant').at(-1)?.content, controlTaskReply);
    assertOwnedDesktopControlJourney(controlTaskMessages, controlTask);
    const reorderedResults = structuredClone(controlTaskMessages).reverse();
    for (const message of reorderedResults) message.toolResults?.reverse();
    assertOwnedDesktopControlJourney(reorderedResults, controlTask);
    // Exercise the gate against this actual durable journey, without executing
    // extra actions. Missing evidence, replay, false receipts and broken order
    // must fail rather than be excused as compression.
    for (const scenario of ['missing-result', 'duplicate-result', 'duplicate-call', 'reversed-calls',
      'broken-parent', 'wrong-app', 'stale-reference', 'unverified-receipt', 'extra-effect',
      'disguised-effect', 'orphan-result', 'missing-state', 'contradictory-state']) {
      const invalid = structuredClone(controlTaskMessages);
      const owner = (id) => invalid.find((message) => message.toolCalls?.some((call) => call.id === id));
      const call = (id) => owner(id).toolCalls.find((item) => item.id === id);
      const resultOwner = (id) => invalid.find((message) => message.toolResults?.some((item) => item.callId === id));
      const result = (id) => resultOwner(id).toolResults.find((item) => item.callId === id);
      if (scenario === 'missing-result') resultOwner('cu-check').toolResults = resultOwner('cu-check')
        .toolResults.filter((item) => item.callId !== 'cu-check');
      if (scenario === 'duplicate-result') resultOwner('cu-check').toolResults.push(structuredClone(result('cu-check')));
      if (scenario === 'duplicate-call') owner('cu-check').toolCalls.push(structuredClone(call('cu-check')));
      if (scenario === 'reversed-calls') owner('cu-check').toolCalls.reverse();
      if (scenario === 'broken-parent') owner('cu-uncheck').parentId = invalid
        .find((message) => message.role === 'user' && message.content === controlTask).id;
      if (scenario === 'wrong-app') call('cu-root-before').arguments = JSON.stringify({ app_id: 'wrong-app', mode: 'controls' });
      if (scenario === 'stale-reference') {
        const output = JSON.parse(result('cu-region-after-uncheck').output.replace(/^Result: /, ''));
        const earlier = JSON.parse(result('cu-region-before').output.replace(/^Result: /, ''));
        output.controls.find((control) => control.name === 'Region checkbox').controlRef = earlier.controls
          .find((control) => control.name === 'Region checkbox').controlRef;
        result('cu-region-after-uncheck').output = JSON.stringify(output);
      }
      if (scenario === 'missing-state' || scenario === 'contradictory-state') {
        const output = JSON.parse(result('cu-region-after-check').output.replace(/^Result: /, ''));
        const checkbox = output.controls.find((control) => control.name === 'Region checkbox');
        if (scenario === 'missing-state') delete checkbox.state;
        else checkbox.state = { toggle: 'off' };
        result('cu-region-after-check').output = JSON.stringify(output);
      }
      if (scenario === 'unverified-receipt') result('cu-check').output = 'Confirmation approved: '
        + JSON.stringify({ adapter: 'windows-native-uia', status: 'dispatched', target: 'desktop-e2e-window', action: 'check', detail: 'Not verified' });
      if (scenario === 'extra-effect') owner('cu-uncheck').toolCalls.push({ ...call('cu-uncheck'), id: 'cu-extra' });
      if (scenario === 'disguised-effect') owner('cu-uncheck').toolCalls.push({
        ...call('cu-uncheck'), id: 'context-compaction-999',
      });
      if (scenario === 'orphan-result') resultOwner('cu-uncheck').toolResults.push({
        ...result('cu-uncheck'), callId: 'cu-extra',
      });
      assert.throws(() => assertOwnedDesktopControlJourney(invalid, controlTask), undefined,
        `durable desktop gate accepted ${scenario}`);
    }
    assert.equal(new Set(controlTaskMessages.map((message) => message.sessionId)).size, 1,
      'desktop work must stay in the existing Main conversation');
    assert.ok(controlTaskMessages.every((message) => message.content !== '该操作需要你的确认后才能继续。'),
      'settled confirmations must not retain host-generated waiting prose');
    assert.ok(!(await browser.$('.chat-area').getText()).includes('该操作需要你的确认后才能继续。'),
      'the completed desktop task must not show stale waiting prose');
    await browser.saveScreenshot(join(screenshotDir, 'desktop-window-task-result.png'));

    // A provider-truncated text reply is not a completed task, even with no
    // tool calls. Preserve the useful fragment and the same durable warning
    // across a WebView reload. Only an explicit new user message may continue.
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('请说明今天安排，不操作任何文件或应用。');
    await browser.$('button[title="发送"]').click();
    await waitForVisible(browser, '//*[contains(text(), "回复达到模型输出长度限制")]', { timeout: 20_000 });
    await browser.waitUntil(async () => !(await browser.$('button[title="停止运行"]').isExisting()),
      { timeout: 10_000, timeoutMsg: 'truncated reply did not pause' });
    const partial = (await readImageJourneyMessages()).filter((message) => message.role === 'assistant').at(-1);
    assert.ok(partial.content.startsWith('今日安排已整理到这一步。\n\n回复状态：'));
    assert.match(partial.content, /任务尚未确认完成/);
    assert.match(partial.content, /不会自动重复/);
    assert.equal(partial.taskRun?.status, 'needs_attention');
    assert.equal(partial.taskRun?.resumable, true);
    const partialResults = partial.toolResults.filter((result) => result.toolName !== 'context_compaction');
    assert.equal(partialResults.length, 1, 'only the host attention marker, no external tool');
    assert.equal(partialResults[0].toolName, 'agent_loop');
    assert.equal(partialResults[0].success, false);
    assert.ok(partial.toolResults.filter((result) => result.toolName === 'context_compaction')
      .every((result) => result.success), 'context bookkeeping is not an external action');

    await browser.refresh();
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').waitForDisplayed({ timeout: 10_000 });
    assert.ok((await browser.$('.chat-area').getText()).includes(partial.content.split('\n\n')[0]));
    assert.ok((await browser.$('.chat-area').getText()).includes('回复达到模型输出长度限制'));
    const reloaded = (await readImageJourneyMessages()).find((message) => message.id === partial.id);
    assert.equal(reloaded.content, partial.content);
    assert.equal(reloaded.taskRun.resumable, true);
    assert.equal(reloaded.taskRun.status, partial.taskRun.status);
    await browser.saveScreenshot(join(screenshotDir, 'desktop-partial-model-reply.png'));

    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('先核对现有结果，只补充说明，不重复操作。');
    await browser.$('button[title="发送"]').click();
    const followupText = '已核对现有回复；本轮只补充说明，没有重复任何操作。';
    await waitForVisible(browser, `//*[contains(text(), "${followupText}")]`, { timeout: 20_000 });
    const followup = (await readImageJourneyMessages()).filter((message) => message.role === 'assistant').at(-1);
    assert.equal(followup.content, followupText);
    assert.equal(followup.taskRun?.status, 'completed');
    assert.ok((followup.toolResults ?? []).every((result) =>
      result.toolName === 'context_compaction' && result.success), 'followup must not repeat external actions');
    await waitForOwnedCheckboxState(false, 1, 1);
  });
});
