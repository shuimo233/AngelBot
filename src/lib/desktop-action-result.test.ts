import { describe, expect, it } from 'vitest';
import { desktopActionReceipt, isResultUnknown } from './desktop-action-result';

describe('desktop action result semantics', () => {
  it('detects a structured unknown result without mistaking ordinary failures for it', () => {
    expect(isResultUnknown({ success: false, output: 'Error: {"code":"RESULT_UNKNOWN"}' })).toBe(true);
    expect(isResultUnknown({ success: false, output: 'Error: {"code":"TARGET_CHANGED"}' })).toBe(false);
    expect(isResultUnknown({ success: true, output: '{"code":"RESULT_UNKNOWN"}' })).toBe(false);
  });
  it('keeps dispatch separate from verified completion', () => {
    expect(desktopActionReceipt('open_trusted_app', {
      success: true,
      output: JSON.stringify({ status: 'dispatched', target: 'notes' }),
    })).toMatchObject({ status: 'dispatched', label: '已发出请求' });
    expect(desktopActionReceipt('prepare_message_draft', {
      success: true,
      output: JSON.stringify({ status: 'verified', target: 'notes' }),
    })).toMatchObject({ status: 'verified', label: '已校验' });
    expect(desktopActionReceipt('open_trusted_app', {
      success: true,
      output: 'Confirmation approved: {"status":"dispatched","target":"notes"}',
    })).toMatchObject({ status: 'dispatched', label: '已发出请求' });
    expect(desktopActionReceipt('prepare_message_draft', {
      success: true,
      output: 'Confirmation approved: {"status":"verified","target":"notes"}',
    })).toMatchObject({ status: 'verified', label: '已校验' });
    expect(desktopActionReceipt('set_trusted_app_text', {
      success: true,
      output: 'Confirmation approved: {"status":"verified","target":"notes"}',
    })).toMatchObject({ status: 'verified', explanation: '目标输入框已填写并回读校验；应用可能自动保存或同步。' });
  });

  it('presents invocation as dispatch requiring observation, never verified goal completion', () => {
    expect(desktopActionReceipt('operate_trusted_app_control', {
      success: true, output: 'Confirmation approved: {"status":"dispatched","action":"invoke"}',
    })).toMatchObject({
      status: 'dispatched', label: '已发出请求',
      explanation: '操作已发出，需重新观察目标确认结果。',
    });
    expect(desktopActionReceipt('operate_trusted_app_control', {
      success: true, output: '{"status":"verified","action":"invoke"}',
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
    expect(desktopActionReceipt('operate_trusted_app_control', {
      success: true, output: 'legacy success',
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
  });

  it.each([
    { action: 'select', state: '选中' }, { action: 'expand', state: '展开' }, { action: 'collapse', state: '收起' },
  ])('limits a verified $action receipt to the control state', ({ action, state }) => {
    expect(desktopActionReceipt('operate_trusted_app_control', {
      success: true, output: `Confirmation approved: ${JSON.stringify({ status: 'verified', action })}`,
    })).toEqual({
      status: 'verified', label: '控件状态已确认',
      explanation: `已确认控件${state}状态，仍需检查任务结果。`,
    });
  });

  it.each([
    { action: 'scrollup', direction: '向上' },
    { action: 'scrolldown', direction: '向下' },
  ])('limits verified $action receipts to scroll state and requires fresh observation', ({ action, direction }) => {
    expect(desktopActionReceipt('operate_trusted_app_control', {
      success: true, output: `Confirmation approved: ${JSON.stringify({ status: 'verified', action })}`,
    })).toEqual({
      status: 'verified', label: '控件状态已确认',
      explanation: `已确认${direction}滚动方向或已到边界；这不代表任务完成。请重新观察页面并获取新的控件引用。`,
    });
  });

  it.each([undefined, null, 'unknown', 'Invoke', 'ScrollUp', 'scrollDown', 'scroll_up', 'scrollup ', [], { action: 'select' }])('never marks malformed control-state receipts as verified: %j', (action) => {
    expect(desktopActionReceipt('operate_trusted_app_control', {
      success: true, output: JSON.stringify({ status: 'verified', action }),
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
  });

  it('renders completed legacy invocation only as dispatch, not state verification', () => {
    expect(desktopActionReceipt('invoke_trusted_app_control', {
      success: true, output: '{"status":"dispatched"}',
    })).toMatchObject({ status: 'dispatched' });
    expect(desktopActionReceipt('invoke_trusted_app_control', {
      success: true, output: '{"status":"verified","action":"select"}',
    })).toMatchObject({ status: 'unconfirmed' });
  });

  it('does not claim verification when a successful result has no recognized status', () => {
    expect(desktopActionReceipt('open_windows_setting', {
      success: true, output: 'legacy success',
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
    expect(desktopActionReceipt('reveal_workspace_item', {
      success: true, output: '{"status":"verified"}',
    })).toMatchObject({ status: 'unconfirmed', label: '状态未确认' });
  });

  it('treats a stopped draft with unknown result as needing manual inspection, not a retry', () => {
    expect(desktopActionReceipt('prepare_message_draft', {
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
    expect(desktopActionReceipt('set_trusted_app_text', {
      success: false, output: 'Error: {"code":"RESULT_UNKNOWN"}',
    })).toMatchObject({
      status: 'result_unknown', explanation: '操作停止后输入框可能已改变；请先去目标应用核对，勿自动重试。',
    });
  });

  it('requires manual observation after an uncertain invocation rather than retrying', () => {
    expect(desktopActionReceipt('operate_trusted_app_control', {
      success: false, output: 'Error: {"code":"RESULT_UNKNOWN"}',
    })).toMatchObject({
      status: 'result_unknown',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。',
    });
  });

  it('does not produce a success receipt for failed or unrelated tools', () => {
    expect(desktopActionReceipt('open_trusted_app', {
      success: false, output: '{"status":"dispatched"}',
    })).toBeNull();
    expect(desktopActionReceipt('write_file', {
      success: true, output: '{"status":"verified"}',
    })).toBeNull();
  });
});
