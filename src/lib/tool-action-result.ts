import type { ToolResult } from '$types';

const DESKTOP_ACTION_TOOLS = new Set([
  'open_trusted_app',
  'open_windows_setting',
  'reveal_workspace_item',
  'prepare_message_draft',
  'set_trusted_app_text',
  'operate_trusted_app_control',
  'click_trusted_app_image',
  // Receipt rendering for completed legacy history only, not an active tool.
  'invoke_trusted_app_control',
]);

export type ToolActionReceipt = {
  status: 'dispatched' | 'verified' | 'unconfirmed' | 'result_unknown' | 'failed';
  label: string;
  explanation: string;
};

export const PENDING_ACTIONS_BLOCKED_MESSAGE = '本任务有操作结果待核对，暂不能继续批准；仍可拒绝剩余请求。';

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

/** Tool receipts describe observed execution, not business completion. */
export function toolActionReceipt(
  toolName: string,
  result?: Pick<ToolResult, 'success' | 'output' | 'error' | 'confirmationStatus'>,
): ToolActionReceipt | null {
  if (!result) return null;
  if (result.confirmationStatus === 'pending' || result.confirmationStatus === 'rejected'
    || result.confirmationStatus === 'cancelled') return null;
  if (toolName.startsWith('mcp_') && isResultUnknown(result)) {
    return {
      status: 'result_unknown',
      label: '结果未知',
      explanation: '外部操作可能已部分生效；请先检查目标应用或服务，再决定是否发起新操作。不要重复批准或自动重试。',
    };
  }
  // A refused image is an observation failure, never a verified action. Reuse
  // the existing result presentation without granting a retry or capture scope.
  if (toolName === 'observe_trusted_app_window' && !result.success) {
    const raw = result.error ?? result.output;
    if (structuredResult(raw)?.code === 'FRAME_CHANGED'
      || raw.startsWith('Error: FRAME_CHANGED:') || raw.startsWith('FRAME_CHANGED:')) {
      return {
        status: 'failed', label: '未取得窗口图像',
        explanation: '捕获帧与窗口尺寸不一致，未返回图像。请等待窗口稳定后重新发起观察；不要为此扩大观察范围。',
      };
    }
  }
  if (!DESKTOP_ACTION_TOOLS.has(toolName)) return null;
  if (!result.success) {
    const failure = structuredResult(result.error ?? result.output);
    if (failure?.code !== 'RESULT_UNKNOWN') return {
      status: 'failed',
      label: '操作未完成',
      explanation: '桌面操作未成功完成；请检查目标应用和授权。部分操作可能已经生效，重试前请先核对。',
    };
    return {
      status: 'result_unknown',
      label: '结果未知',
      explanation: toolName === 'prepare_message_draft'
        ? '操作停止后草稿可能已写入；请先去目标应用核对，勿自动重试。'
        : toolName === 'set_trusted_app_text'
          ? '操作停止后输入框可能已改变；请先去目标应用核对，勿自动重试。'
          : toolName === 'operate_trusted_app_control' || toolName === 'invoke_trusted_app_control' || toolName === 'click_trusted_app_image'
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
        : toolName === 'operate_trusted_app_control' || toolName === 'invoke_trusted_app_control' || toolName === 'click_trusted_app_image'
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
      explanation: '目标输入框已按请求修改并回读校验；应用可能自动保存或同步。这不代表任务完成。',
    };
  }
  if (status === 'verified' && toolName === 'operate_trusted_app_control') {
    const action = receipt?.action;
    const choice = receipt?.choice;
    if (action === 'choose' && choice && typeof choice === 'object' && !Array.isArray(choice)
      && 'controlName' in choice && typeof choice.controlName === 'string' && choice.controlName.trim().length > 0 && choice.controlName.length <= 80
      && 'option' in choice && typeof choice.option === 'string' && choice.option.trim().length > 0 && choice.option.length <= 80
      && 'expanded' in choice && choice.expanded === false) return {
      status,
      label: '控件状态已确认',
      explanation: `已确认“${choice.controlName}”已选择“${choice.option}”并收起；仍需检查任务结果，不代表业务已完成。`,
    };
    if (action === 'scrollup' || action === 'scrolldown') return {
      status,
      label: '控件状态已确认',
      explanation: `已确认${action === 'scrollup' ? '向上' : '向下'}滚动方向或已到边界；这不代表任务完成。请重新观察页面并获取新的控件引用。`,
    };
    const stateLabel = action === 'select' ? '选中' : action === 'expand' ? '展开' : action === 'collapse' ? '收起'
      : action === 'check' ? '勾选' : action === 'uncheck' ? '取消勾选' : null;
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
