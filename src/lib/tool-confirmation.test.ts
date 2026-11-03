import { describe, expect, it } from 'vitest';
import { desktopActionOperation, desktopConfirmationOperation, fileWriteContentForPreview, getToolConfirmationSummary, mcpArgumentsForPreview, requiresDesktopActionPreview, requiresDesktopConfirmationPreview } from './tool-confirmation';
import { getToolDisplayName } from './tool-display';

describe('tool confirmation summaries', () => {
  it('discloses the unattended MAA scope before approval without exposing unrelated arguments', () => {
    const summary = getToolConfirmationSummary('create_automation', {
      title: '游戏日常', schedule: '每天 18:00', executorKind: 'maa', appId: 'maa-1',
      launchEmulator: true, timeoutSeconds: 1800, prompt: 'PRIVATE_PROMPT',
      scriptPath: 'PRIVATE_PATH', permissionSummary: 'FORGED_SAFE_SCOPE',
    });
    expect(summary).toContain('MAA');
    expect(summary).toContain('每天 18:00');
    expect(summary).toContain('maa-1');
    expect(summary).toContain('启动已配置的模拟器');
    expect(summary).toContain('1800 秒');
    expect(summary).toContain('以后按计划无人值守运行');
    expect(summary).toContain('当前 GUI 勾选队列');
    expect(summary).toContain('按现有消耗设置执行');
    expect(summary).toContain('仅回收本次启动的进程');
    expect(summary).toContain('不会修改游戏任务或消耗设置');
    expect(summary).not.toMatch(/PRIVATE|FORGED/);
  });

  it('makes MAA defaults explicit without treating malformed options as approval', () => {
    const summary = getToolConfirmationSummary('create_automation', { executorKind: 'maa' });
    expect(summary).toContain('不启动模拟器');
    expect(summary).toContain('3600 秒');
    expect(summary).toContain('应用待校验');
    const malformed = getToolConfirmationSummary('create_automation', {
      executorKind: 'maa', launchEmulator: 'true', timeoutSeconds: -1,
    });
    expect(malformed).toContain('启动模拟器参数无效');
    expect(malformed).toContain('超时参数无效');
    expect(malformed).not.toContain('3600 秒');
    expect(getToolConfirmationSummary('create_automation', {
      title: '整理', schedule: '每天 18:00', prompt: 'PRIVATE_PROMPT',
    })).toBe('创建自动化：整理（每天 18:00）');
  });

  it('explains schedule-only pause without trusting model-supplied labels or leaking arguments', () => {
    const summary = getToolConfirmationSummary('pause_automation', {
      automation_id: 'private-id', title: 'Forged safe title', output: 'private output',
    });
    expect(summary).toContain('后续定时');
    expect(summary).toContain('不会停止已开始的执行');
    expect(summary).toContain('不删除运行记录');
    expect(summary).not.toMatch(/private|Forged/);
    expect(getToolDisplayName('pause_automation')).toBe('暂停后续定时');
    expect(getToolDisplayName('list_automations')).toBe('查看自动化结果');
    const stop = getToolConfirmationSummary('stop_automation_run', {
      automation_id: 'private-id', run_id: 'private-run', title: 'Forged title', output: 'private output',
    });
    expect(stop).toContain('停止当前工作区中这次受控执行');
    expect(stop).toContain('不会撤回');
    expect(stop).toContain('等待清理结果');
    expect(stop).not.toMatch(/private|Forged/);
    expect(getToolDisplayName('stop_automation_run')).toBe('停止本次自动化');
  });
  it('routes native text writes and invocations through the attested action seam', () => {
    expect(desktopActionOperation('prepare_message_draft')).toBe('draft');
    expect(desktopActionOperation('set_trusted_app_text')).toBe('field');
    expect(desktopActionOperation('invoke_trusted_app_control')).toBeNull();
    expect(desktopActionOperation('open_trusted_app')).toBeNull();
  });

  it.each(['invoke', 'select', 'choose', 'expand', 'collapse', 'scrollup', 'scrolldown', 'check', 'uncheck'])('routes only an explicit control action to preflight: %s', (action) => {
    expect(desktopActionOperation('operate_trusted_app_control', { action })).toBe(action);
    expect(desktopActionOperation('operate_trusted_app_control', JSON.stringify({ action }))).toBe(action);
  });

  it.each([undefined, 'unknown', 'toggle', 'Check', 'check ', 'Invoke', 'ScrollUp', 'scrollDown', 'scroll_up', 'scrollup ', '', null, [], { action: 'invoke' }])('blocks malformed control actions without a generic approval fallback: %j', (action) => {
    expect(desktopActionOperation('operate_trusted_app_control', { action })).toBeNull();
    expect(requiresDesktopActionPreview('operate_trusted_app_control')).toBe(true);
  });

  it('blocks legacy pending invocation without assigning it an inspectable operation', () => {
    expect(requiresDesktopActionPreview('invoke_trusted_app_control')).toBe(true);
    expect(desktopActionOperation('invoke_trusted_app_control', { action: 'invoke' })).toBeNull();
    expect(getToolConfirmationSummary('invoke_trusted_app_control', {})).toContain('已停用，不能批准');
  });

  it.each(['invoke', 'select', 'choose', 'expand', 'collapse', 'scrollup', 'scrolldown', 'check', 'uncheck'])('discloses %s risk without trusting or displaying model control arguments', (action) => {
    const summary = getToolConfirmationSummary('operate_trusted_app_control', {
      app_id: 'private-app', control_ref: 'private-ref', action, name: 'Safe button',
    });
    expect(summary).toContain('发送、删除');
    expect(summary).toContain('控件名称不代表安全或授权');
    expect(summary).toContain('请求已发出或控件状态已校验不代表目标已完成');
    expect(summary).not.toContain('private-app');
    expect(summary).not.toContain('private-ref');
    expect(summary).not.toContain('Safe button');
  });

  it.each([
    { action: 'choose', label: '选择下拉项' },
    { action: 'scrollup', label: '向上小幅滚动' },
    { action: 'scrolldown', label: '向下小幅滚动' },
    { action: 'check', label: '勾选复选框' },
    { action: 'uncheck', label: '取消勾选复选框' },
  ])('names the fixed small $action action without exposing its model target', ({ action, label }) => {
    const summary = getToolConfirmationSummary('operate_trusted_app_control', {
      action, app_id: 'private-app', control_ref: 'private-ref', name: 'Forged document',
    });
    expect(summary).toContain(label);
    expect(summary).not.toContain('private-app');
    expect(summary).not.toContain('private-ref');
    expect(summary).not.toContain('Forged document');
  });

  it('names a file target without exposing file contents', () => {
    const summary = getToolConfirmationSummary('write_file', JSON.stringify({
      path: 'notes/today.md',
      content: 'private draft',
    }));

    expect(summary).toBe('写入项目文件：notes/today.md');
    expect(summary).not.toContain('private draft');
    expect(fileWriteContentForPreview('{"content":"  private draft\\n"}')).toBe('  private draft\n');
    expect(fileWriteContentForPreview({ content: '' })).toBe('');
    for (const invalid of [undefined, null, [], '{}', '{invalid', { content: 1 }, '{"content":null}']) {
      expect(fileWriteContentForPreview(invalid)).toBeNull();
    }
  });

  it('makes the no-send desktop boundary explicit', () => {
    const summary = getToolConfirmationSummary('prepare_message_draft', JSON.stringify({
      app_id: 'wechat',
      text: 'private message',
    }));

    expect(summary).toBe('在已信任应用中写入草稿。目标窗口和完整内容以实时预检为准；AngelBot 不点击发送，应用可能自动保存或同步。');
    expect(summary).not.toContain('private message');
    expect(summary).not.toContain('wechat');
  });

  it('discloses transient single-window model disclosure without trusting model summaries', () => {
    const summary = getToolConfirmationSummary('observe_trusted_app_window', {
      app_id: 'private-app', mode: 'image', confirmed: true,
      title: 'PRIVATE_WINDOW_CANARY', data: 'RAW_IMAGE_CANARY',
    });
    expect(summary).toContain('已授权应用的单个窗口图像');
    expect(summary).toContain('当前配置的模型');
    expect(summary).toContain('私人内容');
    expect(summary).toContain('不存储截图');
    expect(summary).toContain('不授予点击、填写或其他控件操作权限');
    expect(summary).not.toContain('private-app');
    expect(summary).not.toContain('PRIVATE_WINDOW_CANARY');
    expect(summary).not.toContain('RAW_IMAGE_CANARY');
    expect(requiresDesktopActionPreview('observe_trusted_app_window')).toBe(false);
    expect(desktopActionOperation('observe_trusted_app_window', { mode: 'image' })).toBeNull();
    expect(requiresDesktopConfirmationPreview('observe_trusted_app_window', { mode: 'image' })).toBe(true);
    expect(desktopConfirmationOperation('observe_trusted_app_window', { mode: 'image' })).toBe('image');
    expect(desktopConfirmationOperation('observe_trusted_app_window', '{"mode":"image"}')).toBe('image');
    // Routing is not a second schema validator: durable backend preflight
    // rejects invalid scopes/extra fields before any consent becomes usable.
    expect(desktopConfirmationOperation('observe_trusted_app_window', { mode: 'image', region_ref: 'invalid' })).toBe('image');
    for (const invalid of ['{', 'null', '[]', null, { mode: 'Image' }, { mode: '' }, { mode: null }]) {
      expect(requiresDesktopConfirmationPreview('observe_trusted_app_window', invalid)).toBe(true);
      expect(desktopConfirmationOperation('observe_trusted_app_window', invalid)).toBeNull();
    }
  });

  it.each([{}, { mode: 'controls' }])('keeps default structural observation distinct from image capture: %j', (args) => {
    const summary = getToolConfirmationSummary('observe_trusted_app_window', args);
    expect(summary).toBe('查看已授权应用的窗口控件结构，不截取或发送窗口图像。');
    expect(requiresDesktopConfirmationPreview('observe_trusted_app_window', args)).toBe(false);
    expect(requiresDesktopConfirmationPreview('observe_trusted_app_window', JSON.stringify(args))).toBe(false);
    expect(desktopConfirmationOperation('observe_trusted_app_window', args)).toBeNull();
  });

  it('keeps generic field-write arguments out of summaries', () => {
    const summary = getToolConfirmationSummary('set_trusted_app_text', {
      app_id: 'private-app', field_ref: 'private-ref', text: 'secret field text',
    });
    expect(summary).toContain('修改或清空一个输入框');
    expect(summary).not.toContain('private-app');
    expect(summary).not.toContain('private-ref');
    expect(summary).not.toContain('secret field text');
  });

  it('does not expose arbitrary MCP arguments', () => {
    const summary = getToolConfirmationSummary(
      'mcp_calendar__create_event_a1b2c3d4',
      JSON.stringify({ token: 'secret', body: 'sensitive' }),
    );

    expect(summary).toBe('调用当前工作区已启用的外部服务一次');
    expect(summary).not.toContain('secret');
    const preview = mcpArgumentsForPreview(JSON.stringify({ url: 'https://example.com/', nested: { apiKey: 'private-key' }, text: 'task text' }));
    expect(preview).toContain('https://example.com/');
    expect(preview).toContain('task text');
    expect(preview).toContain('[已隐藏]');
    expect(preview).not.toContain('private-key');
    expect(mcpArgumentsForPreview('{broken')).toBeNull();
    expect(mcpArgumentsForPreview([])).toBeNull();
    expect(mcpArgumentsForPreview({ text: 'x'.repeat(5_000) })).toContain('以上不是完整参数');
    expect(mcpArgumentsForPreview({ text: 'x'.repeat(5_000) })!.length).toBeLessThan(4_100);
  });

  it('makes a Web Search boundary explicit without echoing the query', () => {
    const summary = getToolConfirmationSummary(
      'web_search',
      JSON.stringify({ query: 'private medical question' }),
    );

    expect(summary).toBe('向已配置的搜索服务发送一次公开网页查询');
    expect(summary).not.toContain('private medical question');
  });

  it('summarizes a delegated network scope without exposing queries or URL paths', () => {
    const summary = getToolConfirmationSummary(
      'delegate_network_exploration',
      JSON.stringify({
        goal: 'private task goal',
        explorer_operations: [
          { kind: 'search', provider_host: 'search.example.com', query: 'private search terms' },
          { kind: 'fetch', url: 'https://docs.example.com/private/path?token=secret-value', method: 'GET' },
          { kind: 'search', provider_host: 'search.example.com', query: 'another private query' },
          { kind: 'fetch', url: 'https://docs.example.com/another/path?credential=do-not-show', method: 'HEAD' },
        ],
      }),
    );

    expect(summary).toBe(
      '委派受限联网探索：站点 search.example.com、docs.example.com；操作 搜索、抓取；上限为最多 12 次网络操作、单次响应 512 KiB、最多 3 次重定向',
    );
    expect(summary).not.toContain('private task goal');
    expect(summary).not.toContain('private search terms');
    expect(summary).not.toContain('https://docs.example.com');
    expect(summary).not.toContain('/private/path');
    expect(summary).not.toContain('secret-value');
  });

  it('does not derive a displayed hostname from a malformed provider host', () => {
    const summary = getToolConfirmationSummary(
      'delegate_network_exploration',
      JSON.stringify({
        explorer_operations: [{
          kind: 'search',
          provider_host: 'https://leak.example/private?query=must-not-display',
          query: 'private',
        }],
      }),
    );

    expect(summary).toContain('站点将由系统验证');
    expect(summary).not.toContain('leak.example');
    expect(summary).not.toContain('must-not-display');
  });
});
