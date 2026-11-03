import { describe, expect, it } from 'vitest';
import { toolActionReceipt, isResultUnknown } from './tool-action-result';

describe('tool action result semantics', () => {
  it('detects a structured unknown result without mistaking ordinary failures for it', () => {
    expect(isResultUnknown({ success: false, output: 'Error: {"code":"RESULT_UNKNOWN"}' })).toBe(true);
    expect(isResultUnknown({ success: false, output: 'Error: {"code":"TARGET_CHANGED"}' })).toBe(false);
    expect(isResultUnknown({ success: true, output: '{"code":"RESULT_UNKNOWN"}' })).toBe(false);
    expect(toolActionReceipt('mcp_browser__browser_click_a1b2c3d4', {
      success: false, output: 'Error: {"code":"RESULT_UNKNOWN","message":"untrusted details"}',
    })).toMatchObject({ status: 'result_unknown', label: '结果未知' });
    expect(toolActionReceipt('mcp_browser__browser_click_a1b2c3d4', {
      success: true, output: '{"status":"verified"}',
    })).toBeNull();
  });
  it('keeps dispatch separate from verified completion', () => {
    expect(toolActionReceipt('open_trusted_app', {
      success: true,
      output: JSON.stringify({ status: 'dispatched', target: 'notes' }),
    })).toMatchObject({ status: 'dispatched', label: '已发出请求' });
    expect(toolActionReceipt('prepare_message_draft', {
      success: true,
      output: JSON.stringify({ status: 'verified', target: 'notes' }),
    })).toMatchObject({ status: 'verified', label: '已校验' });
    expect(toolActionReceipt('open_trusted_app', {
      success: true,
      output: 'Confirmation approved: {"status":"dispatched","target":"notes"}',
    })).toMatchObject({ status: 'dispatched', label: '已发出请求' });
    expect(toolActionReceipt('prepare_message_draft', {
      success: true,
      output: 'Confirmation approved: {"status":"verified","target":"notes"}',
    })).toMatchObject({ status: 'verified', label: '已校验' });
    expect(toolActionReceipt('set_trusted_app_text', {
      success: true,
      output: 'Confirmation approved: {"status":"verified","target":"notes"}',
    })).toMatchObject({ status: 'verified', explanation: '目标输入框已按请求修改并回读校验；应用可能自动保存或同步。这不代表任务完成。' });
  });

  it('presents invocation as dispatch requiring observation, never verified goal completion', () => {
    expect(toolActionReceipt('operate_trusted_app_control', {
      success: true, output: 'Confirmation approved: {"status":"dispatched","action":"invoke"}',
    })).toMatchObject({
      status: 'dispatched', label: '已发出请求',
      explanation: '操作已发出，需重新观察目标确认结果。',
    });
    expect(toolActionReceipt('operate_trusted_app_control', {
      success: true, output: '{"status":"verified","action":"invoke"}',
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
    expect(toolActionReceipt('operate_trusted_app_control', {
      success: true, output: 'legacy success',
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
  });

  it('keeps an approved visual click at dispatch and refuses to certify business completion', () => {
    expect(toolActionReceipt('click_trusted_app_image', {
      success: true, output: '{"status":"dispatched"}',
    })).toMatchObject({ status: 'dispatched', explanation: '操作已发出，需重新观察目标确认结果。' });
    expect(toolActionReceipt('click_trusted_app_image', {
      success: true, output: '{"status":"verified"}',
    })).toMatchObject({ status: 'unconfirmed' });
    expect(toolActionReceipt('click_trusted_app_image', {
      success: false, output: 'Error: {"code":"RESULT_UNKNOWN"}',
    })).toMatchObject({ status: 'result_unknown', explanation: expect.stringContaining('勿自动重试') });
  });

  it.each([
    { action: 'select', state: '选中' }, { action: 'expand', state: '展开' }, { action: 'collapse', state: '收起' },
    { action: 'check', state: '勾选' }, { action: 'uncheck', state: '取消勾选' },
  ])('limits a verified $action receipt to the control state', ({ action, state }) => {
    expect(toolActionReceipt('operate_trusted_app_control', {
      success: true, output: `Confirmation approved: ${JSON.stringify({ status: 'verified', action })}`,
    })).toEqual({
      status: 'verified', label: '控件状态已确认',
      explanation: `已确认控件${state}状态，仍需检查任务结果。`,
    });
  });

  it('limits bounded dropdown verification to selection and collapse, not business completion', () => {
    expect(toolActionReceipt('operate_trusted_app_control', {
      success: true, output: 'Confirmation approved: {"status":"verified","action":"choose","choice":{"controlName":"Fixture choices","option":"Combo option two","expanded":false}}',
    })).toEqual({
      status: 'verified', label: '控件状态已确认',
      explanation: '已确认“Fixture choices”已选择“Combo option two”并收起；仍需检查任务结果，不代表业务已完成。',
    });
    for (const choice of [undefined, null, [], {}, { controlName: 'Fixture choices', option: 'Combo option two', expanded: true }]) {
      expect(toolActionReceipt('operate_trusted_app_control', {
        success: true, output: JSON.stringify({ status: 'verified', action: 'choose', choice }),
      })).toMatchObject({ status: 'unconfirmed' });
    }
  });

  it.each([
    { action: 'scrollup', direction: '向上' },
    { action: 'scrolldown', direction: '向下' },
  ])('limits verified $action receipts to scroll state and requires fresh observation', ({ action, direction }) => {
    expect(toolActionReceipt('operate_trusted_app_control', {
      success: true, output: `Confirmation approved: ${JSON.stringify({ status: 'verified', action })}`,
    })).toEqual({
      status: 'verified', label: '控件状态已确认',
      explanation: `已确认${direction}滚动方向或已到边界；这不代表任务完成。请重新观察页面并获取新的控件引用。`,
    });
  });

  it.each([undefined, null, 'unknown', 'toggle', 'Check', 'check ', 'Invoke', 'ScrollUp', 'scrollDown', 'scroll_up', 'scrollup ', [], { action: 'select' }])('never marks malformed control-state receipts as verified: %j', (action) => {
    expect(toolActionReceipt('operate_trusted_app_control', {
      success: true, output: JSON.stringify({ status: 'verified', action }),
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
  });

  it('renders completed legacy invocation only as dispatch, not state verification', () => {
    expect(toolActionReceipt('invoke_trusted_app_control', {
      success: true, output: '{"status":"dispatched"}',
    })).toMatchObject({ status: 'dispatched' });
    expect(toolActionReceipt('invoke_trusted_app_control', {
      success: true, output: '{"status":"verified","action":"select"}',
    })).toMatchObject({ status: 'unconfirmed' });
  });

  it('does not claim verification when a successful result has no recognized status', () => {
    expect(toolActionReceipt('open_windows_setting', {
      success: true, output: 'legacy success',
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
    expect(toolActionReceipt('reveal_workspace_item', {
      success: true, output: '{"status":"verified"}',
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
  });

  it('treats a stopped draft with unknown result as needing manual inspection, not a retry', () => {
    expect(toolActionReceipt('prepare_message_draft', {
      success: false,
      output: '',
      error: 'Error: {"code":"RESULT_UNKNOWN","message":"cancelled after dispatch"}',
    })).toMatchObject({
      status: 'result_unknown',
      label: '结果未知',
      explanation: '操作停止后草稿可能已写入；请先去目标应用核对，勿自动重试。',
    });
  });

  it('keeps an uncertain generic field write distinct from a failure or retryable action', () => {
    expect(toolActionReceipt('set_trusted_app_text', {
      success: false, output: 'Error: {"code":"RESULT_UNKNOWN"}',
    })).toMatchObject({
      status: 'result_unknown', explanation: '操作停止后输入框可能已改变；请先去目标应用核对，勿自动重试。',
    });
  });

  it('requires manual observation after an uncertain invocation rather than retrying', () => {
    expect(toolActionReceipt('operate_trusted_app_control', {
      success: false, output: 'Error: {"code":"RESULT_UNKNOWN"}',
    })).toMatchObject({
      status: 'result_unknown',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。',
    });
  });

  it('shows an ordinary desktop failure without claiming success or exposing raw errors', () => {
    expect(toolActionReceipt('open_trusted_app', {
      success: false, output: '{"status":"dispatched"}', error: 'PRIVATE_ERROR_PATH',
    })).toMatchObject({ status: 'failed', label: '操作未完成' });
    const result = toolActionReceipt('set_trusted_app_text', {
      success: false, output: 'PRIVATE_OUTPUT', error: 'PRIVATE_ERROR_PATH',
    });
    expect(result?.explanation).toContain('重试前请先核对');
    expect(JSON.stringify(result)).not.toContain('PRIVATE');
    expect(toolActionReceipt('write_file', {
      success: true, output: '{"status":"verified"}',
    })).toBeNull();
    for (const error of [
      'Error: FRAME_CHANGED: PRIVATE_FRAME_DETAILS',
      'FRAME_CHANGED: PRIVATE_FRAME_DETAILS',
      'Error: {"code":"FRAME_CHANGED","message":"PRIVATE_FRAME_DETAILS"}',
      '{"code":"FRAME_CHANGED","message":"PRIVATE_FRAME_DETAILS"}',
    ]) {
      const observation = toolActionReceipt('observe_trusted_app_window', {
        success: false, output: error,
      });
      expect(observation).toMatchObject({ status: 'failed', label: '未取得窗口图像' });
      expect(observation?.explanation).toContain('等待窗口稳定后重新发起观察');
      expect(observation?.explanation).toContain('未返回图像');
      expect(observation?.explanation).toContain('不要为此扩大观察范围');
      expect(JSON.stringify(observation)).not.toContain('PRIVATE');
      expect(isResultUnknown({ success: false, output: error })).toBe(false);
      expect(toolActionReceipt('observe_trusted_app_window', {
        success: true, output: error,
      })).toBeNull();
      expect(toolActionReceipt('write_file', { success: false, output: error })).toBeNull();
    }
    for (const error of ['Error: FRAME_CHANGED_EXTRA: private', 'Error: TARGET_CHANGED: private',
      'private FRAME_CHANGED: private', '{"code":"FRAME_CHANGED ","message":"private"}']) {
      expect(toolActionReceipt('observe_trusted_app_window', { success: false, output: error })).toBeNull();
    }
    expect(toolActionReceipt('observe_trusted_app_window', {
      success: false, output: 'Error: FRAME_CHANGED: private', error: 'PRIVATE_OTHER_FAILURE',
    })).toBeNull();
    expect(toolActionReceipt('observe_trusted_app_window', {
      success: false, output: 'PRIVATE_OUTPUT', error: 'Error: FRAME_CHANGED: private',
    })).toMatchObject({ status: 'failed', label: '未取得窗口图像' });
  });

  it('does not turn an unexecuted desktop confirmation into a failed operation', () => {
    for (const confirmationStatus of ['pending', 'rejected', 'cancelled'] as const) {
      expect(toolActionReceipt('operate_trusted_app_control', {
        success: false, output: '', confirmationStatus,
      })).toBeNull();
      expect(toolActionReceipt('observe_trusted_app_window', {
        success: false, output: 'Error: FRAME_CHANGED: private', confirmationStatus,
      })).toBeNull();
    }
  });
});
