import type { ToolResult } from '$types';

const DESKTOP_ACTION_TOOLS = new Set([
  'open_trusted_app',
  'open_windows_setting',
  'reveal_workspace_item',
  'prepare_message_draft',
  'set_trusted_app_text',
  'operate_trusted_app_control',
  // Receipt rendering for completed legacy history only, not an active tool.
  'invoke_trusted_app_control',
]);

export type DesktopActionReceipt = {
  status: 'dispatched' | 'verified' | 'unconfirmed' | 'result_unknown';
  label: string;
  explanation: string;
};

function structuredResult(raw: string): Record<string, unknown> | null {
  try {
    // A direct tool result is JSON; confirmation settlement wraps it in a
    // stable success/error prefix before it reaches the conversation history.
    const payload = raw.startsWith('Error: ')
      ? raw.slice('Error: '.length)
      : raw.startsWith('Confirmation approved: ')
        ? raw.slice('Confirmation approved: '.length)
        : raw;
    const value: unknown = JSON.parse(payload);
    return value && typeof value === 'object' && !Array.isArray(value)
      ? value as Record<string, unknown>
      : null;
  } catch {
    return null;
  }
}

/** An interrupted action is not proof of failure and must never auto-continue. */
export function isResultUnknown(result: Pick<ToolResult, 'success' | 'output' | 'error'>): boolean {
  return !result.success && structuredResult(result.error ?? result.output)?.code === 'RESULT_UNKNOWN';
}

/** Dispatch is not proof that the desktop action's intended outcome occurred. */
export function desktopActionReceipt(
  toolName: string,
  result?: Pick<ToolResult, 'success' | 'output' | 'error'>,
): DesktopActionReceipt | null {
  if (!DESKTOP_ACTION_TOOLS.has(toolName) || !result) return null;
  if (!result.success) {
    const failure = structuredResult(result.error ?? result.output);
    if (failure?.code !== 'RESULT_UNKNOWN') return null;
    return {
      status: 'result_unknown',
      label: '结果未知',
      explanation: toolName === 'prepare_message_draft'
        ? '操作停止后草稿可能已写入；请先去目标应用核对，勿自动重试。'
        : toolName === 'set_trusted_app_text'
          ? '操作停止后输入框可能已改变；请先去目标应用核对，勿自动重试。'
          : toolName === 'operate_trusted_app_control' || toolName === 'invoke_trusted_app_control'
            ? '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。'
            : '操作停止后结果未知；请先检查目标窗口，勿自动重试。',
    };
  }

  // Older or malformed tool results must not be presented as verified work.
  const receipt = structuredResult(result.output);
  const status = receipt?.status;

  if (status === 'dispatched') {
    return {
      status,
      label: '已发出请求',
      explanation: toolName === 'reveal_workspace_item'
        ? '已请求文件管理器定位；尚未确认窗口已显示。'
        : toolName === 'operate_trusted_app_control' || toolName === 'invoke_trusted_app_control'
          ? '操作已发出，需重新观察目标确认结果。'
          : '已向 Windows 发出打开请求；尚未确认目标窗口已就绪。',
    };
  }
  if (status === 'verified' && toolName === 'prepare_message_draft') {
    return {
      status,
      label: '已校验',
      explanation: '草稿已填写并校验；AngelBot 未点击发送，目标应用可能自动保存。',
    };
  }
  if (status === 'verified' && toolName === 'set_trusted_app_text') {
    return {
      status,
      label: '已校验',
      explanation: '目标输入框已填写并回读校验；应用可能自动保存或同步。',
    };
  }
  if (status === 'verified' && toolName === 'operate_trusted_app_control') {
    const action = receipt?.action;
    if (action === 'scrollup' || action === 'scrolldown') return {
      status,
      label: '控件状态已确认',
      explanation: `已确认${action === 'scrollup' ? '向上' : '向下'}滚动方向或已到边界；这不代表任务完成。请重新观察页面并获取新的控件引用。`,
    };
    const stateLabel = action === 'select' ? '选中' : action === 'expand' ? '展开' : action === 'collapse' ? '收起' : null;
    if (stateLabel) return {
      status,
      label: '控件状态已确认',
      explanation: `已确认控件${stateLabel}状态，仍需检查任务结果。`,
    };
  }
  return {
    status: 'unconfirmed',
    label: '状态未确认',
    explanation: '操作已返回，但没有可确认的桌面执行状态。',
  };
}
