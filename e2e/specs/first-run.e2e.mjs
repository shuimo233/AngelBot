import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const projectRoot = mkdtempSync(join(tmpdir(), 'angelbot-e2e-project-'));
writeFileSync(join(projectRoot, 'README.md'), '# Desktop E2E Project\n\nThis file verifies the project workbench preview.\n');
const notepadPath = join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'notepad.exe');
const mcpFixturePath = fileURLToPath(new URL('../fixtures/mcp-stdio.cjs', import.meta.url));

after(() => {
  rmSync(projectRoot, { recursive: true, force: true });
});

describe('first-run desktop experience', () => {
  it('guides the first task without blocking or sending on the user\'s behalf', async () => {
    const location = await browser.tauri.execute(() => window.location.href);
    assert.match(location, /^https?:\/\/tauri\.localhost\//);
    const screenshotDir = join(process.cwd(), 'src-tauri', 'target', 'e2e');
    mkdirSync(screenshotDir, { recursive: true });

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
    await composer.setValue('帮我设置一个本地提醒，稍后核对今天的安排。');
    await sendButton.waitForEnabled({ timeout: 5_000 });
    await sendButton.click();
    await browser.$('//*[contains(text(), "本地提醒已设置，你可以在当前对话中回看")]').waitForDisplayed({ timeout: 20_000 });
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
    assert.equal(reminderRuns[0].status, 'completed');
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
      async () => (await mcpItem.$('.status-text').getText()) === '未连接',
      { timeout: 5_000, timeoutMsg: 'MCP status did not return to stopped' },
    );
    const stoppedMcp = await browser.tauri.execute(({ core }) => core.invoke('get_mcp_server_status', {
      serverId: window.__angelbotE2eMcpId,
    }));
    assert.equal(stoppedMcp.status, 'stopped');
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

    const approvedFileLocator = await browser.$('[aria-label="在工作台中定位 approved-by-e2e.txt"]');
    await approvedFileLocator.waitForDisplayed({ timeout: 20_000 });
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
    await browser.$('button=自动化').click();
    const automationPage = await browser.$('.automation-page');
    await automationPage.waitForDisplayed({ timeout: 10_000 });
    assert.match(await automationPage.getText(), /每日邮件行动清单/);
    await browser.saveScreenshot(join(screenshotDir, 'automation-workspace.png'));

    await browser.$('button=资料库').click();
    const libraryPage = await browser.$('.library-page');
    await libraryPage.waitForDisplayed({ timeout: 10_000 });
    assert.match(await libraryPage.getText(), /资料、记忆和已添加的技能/);
    await libraryPage.$('button=记忆').click();
    await libraryPage.$('.memory-page').waitForDisplayed({ timeout: 10_000 });
    await browser.saveScreenshot(join(screenshotDir, 'library-memory.png'));

    await browser.$('button=对话').click();
    await browser.$('.chat-area').waitForDisplayed({ timeout: 10_000 });
    const workbenchButton = await browser.$('[aria-label="打开工作台"]');
    await workbenchButton.waitForDisplayed({ timeout: 20_000 });
    const workbenchAlreadyOpen = await browser.$('.right-panel').isDisplayed();
    if (!workbenchAlreadyOpen) {
      await browser.execute(() => document.querySelector('[aria-label="打开工作台"]')?.click());
    }
    await browser.pause(250);
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
    // survive replay/edit without granting Personal a filesystem root. A real
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
    await browser.$('//*[contains(text(), "已读取你选择的日常行动清单")]').waitForDisplayed({ timeout: 20_000 });
    const personalMessages = await browser.tauri.execute(async ({ core }) => {
      const personal = (await core.invoke('get_workspaces')).find((item) => item.kind === 'personal');
      if (!personal?.activeSessionId) throw new Error('Personal Main conversation identity missing');
      return core.invoke('get_messages', { sessionId: personal.activeSessionId });
    });
    const attachmentMessage = personalMessages.find((item) => item.role === 'user' && item.textAttachments?.[0]?.name === 'daily-actions.csv');
    assert.ok(attachmentMessage, 'explicit file-only turn was not persisted');
    assert.equal(attachmentMessage.content, '', 'file snapshot must not be flattened into the visible prompt');
    assert.deepEqual(attachmentMessage.textAttachments, [{ name: 'daily-actions.csv', text: attachmentText }]);
    await browser.saveScreenshot(join(screenshotDir, 'daily-file-snapshot.png'));

    await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').setValue('继续整理上一份清单。');
    await browser.$('button[title="发送"]').waitForEnabled({ timeout: 5_000 });
    await browser.$('button[title="发送"]').click();
    await browser.$('//*[contains(text(), "上一轮的文件快照仍在当前日常会话中")]').waitForDisplayed({ timeout: 20_000 });
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
    await browser.$('//*[contains(text(), "已按编辑后的要求重新整理")]').waitForDisplayed({ timeout: 20_000 });
    const editedMessages = await browser.tauri.execute(async ({ core }) => {
      const personal = (await core.invoke('get_workspaces')).find((item) => item.kind === 'personal');
      if (!personal?.activeSessionId) throw new Error('Personal Main conversation identity missing');
      return core.invoke('get_messages', { sessionId: personal.activeSessionId });
    });
    const editedAttachmentMessage = editedMessages.find((item) => item.id === attachmentMessage.id);
    assert.equal(editedAttachmentMessage?.content, '按优先级重新整理这份附件。');
    assert.deepEqual(editedAttachmentMessage?.textAttachments, attachmentMessage.textAttachments);
    assert.ok(!editedMessages.some((item) => item.content === '继续整理上一份清单。'), 'resend must replace the old future branch');
  });
});
