import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';

// Opt-in native acceptance. A person or supported Windows Computer Use controls
// the real picker; this spec never supplies a destination to application IPC.
describe('native reply export', () => {
  let report;
  let reportPath;
  let exportDirectory;
  afterEach(function () {
    if (!report) return;
    if (this.currentTest.state !== 'passed') report.status = 'failed';
    writeFileSync(reportPath, JSON.stringify(report, null, 2));
    if (report.status === 'failed' && exportDirectory && isAbsolute(exportDirectory) && existsSync(exportDirectory)) {
      writeFileSync(join(exportDirectory, 'stage.json'), JSON.stringify({ phase: 'failed', failedPhase: report.phase }));
    }
  });
  it('cancels and saves exact reply snapshots without changing the conversation', async () => {
    const evidence = join(process.cwd(), 'src-tauri', 'target', 'e2e');
    mkdirSync(evidence, { recursive: true });
    reportPath = join(evidence, 'reply-export-desktop.json');
    report = { status: 'running', phase: 'startup', nativeCancellation: false,
      markdownExact: false, textExact: false, transcriptUnchanged: false,
      dailyPreviewExact: false, dailyNativeCancellation: false, binaryExportExact: false };
    // Invalidate an older pass before touching the app. A failed/aborted run
    // must not leave a stale success report that looks like current evidence.
    writeFileSync(reportPath, JSON.stringify(report, null, 2));
    const directory = process.env.ANGELBOT_DESKTOP_E2E_EXPORT_DIR;
    exportDirectory = directory;
    assert.ok(directory && isAbsolute(directory), 'owned export directory is required');
    const stageFile = join(directory, 'stage.json');
    const workspaces = await browser.tauri.execute(({ core }) => core.invoke('get_workspaces'));
    assert.equal(workspaces.length, 1);
    assert.equal(workspaces[0].kind, 'personal');
    const sessionId = workspaces[0].activeSessionId;
    const composer = await browser.$('textarea[placeholder="给 AngelBot 发消息…"]');
    await composer.waitForDisplayed({ timeout: 20_000 });
    await composer.setValue('生成一份用于本机保存回复验收的纯文字清单。');
    await browser.$('button[title="发送"]').click();
    const save = await browser.$('button[title="保存为 Markdown 或文本新文件，不覆盖已有文件"]');
    await save.waitForDisplayed({ timeout: 20_000 });
    const before = await browser.tauri.execute(({ core }, sessionId) => core.invoke('get_messages', { sessionId }), sessionId);
    assert.deepEqual(before.map(row => row.role), ['user', 'assistant']);
    const reply = before[1];
    assert.match(reply.content, /保存回复验收/);
    const bytes = Buffer.from(reply.content, 'utf8');
    const checkpoint = (phase, name) => {
      report.phase = phase;
      writeFileSync(reportPath, JSON.stringify(report, null, 2));
      writeFileSync(stageFile, JSON.stringify({ phase, outputPath: name ? join(directory, name) : null }));
      console.log(`Native export: ${phase}`);
    };
    const settle = async () => {
      await browser.waitUntil(() => save.isEnabled(), { timeout: 120_000, timeoutMsg: 'Complete or cancel the owned native save dialog' });
    };

    checkpoint('cancel');
    await save.click();
    await settle();
    assert.deepEqual(readdirSync(directory), ['stage.json'], 'cancel must not create a file');
    assert.equal(await browser.$('.message-actions [role="status"]').isExisting(), false);
    assert.equal(await browser.$('.message-actions [role="alert"]').isExisting(), false);
    report.nativeCancellation = true;

    for (const [phase, name] of [['save_markdown', '验收清单.md'], ['save_text', 'reply.txt']]) {
      const output = join(directory, name);
      assert.equal(existsSync(output), false);
      checkpoint(phase, name);
      await save.click();
      await settle();
      assert.ok(readFileSync(output).equals(bytes), 'saved bytes must match the durable reply exactly');
      assert.equal(await browser.$('.message-actions [role="status"]').getText(), `已保存回复快照：${name}`);
      assert.equal(await browser.$('.message-actions [role="alert"]').isExisting(), false);
      report[phase === 'save_markdown' ? 'markdownExact' : 'textExact'] = true;
    }
    assert.deepEqual(readdirSync(directory).sort(), ['reply.txt', 'stage.json', '验收清单.md'].sort());
    // Files are seeded synthetic inputs, not evidence that the scripted model
    // generated/downloaded them. This part checks the actual Daily UI + picker.
    checkpoint('daily_preview');
    await browser.$('button[title="在工作台中定位 验收结果.csv"]').click();
    const panel = await browser.$('.right-panel');
    await panel.waitForDisplayed({ timeout: 10_000 });
    assert.match(await panel.getText(), /日常结果/);
    const preview = await browser.$('.file-preview-content');
    await preview.waitForDisplayed({ timeout: 10_000 });
    assert.equal(await preview.getText(), '事项,状态\n本机验收,完成\n');
    report.dailyPreviewExact = true;
    await browser.$('button[aria-label="打开 download.bin"]').click();
    const binarySave = await browser.$('button[aria-label="另存 download.bin"]');
    await binarySave.waitForDisplayed({ timeout: 10_000 });
    checkpoint('cancel_daily');
    await binarySave.click();
    await browser.waitUntil(() => binarySave.isEnabled(), { timeout: 120_000 });
    assert.equal(existsSync(join(directory, 'download.bin')), false);
    assert.equal(await browser.$('.right-panel [role="status"]').isExisting(), false);
    report.dailyNativeCancellation = true;
    checkpoint('save_binary', 'download.bin');
    await binarySave.click();
    await browser.waitUntil(() => binarySave.isEnabled(), { timeout: 120_000 });
    assert.ok(readFileSync(join(directory, 'download.bin')).equals(Buffer.from([0, 255, 1, 128])));
    assert.equal(await browser.$('.right-panel [role="status"]').getText(), '已另存 download.bin（4 字节），原文件保留。');
    report.binaryExportExact = true;
    const fileRoot = await browser.tauri.execute(({ core }, sessionId) => core.invoke('get_work_dir', { sessionId }), sessionId);
    assert.ok(readFileSync(join(fileRoot, 'download.bin')).equals(Buffer.from([0, 255, 1, 128])));
    assert.ok(readFileSync(join(fileRoot, '验收结果.csv')).equals(Buffer.from('事项,状态\n本机验收,完成\n')));
    assert.deepEqual(readdirSync(directory).sort(), ['download.bin', 'reply.txt', 'stage.json', '验收清单.md'].sort());
    await browser.saveScreenshot(join(evidence, 'reply-export-saved.png'));
    await browser.refresh();
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').waitForDisplayed({ timeout: 20_000 });
    const after = await browser.tauri.execute(({ core }, sessionId) => core.invoke('get_messages', { sessionId }), sessionId);
    assert.deepEqual(after, before, 'export must not mutate transcript or tool state');
    assert.equal(await browser.$('.message-actions [role="status"]').isExisting(), false);
    Object.assign(report, { status: 'passed', transcriptUnchanged: true, bytes: bytes.length, files: ['验收清单.md', 'reply.txt', 'download.bin'] });
    checkpoint('done');
  });
});
