import { useEffect, useState } from 'react';
import {
  preflightPendingDesktopAction,
  type DesktopActionPreview,
} from '$lib/commands/message';
import { DESKTOP_CONTROL_ACTION_LABELS } from '$lib/tool-confirmation';

type PreviewState =
  | { status: 'loading' }
  | { status: 'unavailable' }
  | { status: 'ready'; preview: DesktopActionPreview };

function isReviewable(value: DesktopActionPreview, operation: DesktopActionPreview['operation'] | null): boolean {
  return typeof value?.previewId === 'string' && value.previewId.length > 0
    && value.operation === operation
    && typeof value.appDisplayName === 'string' && value.appDisplayName.length > 0
    && typeof value.executableName === 'string' && value.executableName.length > 0
    && typeof value.controlName === 'string' && value.controlName.trim().length > 0
    && (operation === 'draft' || operation === 'field'
      ? typeof value.text === 'string' && value.text.length > 0
      : value.text === null)
    && typeof value.expiresAt === 'number' && Number.isFinite(value.expiresAt) && value.expiresAt > Date.now();
}

/**
 * Only the caller's validated operation crosses this module's interface; the
 * remaining model-authored tool arguments are deliberately absent.
 * Only the backend's durable-step preflight can supply a reviewable target,
 * operation, exact text where applicable, and the short-lived approval ID.
 */
export function DesktopActionApproval({
  sessionId,
  messageId,
  callId,
  operation,
  layout,
  onResolve,
}: {
  sessionId?: string;
  messageId?: string;
  callId: string;
  operation: DesktopActionPreview['operation'] | null;
  layout: 'compact' | 'stream';
  onResolve: (decision: 'approved' | 'rejected', previewId?: string) => void;
}) {
  const [state, setState] = useState<PreviewState>({ status: 'loading' });
  const [retry, setRetry] = useState(0);
  const [expired, setExpired] = useState(false);

  useEffect(() => {
    let current = true;
    setState({ status: 'loading' });
    setExpired(false);
    if (!sessionId || !messageId || !operation) {
      setState({ status: 'unavailable' });
      return () => { current = false; };
    }
    void preflightPendingDesktopAction({ sessionId, messageId, callId })
      .then((preview) => {
        if (!current) return;
        setState(isReviewable(preview, operation) ? { status: 'ready', preview } : { status: 'unavailable' });
      })
      .catch(() => {
        if (current) setState({ status: 'unavailable' });
      });
    return () => { current = false; };
  }, [sessionId, messageId, callId, operation, retry]);

  useEffect(() => {
    if (state.status !== 'ready') return;
    const remaining = state.preview.expiresAt - Date.now();
    if (remaining <= 0) {
      setExpired(true);
      return;
    }
    const timer = window.setTimeout(() => setExpired(true), remaining);
    return () => window.clearTimeout(timer);
  }, [state]);

  const preview = state.status === 'ready' && !expired && operation && isReviewable(state.preview, operation)
    ? state.preview
    : null;
  const compact = layout === 'compact';
  const isTextWrite = operation === 'draft' || operation === 'field';
  const actionLabel = operation && !isTextWrite ? DESKTOP_CONTROL_ACTION_LABELS[operation] : null;
  const summaryClass = compact ? 'agent-run-confirmation-summary' : 'content-block-tool-confirmation-summary';
  const actionsClass = compact ? 'agent-run-badge-actions' : 'content-block-tool-actions';
  const approveClass = compact ? 'agent-inline-btn agent-inline-btn--approve' : 'content-block-tool-btn content-block-tool-btn--approve';
  const rejectClass = compact ? 'agent-inline-btn agent-inline-btn--reject' : 'content-block-tool-btn content-block-tool-btn--reject';

  return (
    <div className={compact ? 'agent-run-text-confirmation' : 'content-block-tool-confirmation'}>
      <div className={summaryClass}>
        {actionLabel && <strong>待确认操作：{actionLabel}</strong>}
        {preview ? (
          <>
            已核对目标：{preview.appDisplayName}（{preview.executableName}）
            {preview.windowTitle ? ` · ${preview.windowTitle}` : !isTextWrite ? ' · 未命名窗口' : ''}
            {preview.controlName ? ` · ${preview.controlName}` : !isTextWrite ? ' · 未命名控件' : ''}。
            {operation === 'draft'
              ? '仅填写草稿，不点击发送；请先核对当前会话和收件对象，应用可能自动保存或同步。'
              : operation === 'field'
                ? '将修改这一个输入框，不点击提交；请核对目标窗口与内容，应用可能自动保存、同步或响应字段变化。'
                : <>{`将${actionLabel}，可能触发发送、删除等后果；控件名称不代表安全或授权，请核对当前页面及后果。`}{operation === 'invoke'
                  ? '成功回执仅表示操作请求已发出，不代表目标已完成。'
                  : operation === 'scrollup' || operation === 'scrolldown'
                    ? '成功回执仅确认滚动状态，仍需检查任务结果，不代表目标已完成；请重新观察页面后再发起下一步操作。'
                  : '成功回执仅确认控件状态，仍需检查任务结果，不代表目标已完成。'}</>}
          </>
        ) : !operation ? '请求动作缺失、无效或已停用；不能允许本次操作。' : !isTextWrite
          ? '正在核对目标窗口与待操作控件；核对完成前不能允许。'
          : '正在核对目标窗口与待填写内容；核对完成前不能允许。'}
        {preview && isTextWrite && (compact ? (
          <span className="agent-run-text-preview">
            <span>将填写的内容</span>
            <span>{preview.text}</span>
          </span>
        ) : (
          <div className="content-block-tool-confirmation-preview">
            <strong>将填写的内容</strong>
            <pre>{preview.text}</pre>
          </div>
        ))}
        {state.status === 'unavailable' || expired ? (
          <span className={compact ? 'agent-run-text-warning' : 'content-block-tool-confirmation-warning'} role="alert">
            {expired ? '目标预检已过期，请重新核对。' : !operation
              ? '请求动作缺失、无效或已停用。请拒绝本次操作，重新发起带有明确动作的请求。'
              : operation === 'draft'
              ? '无法核对目标窗口或草稿内容。请检查受信任应用及输入框设置，并确保目标窗口已打开；否则拒绝本次操作。'
              : operation === 'field'
                ? '无法核对目标窗口或待填写内容。请重新观察受信任应用并确认输入框仍可用；否则拒绝本次操作。'
                : '无法核对目标窗口或待操作控件。请重新观察受信任应用并确认控件仍可用；否则拒绝本次操作。'}
            {operation && <button type="button" onClick={() => setRetry((value) => value + 1)}>重新核对</button>}
          </span>
        ) : null}
      </div>
      <div className={actionsClass}>
        <button
          type="button"
          className={approveClass}
          disabled={!preview}
          onClick={() => {
            if (!preview || preview.expiresAt <= Date.now()) {
              setExpired(true);
              return;
            }
            onResolve('approved', preview.previewId);
          }}
        >
          {actionLabel ? `允许${actionLabel}一次` : compact ? '允许一次' : '允许'}
        </button>
        <button type="button" className={rejectClass} onClick={() => onResolve('rejected')}>
          拒绝
        </button>
      </div>
    </div>
  );
}
