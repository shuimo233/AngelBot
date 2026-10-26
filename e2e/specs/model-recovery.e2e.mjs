import assert from 'node:assert/strict';
import { existsSync, writeFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';

describe('native main conversation recovery', () => {
  let report;
  let reportPath;
  afterEach(function () {
    if (!reportPath) return;
    report.status = this.currentTest.state === 'passed' ? 'passed' : 'failed';
    writeFileSync(reportPath, JSON.stringify(report, null, 2));
  });

  it('recovers through ordinary messages and retains history after process restart', async () => {
    const phase = process.env.ANGELBOT_DESKTOP_E2E_MODEL_RECOVERY;
    const evidence = process.env.ANGELBOT_DESKTOP_E2E_RECOVERY_EVIDENCE_DIR;
    assert.ok(['first', 'reopen'].includes(phase));
    assert.ok(evidence && isAbsolute(evidence));
    reportPath = join(evidence, `${phase}.json`);
    report = { status: 'running', phase };
    writeFileSync(reportPath, JSON.stringify(report));
    const workspaces = await browser.tauri.execute(({ core }) => core.invoke('get_workspaces'));
    assert.equal(workspaces.length, 1);
    assert.equal(workspaces[0].kind, 'personal');
    const sessionId = workspaces[0].activeSessionId;
    const messages = () => browser.tauri.execute(({ core }, sessionId) =>
      core.invoke('get_messages', { sessionId }), sessionId);
    const send = async (text) => {
      const composer = await browser.$('textarea[placeholder="给 AngelBot 发消息…"]');
      await composer.waitForDisplayed({ timeout: 20_000 });
      await composer.waitForEnabled({ timeout: 10_000 });
      await composer.setValue(text);
      const button = await browser.$('button[title="发送"]');
      await button.waitForEnabled({ timeout: 10_000 });
      await button.click();
    };
    const checkHistory = async (expectedCount) => {
      const rows = await messages();
      assert.equal(rows.length, expectedCount);
      assert.ok(rows[0].content.includes('native-recovery-initial'));
      const failed = rows[1];
      assert.match(failed.content, /模型请求未完成/);
      assert.match(failed.content, /新消息/);
      assert.equal(failed.taskRun?.resumable, false);
      assert.equal(failed.taskRun?.status, 'provider_unavailable');
      const results = rows.flatMap(row => row.toolResults ?? []);
      assert.equal(results.filter(result => result.callId === 'native-recovery-list' && result.success).length, 1);
      assert.doesNotMatch(JSON.stringify(rows), /desktop E2E user-data assertion failed|desktop-e2e index=/);
      assert.doesNotMatch(await browser.$('.messages').getText(), /desktop E2E|desktop-e2e index=/);
      return rows;
    };

    if (phase === 'first') {
      assert.deepEqual(await messages(), []);
      await send('native-recovery-initial：只读查询当前自动化，不创建或运行任务。');
      await browser.waitUntil(async () => (await messages()).some(row =>
        row.role === 'assistant' && row.content.includes('模型请求未完成')), {
        timeout: 60_000, interval: 200, timeoutMsg: 'Model failure did not reach a durable safe reply',
      });
      await checkHistory(2);
      await browser.saveScreenshot(join(evidence, 'failure.png'));
      await send('native-recovery-next：保留此前结果，继续回答，不再查询或执行操作。');
    } else {
      const before = await checkHistory(8);
      assert.match(before[3].content, /native-recovery-ok/);
      assert.equal(before[5].taskRun?.status, 'stopped');
      assert.match(before[7].content, /native-stop-next-ok/);
      assert.ok((await browser.$('.messages').getText()).includes('native-recovery-ok'));
      report.historyRetained = true;
      await send('native-recovery-reopen：请结合之前的两条消息回答，不调用工具。');
    }
    const marker = phase === 'first' ? 'native-recovery-ok' : 'native-recovery-reopened';
    await browser.waitUntil(async () => (await messages()).some(row =>
      row.role === 'assistant' && row.content.includes(marker)), {
      timeout: 30_000, interval: 200, timeoutMsg: 'Ordinary message did not complete after failure/reopen',
    });
    let rows = await checkHistory(phase === 'first' ? 4 : 10);
    assert.equal(rows.at(-1).taskRun?.status, 'completed');
    await browser.waitUntil(async () => (await browser.$('.messages').getText()).includes(marker), { timeout: 10_000 });
    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').waitForEnabled({ timeout: 10_000 });
    assert.equal(await browser.$('.message-bubble--content-stream').isExisting(), false);
    if (phase === 'first') {
      await send('native-stop-initial：进入隔离等待，不调用工具，等我停止。');
      await browser.waitUntil(async () => (await browser.$('.messages').getText()).includes('native-model-waiting'), {
        timeout: 20_000, interval: 100, timeoutMsg: 'Owned model did not enter its pending streaming future',
      });
      await browser.tauri.execute(async ({ core }, sessionId) => {
        await core.invoke('pause_agent', { sessionId });
        if (await core.invoke('get_agent_state', { sessionId }) !== 'Paused') throw new Error('Pause state not shared');
        await core.invoke('resume_agent', { sessionId });
        if (await core.invoke('get_agent_state', { sessionId }) !== 'Running') throw new Error('Resume state not shared');
      }, sessionId);
      const stoppedAt = Date.now();
      await browser.$('button[title="停止运行"]').click();
      await browser.waitUntil(async () => (await messages()).some(row => row.taskRun?.status === 'stopped'), {
        timeout: 4_000, interval: 100, timeoutMsg: 'Native Stop did not finish pending inference promptly',
      });
      const stopped = (await messages()).at(-1);
      assert.equal(stopped.taskRun.status, 'stopped');
      assert.equal(stopped.taskRun.resumable, false);
      assert.equal(stopped.taskRun.stepCount, 0);
      assert.match(stopped.content, /本次运行已停止/);
      assert.equal(stopped.toolResults?.length ?? 0, 0);
      assert.doesNotMatch(stopped.content, /native-model-waiting|模型请求未完成|agent_loop/);
      report.stopMilliseconds = Date.now() - stoppedAt;
      await send('native-stop-next：这是停止后的独立新任务，不调用工具。');
      await browser.waitUntil(async () => (await messages()).at(-1)?.content.includes('native-stop-next-ok'), {
        timeout: 20_000, interval: 100, timeoutMsg: 'Stop did not release the Main session for a new task',
      });
      rows = await checkHistory(8);
      assert.equal(rows.at(-1).taskRun?.status, 'completed');
      report.nativeStopButton = true;
    }
    await browser.saveScreenshot(join(evidence, `${phase}.png`));
    Object.assign(report, { ordinaryMessageCompleted: true, messageCount: rows.length, readOnlyReceiptCount: 1 });
    if (process.env.ANGELBOT_DESKTOP_E2E_RECOVERY_INSPECT === '1') {
      writeFileSync(join(evidence, 'inspection.json'), JSON.stringify({ status: 'ready', phase }));
      console.log(`Read-only native inspection ready: ${evidence}`);
      await browser.waitUntil(() => existsSync(join(evidence, 'inspection-done')), {
        timeout: 120_000, interval: 500, timeoutMsg: 'Native inspection was not released within 120 seconds',
      });
    }
  });
});
